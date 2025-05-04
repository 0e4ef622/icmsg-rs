#![no_std]
use core::{mem::MaybeUninit, sync::atomic::Ordering};

use integer::{BeU16, LeAtomicU32};

/// An ICMsg channel, implemented as described in the [Zephyr documentation][1].
///
/// A simple backend for IPC communication between two cores using a ring buffer in shared memory.
///
/// [1]: https://docs.zephyrproject.org/latest/services/ipc/ipc_service/backends/ipc_service_icmsg.html
pub struct IcMsg<M, const ALIGN: usize>
where
    M: Notifier,
    elain::Align<ALIGN>: elain::Alignment,
{
    send_region: *mut SharedMemoryRegionHeader<ALIGN>,
    recv_region: *mut SharedMemoryRegionHeader<ALIGN>,

    // size of the data field, not including the header. must be a multiple of 4
    send_buffer_len: u32,
    // size of the data field, not including the header. must be a multiple of 4
    recv_buffer_len: u32,
    mbox: M,
}

impl<M, const ALIGN: usize> IcMsg<M, ALIGN>
where
    M: Notifier,
    elain::Align<ALIGN>: elain::Alignment,
{
    /// `send_region` and `recv_region` must be properly aligned and appropriately sized and the
    /// first `size_of::<SharedMemoryRegionHeader<ALIGN>>()` bytes zeroed.
    /// `send_buffer_len`/`recv_buffer_len` are the sizes of the
    /// [`data`][data] fields of the corresponding regions in bytes, which must be a multiple of 4.
    ///
    /// If data caching is enabled, the shared memory region provided to ICMsg must be aligned according
    /// to the cache requirement. If cache is not enabled, the required alignment is [4 bytes][ref].
    ///
    /// [ref]: https://docs.zephyrproject.org/latest/services/ipc/ipc_service/backends/ipc_service_icmsg.html#shared-memory-region-organization
    /// [data]: https://docs.zephyrproject.org/latest/services/ipc/ipc_service/backends/ipc_service_icmsg.html#shared-memory-region-organization
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
        Self {
            send_region,
            recv_region,
            send_buffer_len,
            recv_buffer_len,
            mbox,
        }
    }
    /// Send a message.
    pub fn send(&mut self, msg: &[u8]) -> Result<(), SendError> {
        let mut wr_idx = unsafe { (*self.send_region).wr_idx.value.load(Ordering::Relaxed) };
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
            (*self.send_region)
                .wr_idx
                .value
                .store(wr_idx, Ordering::Release);
            // TODO writeback dcache
            self.mbox.notify();
            Ok(())
        }
    }

    /// Receive a message. On success, returns the size of the message.
    pub fn try_recv(&mut self, msg: &mut [u8]) -> Result<usize, RecvError> {
        // TODO invalidate dcache
        let wr_idx = unsafe { (*self.recv_region).wr_idx.value.load(Ordering::Acquire) };
        let mut rd_idx = unsafe { (*self.recv_region).rd_idx.value.load(Ordering::Relaxed) };
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

            let tail_size = (self.recv_buffer_len - rd_idx) as usize;
            if msg_len as usize > tail_size {
                let (p1, p2) = msg.split_at_mut(tail_size);
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
            (*self.recv_region)
                .rd_idx
                .value
                .store(rd_idx, Ordering::Release);
            Ok(msg_len)
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SendError {
    /// There was not enough space in the buffer to send the message.
    InsufficientCapacity,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RecvError {
    /// The message was bigger than the provided buffer.
    MessageTooBig,
    /// There were no messages to receive.
    Empty,
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
    fn notify(&self);
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
mod tests {
    extern crate std;

    use crate::{IcMsg, Notifier, RecvError, SharedMemoryRegionHeader};
    use core::{alloc::Layout, mem::offset_of};

    #[test]
    fn test_alignment() {
        assert_eq!(offset_of!(SharedMemoryRegionHeader<128>, rd_idx), 0);
        assert_eq!(offset_of!(SharedMemoryRegionHeader<128>, wr_idx), 128);
    }

    #[test]
    fn test_send_recv() {
        let expected_messages: &[&[u8]] = &[
            b"", b"0", b"01", b"012", b"0123", b"01234", b"012345", b"0123456", b"01234567"
        ];

        const ALIGN: usize = 4;
        type Hdr = SharedMemoryRegionHeader<ALIGN>;
        let buf_size = 16;
        let shared_region_layout =
            Layout::from_size_align(size_of::<Hdr>() + buf_size, align_of::<Hdr>()).unwrap();
        let shared_region_1 =
            unsafe { std::alloc::alloc_zeroed(shared_region_layout) }.cast::<()>();
        let shared_region_2 =
            unsafe { std::alloc::alloc_zeroed(shared_region_layout) }.cast::<()>();
        unsafe {
            shared_region_1.write_bytes(0, size_of::<Hdr>());
            shared_region_2.write_bytes(0, size_of::<Hdr>());
        }
        let shared_region_sync_1 = SyncPtr(shared_region_1);
        let shared_region_sync_2 = SyncPtr(shared_region_2);

        let recv_thread = std::thread::spawn(move || {
            let shared_region_1 = { shared_region_sync_1 }.0;
            let shared_region_2 = { shared_region_sync_2 }.0;
            let mut icmsg = unsafe {
                IcMsg::<_, ALIGN>::new(
                    shared_region_2,
                    shared_region_1,
                    buf_size as u32,
                    buf_size as u32,
                    Noop,
                )
            };

            let mut buf = [0; 8];
            for &expected_message in expected_messages {
                loop {
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
            IcMsg::<_, ALIGN>::new(
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
        fn notify(&self) {
            self.0.unpark()
        }
    }

    struct Noop;

    impl Notifier for Noop {
        fn notify(&self) {}
    }

    #[derive(Copy, Clone)]
    struct SyncPtr<T>(*mut T);
    unsafe impl<T> Send for SyncPtr<T> {}
    unsafe impl<T> Sync for SyncPtr<T> {}
}
