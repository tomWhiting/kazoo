//! kazoo-arp — Jupiter-8 style arpeggiator library.
//!
//! Pure note scheduling logic: no audio, no DSP, no allocation in the tick path.
//! Embed this in any instrument crate (kazoo-mini, kazoo-cs80) or run standalone.
//!
//! # Architecture
//!
//! - [`Arpeggiator`]: Core state machine. Manages note pools, pattern index,
//!   octave spanning. Call [`Arpeggiator::step`] to advance and get a note event.
//! - [`ArpClock`]: Sample-accurate tick driver. Call [`ArpClock::tick`] once per
//!   audio sample — it handles step timing, swing, and gate duration.
//! - [`ClockDivision`]: Clock rate from 1/1 (whole) through 1/32 plus triplets.

pub mod clock;
pub mod engine;

pub use clock::{ArpClock, ClockDivision, TickEvents};
pub use engine::{ArpMode, Arpeggiator, HeldNote, NoteEvent};
