use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::path::Path;
use std::time::Instant;

use kazoo_core::ipc::protocol::FrameBuffer;
use kazoo_core::ipc::stream::HubStream;

use super::{Hub, HubNotice, Pending, REGISTRATION_TIMEOUT, RefuseReason, Server};

const MAX_PENDING_TCP: usize = 2 * crate::shared::DESK_CHANNELS;

impl Hub {
    /// The bound network address, including the selected ephemeral port.
    #[must_use]
    pub const fn listen_addr(&self) -> Option<SocketAddr> {
        self.listen
    }
}

pub(super) fn bind(address: Option<SocketAddr>) -> io::Result<Option<TcpListener>> {
    address
        .map(|address| {
            let listener = TcpListener::bind(address).map_err(|error| {
                io::Error::new(error.kind(), format!("cannot listen on {address}: {error}"))
            })?;
            listener.set_nonblocking(true)?;
            Ok(listener)
        })
        .transpose()
}

pub(super) fn bind_claimed(
    address: Option<SocketAddr>,
    socket: &Path,
) -> io::Result<(Option<TcpListener>, Option<SocketAddr>)> {
    let opened = bind(address).and_then(|listener| {
        let address = listener.as_ref().map(TcpListener::local_addr).transpose()?;
        Ok((listener, address))
    });
    match opened {
        Ok(opened) => Ok(opened),
        Err(error) => match std::fs::remove_file(socket) {
            Ok(()) => Err(error),
            Err(cleanup) => Err(io::Error::new(
                error.kind(),
                format!(
                    "{error}; removing the newly claimed socket {} also failed: {cleanup}",
                    socket.display(),
                ),
            )),
        },
    }
}

