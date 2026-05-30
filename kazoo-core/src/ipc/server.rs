//! IPC server: hub-side listener for instrument connections.
//!
//! [`HubIpcServer`] runs a background thread that accepts instrument
//! connections, reads audio frames, and pushes per-instrument audio into
//! ring buffers that the hub's output callback consumes. Transport state
//! changes from the output callback are forwarded to all instruments.
//!
//! # Thread model
//!
//! The server runs a single polling thread:
//! 1. Non-blocking accept on the Unix listener.
//! 2. Blocking registration handshake (short timeout) for new connections.
//! 3. Non-blocking read of audio/control messages from all instruments.
//! 4. Drain transport sync notifications from the output callback.
//! 5. Forward transport changes to all connected instruments.
//! 6. Sleep 200 microseconds between poll cycles (well below the ~2.9ms
//!    buffer period at 128 samples / 44.1 kHz).

use std::io;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::Sender;
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Producer, Split};

use super::protocol::{self, FrameBuffer};
use super::types::{
    self, IpcTransportNotify, MSG_AUDIO, MSG_NOTE_EVENT, MSG_REGISTER, MSG_REGISTERED,
    MSG_SHUTDOWN, MSG_TRANSPORT_REQUEST, MSG_TRANSPORT_SYNC, RegisterMsg, RegisteredMsg,
    TRANSPORT_STOPPED, TransportSyncMsg,
};

/// Maximum number of concurrent instrument connections.
pub const MAX_INSTRUMENTS: usize = 16;

/// Sleep duration between poll cycles in the server thread.
const POLL_INTERVAL: Duration = Duration::from_micros(200);

/// Timeout for the registration handshake with a new instrument.
const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(2);

/// Ring buffer capacity per instrument (in f32 samples).
/// 8 blocks of stereo audio provides ample headroom for timing jitter.
const INSTRUMENT_RB_BLOCKS: usize = 8;

/// Maximum audio callback frame count accepted from instruments. Must be
/// at least as large as the `MAX_CALLBACK_FRAMES` constant in each
/// instrument crate (currently 4096). Used to size the per-instrument
/// decode scratch buffer and hold-last-value block.
const MAX_INSTRUMENT_FRAMES: usize = 4096;

// ---------------------------------------------------------------------------
// IpcInstrumentConsumer
// ---------------------------------------------------------------------------

/// Audio consumer handle for one connected instrument.
///
/// Sent from the IPC server thread to the output callback via a crossbeam
/// channel. The output callback reads audio from `audio_cons` and mixes
/// it into the master bus. If the ring buffer has insufficient data, the
/// callback uses `last_block` (hold-last-value).
pub struct IpcInstrumentConsumer {
    /// Ring buffer consumer for this instrument's audio.
    pub audio_cons: ringbuf::HeapCons<f32>,
    /// Number of audio channels (1 or 2).
    pub channel_count: u8,
    /// Assigned mixer strip index.
    pub strip_index: u8,
    /// Instrument name for display. Uses `Arc<str>` so the clone in the
    /// audio callback's display snapshot is a cheap ref-count bump.
    pub name: Arc<str>,
    /// Hold-last-value buffer (stereo, `buffer_size * 2` samples).
    pub last_block: Vec<f32>,
    /// Connection status flag. Set to `false` by the server thread when
    /// the instrument disconnects.
    pub connected: Arc<AtomicBool>,
}

// HeapCons is not Debug; implement manually.
impl std::fmt::Debug for IpcInstrumentConsumer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IpcInstrumentConsumer")
            .field("channel_count", &self.channel_count)
            .field("strip_index", &self.strip_index)
            .field("name", &self.name)
            .field("connected", &self.connected.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// InstrumentSlot (server-internal)
// ---------------------------------------------------------------------------

/// Server-side state for one connected instrument.
struct InstrumentSlot {
    stream: UnixStream,
    read_buf: FrameBuffer,
    write_buf: FrameBuffer,
    #[allow(dead_code)]
    instrument_id: [u8; 16],
    #[allow(dead_code)]
    name: String,
    #[allow(dead_code)]
    strip_index: u8,
    channel_count: u8,
    audio_prod: ringbuf::HeapProd<f32>,
    /// Pre-allocated scratch for decoding audio samples from wire bytes.
    audio_scratch: Vec<f32>,
    seq: u32,
    connected: Arc<AtomicBool>,
}

// ---------------------------------------------------------------------------
// HubIpcServer
// ---------------------------------------------------------------------------

/// Hub-side IPC server managing all instrument connections.
///
/// Start with [`HubIpcServer::start`], which spawns a background thread.
/// Shut down by calling [`shutdown`](Self::shutdown) or dropping the server
/// (which calls shutdown automatically).
pub struct HubIpcServer {
    thread: Option<JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
}

impl std::fmt::Debug for HubIpcServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubIpcServer")
            .field("running", &self.thread.is_some())
            .finish_non_exhaustive()
    }
}

