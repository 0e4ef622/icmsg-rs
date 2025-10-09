//! HCI transport layers [📖](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-54/out/en/host-controller-interface.html)

use core::mem::MaybeUninit;

use bt_hci::transport::WithIndicator;
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::mutex::Mutex;
use embedded_io::{Error as _, ErrorKind, ErrorType};

use bt_hci::{ControllerToHostPacket, FromHciBytes as _, HostToControllerPacket, WriteHci};

use crate::icmsg_config;
use crate::uninit_write_buf::UninitWriteBuf;

const STASH_MAX: usize = 1024;

pub struct MyTransport<M: RawMutex, W: icmsg::WaitForNotify, N: icmsg::Notifier> {
    reader: Mutex<M, icmsg::Receiver<W, { icmsg_config::ALIGN }>>,
    writer: Mutex<M, icmsg::Sender<N, { icmsg_config::ALIGN }>>,
}

impl<M: RawMutex, W: icmsg::WaitForNotify, N: icmsg::Notifier> MyTransport<M, W, N> {
    pub fn new(
        reader: icmsg::Receiver<W, { icmsg_config::ALIGN }>,
        writer: icmsg::Sender<N, { icmsg_config::ALIGN }>,
    ) -> Self {
        Self {
            reader: Mutex::new(reader),
            writer: Mutex::new(writer),
        }
    }
}

impl<M: RawMutex, W: icmsg::WaitForNotify, N: icmsg::Notifier> ErrorType for MyTransport<M, W, N> {
    type Error = bt_hci::transport::Error<ErrorKind>;
}

impl<M: RawMutex, W: icmsg::WaitForNotify, N: icmsg::Notifier>
    bt_hci::transport::Transport for MyTransport<M, W, N>
{
    async fn read<'a>(&self, rx: &'a mut [u8]) -> Result<ControllerToHostPacket<'a>, Self::Error> {
        let mut r = self.reader.lock().await;
        let n = r.recv(rx).await.map_err(|e| e.kind())?;

        Ok(ControllerToHostPacket::from_hci_bytes_complete(&rx[..n])?)
    }

    async fn write<T: HostToControllerPacket>(&self, tx: &T) -> Result<(), Self::Error> {
        let needed = tx.size() + 1;
        assert!(needed <= STASH_MAX);

        let mut storage: [MaybeUninit<u8>; STASH_MAX] =
            [const { MaybeUninit::uninit() }; STASH_MAX];
        let mut sink = UninitWriteBuf::new(&mut storage[..needed]);

        defmt::unwrap!(WithIndicator::new(tx).write_hci_async(&mut sink).await);

        let buf = sink.as_init();

        let mut w = self.writer.lock().await;
        Ok(w.send(buf)
            .map_err(|e| e.kind())?)
    }
}
