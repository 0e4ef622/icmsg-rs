#![no_std]
#![no_main]

mod ble_bas_peripheral_bonding;
mod init;
mod transport;
mod uninit_write_buf;

use crate::transport::MyTransport;
use bt_hci::controller::ExternalController;
use defmt::Debug2Format;
use embassy_embedded_hal::adapter::BlockingAsync;
use embassy_executor::Spawner;
use embassy_nrf::{
    ipc::{self, Ipc, IpcChannel},
    peripherals,
};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time::Delay;
use icmsg::{IcMsg, Notifier, WaitForNotify};

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
        static __ram_start: u32;
        static __ram_size: u32;
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
        for i in first..last + 1 {
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

    let (send, recv) = icmsg.split();

    let driver: MyTransport<NoopRawMutex, _, _> = MyTransport::new(recv, send);
    let controller: ExternalController<_, 10> = ExternalController::new(driver);

    let mut nvmc = BlockingAsync::new(embassy_nrf::nvmc::Nvmc::new(p.NVMC));
    ble_bas_peripheral_bonding::run(controller, &mut nvmc).await
}

struct IpcNotify<'d> {
    trigger: ipc::EventTrigger<'d>,
}

struct IpcWait<'d> {
    event: ipc::Event<'d>,
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