impl HubIpcServer {
    /// Start the IPC server on the default socket path.
    ///
    /// # Arguments
    ///
    /// * `sample_rate` — Hub's audio sample rate.
    /// * `buffer_size` — Hub's audio buffer size in samples.
    /// * `instrument_tx` — Channel to send new [`IpcInstrumentConsumer`]
    ///   handles to the output callback when instruments connect.
    /// * `transport_cons` — Ring buffer to receive [`IpcTransportNotify`]
    ///   from the output callback for forwarding to instruments.
    pub fn start(
        sample_rate: u32,
        buffer_size: usize,
        instrument_tx: Sender<IpcInstrumentConsumer>,
        transport_cons: ringbuf::HeapCons<IpcTransportNotify>,
    ) -> io::Result<Self> {
        let socket_path = super::discovery::default_socket_path();
        Self::start_at(
            &socket_path,
            sample_rate,
            buffer_size,
            instrument_tx,
            transport_cons,
        )
    }

    /// Start the IPC server at the given socket path.
    ///
    /// This is the lower-level entry point used by [`start`](Self::start)
    /// and by tests that need an explicit path.
    pub fn start_at(
        socket_path: &std::path::Path,
        sample_rate: u32,
        buffer_size: usize,
        instrument_tx: Sender<IpcInstrumentConsumer>,
        transport_cons: ringbuf::HeapCons<IpcTransportNotify>,
    ) -> io::Result<Self> {
        // Ensure runtime directory exists.
        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Remove stale socket file.
        super::discovery::remove_socket(socket_path);

        let listener = UnixListener::bind(socket_path)?;
        listener.set_nonblocking(true)?;

        // Write PID file for instrument discovery.
        super::discovery::write_pid_file(socket_path)?;

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);
        let socket_path_owned = socket_path.to_path_buf();

        let thread = thread::Builder::new()
            .name("kazoo-ipc".into())
            .spawn(move || {
                server_thread(
                    listener,
                    sample_rate,
                    buffer_size,
                    instrument_tx,
                    transport_cons,
                    shutdown_clone,
                );
                // Cleanup on exit.
                super::discovery::remove_socket(&socket_path_owned);
                super::discovery::remove_pid_file();
            })
            .map_err(|e| io::Error::other(format!("spawn failed: {e}")))?;

