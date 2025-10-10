#![no_std]
#![no_main]
#![recursion_limit = "256"]

mod cmd_dispatch;

use core::cell::RefCell;

use crate::{cmd_dispatch::exec_cmd_by_opcode, icmsg_config::ALIGN};
use bt_hci::{
    cmd::{self, Opcode, OpcodeGroup}, data::{AclPacketHeader, IsoPacketHeader, SyncPacketHeader}, event::EventPacketHeader, param, FromHciBytes, FromHciBytesError, PacketKind
};
use defmt::{Debug2Format, unwrap};
use embassy_executor::Spawner;
use embassy_nrf::{
    config::Config,
    ipc::{self, Ipc, IpcChannel},
    mode::Async,
    peripherals::{self, RNG},
    rng::{self, Rng},
};
use embassy_sync::{blocking_mutex::{raw::NoopRawMutex, Mutex}};
use embassy_time::Delay;
use icmsg::{IcMsg, Notifier, WaitForNotify};
use nrf_sdc::{self as sdc, mpsl};
use sdc::mpsl::MultiprotocolServiceLayer;
use static_cell::StaticCell;

use {defmt_rtt as _, panic_probe as _};

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

/// How many outgoing L2CAP buffers per link
const L2CAP_TXQ: u8 = 3;

/// How many incoming L2CAP buffers per link
const L2CAP_RXQ: u8 = 3;

/// Size of L2CAP packets
const L2CAP_MTU: usize = 72;

fn build_sdc<'d, const N: usize>(
    p: nrf_sdc::Peripherals<'d>,
    rng: &'d mut rng::Rng<Async>,
    mpsl: &'d MultiprotocolServiceLayer,
    mem: &'d mut sdc::Mem<N>,
) -> Result<nrf_sdc::SoftdeviceController<'d>, nrf_sdc::Error> {
    sdc::Builder::new()?
        .support_adv()?
        .support_peripheral()?
        .peripheral_count(1)?
        .buffer_cfg(L2CAP_MTU as u16, L2CAP_MTU as u16, L2CAP_TXQ, L2CAP_RXQ)?
        .build(p, rng, mpsl, mem)
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

    // give myself a second to attach without panic. uncomment for debug
    // embassy_time::Timer::after_secs(3).await;

    let mut ipc = Ipc::new(p.IPC, Irqs);
    ipc.event0.configure_trigger([IpcChannel::Channel0]);
    ipc.event0.configure_wait([IpcChannel::Channel1]);

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

    // sdc nonsense
    let mpsl_p = mpsl::Peripherals::new(
        p.RTC0, p.TIMER0, p.TIMER1, p.TEMP, p.PPI_CH0, p.PPI_CH1, p.PPI_CH2,
    );
    let lfclk_cfg = mpsl::raw::mpsl_clock_lfclk_cfg_t {
        source: mpsl::raw::MPSL_CLOCK_LF_SRC_RC as u8,
        rc_ctiv: mpsl::raw::MPSL_RECOMMENDED_RC_CTIV as u8,
        rc_temp_ctiv: mpsl::raw::MPSL_RECOMMENDED_RC_TEMP_CTIV as u8,
        accuracy_ppm: mpsl::raw::MPSL_DEFAULT_CLOCK_ACCURACY_PPM as u16,
        skip_wait_lfclk_started: mpsl::raw::MPSL_DEFAULT_SKIP_WAIT_LFCLK_STARTED != 0,
    };
    static MPSL: StaticCell<MultiprotocolServiceLayer> = StaticCell::new();
    let mpsl = MPSL.init(unwrap!(mpsl::MultiprotocolServiceLayer::new(
        mpsl_p, Irqs, lfclk_cfg
    )));
    spawner.must_spawn(mpsl_task(&*mpsl));

    let sdc_p = sdc::Peripherals::new(
        p.PPI_CH3, p.PPI_CH4, p.PPI_CH5, p.PPI_CH6, p.PPI_CH7, p.PPI_CH8, p.PPI_CH9, p.PPI_CH10,
        p.PPI_CH11, p.PPI_CH12,
    );

    static RNG_CELL: StaticCell<Rng<Async>> = StaticCell::new();
    let rng = RNG_CELL.init(Rng::new(p.RNG, Irqs));

    static SDC_MEM: StaticCell<sdc::Mem<2480>> = StaticCell::new();
    let sdc_mem = SDC_MEM.init(sdc::Mem::<2480>::new());
    static SDC_CELL: StaticCell<sdc::SoftdeviceController> = StaticCell::new();
    let sdc = SDC_CELL.init(unwrap!(build_sdc(sdc_p, rng, mpsl, sdc_mem)));

    let (send, recv) = icmsg.split();

    static SEND: StaticCell<Mutex<NoopRawMutex, RefCell<icmsg::Sender<IpcNotify<'static>, ALIGN>>>> = StaticCell::new();
    let send = SEND.init(Mutex::new(RefCell::new(send)));
    spawner.must_spawn(receive_task(send, recv, sdc));

    const OUT_MAX: usize = nrf_sdc::raw::HCI_MSG_BUFFER_MAX_SIZE as usize + 1;
    let mut buffer = [0u8; OUT_MAX];

    loop {
        let pkt_data = &mut buffer[1..];
        let kind = unwrap!(sdc.hci_get(pkt_data).await);
        let used = unwrap!(packet_len(kind, pkt_data));

        assert!(1 + used <= buffer.len());

        buffer[0] = kind as u8;
        let n = 1 + used;

        defmt::trace!("send {}: {=[u8]:x}", n, &buffer[..n]);
        send.lock(|x| x.borrow_mut().send(&buffer[..n]).unwrap());
    }
}

