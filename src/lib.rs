//! A Rust implementation of [ICMsg][1].
//!
//! A simple backend for IPC communication between two cores using a ring buffer in shared memory.
//!
//! [1]: https://docs.zephyrproject.org/latest/services/ipc/ipc_service/backends/ipc_service_icmsg.html

#![no_std]

use core::pin::pin;

use embassy_futures::select::{Either, select};
use embedded_hal_async::delay::DelayNs;
use transport::IcMsgTransport;
pub use transport::Notifier;
pub mod transport;
#[macro_use]
mod poll;

const MAGIC: [u8; 13] = [
    0x45, 0x6d, 0x31, 0x6c, 0x31, 0x4b, 0x30, 0x72, 0x6e, 0x33, 0x6c, 0x69, 0x34,
];

pub struct IcMsg<M, W, const ALIGN: usize>
where
    M: Notifier,
    W: WaitForNotify,
    elain::Align<ALIGN>: elain::Alignment,
{
    sender: Sender<M, ALIGN>,
    receiver: Receiver<W, ALIGN>,
}

impl<M, W, const ALIGN: usize> IcMsg<M, W, ALIGN>
where
    M: Notifier,
    W: WaitForNotify,
    elain::Align<ALIGN>: elain::Alignment,
{
    /// Create a new IcMsg channel and perform [bonding][bond].
    ///
    /// # Safety
    ///
    /// The provided [`MemoryConfig`] must be correct.
    ///
    /// [bond]: https://docs.zephyrproject.org/latest/services/ipc/ipc_service/backends/ipc_service_icmsg.html#bonding
    pub async unsafe fn init(
        config: MemoryConfig,
        notifier: M,
        mut waiter: W,
        mut delay: impl DelayNs,
    ) -> Result<Self, InitError> {
        if config.send_buffer_len % 4 != 0 || config.recv_buffer_len % 4 != 0 {
            return Err(InitError::InvalidSize);
        }

        if config.send_buffer_len < 24 || config.recv_buffer_len < 24 {
            return Err(InitError::TooSmall);
        }

        let mut transport = unsafe {
            IcMsgTransport::new(
                config.send_region,
                config.recv_region,
                config.send_buffer_len,
                config.recv_buffer_len,
                notifier,
            )
        };

        transport
            .send(&MAGIC)
            .map_err(InitError::BondingSendError)?;

        // Repeat the notification every 1 ms until a notification is received.
        {
            let mut wait_fut = pin!(waiter.wait_for_notify());
            loop {
                let timeout = delay.delay_ms(1);
                match select(wait_fut.as_mut(), timeout).await {
                    Either::First(_) => break,
                    Either::Second(_) => transport.notify(),
                }
            }
            transport.notify();
        }

        // Allow larger messages for forward compatibility.
        let mut message = [0; 32];
        transport
            .try_recv(&mut message)
            .map_err(InitError::BondingRecvError)?;

        if message.get(..MAGIC.len()) != Some(&MAGIC) {
            return Err(InitError::BondingWrongMagic);
        }

        let (s, r) = transport.split();
        let sender = Sender { transport: s };
        let receiver = Receiver {
            transport: r,
            waiter,
        };

        Ok(Self { sender, receiver })
    }

    /// Send a message
    pub fn send(&mut self, msg: &[u8]) -> Result<(), transport::SendError> {
        self.sender.send(msg)
    }

    /// Receive a message. On success, returns the size of the message.
    pub fn try_recv(&mut self, msg: &mut [u8]) -> Result<usize, transport::RecvError> {
        self.receiver.try_recv(msg)
    }

    pub fn recv(
        &mut self,
        msg: &mut [u8],
    ) -> impl Future<Output = Result<usize, transport::RecvError>> {
        self.receiver.recv(msg)
    }

    pub fn split(self) -> (Sender<M, ALIGN>, Receiver<W, ALIGN>) {
        (self.sender, self.receiver)
    }

    pub fn split_mut(&mut self) -> (&mut Sender<M, ALIGN>, &mut Receiver<W, ALIGN>) {
        (&mut self.sender, &mut self.receiver)
    }
}

