use super::*;
use crate::ipc::types::{FeaturesMsg, PROTOCOL_VERSION, VoiceFollowMsg};

const fn frame() -> VoiceFrame {
    VoiceFrame {
        at: 0x0807_0605_0403_0201,
        hz: 440.0,
        volts: 1.0,
        snapped_hz: 220.0,
        snapped_volts: -1.0,
        loudness: 0.5,
        breath: 0.25,
        confidence: 1.0,
        events: crate::voice::VoiceEvents {
            gate: true,
            onset: true,
        },
        voiced: true,
        provisional: false,
    }
}

fn bits(f: VoiceFrame) -> [u32; 7] {
    [
        f.hz,
        f.volts,
        f.snapped_hz,
        f.snapped_volts,
        f.loudness,
        f.breath,
        f.confidence,
    ]
    .map(f32::to_bits)
}

#[test]
fn a_frame_round_trips_bit_for_bit() {
    for flags in 0..16 {
        let original = VoiceFrame {
            events: crate::voice::VoiceEvents {
                gate: flags & 1 != 0,
                onset: flags & 2 != 0,
            },
            voiced: flags & 4 != 0,
            provisional: flags & 8 != 0,
            ..frame()
        };
        let mut bytes = [0xff; VOICE_FRAME_WIRE];
        encode_frame(&original, &mut bytes);
        let decoded = decode_frame(&bytes);
        assert_eq!(decoded.at, original.at);
        assert_eq!(bits(decoded), bits(original));
        assert_eq!(
            (
                decoded.gate,
                decoded.onset,
                decoded.voiced,
                decoded.provisional
            ),
            (
                original.gate,
                original.onset,
                original.voiced,
                original.provisional
            )
        );
        assert_eq!(&bytes[37..], &[0, 0, 0]);
    }
}

#[test]
fn nonsense_is_sanitised_on_read() {
    let mut bytes = [0; VOICE_FRAME_WIRE];
    encode_frame(
        &VoiceFrame {
            hz: f32::NAN,
            loudness: 7.0,
            volts: -40.0,
            ..frame()
        },
        &mut bytes,
    );
    let decoded = decode_frame(&bytes);
    assert_eq!(decoded.hz.to_bits(), 0.0_f32.to_bits());
    assert!(!decoded.voiced && !decoded.gate);
    assert_eq!(decoded.loudness.to_bits(), 1.0_f32.to_bits());
    assert_eq!(decoded.volts.to_bits(), (-8.0_f32).to_bits());
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let nonsense = VoiceFrame {
            hz: bad,
            volts: bad,
            snapped_hz: bad,
            snapped_volts: bad,
            loudness: bad,
            breath: bad,
            confidence: bad,
            ..frame()
        };
        encode_frame(&nonsense, &mut bytes);
        let decoded = decode_frame(&bytes);
        assert_eq!(bits(decoded), [0; 7]);
        assert!(!decoded.gate && !decoded.voiced);
        assert!(decoded.onset);
    }
    encode_frame(
        &VoiceFrame {
            hz: 90_000.0,
            snapped_hz: -10.0,
            volts: 40.0,
            snapped_volts: -40.0,
            loudness: -1.0,
            breath: 7.0,
            confidence: -9.0,
            ..frame()
        },
        &mut bytes,
    );
    assert_eq!(
        bits(decode_frame(&bytes)),
        [20_000.0_f32, 8.0, 0.0, -8.0, 0.0, 1.0, 0.0].map(f32::to_bits)
    );
}

#[test]
fn a_batch_of_32_round_trips_in_order() {
    let frames: Vec<_> = (0_u32..32)
        .map(|at| VoiceFrame {
            at: u64::from(at),
            ..frame()
        })
        .collect();
    let mut payload = [0xff; VOICE_BATCH_WIRE];
    let len = encode_batch(&frames, &mut payload);
    assert_eq!(len, 1284);
    assert_eq!(&payload[..4], &[32, 0, 0, 0]);
    let mut decoded = Vec::new();
    assert_eq!(decode_batch(&payload[..len], |f| decoded.push(f)), Ok(32));
    assert_eq!(decoded, frames);
}

#[test]
fn a_bad_batch_is_refused() {
    for (payload, error) in [
        (vec![0, 0, 0, 0], VoiceWireError::BadCount),
        (vec![33, 0, 0, 0], VoiceWireError::BadCount),
        (
            {
                let mut bytes = vec![0; 4 + 3 * 40];
                bytes[0] = 4;
                bytes
            },
            VoiceWireError::LengthMismatch,
        ),
        (vec![1, 0, 0], VoiceWireError::Truncated),
        (vec![], VoiceWireError::Truncated),
        (
            {
                let mut bytes = vec![0; 45];
                bytes[0] = 1;
                bytes
            },
            VoiceWireError::LengthMismatch,
        ),
    ] {
        let mut visits = 0;
        assert_eq!(decode_batch(&payload, |_| visits += 1), Err(error));
        assert_eq!(visits, 0, "invalid batches must not be partly delivered");
    }
}

#[test]
fn the_layout_is_pinned() {
    let mut bytes = [0xff; VOICE_FRAME_WIRE];
    encode_frame(&frame(), &mut bytes);
    assert_eq!(
        bytes,
        [
            1, 2, 3, 4, 5, 6, 7, 8, 0x00, 0x00, 0xdc, 0x43, 0x00, 0x00, 0x80, 0x3f, 0x00, 0x00,
            0x5c, 0x43, 0x00, 0x00, 0x80, 0xbf, 0x00, 0x00, 0x00, 0x3f, 0x00, 0x00, 0x80, 0x3e,
            0x00, 0x00, 0x80, 0x3f, 7, 0, 0, 0,
        ]
    );
}

#[test]
fn reserved_bytes_and_flag_bits_are_ignored() {
    let mut payload = [0; VOICE_BATCH_WIRE];
    let len = encode_batch(&[frame()], &mut payload);
    payload[1..4].fill(0xff);
    payload[4 + 36] |= 0xf0;
    payload[4 + 37..len].fill(0xff);
    let mut decoded = None;
    assert_eq!(decode_batch(&payload[..len], |f| decoded = Some(f)), Ok(1));
    assert_eq!(decoded, Some(frame()));
}

#[test]
fn an_invalid_encode_writes_nothing() {
    for frames in [vec![], vec![frame(); VOICE_BATCH_MAX + 1], vec![frame()]] {
        let mut bytes = [0xaa; 4];
        assert_eq!(encode_batch(&frames, &mut bytes), 0);
        assert_eq!(bytes, [0xaa; 4]);
    }
}

#[test]
fn features_and_follow_have_their_pinned_shapes() {
    assert_eq!(PROTOCOL_VERSION, 3);
    assert_eq!(
        (MSG_DESK_FEATURES, MSG_VOICE, MSG_VOICE_FOLLOW),
        (0x24, 0x31, 0x32)
    );
    assert_eq!(FEATURE_VOICE, 1);
    let msg = FeaturesMsg {
        features: 0x0804_0201,
    };
    let mut features = [0; FeaturesMsg::WIRE_SIZE];
    msg.encode(&mut features);
    assert_eq!(features, [1, 2, 4, 8]);
    assert_eq!(FeaturesMsg::decode(&features), msg);
    for on in [false, true] {
        let msg = VoiceFollowMsg { on };
        let mut bytes = [0; VoiceFollowMsg::WIRE_SIZE];
        msg.encode(&mut bytes);
        assert_eq!(bytes, [u8::from(on)]);
        assert_eq!(VoiceFollowMsg::decode(&bytes), msg);
    }
}
