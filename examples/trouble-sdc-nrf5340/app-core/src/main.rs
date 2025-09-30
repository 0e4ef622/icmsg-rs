#![no_std]
#![no_main]
use bt_hci::{controller::ExternalController, transport::SerialTransport};
use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_nrf::{config::Config, ipc::{self, Ipc, IpcChannel}, peripherals};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time::{Delay, Duration, Timer};
use icmsg::{IcMsg, Notifier, WaitForNotify};
use trouble_host::{Host, HostResources, Stack, prelude::{AdStructure, Advertisement, AdvertisementParameters, BR_EDR_NOT_SUPPORTED, DefaultPacketPool, LE_GENERAL_DISCOVERABLE}};
use {
    defmt_rtt as _,
    panic_probe as _,
};

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
            let send_buffer_len = (&raw const __icmsg_tx_end)
                .byte_offset_from(&raw const __icmsg_tx_start) as u32
                - size_of::<icmsg::transport::SharedMemoryRegionHeader<ALIGN>>() as u32;
            let recv_buffer_len = (&raw const __icmsg_rx_end)
                .byte_offset_from(&raw const __icmsg_rx_start) as u32
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

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let config = Config::default();
    let p = embassy_nrf::init(config);

    defmt::info!("Hello, world!");

    let mut ipc = Ipc::new(p.IPC, Irqs);
    ipc.event0.configure_trigger([IpcChannel::Channel0]);
    ipc.event0.configure_wait([IpcChannel::Channel1]);

    let icmsg_config = icmsg_config::get_icmsg_config();
    defmt::info!("{:?}", icmsg_config);
    let icmsg = unsafe {
        IcMsg::<_, _, { icmsg_config::ALIGN }>::init(
            icmsg_config::get_icmsg_config(),
            IpcNotify { trigger: ipc.event0.trigger_handle() },
            IpcWait { event: ipc.event0 },
            Delay,
        ).await
    };
    let icmsg = match icmsg {
        Err(e) => {
            defmt::info!("error: {:?}", e);
            return;
        }
        Ok(icmsg) => {
            defmt::info!("Connected!");
            icmsg
        }
    };

    let mut resources: HostResources<DefaultPacketPool, 1, 0, 1> = HostResources::new();
    
    let (send, recv) = icmsg.split();

    let driver: SerialTransport<NoopRawMutex, _, _> = SerialTransport::new(recv, send);
    let controller: ExternalController<_, 10> = ExternalController::new(driver);

    let stack: Stack<'_, _, _> = trouble_host::new(controller, &mut resources);
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
                        adv_data: &adv_data[..len],
                        scan_data: &[],
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