pub struct Sender<M, const ALIGN: usize>
where
    M: Notifier,
    elain::Align<ALIGN>: elain::Alignment,
{
    transport: transport::Sender<M, ALIGN>,
}

impl<M, const ALIGN: usize> Sender<M, ALIGN>
where
    M: Notifier,
    elain::Align<ALIGN>: elain::Alignment,
{
    pub fn send(&mut self, msg: &[u8]) -> Result<(), transport::SendError> {
        self.transport.send(msg)
    }
}

pub struct Receiver<W, const ALIGN: usize>
where
    W: WaitForNotify,
    elain::Align<ALIGN>: elain::Alignment,
{
    transport: transport::Receiver<ALIGN>,
    waiter: W,
}

impl<W, const ALIGN: usize> Receiver<W, ALIGN>
where
    W: WaitForNotify,
    elain::Align<ALIGN>: elain::Alignment,
{
    /// Try to receive a message if one is available. On success, returns the size of the message.
    pub fn try_recv(&mut self, msg: &mut [u8]) -> Result<usize, transport::RecvError> {
        self.transport.try_recv(msg)
    }

    /// Wait for and receive a message. On success, returns the size of the message.
    pub async fn recv(&mut self, msg: &mut [u8]) -> Result<usize, transport::RecvError> {
        loop {
            // Let the waiter register its waker before attempting to recv
            let mut wait_fut = pin!(self.waiter.wait_for_notify());
            let r = poll!(wait_fut.as_mut());

            match self.transport.try_recv(msg) {
                Ok(n) => return Ok(n),
                Err(transport::RecvError::Empty) => {
                    if r.is_pending() {
                        wait_fut.await;
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }
}

/// The memory configuration of the channel.
///
/// `send_region` and `recv_region` must be properly aligned and appropriately sized.
/// `send_buffer_len`/`recv_buffer_len` are the sizes of the
/// [`data`][data] fields of the corresponding regions in bytes, which must be a multiple of 4.
/// They should be at least 24 bytes large.
///
/// If data caching is enabled, the shared memory region provided to ICMsg must be aligned according
/// to the cache requirement. If cache is not enabled, the required alignment is [4 bytes][ref].
///
/// [ref]: https://docs.zephyrproject.org/latest/services/ipc/ipc_service/backends/ipc_service_icmsg.html#shared-memory-region-organization
/// [data]: https://docs.zephyrproject.org/latest/services/ipc/ipc_service/backends/ipc_service_icmsg.html#shared-memory-region-organization
#[derive(Debug, Copy, Clone)]
pub struct MemoryConfig {
    /// Pointer to the send memory region.
    pub send_region: *mut (),
    /// Pointer to the recv memory region.
    pub recv_region: *mut (),
    /// Size of the [data][data] field of the send memory region in bytes.
    ///
    /// [data]: https://docs.zephyrproject.org/latest/services/ipc/ipc_service/backends/ipc_service_icmsg.html#shared-memory-region-organization
    pub send_buffer_len: u32,
    /// Size of the [data][data] field of the recv memory region in bytes.
    ///
    /// [data]: https://docs.zephyrproject.org/latest/services/ipc/ipc_service/backends/ipc_service_icmsg.html#shared-memory-region-organization
    pub recv_buffer_len: u32,
}

pub trait WaitForNotify {
    fn wait_for_notify(&mut self) -> impl Future<Output = ()>;
}

#[derive(Debug, Copy, Clone)]
pub enum InitError {
    /// The send or recv regions were too small
    TooSmall,
    /// The send or recv buffer lengths were not a multiple of 4.
    InvalidSize,
    /// A [`SendError`][`transport::SendError`] occurred during bonding.
    BondingSendError(transport::SendError),
    /// A [`RecvError`][`transport::RecvError`] occurred during bonding.
    BondingRecvError(transport::RecvError),
    /// The magic sequence was not received during bonding.
    BondingWrongMagic,
}

#[cfg(test)]
mod tests {
    extern crate std;

    use embedded_hal_async::delay::DelayNs;
    use std::sync::Arc;
    use tokio::sync::Notify;

    use crate::{
        Notifier, WaitForNotify,
        transport::{SharedMemoryRegionHeader, tests::SyncThing},
    };

    use super::{IcMsg, MemoryConfig};
    use core::{alloc::Layout, time::Duration};

    #[tokio::main]
    #[test]
    async fn test_bonding() {
        let expected_messages: &[&[u8]] = &[
            b"",
            b"0",
            b"01",
            b"012",
            b"0123",
            b"01234",
            b"012345",
            b"0123456",
            b"01234567",
        ];

        const ALIGN: usize = 4;
        type Hdr = SharedMemoryRegionHeader<ALIGN>;
        let buf_size = 24;
        let shared_region_layout =
            Layout::from_size_align(size_of::<Hdr>() + buf_size, align_of::<Hdr>()).unwrap();
        let shared_region_1 = unsafe { std::alloc::alloc(shared_region_layout) }.cast::<()>();
        let shared_region_2 = unsafe { std::alloc::alloc(shared_region_layout) }.cast::<()>();
        let notify_1 = Arc::new(Notify::new());
        let notify_2 = Arc::new(Notify::new());

        let recv_task = tokio::spawn(SyncThing({
            let notify_1 = Arc::clone(&notify_1);
            let notify_2 = Arc::clone(&notify_2);
            async move {
                let config = MemoryConfig {
                    send_region: shared_region_2,
                    recv_region: shared_region_1,
                    send_buffer_len: buf_size as u32,
                    recv_buffer_len: buf_size as u32,
                };
                let mut icmsg = unsafe {
                    IcMsg::<_, _, ALIGN>::init(config, &*notify_2, &*notify_1, TokioDelay)
                        .await
                        .unwrap()
                };

                // receive messages
                let mut buf = [0; 8];
                for &expected_message in expected_messages {
                    let n = icmsg.recv(&mut buf).await.unwrap();
                    std::eprintln!("task 2 recv'd {:?}", &buf[..n]);
                    assert_eq!(&buf[..n], expected_message);
                }

                // send messages
                for msg in expected_messages {
                    loop {
                        let r = icmsg.send(msg);
                        if r.is_err() {
                            tokio::task::yield_now().await;
                            continue;
                        }
                        std::eprintln!("task 2 sent {msg:?}");
                        break;
                    }
                }
            }
        }));

        let config = MemoryConfig {
            send_region: shared_region_1,
            recv_region: shared_region_2,
            send_buffer_len: buf_size as u32,
            recv_buffer_len: buf_size as u32,
        };
        let mut icmsg = unsafe {
            IcMsg::<_, _, ALIGN>::init(config, &*notify_1, &*notify_2, TokioDelay)
                .await
                .unwrap()
        };

        // send messages
        for msg in expected_messages {
            loop {
                let r = icmsg.send(msg);
                if r.is_err() {
                    tokio::task::yield_now().await;
                    continue;
                }
                std::eprintln!("task 1 sent {msg:?}");
                break;
            }
        }

        // receive messages
        let mut buf = [0; 8];
        for &expected_message in expected_messages {
            let n = icmsg.recv(&mut buf).await.unwrap();
            std::eprintln!("task 1 recv'd {:?}", &buf[..n]);
            assert_eq!(&buf[..n], expected_message);
        }

        recv_task.await.unwrap();
        unsafe {
            std::alloc::dealloc(shared_region_1.cast(), shared_region_layout);
            std::alloc::dealloc(shared_region_2.cast(), shared_region_layout);
        }
    }

    impl Notifier for &'_ Notify {
        fn notify(&mut self) {
            self.notify_waiters()
        }
    }

    impl WaitForNotify for &'_ Notify {
        fn wait_for_notify(&mut self) -> impl Future<Output = ()> {
            self.notified()
        }
    }

    struct TokioDelay;
    impl DelayNs for TokioDelay {
        fn delay_ns(&mut self, ns: u32) -> impl Future<Output = ()> {
            tokio::time::sleep(Duration::from_nanos(ns as u64))
        }

        fn delay_us(&mut self, us: u32) -> impl Future<Output = ()> {
            tokio::time::sleep(Duration::from_micros(us as u64))
        }

        fn delay_ms(&mut self, ms: u32) -> impl Future<Output = ()> {
            tokio::time::sleep(Duration::from_millis(ms as u64))
        }
    }
}
