use super::{InputProfile, SCORE_STEP, fixtures, instrument_frames, instrument_suite};

#[test]
fn provisional_arrival_measurements() {
    let takes = instrument_suite(fixtures::RATE);
    for (name, profile) in [
        ("bass_open", InputProfile::Bass),
        ("guitar_open", InputProfile::Guitar),
    ] {
        let take = fixtures::named(&takes, name);
        let frames = instrument_frames(take, profile, true);
        for (note, &onset) in take.onsets.iter().enumerate() {
            let end = fixtures::note_end(take, note);
            let truth = fixtures::truth_at(take, onset + SCORE_STEP).expect("note truth");
            let first = frames.iter().find(|frame| {
                frame.at >= onset as u64
                    && frame.at < end as u64
                    && frame.voiced
                    && frame.gate
                    && fixtures::cents(frame.hz, truth) <= 50.0
            });
            let latency = first.map(|frame| (frame.at - onset as u64) as f32 / 48.0);
            println!(
                "{name} note={note} hz={truth:.5} after_ms={latency:?} limit_ms={:.5}",
                1500.0 / truth + 2.0
            );
        }
        let provisional = frames.iter().filter(|frame| frame.provisional).count();
        println!("{name}: provisional_frames={provisional}");
    }
}
