use super::{InputProfile, SCORE_STEP, fixtures, instrument_suite};

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
            if let Some(pitch) = judge.pitch() {
                tracker.judge(pitch);
            }
            tracker.process(std::slice::from_ref(&sample), |frame| {
                judge.observe(frame);
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
        ("bass_open_noise", InputProfile::Bass),
        ("guitar_open_noise", InputProfile::Guitar),
    ] {
        let take = fixtures::named(&takes, name);
        let mut tracker = crate::voice::VoiceTracker::with_settings(take.rate, profile.tracking());
        let mut judge =
            crate::analysis::cycle::ProvisionalJudge::new(take.rate, profile.tracking().cycle);
        let mut frames = Vec::new();
        let mut readings = Vec::new();
        for (at, &sample) in take.samples.iter().enumerate() {
            if let Some(estimate) = judge.push(sample) {
                readings.push((at + 1, estimate));
                tracker.judge(crate::voice::judge::JudgeResult::Provisional(estimate));
            }
            if let Some(pitch) = judge.pitch() {
                tracker.judge(pitch);
            }
            tracker.process(std::slice::from_ref(&sample), |frame| {
                judge.observe(frame);
                frames.push((at + 1, frame));
            });
        }
        for (note, &onset) in take.onsets.iter().enumerate() {
            let end = fixtures::note_end(take, note);
            let truth = fixtures::truth_at(take, onset + SCORE_STEP).expect("note truth");
            let first = frames.iter().find(|(_, frame)| {
                frame.at >= onset as u64
                    && frame.at < end as u64
                    && frame.voiced
                    && frame.gate
                    && fixtures::cents(frame.hz, truth) <= 50.0
            });
            let latency = first.map(|(_, frame)| (frame.at - onset as u64) as f32 / 48.0);
            let delivered = frames
                .iter()
                .find(|(at, frame)| {
                    *at >= onset
                        && *at < end
                        && frame.voiced
                        && frame.gate
                        && fixtures::cents(frame.hz, truth) <= 50.0
                })
                .map(|(at, _)| (*at - onset) as f32 / 48.0);
            let candidate = readings.iter().find(|(at, _)| *at >= onset && *at < end);
            println!(
                "{name} note={note} delivered_ms={delivered:?} first_provisional={candidate:?}"
            );
            println!(
                "{name} note={note} hz={truth:.5} after_ms={latency:?} limit_ms={:.5}",
                if name.ends_with("_noise") {
                    2500.0
                } else {
                    1500.0
                } / truth
                    + 2.0
            );
        }
        let provisional = frames.iter().filter(|(_, frame)| frame.provisional).count();
        println!("{name}: provisional_frames={provisional}");
    }
}

#[test]
fn motion_following_measurements() {
    let takes = instrument_suite(fixtures::RATE);
    for profile in [InputProfile::Guitar, InputProfile::Bass] {
        for name in ["slide", "bend", "vibrato", "glide"] {
            let take = fixtures::named(&takes, name);
            let mut delays = [0.0_f32; 2];
            for (early, delay) in [false, true].into_iter().zip(&mut delays) {
                let mut settings = profile.tracking();
                settings.cycle.early = early;
                let mut tracker = crate::voice::VoiceTracker::with_settings(take.rate, settings);
                let mut judge =
                    crate::analysis::cycle::ProvisionalJudge::new(take.rate, settings.cycle);
                let mut frames = Vec::new();
                let mut onsets = 0;
                for (at, &sample) in take.samples.iter().enumerate() {
                    if let Some(estimate) = judge.push(sample) {
                        tracker.judge(crate::voice::judge::JudgeResult::Provisional(estimate));
                    }
                    if let Some(pitch) = judge.pitch() {
                        tracker.judge(pitch);
                    }
                    tracker.process(std::slice::from_ref(&sample), |frame| {
                        judge.observe(frame);
                        onsets += usize::from(frame.onset);
                        let time = (at + 1 - take.onsets[0].min(at + 1)) as f32 / take.rate as f32;
                        let moving = match name {
                            "bend" => (0.22..0.35).contains(&time) || (0.67..0.8).contains(&time),
                            "slide" => (0.22..0.6).contains(&time) || (0.62..1.0).contains(&time),
                            _ => time > 0.2,
                        };
                        if frame.voiced && moving && fixtures::truth_at(take, at + 1).is_some() {
                            frames.push((at + 1, frame.hz));
                        }
                    });
                }
                let (lag, error) = (0..=2400)
                    .step_by(32)
                    .map(|lag| {
                        let error = frames
                            .iter()
                            .map(|&(at, hz)| {
                                let truth = fixtures::truth_at(take, at.saturating_sub(lag))
                                    .expect("motion truth");
                                let cents = 1200.0 * (hz / truth).log2();
                                f64::from(cents * cents)
                            })
                            .sum::<f64>()
                            / frames.len() as f64;
                        (lag, error)
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .expect("lag search");
                *delay = lag as f32 / 48.0;
                println!(
                    "FOLLOW profile={profile:?} take={name} early={early} following_ms={delay:.6} moving_frames={} onsets={onsets} rms_cents={:.6}",
                    frames.len(),
                    error.sqrt()
                );
            }
            println!(
                "MOTION profile={profile:?} take={name} baseline_ms={:.6} final_ms={:.6}",
                delays[0], delays[1]
            );
        }
    }
}
