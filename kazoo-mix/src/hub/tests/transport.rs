use super::*;
use kazoo_core::ipc::protocol::{AUDIO_PAYLOAD_HEADER, encode_audio_payload};
use kazoo_core::ipc::types::MSG_AUDIO;

struct Writer<'a>(&'a mut HubIpcClient);
impl std::io::Write for Writer<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        loop {
            match self.0.write_audio_bytes(bytes) {
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => thread::yield_now(),
                result => return result,
            }
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn raw_audio(client: &mut HubIpcClient, stream_frame: u64, frames: u32) {
    let samples = vec![0.25; frames as usize * 2];
    let mut buffer = FrameBuffer::new();
    let len = encode_audio_payload(stream_frame, frames, &samples, buffer.payload_mut());
    buffer
        .write_frame(MSG_AUDIO, 0, len, &mut Writer(client))
        .unwrap();
}

#[test]
fn an_instrument_sending_8192_frame_blocks_is_heard() {
    let mut rig = Rig::new([false; DESK_CHANNELS]);
    let register = RegisterMsg::new("kazoo-test", 2, RATE, 8_192);
    let mut client = HubIpcClient::register_at(&rig.socket, &register).unwrap();
    assert!(matches!(rig.notice(), HubNotice::Joined { .. }));
    rig.until("strip attached", |rig| {
        rig.shared.channel_readout(0).connected
    });
    raw_audio(&mut client, 0, 8_192);
    rig.until("large block heard", |rig| {
        rig.render().iter().any(|s| (*s - 0.25).abs() < 0.02)
    });
    assert!(rig.hub.snapshot().blocks > 0);
}

#[test]
fn an_instrument_whose_blocks_would_not_fit_its_strip_twice_is_refused() {
    let mut rig = Rig::new([false; DESK_CHANNELS]);
    let register = RegisterMsg::new("kazoo-test", 2, RATE, 32_768);
    let refused = HubIpcClient::register_at(&rig.socket, &register).unwrap_err();
    assert!(
        refused.to_string().contains("blocks of 32768 frames"),
        "{refused}"
    );
    assert!(matches!(
        rig.notice(),
        HubNotice::Refused {
            reason: RefuseReason::BlockTooLarge {
                frames: 32_768,
                most: 16_384
            },
            ..
        }
    ));
}

#[test]
fn a_version_2_instrument_is_told_to_rebuild() {
    let mut rig = Rig::new([false; DESK_CHANNELS]);
    let mut register = RegisterMsg::new("kazoo-test", 2, RATE, 256);
    register.protocol = 2;
    let refused = HubIpcClient::register_at(&rig.socket, &register).unwrap_err();
    let text = refused.to_string();
    assert!(
        text.contains("protocol 2") && text.contains('3') && text.contains("rebuild"),
        "{text}"
    );
    assert!(matches!(rig.notice(), HubNotice::Refused { .. }));
}

#[test]
fn extra_lead_is_held_to_the_instruments_own_block_size() {
    let mut rig = Rig::new([false; DESK_CHANNELS]);
    let mut leads = Vec::new();
    let mut clients = Vec::new();
    for frames in [256, 16_384] {
        let mut register = RegisterMsg::new("kazoo-test", 2, RATE, frames);
        register.pace_lead_frames = 4_800;
        let mut client = HubIpcClient::register_at(&rig.socket, &register).unwrap();
        assert!(matches!(rig.notice(), HubNotice::Joined { .. }));
        raw_audio(&mut client, 0, frames);
        let mut found = None;
        rig.until("pace", |rig| {
            rig.render();
            while client.try_recv().unwrap().is_some() {}
            found = client.take_desk_pace();
            found.is_some()
        });
        leads.push(found.unwrap().lead_frames);
        clients.push(client);
    }
    assert_eq!(clients.len(), 2);
    assert_eq!(rig.hub.snapshot().instruments, 2);
    let small_base = 256 + 256 + 144;
    let large_base = 256 + 16_384 + 144;
    assert!(leads[0] >= small_base + 4_800, "{leads:?}");
    let half = ring_frames(strip_ring(0, 2)) as u32 / 2;
    assert_eq!(
        leads[1],
        large_base + half.saturating_sub(large_base),
        "{leads:?}"
    );
    assert_eq!(AUDIO_PAYLOAD_HEADER, 12);
}