        Ok(Self {
            thread: Some(thread),
            shutdown,
        })
    }

    /// Signal the server to shut down and wait for the thread to exit.
    pub fn shutdown(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for HubIpcServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ---------------------------------------------------------------------------
// Server thread
// ---------------------------------------------------------------------------

#[allow(clippy::needless_pass_by_value)] // Owned values moved into thread via `move` closure.
fn server_thread(
    listener: UnixListener,
    sample_rate: u32,
    buffer_size: usize,
    instrument_tx: Sender<IpcInstrumentConsumer>,
    mut transport_cons: ringbuf::HeapCons<IpcTransportNotify>,
    shutdown: Arc<AtomicBool>,
) {
    let mut connections: Vec<InstrumentSlot> = Vec::with_capacity(MAX_INSTRUMENTS);
    let mut last_transport = IpcTransportNotify {
        state: TRANSPORT_STOPPED,
        bpm: 120.0,
        position_samples: 0,
        timestamp_nanos: 0,
    };

    while !shutdown.load(Ordering::Acquire) {
        // 1. Accept new connections.
        accept_connections(
            &listener,
            &mut connections,
            sample_rate,
            buffer_size,
            &instrument_tx,
        );

        // 2. Read messages from all connected instruments.
        read_instrument_messages(&mut connections);

        // 3. Drain transport sync notifications and forward to instruments.
        forward_transport(&mut transport_cons, &mut connections, &mut last_transport);

        // 4. Remove disconnected slots (swap_remove for O(1)).
        connections.retain(|slot| slot.connected.load(Ordering::Relaxed));

        thread::sleep(POLL_INTERVAL);
    }

    // Shutdown: send Shutdown message to all instruments.
    for slot in &mut connections {
        let _ = slot
            .write_buf
            .write_frame(MSG_SHUTDOWN, slot.seq, 0, &mut &slot.stream);
    }
}

// ---------------------------------------------------------------------------
// Accept new connections
// ---------------------------------------------------------------------------

fn accept_connections(
    listener: &UnixListener,
    connections: &mut Vec<InstrumentSlot>,
    sample_rate: u32,
    buffer_size: usize,
    instrument_tx: &Sender<IpcInstrumentConsumer>,
) {
    // Accept up to a few connections per poll cycle to avoid starving
    // the audio read loop.
    for _ in 0..4 {
        let stream = match listener.accept() {
            Ok((s, _)) => s,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(_) => break,
        };

        if connections.len() >= MAX_INSTRUMENTS {
            // At capacity — reject by dropping the stream.
            drop(stream);
            continue;
        }

        let strip_index = allocate_strip_index(connections);
        if let Ok(slot) =
            handle_registration(stream, strip_index, sample_rate, buffer_size, instrument_tx)
        {
            connections.push(slot);
        }
    }
}

/// Find the lowest strip index not currently in use.
///
/// Scans the live connections to find a free slot, guaranteeing that no
/// two concurrent instruments share an index. Bounded by `MAX_INSTRUMENTS`
/// (16), so the scan is trivially fast.
fn allocate_strip_index(connections: &[InstrumentSlot]) -> u8 {
    let mut used = [false; MAX_INSTRUMENTS];
    for slot in connections {
        let idx = usize::from(slot.strip_index);
        if idx < MAX_INSTRUMENTS {
            used[idx] = true;
        }
    }
    for (i, &occupied) in used.iter().enumerate() {
        if !occupied {
            #[allow(clippy::cast_possible_truncation)]
            return i as u8;
        }
    }
    // Unreachable: accept_connections checks len < MAX_INSTRUMENTS before
    // calling this function, so there is always a free slot.
    0
}

fn handle_registration(
    stream: UnixStream,
    strip_index: u8,
    sample_rate: u32,
    buffer_size: usize,
    instrument_tx: &Sender<IpcInstrumentConsumer>,
) -> io::Result<InstrumentSlot> {
    // Use blocking I/O with a short timeout for the handshake.
    stream.set_read_timeout(Some(REGISTRATION_TIMEOUT))?;
    stream.set_write_timeout(Some(REGISTRATION_TIMEOUT))?;

    let mut read_buf = FrameBuffer::new();
    let header = read_buf.read_frame(&mut &stream)?;

    if header.msg_type != MSG_REGISTER {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected Register (0x01), got 0x{:02X}", header.msg_type),
        ));
    }

    let reg = RegisterMsg::decode(read_buf.payload());

    // Send Registered response.
    let resp = RegisteredMsg {
        strip_index,
        hub_sample_rate: sample_rate,
        hub_buffer_size: u32::try_from(buffer_size).unwrap_or(u32::MAX),
        transport_state: TRANSPORT_STOPPED,
        bpm: 120.0,
        position: 0,
    };
    let mut write_buf = FrameBuffer::new();
    resp.encode(write_buf.payload_mut());
    write_buf.write_frame(MSG_REGISTERED, 0, RegisteredMsg::WIRE_SIZE, &mut &stream)?;

    // Switch to non-blocking for the audio loop.
    stream.set_nonblocking(true)?;

    // Create per-instrument ring buffer.
    let ch = usize::from(reg.channel_count.max(1));
    let block_size = buffer_size.max(MAX_INSTRUMENT_FRAMES);
    let rb_capacity = block_size * ch * INSTRUMENT_RB_BLOCKS;
    let rb = HeapRb::<f32>::new(rb_capacity);
    let (prod, cons) = rb.split();

    let connected = Arc::new(AtomicBool::new(true));
    let name: Arc<str> = reg.name_str().into();

    // Send the consumer to the output callback.
    let consumer = IpcInstrumentConsumer {
        audio_cons: cons,
        channel_count: reg.channel_count,
        strip_index,
        name: name.clone(),
        last_block: vec![0.0f32; MAX_INSTRUMENT_FRAMES * 2], // Stereo, worst-case callback size.
        connected: Arc::clone(&connected),
    };
    instrument_tx.send(consumer).map_err(|_| {
        io::Error::new(io::ErrorKind::BrokenPipe, "instrument channel disconnected")
    })?;

    let audio_scratch = vec![0.0f32; MAX_INSTRUMENT_FRAMES * ch];

    Ok(InstrumentSlot {
        stream,
        read_buf,
        write_buf,
        instrument_id: reg.instrument_id,
        name: name.to_string(),
        strip_index,
        channel_count: reg.channel_count,
        audio_prod: prod,
        audio_scratch,
        seq: 1,
        connected,
    })
}

