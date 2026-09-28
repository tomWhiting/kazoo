//! MIDI input thread: discovers and connects to USB MIDI controllers.
//!
//! Uses `midir` for cross-platform MIDI I/O. On macOS, this goes through
//! Core MIDI and works out of the box with class-compliant USB devices.
//!
//! The thread automatically connects to the first available MIDI input port.
//! Received MIDI messages are parsed and converted to [`EngineCommand`]
//! variants, then sent to the engine via the command channel.

use std::sync::Arc;

use crossbeam_channel::Sender;
use midir::{MidiInput, MidiInputConnection};

use super::command::EngineCommand;
use super::stats::EngineStats;

/// Name used for the midir client.
const CLIENT_NAME: &str = "kazoo-midi";

/// Active MIDI connection handle. Dropping this stops the MIDI callback.
///
/// Held by the engine to keep the MIDI thread alive for the engine's
/// lifetime. The midir callback runs on its own OS thread and sends
/// parsed MIDI commands through the engine's command channel.
pub struct MidiHandle {
    /// The active midir connection. Dropping disconnects.
    _connection: MidiInputConnection<()>,
    /// Name of the connected MIDI port.
    port_name: String,
}

impl MidiHandle {
    /// The name of the connected MIDI port.
    #[must_use]
    pub fn port_name(&self) -> &str {
        &self.port_name
    }
}

impl std::fmt::Debug for MidiHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MidiHandle")
            .field("port", &self.port_name)
            .finish_non_exhaustive()
    }
}

/// Attempt to connect to the first available MIDI input port.
///
/// Returns `Some(MidiHandle)` if a port was found and connected, or `None`
/// if no MIDI devices are available. MIDI messages received on the port
/// are parsed and sent as [`EngineCommand`] variants through `command_tx`;
/// messages that do not fit in the (bounded) command channel are counted in
/// `stats`.
///
/// This function is non-blocking — the midir callback runs on a dedicated
/// OS thread managed by the midir/Core MIDI runtime.
#[must_use]
pub fn connect_first_port(
    command_tx: Sender<EngineCommand>,
    stats: Arc<EngineStats>,
) -> Option<MidiHandle> {
    let midi_in = match MidiInput::new(CLIENT_NAME) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("MIDI: failed to create input client: {e}");
            return None;
        }
    };

    let ports = midi_in.ports();
    if ports.is_empty() {
        eprintln!("MIDI: no input ports found");
        return None;
    }

    // Log all available ports for debugging.
    for (i, p) in ports.iter().enumerate() {
        let name = midi_in.port_name(p).unwrap_or_else(|_| "?".into());
        eprintln!("MIDI: port {i}: {name}");
    }

    // Pick the first available port.
    let port = &ports[0];
    let port_name = midi_in.port_name(port).unwrap_or_else(|_| "Unknown".into());

    let connection = match midi_in.connect(
        port,
        "kazoo-midi-in",
        move |_timestamp_us, message, _data| {
            // A disconnected channel means the engine is shutting down and
            // this connection is about to be dropped; a full one loses the
            // event, which is counted.
            let Some(cmd) = parse_midi_message(message) else {
                return;
            };
            if let Err(crossbeam_channel::TrySendError::Full(_)) = command_tx.try_send(cmd) {
                stats.command_dropped();
            }
        },
        (),
    ) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("MIDI: failed to connect to port '{port_name}': {e}");
            return None;
        }
    };

    Some(MidiHandle {
        _connection: connection,
        port_name,
    })
}

/// List all available MIDI input port names.
///
/// Useful for UI display and device selection.
///
/// # Errors
///
/// Returns [`crate::Error::AudioDevice`] if the MIDI client cannot be
/// created or a port's name cannot be read (e.g. the device was unplugged
/// while enumerating).
pub fn list_input_ports() -> crate::Result<Vec<String>> {
    let midi_in = MidiInput::new(CLIENT_NAME)
        .map_err(|e| crate::Error::AudioDevice(format!("MIDI client: {e}")))?;
    midi_in
        .ports()
        .iter()
        .map(|p| {
            midi_in
                .port_name(p)
                .map_err(|e| crate::Error::AudioDevice(format!("MIDI port name: {e}")))
        })
        .collect()
}

