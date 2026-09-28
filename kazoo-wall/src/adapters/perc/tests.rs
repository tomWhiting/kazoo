//! Percussion tests: every voice and rhythm generator in the engine without
//! allocating, triggers on their exact sample, velocity, accent and choke.

use std::time::Instant;

use crate::catalogue::Kind;
use crate::daemon::wall::Wall;
use crate::dsp::testing::Bench;
use crate::engine::{EngineConfig, engine};
use crate::patch::Patch;
use crate::protocol::{Request, What};

/// Run `f` as audio-thread code, failing the test if it allocates or frees.
fn on_audio_thread<T>(f: impl FnOnce() -> T) -> T {
    let before = assert_no_alloc::violation_count();
    let result = assert_no_alloc::assert_no_alloc(f);
    assert_eq!(
        assert_no_alloc::violation_count(),
        before,
        "the audio thread allocated or freed memory"
    );
    result
}

fn family(name: &str) -> Vec<Kind> {
    Kind::all()
        .filter(|kind| kind.spec().family == name)
        .collect()
}

fn added(patch: &mut Patch, kind: &str) -> String {
    match patch.add(kind, None, None).unwrap() {
        What::Add { module } => module.id,
        other => panic!("{other:?}"),
    }
}

/// Render `patch` for `seconds` on the audio thread; returns the peak.
fn render(patch: Patch, seconds: usize) -> f32 {
    let (mut engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let mut wall = Wall::new(patch, control, None, Vec::new(), None);
    let mut buffer = vec![0.0_f32; 4_800 * 2];
    let mut peak = 0.0_f32;
    for _ in 0..seconds * 10 {
        on_audio_thread(|| engine.render(&mut buffer, 2));
        assert!(buffer.iter().all(|s| s.is_finite()));
        peak = buffer.iter().fold(peak, |m, s| m.max(s.abs()));
        wall.tick(Instant::now());
    }
    peak
}

#[test]
fn every_voice_sounds_in_the_engine_without_allocating() {
    let voices = family("perc");
    assert_eq!(voices.len(), kazoo_perc::catalogue().count());
    for kind in voices {
        let mut patch = Patch::empty(120.0);
        let clock = added(&mut patch, "clock");
        let voice = added(&mut patch, kind.name());
        let out = added(&mut patch, "out");
        patch.turn(&clock, "division", 2.0, Some(0.0)).unwrap();
        patch.turn(&out, "level", 1.0, Some(0.0)).unwrap();
        patch
            .plug(
                &format!("{clock}.out"),
                &format!("{voice}.trigger"),
                None,
                None,
            )
            .unwrap();
        patch
            .plug(&format!("{voice}.out"), &format!("{out}.left"), None, None)
            .unwrap();
        let peak = render(patch, 1);
        assert!(peak > 0.001, "{kind} is silent");
    }
}

#[test]
fn every_rhythm_steps_in_the_engine_without_allocating() {
    let rhythms = family("rhythm");
    assert_eq!(rhythms.len(), kazoo_perc::rhythms().count());
    for kind in rhythms {
        let spec = kind.spec();
        let mut patch = Patch::empty(120.0);
        let clock = added(&mut patch, "clock");
        let rhythm = added(&mut patch, kind.name());
        let out = added(&mut patch, "out");
        patch.turn(&clock, "division", 0.0, Some(0.0)).unwrap();
        patch.turn(&out, "level", 1.0, Some(0.0)).unwrap();
        patch
            .plug(
                &format!("{clock}.out"),
                &format!("{rhythm}.clock"),
                None,
                None,
            )
            .unwrap();
        // Every lane strikes a voice of its own, all into one out.
        let mix = added(&mut patch, "mix");
        for (lane, input) in spec.outputs.iter().zip(["a", "b", "c", "d"]) {
            let voice = added(&mut patch, "hat");
            patch
                .plug(
                    &format!("{rhythm}.{}", lane.name),
                    &format!("{voice}.trigger"),
                    None,
                    None,
                )
                .unwrap();
            patch
                .plug(
                    &format!("{voice}.out"),
                    &format!("{mix}.{input}"),
                    None,
                    None,
                )
                .unwrap();
        }
        patch
            .plug(&format!("{mix}.out"), &format!("{out}.left"), None, None)
            .unwrap();
        let peak = render(patch, 2);
        assert!(peak > 0.001, "{kind} never struck anything");
    }
}

/// A trigger rising at `frame` of the first sub-block, held high.
fn strike_at(kind: &str, frame: usize, level: f32) -> Vec<f32> {
    let mut bench = Bench::new(Kind::from_name(kind).unwrap());
    bench.render_fed(0, 4_800, Some("trigger"), |at| {
        if at >= frame { level } else { 0.0 }
    })
}

/// `kind`'s voice built and struck directly at `frame` with `velocity`,
/// 4 800 frames at the bench's rate.
fn struck_directly(kind: &str, frame: usize, velocity: f32) -> Vec<f32> {
    let voice_kind = kazoo_perc::catalogue()
        .find(|voice| voice.id == kind)
        .unwrap();
    let mut voice = (voice_kind.build)();
    voice.prepare(crate::dsp::testing::RATE);
    let mut out = vec![0.0; 4_800];
    // In sub-blocks, as the wall runs it.
    let mut from = 0;
    while from < out.len() {
        let to = (from - from % 32 + 32).min(out.len());
        let to = if from < frame && frame < to {
            frame
        } else {
            to
        };
        if from == frame {
            voice.trigger(velocity, false);
        }
        voice.process(&mut out[from..to]);
        from = to;
    }
    out
}

fn bits(samples: &[f32]) -> Vec<u32> {
    samples.iter().map(|sample| sample.to_bits()).collect()
}

#[test]
fn a_hit_lands_on_the_sample_its_trigger_rises() {
    for frame in [0, 1, 13, 31, 32, 45] {
        assert_eq!(
            bits(&strike_at("kick", frame, 1.0)),
            bits(&struck_directly("kick", frame, 1.0)),
            "struck at {frame}"
        );
    }
}

#[test]
fn a_soft_hit_is_as_soft_as_its_gate_is_low() {
    // A gate is high above 0.5: the part above is the velocity, so 0.75 is
    // half, and a gate only just high is barely there.
    assert_eq!(
        bits(&strike_at("kick", 0, 0.75)),
        bits(&struck_directly("kick", 0, 0.5))
    );
    // 0.55 is not exact in f32: its part above 0.5, doubled, is a hair
    // over 0.1, and that is the velocity.
    assert_eq!(
        bits(&strike_at("kick", 0, 0.55)),
        bits(&struck_directly("kick", 0, (0.55_f32 - 0.5) * 2.0))
    );
    assert_eq!(
        bits(&strike_at("kick", 0, 3.0)),
        bits(&struck_directly("kick", 0, 1.0))
    );
}

#[test]
fn the_trigger_s_level_is_the_velocity_and_accent_plays_harder() {
    let peak = |samples: &[f32]| samples.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
    let soft = peak(&strike_at("snare", 0, 0.6));
    let full = peak(&strike_at("snare", 0, 1.0));
    let beyond = peak(&strike_at("snare", 0, 5.0));
    assert!(soft < full, "{soft} {full}");
    assert!(
        (beyond - full).abs() < 1e-6,
        "a gate above 1 is full velocity"
    );

    let mut bench = Bench::new(Kind::from_name("snare").unwrap());
    bench.hold("accent", 1.0);
    let accented = peak(&bench.render_fed(0, 4_800, Some("trigger"), |_| 1.0));
    assert!(accented > full, "{accented} {full}");
}

#[test]
fn choke_damps_a_ringing_voice() {
    let mut bench = Bench::new(Kind::from_name("cymbal").unwrap());
    let ringing = bench.render_fed(0, 4_800, Some("trigger"), |_| 1.0);
    assert!(ringing[4_000..].iter().any(|s| s.abs() > 0.001));
    let choked = bench.render_fed(0, 4_800, Some("choke"), |_| 1.0);
    // A few milliseconds to fade, then silence.
    assert!(
        choked[1_000..].iter().all(|s| s.abs() < 1e-4),
        "still ringing"
    );
}

#[test]
fn nonsense_in_the_gates_is_ignored() {
    for kind in family("perc").into_iter().chain(family("rhythm")) {
        crate::dsp::testing::survives_nonsense(kind);
    }
}

#[test]
fn voices_and_rhythms_are_patchable_by_their_ports() {
    let (_engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let mut wall = Wall::new(Patch::empty(120.0), control, None, Vec::new(), None);
    let now = Instant::now();
    for kind in ["kick", "euclid"] {
        let add = Request::Add {
            kind: kind.to_string(),
            name: None,
            place: None,
        };
        wall.request("Tom", &add, now).unwrap();
    }
    let patch = Request::Patch {
        from: "euclid1.gate".to_string(),
        to: "kick1.trigger".to_string(),
        amount: None,
    };
    assert!(wall.request("Tom", &patch, now).is_ok());
    let look = wall.snapshot();
    let kick = look.modules.iter().find(|m| m.id == "kick1").unwrap();
    assert_eq!(kick.inputs, vec!["trigger", "accent", "choke"]);
    assert_eq!(kick.outputs, vec!["out"]);
}
