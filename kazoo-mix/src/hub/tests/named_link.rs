//! A link that names its streams, against this desk.

use super::*;
use kazoo_core::host::link::connect_config_or_direct;
use kazoo_core::ipc::link::{HubAddress, LinkConfig, StreamConfig};

#[test]
fn a_link_with_two_streams_stays_plugged_in_and_its_first_stream_is_heard() {
    let mut rig = Rig::new([false; DESK_CHANNELS]);
    let (link, mut audio) = connect_config_or_direct(LinkConfig {
        address: HubAddress::Socket(rig.socket.clone()),
        streams: vec![
            StreamConfig {
                name: "main".to_string(),
                channels: 2,
            },
            StreamConfig {
                name: "pad".to_string(),
                channels: 2,
            },
        ],
        ..LinkConfig::new("kazoo-808", 2, RATE, 256)
    })
    .unwrap();
    rig.until("the link to plug in", |rig| {
        rig.shared.channel_readout(0).connected
    });
    // Stream 0 carries a steady level; stream 1 is silent, so the desk's
    // output is stream 0's level whether or not it seats stream 1.
    let main = vec![0.25_f32; 512];
    let pad = vec![0.0_f32; 512];
    let mut heard = false;
    for _ in 0..400 {
        audio.send_audio(256, &main);
        audio.send_stream_audio(1, 256, &pad);
        while audio.try_recv().is_some() {}
        thread::sleep(Duration::from_millis(1));
        if rig.render().iter().any(|s| (s - 0.25).abs() < 0.02) {
            heard = true;
            break;
        }
    }
    assert!(heard, "stream 0 never reached the desk output");
    for _ in 0..100 {
        audio.send_audio(256, &main);
        audio.send_stream_audio(1, 256, &pad);
        while audio.try_recv().is_some() {}
        thread::sleep(Duration::from_millis(1));
        rig.render();
    }
    let status = link.status();
    assert!(status.connected, "{status:?}");
    assert_eq!(status.connections, 1, "{status:?}");
    assert_eq!(rig.hub.snapshot().instruments, 1);
}