impl Server {
    pub(super) fn accept_tcp(&mut self) {
        // Bound each accepting pass so queued connections cannot starve audio.
        for _ in 0..MAX_PENDING_TCP {
            let accepted = match &self.network {
                Some(listener) => listener.accept(),
                None => return,
            };
            match accepted {
                Ok((stream, peer)) => {
                    if let Err(error) = stream
                        .set_nonblocking(true)
                        .and_then(|()| stream.set_nodelay(true))
                    {
                        self.notify(HubNotice::Refused {
                            name: peer.to_string(),
                            reason: RefuseReason::Handshake(format!("TCP setup failed: {error}")),
                            told: false,
                        });
                        continue;
                    }
                    self.accept_tcp_peer(HubStream::Tcp(stream), peer);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return,
                Err(error) => {
                    if !self.accept_tcp_error(&error) {
                        return;
                    }
                }
            }
        }
    }

    fn accept_tcp_error(&mut self, error: &io::Error) -> bool {
        if matches!(
            error.kind(),
            io::ErrorKind::Interrupted | io::ErrorKind::ConnectionAborted
        ) {
            return true;
        }
        self.notify(HubNotice::Refused {
            name: String::new(),
            reason: RefuseReason::Handshake(format!("TCP accept failed: {error}")),
            told: false,
        });
        false
    }

    fn accept_tcp_peer(&mut self, stream: HubStream, peer: SocketAddr) {
        if let Err(reason) = check_peer(peer) {
            self.refuse(&stream, peer.to_string(), reason);
            return;
        }
        let waiting = self
            .pending
            .iter()
            .filter(|pending| matches!(pending.stream, HubStream::Tcp(_)))
            .count();
        if waiting >= MAX_PENDING_TCP {
            self.refuse(
                &stream,
                peer.to_string(),
                RefuseReason::Handshake(format!(
                    "too many TCP connections waiting to register (limit {MAX_PENDING_TCP})"
                )),
            );
            return;
        }
        self.pending.push(Pending {
            stream,
            read_buf: FrameBuffer::new(),
            deadline: Instant::now() + REGISTRATION_TIMEOUT,
        });
    }
}

fn check_peer(peer: SocketAddr) -> Result<(), RefuseReason> {
    let local = match peer.ip() {
        IpAddr::V4(ip) => local_v4(ip),
        IpAddr::V6(ip) => ip.to_ipv4_mapped().map_or_else(
            || {
                ip.is_loopback()
                    || ip.segments()[0] & 0xfe00 == 0xfc00
                    || ip.segments()[0] & 0xffc0 == 0xfe80
            },
            local_v4,
        ),
    };
    if local {
        Ok(())
    } else {
        Err(RefuseReason::Handshake(format!(
            "peer {peer} is outside the local network"
        )))
    }
}

const fn local_v4(ip: Ipv4Addr) -> bool {
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || (ip.octets()[0] == 100 && ip.octets()[1] & 0xc0 == 64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> (
        Server,
        std::sync::mpsc::Receiver<HubNotice>,
        tempfile::TempDir,
    ) {
        use crate::hub::{HubConfig, HubStats, NOTICE_BACKLOG, PATCH_CAPACITY};
        use crate::patchbay::patchbay;
        use crate::shared::{DESK_CHANNELS, SharedState};
        use std::os::unix::net::UnixListener;
        use std::sync::{Arc, mpsc::sync_channel};
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("hub.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let mut config = HubConfig::standard(48_000, [false; DESK_CHANNELS]);
        config.socket = socket;
        config.advertise = false;
        let (patches, callback) = patchbay(PATCH_CAPACITY);
        let (notices, received) = sync_channel(NOTICE_BACKLOG);
        let stats = Arc::new(HubStats::default());
        let server = Server::new(
            listener,
            None,
            config,
            Arc::new(SharedState::new()),
            patches,
            notices,
            &stats,
        );
        drop(callback);
        (server, received, directory)
    }

    #[test]
    fn tcp_accept_errors_retry_transients_and_report_refusals() {
        let (mut server, received, directory) = server();
        assert!(server.accept_tcp_error(&io::ErrorKind::Interrupted.into()));
        assert!(server.accept_tcp_error(&io::ErrorKind::ConnectionAborted.into()));
        assert!(matches!(
            received.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        assert!(!server.accept_tcp_error(&io::Error::other("no file descriptors")));
        assert!(
            matches!(received.recv().unwrap(), HubNotice::Refused { told: false, reason, .. }
            if reason.to_string().contains("no file descriptors"))
        );
        drop(directory);
    }

    #[test]
    fn tcp_pending_registrations_are_bounded_and_report_the_peer() {
        use std::net::TcpStream;
        let (mut server, received, directory) = server();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let clients: Vec<_> = (0..=2 * crate::shared::DESK_CHANNELS)
            .map(|_| {
                let client = TcpStream::connect(address).unwrap();
                let (stream, peer) = listener.accept().unwrap();
                stream.set_nonblocking(true).unwrap();
                server.accept_tcp_peer(HubStream::Tcp(stream), peer);
                client
            })
            .collect();
        assert_eq!(server.pending.len(), 2 * crate::shared::DESK_CHANNELS);
        assert!(
            matches!(received.recv().unwrap(), HubNotice::Refused { name, told: true, reason }
            if name == clients.last().unwrap().local_addr().unwrap().to_string()
                && reason.to_string().contains("waiting to register"))
        );
        drop(directory);
    }

    #[test]
    fn a_public_peer_is_told_why_and_never_enters_registration() {
        use std::os::unix::net::{UnixListener, UnixStream};
        use std::sync::Arc;
        use std::sync::mpsc::sync_channel;

        use crate::hub::{HubConfig, HubStats, NOTICE_BACKLOG, PATCH_CAPACITY};
        use crate::patchbay::patchbay;
        use crate::shared::{DESK_CHANNELS, SharedState};
        use kazoo_core::ipc::types::MSG_REFUSED;

        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("hub.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let mut config = HubConfig::standard(48_000, [false; DESK_CHANNELS]);
        config.socket = socket;
        config.advertise = false;
        let (patches, callback) = patchbay(PATCH_CAPACITY);
        let (notices, received) = sync_channel(NOTICE_BACKLOG);
        let stats = Arc::new(HubStats::default());
        let mut server = Server::new(
            listener,
            None,
            config,
            Arc::new(SharedState::new()),
            patches,
            notices,
            &stats,
        );
        let (mut client, stream) = UnixStream::pair().unwrap();
        stream.set_nonblocking(true).unwrap();
        let peer = "203.0.113.5:7700".parse().unwrap();
        server.accept_tcp_peer(HubStream::Unix(stream), peer);
        assert!(server.pending.is_empty());
        assert!(server.instruments.is_empty());
        let mut frame = FrameBuffer::new();
        let header = frame.read_frame(&mut client).unwrap();
        assert_eq!(header.msg_type, MSG_REFUSED);
        let reason = std::str::from_utf8(&frame.payload()[..header.payload_len as usize]).unwrap();
        assert!(reason.contains("203.0.113.5:7700"), "{reason}");
        assert!(reason.contains("outside the local network"), "{reason}");
        assert!(
            matches!(received.recv().unwrap(), HubNotice::Refused { name, told: true, .. }
            if name == peer.to_string())
        );
        assert!(stats.refusals.lines(Instant::now())[0].contains("203.0.113.5:7700"));
        drop(callback);
    }

    #[test]
    fn listening_is_opt_in_and_bind_errors_name_the_address() {
        assert!(bind(None).unwrap().is_none());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let error = bind(Some(address)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
        assert!(error.to_string().contains(&address.to_string()));
    }

    #[test]
    fn local_network_ranges_include_the_house_and_tailscale() {
        for ip in [
            "127.0.0.1",
            "127.255.255.254",
            "10.0.0.0",
            "10.255.255.255",
            "172.16.0.0",
            "172.31.255.255",
            "192.168.0.0",
            "192.168.255.255",
            "169.254.0.0",
            "169.254.255.255",
            "100.64.0.0",
            "100.127.255.255",
            "::1",
            "fc00::",
            "fdff:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
            "fe80::",
            "febf:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
            "::ffff:192.168.1.1",
            "::ffff:192.168.1.5",
            "::ffff:100.64.0.1",
            "::ffff:127.0.0.1",
            "fd00::",
            "fd12:3456:789a::1",
            "fe80::1",
        ] {
            let peer = SocketAddr::new(ip.parse().unwrap(), 7700);
            assert_eq!(check_peer(peer), Ok(()), "{peer}");
        }
    }

    #[test]
    fn public_and_unspecified_peers_are_refused_with_their_address() {
        for ip in [
            "0.0.0.0",
            "8.8.8.8",
            "9.255.255.255",
            "11.0.0.0",
            "172.15.255.255",
            "172.32.0.0",
            "192.167.255.255",
            "192.169.0.0",
            "169.253.255.255",
            "169.255.0.0",
            "100.63.255.255",
            "100.128.0.0",
            "224.0.0.1",
            "::",
            "2001:4860:4860::8888",
            "fbff::",
            "fe00::",
            "fe7f::",
            "fec0::",
            "::ffff:8.8.8.8",
            "::ffff:172.32.0.0",
            "::ffff:100.128.0.0",
            "ff02::1",
        ] {
            let peer = SocketAddr::new(ip.parse().unwrap(), 7700);
            let error = check_peer(peer).unwrap_err().to_string();
            assert!(error.contains(&peer.to_string()), "{error}");
        }
    }
}