// ---------------------------------------------------------------------------
// Read instrument messages
// ---------------------------------------------------------------------------

fn read_instrument_messages(connections: &mut [InstrumentSlot]) {
    for slot in connections.iter_mut() {
        if !slot.connected.load(Ordering::Relaxed) {
            continue;
        }

        // Read up to several frames per cycle to keep up with audio rate.
        for _ in 0..8 {
            match slot.read_buf.try_read_frame(&mut &slot.stream) {
                Ok(Some(header)) => {
                    handle_instrument_frame(slot, &header);
                }
                Ok(None) => break,
                Err(_) => {
                    slot.connected.store(false, Ordering::Relaxed);
                    break;
                }
            }
        }
    }
}

fn handle_instrument_frame(slot: &mut InstrumentSlot, header: &protocol::FrameHeader) {
    match header.msg_type {
        MSG_AUDIO => {
            let payload = slot.read_buf.payload();
            let frame_count = protocol::decode_audio_frame_count(payload) as usize;
            let sample_count = frame_count * usize::from(slot.channel_count.max(1));
            let count = sample_count.min(slot.audio_scratch.len());

            protocol::decode_audio_samples(payload, &mut slot.audio_scratch, count);
            let _ = slot.audio_prod.push_slice(&slot.audio_scratch[..count]);
        }
        MSG_TRANSPORT_REQUEST => {
            // Acknowledged but not yet forwarded. Implementing this requires
            // passing a Sender<EngineCommand> into the server thread and
            // converting TransportRequestMsg to TransportCommand variants.
            let _req = types::TransportRequestMsg::decode(slot.read_buf.payload());
        }
        MSG_NOTE_EVENT => {
            // Acknowledged but not yet routed. Implementing this requires
            // looking up the target instrument's slot by UUID and forwarding
            // the note event to the matching connection.
            let _event = types::NoteEventMsg::decode(slot.read_buf.payload());
        }
        MSG_SHUTDOWN => {
            slot.connected.store(false, Ordering::Relaxed);
        }
        _ => {
            // Unknown message type — skip.
        }
    }
}

// ---------------------------------------------------------------------------
// Transport sync forwarding
// ---------------------------------------------------------------------------

