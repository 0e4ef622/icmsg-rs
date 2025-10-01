//! HCI transport layers [📖](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-54/out/en/host-controller-interface.html)

use core::mem::MaybeUninit;

use bt_hci::transport::WithIndicator;
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::mutex::Mutex;
use embedded_io::ErrorType;

use bt_hci::controller::blocking::TryError;
use bt_hci::{ControllerToHostPacket, HostToControllerPacket, ReadHci, WriteHci};

use crate::uninit_write_buf::UninitWriteBuf;

const STASH_MAX: usize = 512;

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
        n
    }
    fn refill_from_slice(&mut self, src: &[u8]) {
        self.head = 0;
        self.tail = src.len();
        self.buf[..self.tail].copy_from_slice(src);
    }
}

pub struct MyTransport<M: RawMutex, R, W> {
    reader: Mutex<M, R>,
    writer: Mutex<M, W>,
    stash: Mutex<M, Stash>,
}

impl<M: RawMutex, R: embedded_io_async::Read, W: embedded_io_async::Write> MyTransport<M, R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader: Mutex::new(reader),
            writer: Mutex::new(writer),
            stash: Mutex::new(Stash::new()),
        }
    }
}

impl<
    M: RawMutex,
    R: embedded_io::ErrorType<Error = E>,
    W: embedded_io::ErrorType<Error = E>,
    E: embedded_io::Error,
> ErrorType for MyTransport<M, R, W>
{
    type Error = bt_hci::transport::Error<E>;
}

struct StashReaderAsync<'a, R> {
    stash: &'a mut Stash,
    inner: &'a mut R,
}
impl<'a, R: embedded_io_async::Read> StashReaderAsync<'a, R> {
    async fn read(&mut self, out: &mut [u8]) -> Result<usize, R::Error> {
        if self.stash.available() == 0 {
            let mut tmp = [0u8; STASH_MAX];
            let n = self.inner.read(&mut tmp).await?;
            self.stash.refill_from_slice(&tmp[..n]);
        }
        Ok(self.stash.take_into(out))
    }
}
impl<'a, R: embedded_io_async::Read> embedded_io_async::ErrorType for StashReaderAsync<'a, R> {
    type Error = R::Error;
}
impl<'a, R: embedded_io_async::Read> embedded_io_async::Read for StashReaderAsync<'a, R> {
    async fn read(&mut self, out: &mut [u8]) -> Result<usize, Self::Error> {
        StashReaderAsync::read(self, out).await
    }
}

impl<
    M: RawMutex,
    R: embedded_io_async::Read<Error = E>,
    W: embedded_io_async::Write<Error = E>,
    E: embedded_io::Error,
> bt_hci::transport::Transport for MyTransport<M, R, W>
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

        let mut storage: [MaybeUninit<u8>; STASH_MAX] = [const { MaybeUninit::uninit() }; 512];
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

struct StashReaderBlocking<'a, R> {
    stash: &'a mut Stash,
    inner: &'a mut R,
}
impl<'a, R: embedded_io::Read> StashReaderBlocking<'a, R> {
    fn read(&mut self, out: &mut [u8]) -> Result<usize, R::Error> {
        if self.stash.available() == 0 {
            let mut tmp = [0u8; STASH_MAX];
            let n = self.inner.read(&mut tmp)?;
            self.stash.refill_from_slice(&tmp[..n]);
        }
        Ok(self.stash.take_into(out))
    }
}
impl<'a, R: embedded_io::Read> embedded_io::ErrorType for StashReaderBlocking<'a, R> {
    type Error = R::Error;
}
impl<'a, R: embedded_io::Read> embedded_io::Read for StashReaderBlocking<'a, R> {
    fn read(&mut self, out: &mut [u8]) -> Result<usize, Self::Error> {
        StashReaderBlocking::read(self, out)
    }
}

impl<
    M: RawMutex,
    R: embedded_io::Read<Error = E>,
    W: embedded_io::Write<Error = E>,
    E: embedded_io::Error,
> bt_hci::transport::blocking::Transport for MyTransport<M, R, W>
{
    fn read<'a>(
        &self,
        rx: &'a mut [u8],
    ) -> Result<ControllerToHostPacket<'a>, TryError<Self::Error>> {
        let mut r = self.reader.try_lock().map_err(|_| TryError::Busy)?;
        let mut s = self.stash.try_lock().map_err(|_| TryError::Busy)?;
        let mut adapter = StashReaderBlocking {
            stash: &mut *s,
            inner: &mut *r,
        };

        ControllerToHostPacket::read_hci(&mut adapter, rx)
            .map_err(bt_hci::transport::Error::Read)
            .map_err(TryError::Error)
    }

    fn write<T: HostToControllerPacket>(&self, tx: &T) -> Result<(), TryError<Self::Error>> {
        let needed = tx.size() + 1;
        assert!(needed <= STASH_MAX);

        let mut storage: [MaybeUninit<u8>; STASH_MAX] = [const { MaybeUninit::uninit() }; 512];
        let mut sink = UninitWriteBuf::new(&mut storage[..needed]);

        defmt::unwrap!(WithIndicator::new(tx).write_hci(&mut sink));

        let buf = sink.as_init();

        let mut w = self.writer.try_lock().map_err(|_| TryError::Busy)?;
        w.write(buf)
            .map(|_| ())
            .map_err(bt_hci::transport::Error::Write)
            .map_err(TryError::Error)
    }
}
