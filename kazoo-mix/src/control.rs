//! Control server scaffold for `kazoo-mix`.
//!
//! The control server owns sockets and registration work outside the audio
//! callback. This first slice only creates/listens on the session socket and
//! counts accepted connections; protocol handling will be layered here, never in
//! the callback.

use std::fs;
use std::io;
use std::os::unix::net::UnixListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use color_eyre::Result;

use crate::session::MixSession;

/// Background control server handle.
#[derive(Debug)]
pub struct ControlServer {
    stats: Arc<ControlStats>,
    running: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}

impl ControlServer {
    /// Start the control server for a session.
    pub fn start(session: &MixSession) -> Result<Self> {
        if session.control_socket.exists() {
            fs::remove_file(&session.control_socket)?;
        }

        let listener = UnixListener::bind(&session.control_socket)?;
        listener.set_nonblocking(true)?;

        let stats = Arc::new(ControlStats::default());
        let running = Arc::new(AtomicBool::new(true));
        let thread_stats = Arc::clone(&stats);
        let thread_running = Arc::clone(&running);
        let join = thread::Builder::new()
            .name("kazoo-mix-control".to_string())
            .spawn(move || run_control_loop(&listener, &thread_running, &thread_stats))?;

        Ok(Self {
            stats,
            running,
            join: Some(join),
        })
    }

    /// Snapshot of control server stats.
    #[must_use]
    pub fn snapshot(&self) -> ControlSnapshot {
        self.stats.snapshot()
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn run_control_loop(listener: &UnixListener, running: &AtomicBool, stats: &ControlStats) {
    while running.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((_stream, _addr)) => {
                stats.accepted_connections.fetch_add(1, Ordering::Relaxed);
                // Protocol handshake and client registry live here next. For
                // now we close immediately rather than holding unknown clients.
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => {
                stats.accept_errors.fetch_add(1, Ordering::Relaxed);
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

#[derive(Debug, Default)]
struct ControlStats {
    accepted_connections: AtomicU64,
    accept_errors: AtomicU64,
}

impl ControlStats {
    fn snapshot(&self) -> ControlSnapshot {
        ControlSnapshot {
            accepted_connections: self.accepted_connections.load(Ordering::Relaxed),
            accept_errors: self.accept_errors.load(Ordering::Relaxed),
        }
    }
}

/// UI-safe control server stats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlSnapshot {
    /// Connections accepted since server start.
    pub accepted_connections: u64,
    /// Accept-loop errors since server start.
    pub accept_errors: u64,
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixStream;
    use std::thread;
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn control_server_accepts_local_connection() {
        let session = MixSession::create_default().unwrap();
        let server = ControlServer::start(&session).unwrap();

        UnixStream::connect(&session.control_socket).unwrap();

        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if server.snapshot().accepted_connections > 0 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "control server did not accept connection"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}
