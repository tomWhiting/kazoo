use super::*;

pub fn first_song_tempo(bpm: f32) -> f32 {
    let rig = Rig::with_shared([false; DESK_CHANNELS], SharedState::with_tempo(bpm));
    let mut client = rig.connect("kazoo-test", RATE).unwrap();
    client.send_audio(0, 256, &[0.0; 512]).unwrap();
    loop {
        match client.try_recv().unwrap() {
            Some(HubMessage::TransportSync(sync)) => return sync.bpm,
            Some(_) => {}
            None => thread::yield_now(),
        }
    }
}
