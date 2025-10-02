//! HCI transport layers [📖](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-54/out/en/host-controller-interface.html)

use core::mem::MaybeUninit;

use bt_hci::transport::WithIndicator;
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::mutex::Mutex;
use embedded_io::{Error as _, ErrorKind, ErrorType};

use bt_hci::{ControllerToHostPacket, HostToControllerPacket, ReadHci, WriteHci};
use icmsg::WaitForNotify;

use crate::icmsg_config;
use crate::uninit_write_buf::UninitWriteBuf;

const STASH_MAX: usize = 1024;

struct Stash {
    buf: [u8; STASH_MAX],
    head: usize,
    tail: usize,
}
impl Stash {
    const fn new() -> Self {
        Self {
            buf: [0; STASH_MAX],
            head: 0,
            tail: 0,
        }
    }
    fn available(&self) -> usize {
        self.tail.saturating_sub(self.head)
    }
    fn take_into(&mut self, out: &mut [u8]) -> usize {
        let n = core::cmp::min(out.len(), self.available());
        out[..n].copy_from_slice(&self.buf[self.head..self.head + n]);
        self.head += n;
        defmt::trace!("Reading {} bytes from stash: {=[u8]:x}", n, &out[..n]);
        n
    }
    fn refill_from_slice(&mut self, src: &[u8]) {
        self.head = 0;
        self.tail = src.len();
        self.buf[..self.tail].copy_from_slice(src);
    }
}

pub struct MyTransport<M: RawMutex, N: icmsg::WaitForNotify, W> {
    reader: Mutex<M, icmsg::Receiver<N, { icmsg_config::ALIGN }>>,
    writer: Mutex<M, W>,
    stash: Mutex<M, Stash>,
}

impl<M: RawMutex, N: icmsg::WaitForNotify, W: embedded_io_async::Write> MyTransport<M, N, W> {
    pub fn new(reader: icmsg::Receiver<N, { icmsg_config::ALIGN }>, writer: W) -> Self {
        Self {
            reader: Mutex::new(reader),
            writer: Mutex::new(writer),
            stash: Mutex::new(Stash::new()),
        }
    }
}

impl<
    M: RawMutex,
    N: icmsg::WaitForNotify,
    W: embedded_io_async::ErrorType<Error = ErrorKind>,
> ErrorType for MyTransport<M, N, W>
{
    type Error = bt_hci::transport::Error<ErrorKind>;
}

struct StashReaderAsync<'a, N: WaitForNotify> {
    stash: &'a mut Stash,
    inner: &'a mut icmsg::Receiver<N, {icmsg_config::ALIGN}>,
}
impl<'a, N: WaitForNotify> StashReaderAsync<'a, N> {
    async fn read(&mut self, out: &mut [u8]) -> Result<usize, icmsg::transport::RecvError> {
        if self.stash.available() == 0 {
            let mut tmp = [0u8; STASH_MAX];
            let n = self.inner.recv(&mut tmp).await?;
            self.stash.refill_from_slice(&tmp[..n]);
        }
        Ok(self.stash.take_into(out))
    }
}
impl<'a, N: WaitForNotify> embedded_io_async::ErrorType for StashReaderAsync<'a, N> {
    type Error = ErrorKind;
}
impl<'a, N: WaitForNotify> embedded_io_async::Read for StashReaderAsync<'a, N> {
    async fn read(&mut self, out: &mut [u8]) -> Result<usize, Self::Error> {
        StashReaderAsync::read(self, out).await.map_err(|x| x.kind())
    }
}

impl<
    M: RawMutex,
    N: icmsg::WaitForNotify,
    W: embedded_io_async::Write<Error = ErrorKind>,
> bt_hci::transport::Transport for MyTransport<M, N, W>
{
    async fn read<'a>(&self, rx: &'a mut [u8]) -> Result<ControllerToHostPacket<'a>, Self::Error> {
        let mut r = self.reader.lock().await;
        let mut s = self.stash.lock().await;
        let mut adapter = StashReaderAsync {
            stash: &mut *s,
            inner: &mut *r,
        };

        ControllerToHostPacket::read_hci_async(&mut adapter, rx)
            .await
            .map_err(bt_hci::transport::Error::Read)
    }

    async fn write<T: HostToControllerPacket>(&self, tx: &T) -> Result<(), Self::Error> {
        let needed = tx.size() + 1;
        assert!(needed <= STASH_MAX);

        let mut storage: [MaybeUninit<u8>; STASH_MAX] = [const { MaybeUninit::uninit() }; STASH_MAX];
        let mut sink = UninitWriteBuf::new(&mut storage[..needed]);

        defmt::unwrap!(WithIndicator::new(tx).write_hci_async(&mut sink).await);

        let buf = sink.as_init();

        let mut w = self.writer.lock().await;
        w.write(buf)
            .await
            .map(|_| ())
            .map_err(bt_hci::transport::Error::Write)
    }
}
