//! Low-level ICMsg transport.
//!
//! This provides low-level send and receive primitives and does not include the initial [bonding][1].
//!
//! [1]: https://docs.zephyrproject.org/latest/services/ipc/ipc_service/backends/ipc_service_icmsg.html#bonding

use core::{mem::MaybeUninit, sync::atomic::Ordering};

use integer::{BeU16, LeAtomicU32};

/// The low-level ICMsg transport.
pub struct IcMsgTransport<M, const ALIGN: usize>
where
    M: Notifier,
    elain::Align<ALIGN>: elain::Alignment,
{
    sender: Sender<M, ALIGN>,
    receiver: Receiver<ALIGN>,
}

impl<M, const ALIGN: usize> IcMsgTransport<M, ALIGN>
where
    M: Notifier,
    elain::Align<ALIGN>: elain::Alignment,
{
    /// Create and initialize a new `IcMsgTransport`. This does NOT perform the initial
    /// [bonding][bonding].
    ///
    /// See [`MemoryConfig`][`super::MemoryConfig`] for information about the parameters.
    ///
    /// [bonding]: https://docs.zephyrproject.org/latest/services/ipc/ipc_service/backends/ipc_service_icmsg.html#bonding
    ///
    /// # Safety
    ///
    /// The parameters must follow the requirements detailed in [`MemoryConfig`][`super::MemoryConfig`].
    pub unsafe fn new(
        send_region: *mut (),
        recv_region: *mut (),
        send_buffer_len: u32,
        recv_buffer_len: u32,
        mbox: M,
    ) -> Self {
        let send_region = send_region.cast::<SharedMemoryRegionHeader<ALIGN>>();
        let recv_region = recv_region.cast::<SharedMemoryRegionHeader<ALIGN>>();
        debug_assert!(send_buffer_len % 4 == 0);
        debug_assert!(recv_buffer_len % 4 == 0);
        debug_assert!(send_region.is_aligned());
        debug_assert!(recv_region.is_aligned());
        unsafe {
            (*send_region).wr_idx.value.store(0, Ordering::Relaxed);
            (*send_region).rd_idx.value.store(0, Ordering::Relaxed);
        }

        let sender = Sender {
            send_region,
            send_buffer_len,
            mbox,
            send_wr_idx: 0,
        };
        let receiver = Receiver {
            recv_region,
            recv_buffer_len,
            recv_rd_idx: 0,
        };
        Self { sender, receiver }
    }

    /// Notify the other end.
    pub fn notify(&mut self) {
        self.sender.notify()
    }

    pub fn send(&mut self, msg: &[u8]) -> Result<(), SendError> {
        self.sender.send(msg)
    }

    pub fn try_recv(&mut self, msg: &mut [u8]) -> Result<usize, RecvError> {
        self.receiver.try_recv(msg)
    }

    pub fn split(self) -> (Sender<M, ALIGN>, Receiver<ALIGN>) {
        (self.sender, self.receiver)
    }

    pub fn split_mut(&mut self) -> (&mut Sender<M, ALIGN>, &mut Receiver<ALIGN>) {
        (&mut self.sender, &mut self.receiver)
    }
}

/// The receiving half of the low-level ICMsg transport.
pub struct Receiver<const ALIGN: usize>
where
    elain::Align<ALIGN>: elain::Alignment,
{
    recv_region: *mut SharedMemoryRegionHeader<ALIGN>,

    // size of the data field, not including the header. must be a multiple of 4
    recv_buffer_len: u32,

    // local copies to prevent the other side from interfering
    recv_rd_idx: u32,
}

impl<const ALIGN: usize> Receiver<ALIGN>
where
    elain::Align<ALIGN>: elain::Alignment,
{
    /// Receive a message. On success, returns the size of the message.
    pub fn try_recv(&mut self, msg: &mut [u8]) -> Result<usize, RecvError> {
        // TODO invalidate dcache
        let wr_idx = unsafe { (*self.recv_region).wr_idx.value.load(Ordering::Acquire) };
        let mut rd_idx = self.recv_rd_idx;
        if wr_idx == rd_idx {
            return Err(RecvError::Empty);
        }

        unsafe {
            let data_ptr = self
                .recv_region
                .cast::<u8>()
                .add(size_of::<SharedMemoryRegionHeader<ALIGN>>());
            // Packets are always padded to 4 bytes, and the recv buffer length is a multiple of 4,
            // therefore it is always valid to read 4 bytes at rd_idx.
            let header = data_ptr.add(rd_idx as usize).cast::<PacketHeader>().read();
            rd_idx += 4;
            if rd_idx >= self.recv_buffer_len {
                rd_idx = 0;
            }

            let msg_len = header.len.value() as usize;
            if msg_len > msg.len() {
                return Err(RecvError::MessageTooBig);
            }
            if msg_len as u32 > self.recv_buffer_len {
                return Err(RecvError::InvalidMessage);
            }

            let tail_size = (self.recv_buffer_len - rd_idx) as usize;
            if msg_len > tail_size {
                let (p1, p2) = msg[..msg_len].split_at_mut(tail_size);
                data_ptr
                    .add(rd_idx as usize)
                    .copy_to_nonoverlapping(p1.as_mut_ptr(), p1.len());
                data_ptr.copy_to_nonoverlapping(p2.as_mut_ptr(), p2.len());
            } else {
                data_ptr
                    .add(rd_idx as usize)
                    .copy_to_nonoverlapping(msg.as_mut_ptr(), msg_len);
            }

            let padded_msg_len = msg_len + (4 - msg_len % 4) % 4;
            rd_idx += padded_msg_len as u32;
            if rd_idx >= self.recv_buffer_len {
                rd_idx -= self.recv_buffer_len;
            }
            self.recv_rd_idx = rd_idx;
            (*self.recv_region)
                .rd_idx
                .value
                .store(rd_idx, Ordering::Release);
            Ok(msg_len)
        }
    }
}

