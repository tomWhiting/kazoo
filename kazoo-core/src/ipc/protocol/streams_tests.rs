use super::*;

#[test]
fn stream_audio_head_round_trips_and_shares_offsets_with_audio() {
    let mut buf = [0xff; AUDIO_STREAM_PAYLOAD_HEADER];
    assert_eq!(
        encode_audio_stream_head(0x1234_5678_9abc_def0, 256, 15, &mut buf),
        16
    );
    assert_eq!(decode_audio_stream_frame(&buf), 0x1234_5678_9abc_def0);
    assert_eq!(decode_audio_frame_count(&buf), 256);
    assert_eq!(decode_audio_stream_id(&buf), 15);
    assert_eq!(&buf[14..16], &[0, 0]);
}
