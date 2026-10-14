use super::*;
use crate::ipc::voice::{FEATURE_VOICE, encode_batch};
use crate::voice::VoiceFrame;

#[test]
fn desk_features_are_kept_and_voice_is_not_a_hub_message() {
    let (mut client, mut hub, mut buf) = tests::connected();
    FeaturesMsg {
        features: FEATURE_VOICE,
    }
    .encode(buf.payload_mut());
    buf.write_frame(MSG_DESK_FEATURES, 0, FeaturesMsg::WIRE_SIZE, &mut hub)
        .unwrap();
    let frame = VoiceFrame {
        at: 64,
        hz: 440.0,
        volts: 1.0,
        snapped_hz: 440.0,
        snapped_volts: 1.0,
        gate: true,
        onset: true,
        voiced: true,
        provisional: false,
        loudness: 0.5,
        breath: 0.25,
        confidence: 1.0,
    };
    let len = encode_batch(&[frame], buf.payload_mut());
    buf.write_frame(MSG_VOICE, 1, len, &mut hub).unwrap();
    NoteEventMsg {
        source: [1; 16],
        target: [0; 16],
        event_type: types::NOTE_ON,
        channel: 0,
        note: 60,
        velocity: 100,
    }
    .encode(buf.payload_mut());
    buf.write_frame(MSG_NOTE_EVENT, 2, NoteEventMsg::WIRE_SIZE, &mut hub)
        .unwrap();
    assert!(matches!(
        client.try_read().unwrap(),
        Some(HubRead::DeskFeatures)
    ));
    assert_eq!(client.take_desk_features(), Some(FEATURE_VOICE));
    assert_eq!(client.take_desk_features(), None);
    match client.try_read().unwrap() {
        Some(HubRead::Voice(payload)) => {
            let mut decoded = None;
            assert_eq!(
                super::super::voice::decode_batch(payload, |f| decoded = Some(f)),
                Ok(1)
            );
            assert_eq!(decoded, Some(frame));
        }
        other => panic!("expected borrowed voice bytes, received {other:?}"),
    }
    assert!(
        matches!(client.try_recv().unwrap(), Some(HubMessage::NoteEvent(note)) if note.note == 60)
    );
    assert!(client.try_recv().unwrap().is_none());
}

#[test]
fn an_invalid_feature_announcement_is_a_protocol_error() {
    for len in [0, FeaturesMsg::WIRE_SIZE - 1, FeaturesMsg::WIRE_SIZE + 1] {
        let (mut client, mut hub, mut buf) = tests::connected();
        buf.write_frame(MSG_DESK_FEATURES, 0, len, &mut hub)
            .unwrap();
        assert_eq!(
            client.try_read().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}

#[test]
fn ordinary_readers_skip_valid_voice_and_reject_bad_voice() {
    let (mut client, mut hub, mut buf) = tests::connected();
    let frame = VoiceFrame {
        at: 64,
        hz: 440.0,
        volts: 1.0,
        snapped_hz: 440.0,
        snapped_volts: 1.0,
        gate: true,
        onset: true,
        voiced: true,
        provisional: false,
        loudness: 0.5,
        breath: 0.25,
        confidence: 1.0,
    };
    let len = encode_batch(&[frame], buf.payload_mut());
    buf.write_frame(MSG_VOICE, 0, len, &mut hub).unwrap();
    buf.write_frame(MSG_SHUTDOWN, 1, 0, &mut hub).unwrap();
    assert!(matches!(
        client.try_recv().unwrap(),
        Some(HubMessage::Shutdown)
    ));
    buf.payload_mut()[..4].fill(0);
    buf.write_frame(MSG_VOICE, 0, 4, &mut hub).unwrap();
    assert_eq!(
        client.try_recv().unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
}
