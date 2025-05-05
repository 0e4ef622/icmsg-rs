//! A Rust implementation of [ICMsg][1].
//!
//! A simple backend for IPC communication between two cores using a ring buffer in shared memory.
//!
//! [1]: https://docs.zephyrproject.org/latest/services/ipc/ipc_service/backends/ipc_service_icmsg.html

#![no_std]

use core::pin::pin;

use embassy_futures::select::{select, Either};
use embedded_hal_async::delay::DelayNs;
use transport::IcMsgTransport;
pub use transport::Notifier;
pub mod transport;

const MAGIC: [u8; 13] = [
    0x45, 0x6d, 0x31, 0x6c, 0x31, 0x4b, 0x30, 0x72, 0x6e, 0x33, 0x6c, 0x69, 0x34,
];

pub struct IcMsg<M, W, const ALIGN: usize>
where
    M: Notifier,
    W: WaitForNotify,
    elain::Align<ALIGN>: elain::Alignment,
{
    transport: transport::IcMsgTransport<M, ALIGN>,
    waiter: W,
}

impl<M, W, const ALIGN: usize> IcMsg<M, W, ALIGN>
where
    M: Notifier,
    W: WaitForNotify,
    elain::Align<ALIGN>: elain::Alignment,
{
    /// Create a new IcMsg channel and perform [bonding][bond].
    ///
    /// SAFETY: The provided [`MemoryConfig`] must be correct.
    ///
    /// [bond]: https://docs.zephyrproject.org/latest/services/ipc/ipc_service/backends/ipc_service_icmsg.html#bonding
    pub async unsafe fn init(
        config: MemoryConfig,
        notifier: M,
        waiter: W,
        mut delay: impl DelayNs,
    ) -> Result<Self, InitError> {
        if config.send_buffer_len % 4 != 0 || config.recv_buffer_len % 4 != 0 {
            return Err(InitError::InvalidSize);
        }

        if config.send_buffer_len < 24 || config.recv_buffer_len < 24 {
            return Err(InitError::TooSmall);
        }

        let mut this = unsafe {
            Self {
                transport: IcMsgTransport::new(
                    config.send_region,
                    config.recv_region,
                    config.send_buffer_len,
                    config.recv_buffer_len,
                    notifier,
                ),
                waiter,
            }
        };

        this.transport.send(&MAGIC).map_err(InitError::BondingSendError)?;

        // Repeat the notification every 1 ms until a notification is received.
        {
            let mut wait_fut = pin!(this.waiter.wait_for_notify());
            loop {
                let timeout = delay.delay_ms(1);
                match select(wait_fut.as_mut(), timeout).await {
                    Either::First(_) => break,
                    Either::Second(_) => this.transport.notify(),
                }
            }
        }

        // Allow larger messages for forward compatibility.
        let mut message = [0; 32];
        this.transport.try_recv(&mut message).map_err(InitError::BondingRecvError)?;

        if message.get(..MAGIC.len()) != Some(&MAGIC) {
            return Err(InitError::BondingWrongMagic);
        }

        Ok(this)
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
#[derive(Copy, Clone)]
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
