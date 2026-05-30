//! Inter-process communication for the Kazoo hub-instrument architecture.
//!
//! This module provides the full IPC layer that connects instrument crates
//! (kazoo-808, kazoo-mini, kazoo-cs80, kazoo-arp) to the kazoo-tui hub.
//!
//! # Architecture
//!
//! ```text
//! Instrument Process          Hub Process (kazoo-tui)
//! ┌──────────────┐           ┌─────────────────────────────────┐
//! │  HubIpcClient │──UDS────│  HubIpcServer                   │
//! │  send_audio() │         │  ├── per-instrument ring buffer  │
//! │  try_recv()   │         │  └── transport sync producer     │
//! └──────────────┘           └──────────┬──────────────────────┘
//!                                        │ crossbeam channel
//!                                        ▼
//!                            ┌─────────────────────────────────┐
//!                            │  Output Callback                │
//!                            │  mix_ipc_instruments()          │
//!                            │  → master bus                   │
//!                            └─────────────────────────────────┘
//! ```
//!
//! # Wire Protocol
//!
//! Binary framed messages over Unix domain sockets. 9-byte header
//! (1 type + 4 length LE + 4 sequence LE) followed by variable payload.
//! Zero allocations on the audio hot path.
//!
//! # Modules
//!
//! - [`protocol`] — Frame encoding/decoding, non-blocking read state machine.
//! - [`types`] — Message type definitions (Register, Audio, `TransportSync`, etc.).
//! - [`discovery`] — Socket path resolution, PID file management.
//! - [`client`] — Instrument-side connection to the hub.
//! - [`server`] — Hub-side listener managing all instrument connections.

pub mod client;
pub mod discovery;
pub mod protocol;
pub mod server;
pub mod types;

// Re-export the primary public types for convenience.
pub use client::HubIpcClient;
pub use server::{HubIpcServer, IpcInstrumentConsumer, MAX_INSTRUMENTS};
pub use types::IpcTransportNotify;
