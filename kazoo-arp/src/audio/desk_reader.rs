use std::io;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;

use kazoo_core::ipc::protocol::FrameBuffer;
use kazoo_core::ipc::types::{MSG_TRANSPORT_REQUEST, TransportRequestMsg};

#[derive(Debug)]
pub(super) struct DeskReader {
    socket: UnixStream,
    requests: Receiver<TransportRequestMsg>,
    thread: Option<JoinHandle<io::Result<()>>>,
}

impl DeskReader {
    pub(super) fn new(socket: UnixStream) -> io::Result<Self> {
        let mut stream = socket.try_clone()?;
        let (sender, requests) = mpsc::channel();
        let thread = std::thread::Builder::new().spawn(move || {
            let mut buffer = FrameBuffer::new();
            loop {
                match buffer.read_frame(&mut stream) {
                    Ok(header) => {
                        if header.msg_type == MSG_TRANSPORT_REQUEST
                            && sender
                                .send(TransportRequestMsg::decode(buffer.payload()))
                                .is_err()
                        {
                            return Ok(());
                        }
                    }
                    Err(err)
                        if matches!(
                            err.kind(),
                            io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset
                        ) =>
                    {
                        return Ok(());
                    }
                    Err(err) => return Err(err),
                }
            }
        })?;
        Ok(Self {
            socket,
            requests,
            thread: Some(thread),
        })
    }

    pub(super) fn transport_request(&self) -> TransportRequestMsg {
        self.requests.recv().unwrap()
    }
}

impl Drop for DeskReader {
    fn drop(&mut self) {
        if let Err(err) = self.socket.shutdown(Shutdown::Both) {
            assert_eq!(err.kind(), io::ErrorKind::NotConnected, "{err}");
        }
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap().unwrap();
        }
    }
}