/// Parse a raw MIDI message (1-3 bytes) into an [`EngineCommand`].
///
/// Handles `NoteOn`, `NoteOff`, `ControlChange`, and `PitchBend`.
/// Returns `None` for unsupported message types (`SysEx`, clock, etc.).
fn parse_midi_message(data: &[u8]) -> Option<EngineCommand> {
    if data.is_empty() {
        return None;
    }

    let status = data[0];
    let msg_type = status & 0xF0;
    let channel = status & 0x0F;

    match msg_type {
        // Note Off: 0x80
        0x80 if data.len() >= 3 => Some(EngineCommand::MidiNoteOff {
            note: data[1] & 0x7F,
            channel,
        }),

        // Note On: 0x90
        0x90 if data.len() >= 3 => {
            let note = data[1] & 0x7F;
            let velocity = data[2] & 0x7F;
            // Velocity 0 on a Note On is treated as Note Off (MIDI convention).
            if velocity == 0 {
                Some(EngineCommand::MidiNoteOff { note, channel })
            } else {
                Some(EngineCommand::MidiNoteOn {
                    note,
                    velocity,
                    channel,
                })
            }
        }

        // Control Change: 0xB0
        0xB0 if data.len() >= 3 => Some(EngineCommand::MidiCC {
            cc: data[1] & 0x7F,
            value: data[2] & 0x7F,
            channel,
        }),

        // Pitch Bend: 0xE0 (14-bit: LSB in data[1], MSB in data[2])
        0xE0 if data.len() >= 3 => {
            let lsb = u16::from(data[1] & 0x7F);
            let msb = u16::from(data[2] & 0x7F);
            let value = (msb << 7) | lsb;
            Some(EngineCommand::MidiPitchBend { value, channel })
        }

        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_note_on() {
        let msg = [0x90, 60, 100]; // Note On, C4, velocity 100
        let cmd = parse_midi_message(&msg).unwrap();
        assert!(matches!(
            cmd,
            EngineCommand::MidiNoteOn {
                note: 60,
                velocity: 100,
                channel: 0
            }
        ));
    }

    #[test]
    fn parse_note_on_velocity_zero_is_note_off() {
        let msg = [0x90, 60, 0]; // Note On with velocity 0 = Note Off
        let cmd = parse_midi_message(&msg).unwrap();
        assert!(matches!(
            cmd,
            EngineCommand::MidiNoteOff {
                note: 60,
                channel: 0
            }
        ));
    }

    #[test]
    fn parse_note_off() {
        let msg = [0x80, 60, 64]; // Note Off, C4
        let cmd = parse_midi_message(&msg).unwrap();
        assert!(matches!(
            cmd,
            EngineCommand::MidiNoteOff {
                note: 60,
                channel: 0
            }
        ));
    }

    #[test]
    fn parse_note_on_channel_5() {
        let msg = [0x95, 72, 127]; // Note On, channel 5, C5, velocity 127
        let cmd = parse_midi_message(&msg).unwrap();
        assert!(matches!(
            cmd,
            EngineCommand::MidiNoteOn {
                note: 72,
                velocity: 127,
                channel: 5
            }
        ));
    }

    #[test]
    fn parse_control_change() {
        let msg = [0xB0, 1, 64]; // CC1 (mod wheel), value 64
        let cmd = parse_midi_message(&msg).unwrap();
        assert!(matches!(
            cmd,
            EngineCommand::MidiCC {
                cc: 1,
                value: 64,
                channel: 0
            }
        ));
    }

    #[test]
    fn parse_pitch_bend_center() {
        let msg = [0xE0, 0x00, 0x40]; // Pitch bend center (8192)
        let cmd = parse_midi_message(&msg).unwrap();
        assert!(matches!(
            cmd,
            EngineCommand::MidiPitchBend {
                value: 8192,
                channel: 0
            }
        ));
    }

    #[test]
    fn parse_pitch_bend_max() {
        let msg = [0xE0, 0x7F, 0x7F]; // Pitch bend max (16383)
        let cmd = parse_midi_message(&msg).unwrap();
        assert!(matches!(
            cmd,
            EngineCommand::MidiPitchBend {
                value: 16383,
                channel: 0
            }
        ));
    }

    #[test]
    fn parse_pitch_bend_min() {
        let msg = [0xE0, 0x00, 0x00]; // Pitch bend min (0)
        let cmd = parse_midi_message(&msg).unwrap();
        assert!(matches!(
            cmd,
            EngineCommand::MidiPitchBend {
                value: 0,
                channel: 0
            }
        ));
    }

    #[test]
    fn parse_empty_returns_none() {
        assert!(parse_midi_message(&[]).is_none());
    }

    #[test]
    fn parse_short_message_returns_none() {
        assert!(parse_midi_message(&[0x90]).is_none());
        assert!(parse_midi_message(&[0x90, 60]).is_none());
    }

    #[test]
    fn parse_sysex_returns_none() {
        assert!(parse_midi_message(&[0xF0, 0x7E, 0x7F, 0xF7]).is_none());
    }

    #[test]
    fn parse_clock_returns_none() {
        assert!(parse_midi_message(&[0xF8]).is_none());
    }

    #[test]
    fn high_bits_masked_to_7bit() {
        // Data bytes should be masked to 7 bits (0-127).
        let msg = [0x90, 0xFF, 0xFF];
        let cmd = parse_midi_message(&msg).unwrap();
        assert!(matches!(
            cmd,
            EngineCommand::MidiNoteOn {
                note: 127,
                velocity: 127,
                channel: 0
            }
        ));
    }

    #[test]
    fn list_input_ports_returns_vec() {
        // Actual ports depend on hardware (CI may have no MIDI subsystem);
        // either outcome must be well-formed.
        match list_input_ports() {
            Ok(ports) => assert!(ports.len() < 1000), // sanity check
            Err(e) => assert!(matches!(e, crate::Error::AudioDevice(_)), "{e}"),
        }
    }
}