fn forward_transport(
    transport_cons: &mut ringbuf::HeapCons<IpcTransportNotify>,
    connections: &mut [InstrumentSlot],
    last_transport: &mut IpcTransportNotify,
) {
    // Drain all available notifications, keeping the latest.
    let mut changed = false;
    while let Some(notify) = transport_cons.try_pop() {
        if notify.state != last_transport.state
            || (notify.bpm - last_transport.bpm).abs() > f32::EPSILON
        {
            changed = true;
        }
        *last_transport = notify;
    }

    if !changed {
        return;
    }

    // Convert to wire message and send to all instruments.
    let sync = TransportSyncMsg {
        state: last_transport.state,
        bpm: last_transport.bpm,
        position: last_transport.position_samples,
        timestamp: last_transport.timestamp_nanos,
    };

    for slot in connections.iter_mut() {
        if !slot.connected.load(Ordering::Relaxed) {
            continue;
        }
        sync.encode(slot.write_buf.payload_mut());
        let seq = slot.seq;
        slot.seq = slot.seq.wrapping_add(1);
        // Best-effort: if write fails (e.g. WouldBlock), skip this update.
        let _ = slot.write_buf.write_frame(
            MSG_TRANSPORT_SYNC,
            seq,
            TransportSyncMsg::WIRE_SIZE,
            &mut &slot.stream,
        );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ringbuf::traits::Observer;
    use std::path::PathBuf;

    /// Create a unique temporary socket path for a test.
    fn test_socket_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("kazoo-test");
        let _ = std::fs::create_dir_all(&dir);
        dir.join(format!("{name}-{}.sock", std::process::id()))
    }

    #[test]
    fn register_registered_audio_loop() {
        let socket_path = test_socket_path("ipc-loop");

        // Clean up any stale socket.
        let _ = std::fs::remove_file(&socket_path);

        // Create the instrument channel.
        let (instrument_tx, instrument_rx) =
            crossbeam_channel::bounded::<IpcInstrumentConsumer>(16);

        // Create the transport ring buffer.
        let transport_rb = HeapRb::<IpcTransportNotify>::new(16);
        let (_transport_prod, transport_cons) = transport_rb.split();

        // Start the server.
        let mut server =
            HubIpcServer::start_at(&socket_path, 44_100, 128, instrument_tx, transport_cons)
                .expect("server should start");

        // Give the server thread time to bind.
        thread::sleep(Duration::from_millis(50));

        // Connect a client.
        let mut client = super::super::client::HubIpcClient::connect_to(
            &socket_path,
            "test-808",
            2,
            44_100,
            128,
        )
        .expect("client should connect");

        assert_eq!(client.strip_index(), 0);
        assert_eq!(client.hub_sample_rate(), 44_100);
        assert_eq!(client.hub_buffer_size(), 128);

        // Verify the instrument consumer was sent to the output callback.
        let consumer = instrument_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("should receive instrument consumer");
        assert_eq!(consumer.strip_index, 0);
        assert_eq!(consumer.channel_count, 2);
        assert_eq!(&*consumer.name, "test-808");

        // Send several audio blocks.
        let samples: Vec<f32> = (0..256).map(|i| (i as f32) * 0.001).collect();
        for _ in 0..4 {
            client
                .send_audio(128, &samples)
                .expect("audio send should succeed");
        }

        // Give the server time to read and push audio.
        thread::sleep(Duration::from_millis(50));

        // Read from the consumer's ring buffer.
        let available = consumer.audio_cons.occupied_len();
        assert!(
            available > 0,
            "ring buffer should have audio data, got {available} samples"
        );

        // Send shutdown and verify clean teardown.
        client
            .send_shutdown()
            .expect("shutdown send should succeed");
        thread::sleep(Duration::from_millis(50));

        server.shutdown();

        // Clean up.
        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn multiple_instruments_connect() {
        let socket_path = test_socket_path("ipc-multi");
        let _ = std::fs::remove_file(&socket_path);

        let (instrument_tx, instrument_rx) =
            crossbeam_channel::bounded::<IpcInstrumentConsumer>(16);
        let transport_rb = HeapRb::<IpcTransportNotify>::new(16);
        let (_transport_prod, transport_cons) = transport_rb.split();

        let mut server =
            HubIpcServer::start_at(&socket_path, 44_100, 128, instrument_tx, transport_cons)
                .expect("server should start");

        thread::sleep(Duration::from_millis(50));

        // Connect two instruments with a small gap to avoid racing the
        // server's accept loop (fixes pre-existing BrokenPipe flake).
        let client1 =
            super::super::client::HubIpcClient::connect_to(&socket_path, "inst-1", 1, 44_100, 128)
                .expect("client 1 should connect");

        thread::sleep(Duration::from_millis(50));

        let client2 =
            super::super::client::HubIpcClient::connect_to(&socket_path, "inst-2", 2, 44_100, 128)
                .expect("client 2 should connect");

        // They should get different strip indices.
        assert_eq!(client1.strip_index(), 0);
        assert_eq!(client2.strip_index(), 1);

        // Both consumers should arrive.
        let c1 = instrument_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let c2 = instrument_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(&*c1.name, "inst-1");
        assert_eq!(&*c2.name, "inst-2");

        server.shutdown();
        let _ = std::fs::remove_file(&socket_path);
    }
}
