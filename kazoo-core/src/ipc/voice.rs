//! Batched voice control frames, separate from audio and note messages.

use crate::voice::VoiceFrame;

/// Hub to instrument: capabilities announced after registration.
pub const MSG_DESK_FEATURES: u8 = 0x24;
/// The desk carries voice control frames and follow requests.
pub const FEATURE_VOICE: u32 = 1;
/// Either direction: a batch of voice control frames.
pub const MSG_VOICE: u8 = 0x31;
/// Instrument to hub: begin or end following the voice.
pub const MSG_VOICE_FOLLOW: u8 = 0x32;
/// Bytes occupied by one voice frame, including reserved bytes.
pub const VOICE_FRAME_WIRE: usize = 40;
/// Most frames carried by one message.
pub const VOICE_BATCH_MAX: usize = 32;
/// Largest voice payload, including its four-byte batch header.
pub const VOICE_BATCH_WIRE: usize = 4 + VOICE_BATCH_MAX * VOICE_FRAME_WIRE;

/// A voice payload cannot be visited until its whole layout is valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum VoiceWireError {
    /// The batch header is incomplete.
    #[error("truncated voice batch header")]
    Truncated,
    /// A batch must contain between one and 32 frames.
    #[error("voice batch count is outside 1..=32")]
    BadCount,
    /// The count does not describe the exact payload length.
    #[error("voice batch length does not match its count")]
    LengthMismatch,
}

/// Write the frame's exact bits; reserved bytes and flag bits are zero.
pub fn encode_frame(f: &VoiceFrame, out: &mut [u8; VOICE_FRAME_WIRE]) {
    out.fill(0);
    out[..8].copy_from_slice(&f.at.to_le_bytes());
    let values = [
        f.hz,
        f.volts,
        f.snapped_hz,
        f.snapped_volts,
        f.loudness,
        f.breath,
        f.confidence,
    ];
    for (value, bytes) in values.into_iter().zip(out[8..36].chunks_exact_mut(4)) {
        bytes.copy_from_slice(&value.to_le_bytes());
    }
    out[36] = u8::from(f.gate)
        | (u8::from(f.onset) << 1)
        | (u8::from(f.voiced) << 2)
        | (u8::from(f.provisional) << 3);
}

const fn float_at(b: &[u8; VOICE_FRAME_WIRE], offset: usize) -> f32 {
    f32::from_le_bytes([b[offset], b[offset + 1], b[offset + 2], b[offset + 3]])
}

const fn bounded(value: f32, low: f32, high: f32) -> f32 {
    if value.is_finite() {
        value.clamp(low, high)
    } else {
        0.0
    }
}

/// Read a frame, sanitising every float before it reaches an audio callback.
#[must_use]
pub const fn decode_frame(b: &[u8; VOICE_FRAME_WIRE]) -> VoiceFrame {
    let hz = float_at(b, 8);
    VoiceFrame {
        at: u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
        hz: bounded(hz, 0.0, 20_000.0),
        volts: bounded(float_at(b, 12), -8.0, 8.0),
        snapped_hz: bounded(float_at(b, 16), 0.0, 20_000.0),
        snapped_volts: bounded(float_at(b, 20), -8.0, 8.0),
        loudness: bounded(float_at(b, 24), 0.0, 1.0),
        breath: bounded(float_at(b, 28), 0.0, 1.0),
        confidence: bounded(float_at(b, 32), 0.0, 1.0),
        events: crate::voice::VoiceEvents {
            gate: b[36] & 1 != 0 && hz.is_finite(),
            onset: b[36] & 2 != 0,
        },
        voiced: b[36] & 4 != 0 && hz.is_finite(),
        provisional: b[36] & 8 != 0 && hz.is_finite(),
    }
}

/// Encode one batch. Returns zero without writing if the count is invalid or
/// the destination is too short; otherwise returns the payload length.
pub fn encode_batch(frames: &[VoiceFrame], out: &mut [u8]) -> usize {
    if frames.is_empty() || frames.len() > VOICE_BATCH_MAX {
        return 0;
    }
    let len = 4 + frames.len() * VOICE_FRAME_WIRE;
    if out.len() < len {
        return 0;
    }
    let Ok(count) = u8::try_from(frames.len()) else {
        return 0;
    };
    out[..4].copy_from_slice(&[count, 0, 0, 0]);
    for (frame, bytes) in frames
        .iter()
        .zip(out[4..len].chunks_exact_mut(VOICE_FRAME_WIRE))
    {
        let mut encoded = [0; VOICE_FRAME_WIRE];
        encode_frame(frame, &mut encoded);
        bytes.copy_from_slice(&encoded);
    }
    len
}

/// Validate the entire batch, then visit sanitised frames in wire order.
pub fn decode_batch(
    payload: &[u8],
    mut visit: impl FnMut(VoiceFrame),
) -> Result<usize, VoiceWireError> {
    if payload.len() < 4 {
        return Err(VoiceWireError::Truncated);
    }
    let count = usize::from(payload[0]);
    if !(1..=VOICE_BATCH_MAX).contains(&count) {
        return Err(VoiceWireError::BadCount);
    }
    if payload.len() != 4 + count * VOICE_FRAME_WIRE {
        return Err(VoiceWireError::LengthMismatch);
    }
    for bytes in payload[4..].chunks_exact(VOICE_FRAME_WIRE) {
        let mut frame = [0; VOICE_FRAME_WIRE];
        frame.copy_from_slice(bytes);
        visit(decode_frame(&frame));
    }
    Ok(count)
}

#[cfg(test)]
mod tests;
