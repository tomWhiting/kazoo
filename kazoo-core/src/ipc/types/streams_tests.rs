use super::*;

#[test]
fn register_with_three_streams_round_trips() {
    let mut msg = RegisterMsg::new("test", 2, 48_000, 256);
    msg.streams = vec![
        StreamSpec::new("kick", 1),
        StreamSpec::new("pad", 2),
        StreamSpec::new("lead", 2),
    ];
    let mut buf = vec![0; RegisterMsg::streamed_wire_size(3)];
    assert_eq!(msg.encode(&mut buf), 139);
    assert_eq!(buf[48], 1);
    assert_eq!(buf[63], 3);
    let decoded = RegisterMsg::decode(&buf);
    assert_eq!(decoded.streams, msg.streams);
    assert_eq!(decoded.channel_count, 1);
    assert!(RegisterMsg::valid_len(buf.len()));
    for n in 1..=MAX_STREAMS {
        assert!(RegisterMsg::valid_len(RegisterMsg::streamed_wire_size(n)));
    }
    for len in [0, 58, 60, 62, 64, 65, 88, 465, 489] {
        assert!(!RegisterMsg::valid_len(len));
    }
}

#[test]
fn a_register_without_a_stream_list_is_one_stream_called_main() {
    let msg = RegisterMsg::new("old", 2, 48_000, 256);
    let mut buf = [0; RegisterMsg::PACED_WIRE_SIZE];
    msg.encode(&mut buf);
    for len in [59, 63] {
        let decoded = RegisterMsg::decode(&buf[..len]);
        assert_eq!(decoded.streams, vec![StreamSpec::new("main", 2)]);
        assert!(RegisterMsg::valid_len(len));
    }
}

#[test]
fn check_streams_refuses_a_duplicate_name_three_channels_and_seventeen_streams() {
    let mut msg = RegisterMsg::new("test", 2, 48_000, 256);
    for streams in [
        vec![],
        vec![StreamSpec::new("a", 1), StreamSpec::new("a", 2)],
        vec![StreamSpec::new("a", 3)],
        vec![StreamSpec::new("", 1)],
        vec![StreamSpec::new("a", 1); 17],
    ] {
        msg.streams = streams;
        assert!(msg.check_streams().is_err());
    }
    msg.streams = vec![StreamSpec::new("kick", 1), StreamSpec::new("pad", 2)];
    assert_eq!(msg.check_streams(), Ok(()));
    msg.streams[0].name[0] = 0xff;
    assert!(msg.check_streams().is_err());
}

#[test]
fn registered_with_strips_round_trips_and_an_old_reply_is_one_strip() {
    let msg = RegisteredMsg {
        strip_index: 2,
        hub_sample_rate: 48_000,
        hub_buffer_size: 256,
        transport_state: 0,
        bpm: 120.0,
        position: 0,
        strips: vec![2, NO_STRIP, 5],
    };
    let mut buf = [0; 26];
    assert_eq!(msg.encode(&mut buf), 26);
    assert_eq!(RegisteredMsg::decode(&buf).strips, msg.strips);
    assert_eq!(RegisteredMsg::decode(&buf[..22]).strips, vec![2]);
}

#[test]
fn stream_add_remove_and_ready_round_trip() {
    let add = StreamAddMsg {
        stream: 0x1234,
        spec: StreamSpec::new("lead", 2),
    };
    let mut buf = [0; StreamAddMsg::WIRE_SIZE];
    assert_eq!(add.encode(&mut buf), 27);
    assert_eq!(StreamAddMsg::decode(&buf), add);
    let remove = StreamRemoveMsg { stream: 15 };
    assert_eq!(remove.encode(&mut buf), 2);
    assert_eq!(StreamRemoveMsg::decode(&buf), remove);
    for (strip, reason) in [(6, ""), (NO_STRIP, "every strip is taken")] {
        let ready = StreamReadyMsg { stream: 1, strip };
        ready.encode(&mut buf);
        let mut payload = buf[..3].to_vec();
        payload.extend_from_slice(reason.as_bytes());
        assert_eq!(StreamReadyMsg::decode(&payload), (ready, reason));
    }
}

#[test]
fn stream_names_are_truncated_at_a_utf8_boundary() {
    let name = format!("{}é", "a".repeat(23));
    let spec = StreamSpec::new(&name, 1);
    assert_eq!(spec.name_str(), "a".repeat(23));
}
