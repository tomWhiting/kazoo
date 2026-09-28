//! Inter-process communication for the Kazoo hub-instrument architecture.
//!
//! kazoo-mix is the studio's hub: it runs the desk server (in kazoo-mix
//! itself), and every instrument — the kazoo-tui voice synth included —
//! plugs into it through this module. kazoo-core provides the instrument
//! side of the connection, the wire protocol both sides speak, and the
//! socket discovery the hub uses to claim its address.
//!
//! # Architecture
//!
//! ```text
//! Instrument process                              Hub process (kazoo-mix)
//! ┌──────────────────────────────┐                ┌──────────────────────┐
//! │ audio callback               │                │ desk server          │
//! │  HubLinkAudio ── rings ──┐   │                │ (kazoo-mix/src/hub)  │
//! │                          ▼   │                │                      │
//! │ link thread: HubIpcClient ───┼──── UDS ───────┤                      │
//! │ UI: HubLink (status, notes)  │                │                      │
//! └──────────────────────────────┘                └──────────────────────┘
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
//! - [`discovery`] — Socket path resolution, claiming the hub socket, PID file
//!   management.
//! - [`client`] — Instrument-side connection to the hub.
//! - [`link`] — The real-time-safe split of an instrument's connection: a
//!   link thread that owns the client, and the audio callback's lock-free
//!   half.
//! - [`outbox`] — The lock-free audio queue between the two halves of a link.
//! - [`follow`] — Following the desk's transport to the exact frame.

pub mod client;
pub mod discovery;
pub mod follow;
pub mod link;
pub mod outbox;
pub mod protocol;
pub mod types;

// Re-export the primary public types for convenience.
pub use client::HubIpcClient;
