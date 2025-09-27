#![no_std]
#![no_main]
use bt_hci::WriteHci;
use defmt::unwrap;
use embassy_executor::Spawner;
use embassy_nrf::{config::Config, ipc::{self, Ipc, IpcChannel}, mode::Async, peripherals::{self, RNG}, rng::{self, Rng}};
use embassy_time::Delay;
use icmsg::{IcMsg, Notifier, WaitForNotify};
use nrf_sdc::{self as sdc, mpsl};
use sdc::mpsl::MultiprotocolServiceLayer;
use static_cell::StaticCell;
use {
    defmt_rtt as _,
    panic_probe as _,
};

embassy_nrf::bind_interrupts!(struct Irqs {
    IPC => embassy_nrf::ipc::InterruptHandler<peripherals::IPC>;
    RNG => rng::InterruptHandler<RNG>;
    EGU0 => mpsl::LowPrioInterruptHandler;
    CLOCK_POWER => mpsl::ClockInterruptHandler;
    RADIO => mpsl::HighPrioInterruptHandler;
    TIMER0 => mpsl::HighPrioInterruptHandler;
    RTC0 => mpsl::HighPrioInterruptHandler;
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

fn build_sdc<'d, const N: usize>(
    p: nrf_sdc::Peripherals<'d>,
    rng: &'d mut Rng<RNG, Async>,
    mpsl: &'d MultiprotocolServiceLayer,
    mem: &'d mut sdc::Mem<N>,
) -> Result<nrf_sdc::SoftdeviceController<'d>, nrf_sdc::Error> {
    sdc::Builder::new()?.support_adv()?.build(p, rng, mpsl, mem)
}

#[embassy_executor::task]
async fn mpsl_task(mpsl: &'static MultiprotocolServiceLayer<'static>) -> ! {
    mpsl.run().await
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
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

    // sdc nonsense
    let mpsl_p = mpsl::Peripherals::new(p.RTC0, p.TIMER0, p.TIMER1, p.TEMP, p.PPI_CH0, p.PPI_CH1, p.PPI_CH2);
    let lfclk_cfg = mpsl::raw::mpsl_clock_lfclk_cfg_t {
        source: mpsl::raw::MPSL_CLOCK_LF_SRC_RC as u8,
        rc_ctiv: mpsl::raw::MPSL_RECOMMENDED_RC_CTIV as u8,
        rc_temp_ctiv: mpsl::raw::MPSL_RECOMMENDED_RC_TEMP_CTIV as u8,
        accuracy_ppm: mpsl::raw::MPSL_DEFAULT_CLOCK_ACCURACY_PPM as u16,
        skip_wait_lfclk_started: mpsl::raw::MPSL_DEFAULT_SKIP_WAIT_LFCLK_STARTED != 0,
    };
    static MPSL: StaticCell<MultiprotocolServiceLayer> = StaticCell::new();
    let mpsl = MPSL.init(unwrap!(mpsl::MultiprotocolServiceLayer::new(mpsl_p, Irqs, lfclk_cfg)));
    spawner.must_spawn(mpsl_task(&*mpsl));

    let sdc_p = sdc::Peripherals::new(
        p.PPI_CH3, p.PPI_CH4, p.PPI_CH5, p.PPI_CH6, p.PPI_CH7, p.PPI_CH8, p.PPI_CH9, p.PPI_CH10, p.PPI_CH11,
        p.PPI_CH12,
    );

    static RNG_CELL: StaticCell<Rng<RNG, Async>> = StaticCell::new();
    let rng = RNG_CELL.init(Rng::new(p.RNG, Irqs));

    static SDC_MEM: StaticCell<sdc::Mem<1648>> = StaticCell::new();
    let sdc_mem = SDC_MEM.init(sdc::Mem::<1648>::new());
    static SDC_CELL: StaticCell<sdc::SoftdeviceController> = StaticCell::new();
    let sdc = SDC_CELL.init(unwrap!(build_sdc(sdc_p, rng, mpsl, sdc_mem)));

    let (mut send, recv) = icmsg.split();
    spawner.must_spawn(receive_task(recv, sdc));

    let mut buf = [0; 256];
    loop {
        let packet = unwrap!(sdc.hci_get(&mut buf[1..]).await);
        unwrap!(packet.write_hci_async(&mut buf[..]).await);
        send.send(&buf).unwrap();
    }
}

#[embassy_executor::task]
async fn receive_task(
    mut recv: icmsg::Receiver<IpcWait<'static>, { icmsg_config::ALIGN }>,
    sdc: &'static sdc::SoftdeviceController<'static>,
) {
    let mut buf = [0; 256];
    loop {
        let n = match recv.recv(&mut buf).await {
            Ok(n) => n,
            Err(e) => {
                defmt::info!("Recv error: {:?}", e);
                return;
            }
        };
        defmt::info!("Received {} bytes: {:x}", n, &buf[..n]);
        unwrap!(sdc.hci_data_put(&buf[..n]));
    }
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
