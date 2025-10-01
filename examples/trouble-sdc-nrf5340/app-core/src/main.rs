#![no_std]
#![no_main]

mod fake_rng;
mod init;
mod transport;

use crate::transport::MyTransport;
use bt_hci::controller::ExternalController;
use defmt::Debug2Format;
use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_nrf::{
    ipc::{self, Ipc, IpcChannel},
    peripherals,
};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time::{Delay, Duration, Timer};
use icmsg::{IcMsg, Notifier, WaitForNotify};
use trouble_host::{
    Address, Host, HostResources, Stack,
    prelude::{
        AdStructure, Advertisement, AdvertisementParameters, BR_EDR_NOT_SUPPORTED,
        DefaultPacketPool, LE_GENERAL_DISCOVERABLE,
    },
};

use {defmt_rtt as _, panic_probe as _};

embassy_nrf::bind_interrupts!(struct Irqs {
    IPC => embassy_nrf::ipc::InterruptHandler<peripherals::IPC>;
});

mod icmsg_config {
    unsafe extern "C" {
        static mut __icmsg_tx_start: u32;
        static __icmsg_tx_end: u32;
        static mut __icmsg_rx_start: u32;
        static __icmsg_rx_end: u32;
    }

    pub const ALIGN: usize = 4;
    pub fn get_icmsg_config() -> icmsg::MemoryConfig {
        unsafe {
            let send_buffer_len =
                (&raw const __icmsg_tx_end).byte_offset_from(&raw const __icmsg_tx_start) as u32
                    - size_of::<icmsg::transport::SharedMemoryRegionHeader<ALIGN>>() as u32;
            let recv_buffer_len =
                (&raw const __icmsg_rx_end).byte_offset_from(&raw const __icmsg_rx_start) as u32
                    - size_of::<icmsg::transport::SharedMemoryRegionHeader<ALIGN>>() as u32;
            icmsg::MemoryConfig {
                send_region: (&raw mut __icmsg_tx_start).cast(),
                recv_region: (&raw mut __icmsg_rx_start).cast(),
                send_buffer_len,
                recv_buffer_len,
            }
        }
    }
}

/// Configure SPU so the network core can access the ICMSG {RX,TX} regions.
/// - `extdomain_idx`: which EXTDOMAIN slot to configure (often 0 for the net core).
///
/// Safety: touches SPU_S registers and trusts linker-provided symbol layout.
pub unsafe fn grant_spu(extdomain_idx: Option<usize>) {
    use embassy_nrf::pac::spu::vals;

    unsafe extern "C" {
        static __icmsg_tx_start: u32;
        static __icmsg_tx_end: u32;
        static __icmsg_rx_start: u32;
        static __icmsg_rx_end: u32;
    }

    const RAM_BASE: u32 = 0x2000_0000;
    const REGION_SIZE: u32 = 8 * 1024; // 8 KiB per SPU RAM region (nRF53)
    #[inline]
    fn to_region_index(addr: u32) -> u32 {
        (addr.saturating_sub(RAM_BASE)) / REGION_SIZE
    }

    let tx_start = (&raw const __icmsg_tx_start) as u32;
    let tx_end = (&raw const __icmsg_tx_end) as u32;
    let rx_start = (&raw const __icmsg_rx_start) as u32;
    let rx_end = (&raw const __icmsg_rx_end) as u32;

    let tx_first = to_region_index(tx_start);
    let tx_last = to_region_index(tx_end.saturating_sub(1));
    let rx_first = to_region_index(rx_start);
    let rx_last = to_region_index(rx_end.saturating_sub(1));

    let spu = embassy_nrf::pac::SPU_S;
    let configure_range = |first: u32, last: u32| {
        for i in first..=last {
            spu.ramregion(i as usize).perm().write(|w| {
                w.set_read(true);
                w.set_write(true);
                w.set_execute(true);
                w.set_secattr(false);
            });
        }
    };

    configure_range(tx_first, tx_last);
    configure_range(rx_first, rx_last);

    if let Some(idx) = extdomain_idx {
        spu.extdomain(idx).perm().write(|w| {
            w.set_securemapping(vals::ExtdomainPermSecuremapping::NON_SECURE);
        });
    }
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let (_core_peripherals, p) = init::init();

    defmt::info!("Hello, world!");

    unsafe {
        grant_spu(Some(0));
    }

    let mut ipc = Ipc::new(p.IPC, Irqs);
    ipc.event0.configure_trigger([IpcChannel::Channel1]);
    ipc.event0.configure_wait([IpcChannel::Channel0]);

    let icmsg_config = icmsg_config::get_icmsg_config();
    defmt::info!("{:?}", Debug2Format(&icmsg_config));
    let icmsg = unsafe {
        IcMsg::<_, _, { icmsg_config::ALIGN }>::init(
            icmsg_config::get_icmsg_config(),
            IpcNotify {
                trigger: ipc.event0.trigger_handle(),
            },
            IpcWait { event: ipc.event0 },
            Delay,
        )
        .await
    };
    let icmsg = match icmsg {
        Err(e) => {
            defmt::error!("error: {:?}", Debug2Format(&e));
            return;
        }
        Ok(icmsg) => {
            defmt::info!("Connected!");
            icmsg
        }
    };

    let mut resources: HostResources<DefaultPacketPool, 1, 0, 1> = HostResources::new();

    let (send, recv) = icmsg.split();

    let driver: MyTransport<NoopRawMutex, _, _> = MyTransport::new(recv, send);
    let controller: ExternalController<_, 10> = ExternalController::new(driver);

    // Using a fixed "random" address can be useful for testing. In real scenarios, one would
    // use e.g. the MAC 6 byte array as the address (how to get that varies by the platform).
    let address: Address = Address::random([0xff, 0x8f, 0x19, 0x05, 0xe4, 0xff]);
    defmt::info!("Our address = {}", address);

    let stack: Stack<'_, _, _> =
        trouble_host::new(controller, &mut resources).set_random_address(address);
    let Host {
        mut peripheral,
        mut runner,
        ..
    } = stack.build();

    let mut adv_data = [0; 31];
    let len = AdStructure::encode_slice(
        &[
            AdStructure::CompleteLocalName(b"Trouble Advert"),
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
        ],
        &mut adv_data[..],
    )
    .unwrap();

    defmt::info!("Starting advertising");
    let _ = join(runner.run(), async {
        loop {
            let mut params = AdvertisementParameters::default();
            params.interval_min = Duration::from_millis(100);
            params.interval_max = Duration::from_millis(100);
            let _advertiser = peripheral
                .advertise(
                    &params,
                    Advertisement::NonconnectableScannableUndirected {
                        adv_data: &[],
                        scan_data: &adv_data[..len],
                    },
                )
                .await
                .unwrap();
            loop {
                defmt::info!("Still running");
                Timer::after(Duration::from_secs(60)).await;
            }
        }
    })
    .await;
}

struct IpcNotify<'d> {
    trigger: ipc::EventTrigger<'d, peripherals::IPC>,
}

struct IpcWait<'d> {
    event: ipc::Event<'d, peripherals::IPC>,
}

impl Notifier for IpcNotify<'_> {
    fn notify(&mut self) {
        self.trigger.trigger();
    }
}

impl WaitForNotify for IpcWait<'_> {
    fn wait_for_notify(&mut self) -> impl Future<Output = ()> {
        self.event.wait()
    }
}