fn packet_len(kind: PacketKind, data: &[u8]) -> Result<usize, FromHciBytesError> {
    Ok(match kind {
        PacketKind::Cmd => {
            if data.len() < 3 {
                return Err(FromHciBytesError::InvalidSize)
            } else {
                data[2] as usize + 3
            }
        }
        PacketKind::AclData => AclPacketHeader::from_hci_bytes(data)?.0.data_len() + 4,
        PacketKind::SyncData => SyncPacketHeader::from_hci_bytes(data)?.0.data_len() + 3,
        PacketKind::Event => EventPacketHeader::from_hci_bytes(data)?.0.params_len as usize + 2,
        PacketKind::IsoData => IsoPacketHeader::from_hci_bytes(data)?.0.data_load_len() + 4,
    })
}

type CmdErr = cmd::Error<sdc::Error>;

fn parse_cmd_after_h4(rest: &[u8]) -> Result<(Opcode, &[u8]), CmdErr> {
    if rest.len() < 3 {
        return Err(cmd::Error::Hci(param::Error::INVALID_HCI_PARAMETERS));
    }
    let raw = u16::from_le_bytes([rest[0], rest[1]]);
    let ogf = OpcodeGroup::new(((raw >> 10) & 0x3F) as u8);
    let ocf = raw & 0x03FF;
    let plen = rest[2] as usize;
    if 3 + plen > rest.len() {
        return Err(cmd::Error::Hci(param::Error::INVALID_HCI_PARAMETERS));
    }
    Ok((Opcode::new(ogf, ocf), &rest[3..3 + plen]))
}

macro_rules! hci {
    ($e:expr) => {
        $e.map_err(|_| cmd::Error::Hci(param::Error::INVALID_HCI_PARAMETERS))
    };
}
macro_rules! io {
    ($e:expr) => {
        $e.map_err(cmd::Error::Io)
    };
}

async fn exec_h4_to_sdc(
	send: &'static Mutex<NoopRawMutex, RefCell<icmsg::Sender<IpcNotify<'static>, ALIGN>>>,
    sdc: &sdc::SoftdeviceController<'static>,
    pkt: &[u8],
) -> Result<(), CmdErr> {
    let (kind, rest) = hci!(bt_hci::PacketKind::from_hci_bytes(pkt))?;
    match kind {
        bt_hci::PacketKind::Cmd => {
            let (opcode, payload) = parse_cmd_after_h4(rest)?;
            defmt::debug!("opcode {}", opcode);
            exec_cmd_by_opcode::<sdc::Error>(send, sdc, opcode, payload).await
        }
        bt_hci::PacketKind::AclData => io!(sdc.hci_data_put(rest)),
        bt_hci::PacketKind::IsoData => io!(sdc.hci_iso_data_put(rest)),
        bt_hci::PacketKind::SyncData => Err(cmd::Error::Hci(param::Error::UNSUPPORTED)),
        bt_hci::PacketKind::Event => Err(cmd::Error::Hci(param::Error::UNSPECIFIED)),
    }
}

#[embassy_executor::task]
async fn receive_task(
	send: &'static Mutex<NoopRawMutex, RefCell<icmsg::Sender<IpcNotify<'static>, ALIGN>>>,
    mut recv: icmsg::Receiver<IpcWait<'static>, ALIGN>,
    sdc: &'static sdc::SoftdeviceController<'static>,
) {
    let mut buf = [0; nrf_sdc::raw::HCI_MSG_BUFFER_MAX_SIZE as usize];
    loop {
        let n = match recv.recv(&mut buf).await {
            Ok(n) => n,
            Err(e) => {
                defmt::error!("Recv error: {:?}", defmt::Debug2Format(&e));
                return;
            }
        };
        defmt::info!("Received {} bytes: {=[u8]:x}", n, &buf[..n]);

		if let Err(e) = exec_h4_to_sdc(send, sdc, &buf[..n]).await {
			defmt::warn!("TX->SDC error: {:?}", e);
		}
    }
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
