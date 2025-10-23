#![no_std]
#![no_main]
use embassy_executor::Spawner;
use embassy_nrf::{config::Config, ipc::{self, Ipc, IpcChannel}, pac, peripherals};
use embassy_time::Delay;
use icmsg::{IcMsg, Notifier, WaitForNotify};
use rtt_target::rprintln;
use {
    rtt_target::rtt_init_print,
    panic_probe as _,
};

embassy_nrf::bind_interrupts!(struct Irqs {
    IPC => embassy_nrf::ipc::InterruptHandler<peripherals::IPC>;
});

mod icmsg_config {
    #[allow(improper_ctypes)]
    unsafe extern "C" {
        static mut __icmsg_tx_start: ();
        static __icmsg_tx_end: ();
        static mut __icmsg_rx_start: ();
        static __icmsg_rx_end: ();
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
async fn main(spawner: Spawner) {
    rtt_init_print!();
    let config = Config::default();
    let p = embassy_nrf::init(config);

    // Let the network core do whatever it wants >:)
    pac::SPU.extdomain(0).perm().write(|v| v.set_secattr(true));
    embassy_nrf::reset::release_network_core();

    rprintln!("Hello, world!");

    let mut ipc = Ipc::new(p.IPC, Irqs);
    ipc.event0.configure_trigger([IpcChannel::Channel0]);
    ipc.event0.configure_wait([IpcChannel::Channel1]);

    let icmsg_config = icmsg_config::get_icmsg_config();
    rprintln!("{:?}", icmsg_config);
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
            rprintln!("error: {:?}", e);
            return;
        }
        Ok(icmsg) => {
            rprintln!("Connected!");
            icmsg
        }
    };

    let (mut send, recv) = icmsg.split();
    spawner.must_spawn(receive(recv));

    let msgs: &[&[u8]] = &[
        b"0",
        b"01",
        b"012",
        b"0123",
        b"01234",
        b"012345",
        b"0123456",
    ];

    for msg in msgs {
        let _ = send.send(msg);
        rprintln!("Sent {:x?}", msg);
        embassy_time::Timer::after_secs(1).await;
    }
}

#[embassy_executor::task]
async fn receive(mut recv: icmsg::Receiver<IpcWait<'static>, { icmsg_config::ALIGN }>) {
    let mut buf = [0; 128];
    loop {
        let n = match recv.recv(&mut buf).await {
            Ok(n) => n,
            Err(e) => {
                rprintln!("Recv error: {:?}", e);
                return;
            }
        };
        rprintln!("Received {} bytes: {:x?}", n, &buf[..n]);
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
