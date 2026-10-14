use super::{InputProfile, SCORE_STEP, fixtures, instrument_frames, instrument_suite};

#[test]
fn candidate_and_onset_measurements() {
    use crate::analysis::cycle::{ProvisionalJudge, diagnostics};
    use crate::voice::{VoiceTracker, judge::JudgeResult};

    let takes = instrument_suite(fixtures::RATE);
    for (name, profile) in [
        ("bass_open", InputProfile::Bass),
        ("guitar_open", InputProfile::Guitar),
        ("sweep", InputProfile::Guitar),
    ] {
        let take = fixtures::named(&takes, name);
        let mut tracker = VoiceTracker::with_settings(take.rate, profile.tracking());
        let mut judge = ProvisionalJudge::new(take.rate, profile.tracking().cycle);
        for (at, &sample) in take.samples.iter().enumerate() {
            let note = take.onsets.partition_point(|&onset| onset <= at);
            let tracing = note > 0 && (name == "sweep" || at - take.onsets[note - 1] < 3000);
            diagnostics::enable(tracing);
            if take.onsets.contains(&at) {
                println!(
                    "NOTE take={name} note={} pluck={at} truth={:?}",
                    note - 1,
                    fixtures::truth_at(take, at + SCORE_STEP)
                );
            }
            if let Some(estimate) = judge.push(sample) {
                println!("WORKER take={name} at={at} estimate={estimate:?}");
                tracker.judge(JudgeResult::Provisional(estimate));
            }
            tracker.process(std::slice::from_ref(&sample), |frame| {
                if tracing && (frame.onset || frame.at % 512 == 0) {
                    println!("FRAME take={name} {frame:?}");
                }
            });
        }
        diagnostics::enable(false);
    }
}

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
