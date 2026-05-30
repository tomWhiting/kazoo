//! IPC integration for kazoo-808.
//!
//! Connects to the kazoo-tui hub via Unix domain sockets. If the hub is
//! not running, the instrument falls back to standalone mode with its
//! own audio output.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use kazoo_core::ipc::client::HubIpcClient;

/// Connection state for the hub IPC link.
pub struct HubLink {
    client: Option<HubIpcClient>,
    connected: Arc<AtomicBool>,
}

impl std::fmt::Debug for HubLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubLink")
            .field("connected", &self.is_connected())
            .finish_non_exhaustive()
    }
}

impl HubLink {
    /// Instrument name sent during registration.
    const NAME: &'static str = "kazoo-808";

    /// Maximum time to spend attempting hub connection before falling back
    /// to standalone mode. Keeps app startup responsive.
    const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

    /// Attempt to connect to the hub with a timeout. Returns a connected
    /// link if the hub responds within [`Self::CONNECT_TIMEOUT`], or a
    /// disconnected link for standalone mode. Never blocks the caller for
    /// more than the timeout duration.
    #[must_use]
    pub fn new(channel_count: u8, sample_rate: u32, buffer_size: u32) -> Self {
        let connected = Arc::new(AtomicBool::new(false));

        let (tx, rx) = std::sync::mpsc::channel();
        let _handle = std::thread::Builder::new()
            .name("kazoo-808-ipc-connect".into())
            .spawn(move || {
                let result =
                    HubIpcClient::connect(Self::NAME, channel_count, sample_rate, buffer_size);
                let _ = tx.send(result);
            });

        let client = rx
            .recv_timeout(Self::CONNECT_TIMEOUT)
            .ok()
            .and_then(Result::ok);

        match client {
            Some(c) => {
                connected.store(true, Ordering::Release);
                Self {
                    client: Some(c),
                    connected,
                }
            }
            None => Self {
                client: None,
                connected,
            },
        }
    }

    /// Whether we are connected to the hub.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    /// Send an audio block to the hub (hot path — zero allocations).
    ///
    /// Returns `true` if sent successfully, `false` on error (which also
    /// marks the link as disconnected).
    pub fn send_audio(&mut self, frame_count: u32, samples: &[f32]) -> bool {
        let Some(client) = self.client.as_mut() else {
            return false;
        };
        if client.send_audio(frame_count, samples).is_ok() {
            true
        } else {
            self.connected.store(false, Ordering::Release);
            false
        }
    }

    /// Poll for messages from the hub (non-blocking).
    ///
    /// Returns transport sync, note events, or shutdown requests.
    pub fn try_recv(&mut self) -> Option<kazoo_core::ipc::client::HubMessage> {
        let client = self.client.as_mut()?;
        if let Ok(msg) = client.try_recv() {
            msg
        } else {
            self.connected.store(false, Ordering::Release);
            None
        }
    }

    /// Send a clean shutdown notification to the hub.
    pub fn shutdown(&mut self) {
        if let Some(client) = self.client.as_mut() {
            let _ = client.send_shutdown();
        }
        self.connected.store(false, Ordering::Release);
    }
}

impl Drop for HubLink {
    fn drop(&mut self) {
        self.shutdown();
    }
}