/// The sending half of the low-level ICMsg transport.
pub struct Sender<M, const ALIGN: usize>
where
    M: Notifier,
    elain::Align<ALIGN>: elain::Alignment,
{
    send_region: *mut SharedMemoryRegionHeader<ALIGN>,

    // size of the data field, not including the header. must be a multiple of 4
    send_buffer_len: u32,
    mbox: M,

    // local copies to prevent the other side from interfering
    send_wr_idx: u32,
}

impl<M, const ALIGN: usize> Sender<M, ALIGN>
where
    M: Notifier,
    elain::Align<ALIGN>: elain::Alignment,
{
    /// Send a message.
    pub fn send(&mut self, msg: &[u8]) -> Result<(), SendError> {
        let mut wr_idx = self.send_wr_idx;
        let rd_idx = unsafe { (*self.send_region).rd_idx.value.load(Ordering::Acquire) };

        // The FIFO has one byte less capacity than the data buffer length.
        let free_space = if rd_idx > wr_idx {
            rd_idx - wr_idx - 1
        } else {
            rd_idx + self.send_buffer_len - wr_idx - 1
        };

        let padded_msg_len = msg.len() + (4 - msg.len() % 4) % 4;
        if (free_space as usize) < padded_msg_len + size_of::<PacketHeader>() {
            return Err(SendError::InsufficientCapacity);
        }

        unsafe {
            let data_ptr = self
                .send_region
                .cast::<u8>()
                .add(size_of::<SharedMemoryRegionHeader<ALIGN>>());

            // Packets are always padded to 4 bytes, and the send buffer length is a multiple of 4,
            // therefore it is always valid to write 4 bytes at wr_idx.
            let header = PacketHeader::new(msg.len() as u16);
            data_ptr
                .add(wr_idx as usize)
                .cast::<PacketHeader>()
                .write(header);
            wr_idx += 4;
            if wr_idx >= self.send_buffer_len {
                wr_idx = 0;
            }

            let tail_size = (self.send_buffer_len - wr_idx) as usize;
            if msg.len() > tail_size {
                // Wrap around
                let (p1, p2) = msg.split_at(tail_size);
                data_ptr
                    .add(wr_idx as usize)
                    .copy_from_nonoverlapping(p1.as_ptr(), p1.len());
                data_ptr.copy_from_nonoverlapping(p2.as_ptr(), p2.len());
            } else {
                data_ptr
                    .add(wr_idx as usize)
                    .copy_from_nonoverlapping(msg.as_ptr(), msg.len());
            }

            wr_idx += padded_msg_len as u32;
            if wr_idx >= self.send_buffer_len {
                wr_idx -= self.send_buffer_len;
            }
            self.send_wr_idx = wr_idx;
            (*self.send_region)
                .wr_idx
                .value
                .store(wr_idx, Ordering::Release);
            // TODO writeback dcache
            self.notify();
            Ok(())
        }
    }

    /// Notify the other end.
    pub fn notify(&mut self) {
        self.mbox.notify()
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SendError {
    /// There was not enough space in the buffer to send the message.
    InsufficientCapacity,
    /// The rd_idx of the sending region contained an invalid value. This is a fatal error, likely
    /// caused by a bug in the channel implementation.
    InvalidState,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RecvError {
    /// The message was bigger than the provided buffer.
    MessageTooBig,
    /// There were no messages to receive.
    Empty,
    /// An invalid message was received. e.g. a packet with a length greater than the shared memory
    /// memory region. This is a fatal error, likely caused by a bug in the channel implementation.
    InvalidMessage,
}

#[repr(C)]
pub struct SharedMemoryRegionHeader<const ALIGN: usize>
where
    elain::Align<ALIGN>: elain::Alignment,
{
    rd_idx: Index<ALIGN>,
    wr_idx: Index<ALIGN>,
}

#[repr(C)]
struct Index<const ALIGN: usize>
where
    elain::Align<ALIGN>: elain::Alignment,
{
    _align: elain::Align<ALIGN>,
    value: LeAtomicU32,
}

#[repr(C)]
struct PacketHeader {
    len: BeU16,
    _reserved: [MaybeUninit<u8>; 2],
}

impl PacketHeader {
    fn new(len: u16) -> Self {
        Self {
            len: len.into(),
            _reserved: [MaybeUninit::uninit(); 2],
        }
    }
}

pub trait Notifier {
    fn notify(&mut self);
}

mod integer {
    use core::sync::atomic::{AtomicU32, Ordering};

    /// A big-endian u16.
    #[repr(transparent)]
    pub struct BeU16(u16);

    impl BeU16 {
        pub fn value(self) -> u16 {
            self.into()
        }
    }

    impl From<u16> for BeU16 {
        fn from(t: u16) -> Self {
            BeU16(t.to_be())
        }
    }

    impl From<BeU16> for u16 {
        fn from(t: BeU16) -> Self {
            u16::from_be(t.0)
        }
    }

    /// An atomic little-endian u32.
    #[repr(transparent)]
    pub struct LeAtomicU32(AtomicU32);

    impl LeAtomicU32 {
        pub fn load(&self, order: Ordering) -> u32 {
            u32::from_le(self.0.load(order))
        }
        pub fn store(&self, val: u32, order: Ordering) {
            self.0.store(val.to_le(), order)
        }
    }
}

#[cfg(test)]
pub mod tests {
    extern crate std;

    use super::{IcMsgTransport, Notifier, RecvError, SharedMemoryRegionHeader};
    use core::{alloc::Layout, mem::offset_of};

    #[test]
    fn test_alignment() {
        assert_eq!(offset_of!(SharedMemoryRegionHeader<128>, rd_idx), 0);
        assert_eq!(offset_of!(SharedMemoryRegionHeader<128>, wr_idx), 128);
    }

    #[test]
    fn test_send_recv() {
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
        let buf_size = 16;
        let shared_region_layout =
            Layout::from_size_align(size_of::<Hdr>() + buf_size, align_of::<Hdr>()).unwrap();
        let shared_region_1 = unsafe { std::alloc::alloc(shared_region_layout) }.cast::<()>();
        let shared_region_2 = unsafe { std::alloc::alloc(shared_region_layout) }.cast::<()>();
        let shared_region_sync_1 = SyncThing(shared_region_1);
        let shared_region_sync_2 = SyncThing(shared_region_2);

        let recv_thread = std::thread::spawn(move || {
            let shared_region_1 = { shared_region_sync_1 }.0;
            let shared_region_2 = { shared_region_sync_2 }.0;
            let mut icmsg = unsafe {
                IcMsgTransport::<_, ALIGN>::new(
                    shared_region_2,
                    shared_region_1,
                    buf_size as u32,
                    buf_size as u32,
                    Noop,
                )
            };

            let mut buf = [0; 8];
            let mut first = true;
            for &expected_message in expected_messages {
                loop {
                    if first {
                        std::thread::park();
                        first = false;
                    }
                    let r = icmsg.try_recv(&mut buf);
                    if r == Err(RecvError::Empty) {
                        std::thread::park();
                        continue;
                    }
                    let msg = &buf[..r.unwrap()];
                    std::eprintln!("recv'd {msg:?}");
                    assert_eq!(msg, expected_message);
                    break;
                }
            }
        });
        let mut icmsg = unsafe {
            IcMsgTransport::<_, ALIGN>::new(
                shared_region_1,
                shared_region_2,
                buf_size as u32,
                buf_size as u32,
                ThreadNotifier(recv_thread.thread()),
            )
        };

        for msg in expected_messages {
            loop {
                let r = icmsg.send(msg);
                if r.is_err() {
                    std::thread::yield_now();
                    continue;
                }
                std::eprintln!("sent {msg:?}");
                break;
            }
        }
        recv_thread.join().unwrap();

        unsafe {
            std::alloc::dealloc(shared_region_1.cast(), shared_region_layout);
            std::alloc::dealloc(shared_region_2.cast(), shared_region_layout);
        }
    }

    struct ThreadNotifier<'a>(&'a std::thread::Thread);

    impl Notifier for ThreadNotifier<'_> {
        fn notify(&mut self) {
            self.0.unpark()
        }
    }

    struct Noop;

    impl Notifier for Noop {
        fn notify(&mut self) {}
    }

    /// Make something unconditionally Send + Sync. Use with care.
    #[derive(Copy, Clone)]
    pub(crate) struct SyncThing<T>(pub T);
    unsafe impl<T> Send for SyncThing<T> {}
    unsafe impl<T> Sync for SyncThing<T> {}
    impl<T: Future> core::future::Future for SyncThing<T> {
        type Output = T::Output;

        fn poll(
            self: core::pin::Pin<&mut Self>,
            cx: &mut core::task::Context<'_>,
        ) -> core::task::Poll<Self::Output> {
            unsafe { self.map_unchecked_mut(|x| &mut x.0).poll(cx) }
        }
    }
}
