//! Every voice, heard from outside: silence, bounds, decay, poison,
//! retrigger clicks, chokes and tuning.

use std::f64::consts::TAU;

use kazoo_perc::{Voice, catalogue, find};

const RATE: f32 = 48_000.0;
const BLOCK: usize = 61;

fn voice(id: &str) -> Box<dyn Voice> {
    let kind = find(id).unwrap_or_else(|| panic!("no voice {id}"));
    let mut voice = (kind.build)();
    voice.prepare(RATE);
    voice
}

fn param(id: &str, name: &str) -> usize {
    find(id)
        .and_then(|kind| kind.params.iter().position(|spec| spec.name == name))
        .unwrap_or_else(|| panic!("{id} has no {name}"))
}

fn set(voice: &mut dyn Voice, id: &str, name: &str, value: f32) {
    voice.set_param(param(id, name), value);
}

/// Render `seconds` in odd-sized blocks.
fn render(voice: &mut dyn Voice, seconds: f32) -> Vec<f32> {
    let mut out = vec![1.0; (seconds * RATE) as usize];
    for block in out.chunks_mut(BLOCK) {
        voice.process(block);
    }
    out
}

fn peak(samples: &[f32]) -> f32 {
    samples
        .iter()
        .fold(0.0, |most, sample| most.max(sample.abs()))
}

fn largest_step(samples: &[f32]) -> f32 {
    samples
        .windows(2)
        .fold(0.0, |most, pair| most.max((pair[1] - pair[0]).abs()))
}

/// The strongest frequency between `low` and `high` hertz, found by
/// scanning a Hann-windowed DFT coarsely and then finely.
fn dominant(samples: &[f32], low: f64, high: f64) -> f64 {
    let count = samples.len();
    let windowed: Vec<f64> = samples
        .iter()
        .enumerate()
        .map(|(n, &sample)| {
            let hann = 0.5f64.mul_add(-(TAU * n as f64 / count as f64).cos(), 0.5);
            f64::from(sample) * hann
        })
        .collect();
    let power = |hz: f64| {
        let step = TAU * hz / f64::from(RATE);
        let (mut re, mut im) = (0.0, 0.0);
        for (n, sample) in windowed.iter().enumerate() {
            let phase = step * n as f64;
            re += sample * phase.cos();
            im -= sample * phase.sin();
        }
        re.mul_add(re, im * im)
    };
    let scan = |from: f64, to: f64, step: f64| {
        let mut best = (from, 0.0);
        let count = ((to - from) / step).round() as usize;
        for index in 0..=count {
            let hz = (index as f64).mul_add(step, from);
            let here = power(hz);
            if here > best.1 {
                best = (hz, here);
            }
        }
        best.0
    };
    let coarse = scan(low, high, 2.0);
    scan(coarse - 2.0, coarse + 2.0, 0.05)
}

/// Frequency from rising zero crossings, interpolated.
fn crossing_hz(samples: &[f32]) -> f64 {
    let mut first = None;
    let mut last = 0.0;
    let mut crossings = 0;
    for (n, pair) in samples.windows(2).enumerate() {
        if pair[0] <= 0.0 && pair[1] > 0.0 {
            let at = n as f64 + f64::from(pair[0] / (pair[0] - pair[1]));
            if first.is_none() {
                first = Some(at);
            } else {
                crossings += 1;
            }
            last = at;
        }
    }
    let first = first.unwrap_or(0.0);
    f64::from(crossings) * f64::from(RATE) / (last - first)
}

fn ids() -> Vec<&'static str> {
    catalogue().map(|kind| kind.id).collect()
}

#[test]
fn the_catalogue_has_every_voice() {
    let want = [
        "kick",
        "snare",
        "clap",
        "hat",
        "cymbal",
        "tom",
        "conga",
        "rim",
        "cowbell",
        "shaker",
        "membrane",
        "modal",
        "metal",
        "noiseperc",
    ];
    assert_eq!(ids(), want);
}

#[test]
fn untouched_voices_are_exactly_silent() {
    for id in ids() {
        let mut voice = voice(id);
        let out = render(voice.as_mut(), 0.5);
        assert!(out.iter().all(|&sample| sample == 0.0), "{id} made a sound");
    }
}

#[test]
fn a_hit_sounds_stays_bounded_and_dies_away() {
    for id in ids() {
        for accent in [false, true] {
            let mut voice = voice(id);
            voice.trigger(1.0, accent);
            let out = render(voice.as_mut(), 12.0);
            assert!(out.iter().all(|sample| sample.is_finite()), "{id}");
            let loudest = peak(&out);
            assert!(loudest < 2.0, "{id} peaked at {loudest}");
            assert!(loudest > 0.05, "{id} barely sounded: {loudest}");
            let tail = &out[out.len() - (RATE as usize / 2)..];
            assert!(peak(tail) < 1.0e-4, "{id} still rings: {}", peak(tail));
            // Once it has died away it is exactly silent again.
            let after = render(voice.as_mut(), 0.1);
            assert!(after.iter().all(|&sample| sample == 0.0), "{id}");
        }
    }
}

#[test]
fn an_accent_hits_harder() {
    for id in ids() {
        let loudness = |accent: bool| {
            let mut voice = voice(id);
            voice.trigger(0.6, accent);
            let out = render(voice.as_mut(), 0.1);
            out.iter().map(|sample| sample * sample).sum::<f32>()
        };
        assert!(loudness(true) > loudness(false) * 1.1, "{id}");
    }
}

#[test]
fn every_setting_stays_finite_and_bounded() {
    for kind in catalogue() {
        for corner in 0..3 {
            let mut voice = voice(kind.id);
            for (index, spec) in kind.params.iter().enumerate() {
                let value = match corner {
                    0 => spec.min,
                    1 => spec.max,
                    // Alternate ends, so no two neighbours agree.
                    _ if index % 2 == 0 => spec.min,
                    _ => spec.max,
                };
                voice.set_param(index, value);
            }
            for _ in 0..3 {
                voice.trigger(1.0, true);
                let out = render(voice.as_mut(), 0.7);
                assert!(out.iter().all(|sample| sample.is_finite()), "{}", kind.id);
                assert!(
                    peak(&out) < 2.0,
                    "{} corner {corner}: {}",
                    kind.id,
                    peak(&out)
                );
            }
        }
    }
}

#[test]
fn poison_is_ignored() {
    for kind in catalogue() {
        let mut voice = (kind.build)();
        voice.prepare(f32::NAN);
        for index in 0..kind.params.len() + 2 {
            voice.set_param(index, f32::NAN);
            voice.set_param(index, f32::INFINITY);
            voice.set_param(index, f32::NEG_INFINITY);
        }
        voice.trigger(f32::NAN, true);
        let quiet = render(voice.as_mut(), 0.05);
        assert!(quiet.iter().all(|&sample| sample == 0.0), "{}", kind.id);
        voice.trigger(f32::INFINITY, false);
        let out = render(voice.as_mut(), 0.5);
        assert!(out.iter().all(|sample| sample.is_finite()), "{}", kind.id);
        assert!(peak(&out) < 2.0, "{}", kind.id);
        voice.reset();
        let after = render(voice.as_mut(), 0.05);
        assert!(after.iter().all(|&sample| sample == 0.0), "{}", kind.id);
    }
}

#[test]
fn a_zero_length_block_is_fine() {
    for id in ids() {
        let mut voice = voice(id);
        voice.trigger(1.0, false);
        voice.process(&mut []);
        assert!(peak(&render(voice.as_mut(), 0.05)) > 0.0, "{id}");
    }
}

#[test]
fn a_trigger_lands_on_the_next_sample() {
    for id in ids() {
        let mut voice = voice(id);
        let before = render(voice.as_mut(), 0.01);
        assert!(before.iter().all(|&sample| sample == 0.0));
        voice.trigger(1.0, false);
        let mut first = [0.0f32; 8];
        voice.process(&mut first);
        assert!(peak(&first) > 0.0, "{id} did not start at once");
    }
}

#[test]
fn a_retrigger_mid_decay_does_not_click() {
    for id in ids() {
        // The largest sample-to-sample step of a clean hit, from silence.
        let mut fresh = voice(id);
        let mut clean_run = render(fresh.as_mut(), 0.001);
        fresh.trigger(1.0, false);
        clean_run.extend(render(fresh.as_mut(), 0.3));
        let clean = largest_step(&clean_run);

        let mut again = voice(id);
        again.trigger(1.0, false);
        let mut out = render(again.as_mut(), 0.03);
        again.trigger(1.0, false);
        out.extend(render(again.as_mut(), 0.01));
        let join = &out[out.len() - (0.011 * RATE) as usize..];
        let jump = largest_step(join);
        assert!(
            jump <= clean.mul_add(1.25, 0.02),
            "{id}: retrigger step {jump} against a clean hit's {clean}"
        );
    }
}

#[test]
fn a_zero_velocity_trigger_chokes() {
    for id in ids() {
        let mut voice = voice(id);
        voice.trigger(1.0, false);
        let mut out = render(voice.as_mut(), 0.02);
        let clean = largest_step(&out);
        voice.trigger(0.0, false);
        let choke = render(voice.as_mut(), 0.03);
        out.extend_from_slice(&choke);
        assert!(
            peak(&choke[choke.len() - 100..]) == 0.0,
            "{id} kept ringing"
        );
        let join = &out[out.len() - choke.len() - 1..];
        assert!(
            largest_step(join) <= clean.mul_add(1.25, 0.02),
            "{id} clicked"
        );
    }
}

#[test]
fn an_open_hat_is_cut_by_the_next_hit() {
    let mut hat = voice("hat");
    set(hat.as_mut(), "hat", "decay", 2.0);
    hat.trigger(1.0, false);
    render(hat.as_mut(), 0.1);
    set(hat.as_mut(), "hat", "decay", 0.03);
    render(hat.as_mut(), 0.05);
    hat.trigger(0.5, false);
    let out = render(hat.as_mut(), 0.3);
    assert!(peak(&out[out.len() - 2_000..]) < 1.0e-4);
}

#[test]
fn the_kick_is_in_tune() {
    for model in [0.0, 1.0] {
        for tune in [40.0, 60.0, 100.0] {
            let mut kick = voice("kick");
            set(kick.as_mut(), "kick", "model", model);
            set(kick.as_mut(), "kick", "tune", tune);
            set(kick.as_mut(), "kick", "decay", 3.0);
            set(kick.as_mut(), "kick", "sweep", 0.0);
            set(kick.as_mut(), "kick", "click", 0.0);
            kick.trigger(1.0, false);
            let out = render(kick.as_mut(), 1.0);
            let heard = crossing_hz(&out[(0.2 * RATE) as usize..]);
            let error = (heard / f64::from(tune) - 1.0).abs();
            assert!(error < 0.005, "model {model} at {tune} Hz heard {heard}");
        }
    }
}

#[test]
fn the_sweep_starts_high_and_settles() {
    let cycles = |model: f32, sweep: f32| {
        let mut kick = voice("kick");
        set(kick.as_mut(), "kick", "model", model);
        set(kick.as_mut(), "kick", "sweep", sweep);
        set(kick.as_mut(), "kick", "click", 0.0);
        set(kick.as_mut(), "kick", "decay", 3.0);
        kick.trigger(1.0, false);
        let out = render(kick.as_mut(), 1.0);
        let early = &out[..(0.08 * RATE) as usize];
        let rises = early
            .windows(2)
            .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
            .count();
        (rises, crossing_hz(&out[(0.4 * RATE) as usize..]))
    };
    for model in [0.0, 1.0] {
        let (flat, flat_hz) = cycles(model, 0.0);
        let (swept, swept_hz) = cycles(model, 1.0);
        // The swept kick gets through more cycles early on, then settles
        // on the same pitch.
        assert!(swept > flat, "model {model}: {swept} against {flat}");
        assert!((flat_hz / 50.0 - 1.0).abs() < 0.005, "{flat_hz}");
        assert!((swept_hz / 50.0 - 1.0).abs() < 0.005, "{swept_hz}");
    }
}

#[test]
fn the_tom_is_in_tune() {
    for (range, want) in [(0.0, 90.0), (1.0, 130.0), (2.0, 190.0)] {
        for tune in [-5.0f32, 0.0, 7.0] {
            let mut tom = voice("tom");
            set(tom.as_mut(), "tom", "range", range);
            set(tom.as_mut(), "tom", "tune", tune);
            set(tom.as_mut(), "tom", "bend", 0.0);
            set(tom.as_mut(), "tom", "noise", 0.0);
            set(tom.as_mut(), "tom", "decay", 2.0);
            tom.trigger(1.0, false);
            let out = render(tom.as_mut(), 0.5);
            let expected = want * f64::from((tune / 12.0).exp2());
            let heard = dominant(&out, expected * 0.7, expected * 1.3);
            assert!(
                (heard / expected - 1.0).abs() < 0.005,
                "{expected} heard {heard}"
            );
        }
    }
}

#[test]
fn the_tom_bends_down() {
    let mut tom = voice("tom");
    set(tom.as_mut(), "tom", "bend", 1.0);
    set(tom.as_mut(), "tom", "noise", 0.0);
    set(tom.as_mut(), "tom", "decay", 2.0);
    tom.trigger(1.0, false);
    let out = render(tom.as_mut(), 1.5);
    let early = crossing_hz(&out[..(0.05 * RATE) as usize]);
    let late = crossing_hz(&out[(1.0 * RATE) as usize..]);
    assert!(early > late * 1.2, "{early} against {late}");
}

#[test]
fn bars_bells_and_heads_are_in_tune() {
    for tune in [220.0f32, 440.0, 523.25] {
        let mut marimba = voice("modal");
        set(marimba.as_mut(), "modal", "tune", tune);
        marimba.trigger(1.0, false);
        let out = render(marimba.as_mut(), 0.5);
        let heard = dominant(&out, f64::from(tune) * 0.8, f64::from(tune) * 1.2);
        assert!(
            (heard / f64::from(tune) - 1.0).abs() < 0.003,
            "{tune} heard {heard}"
        );
    }
    let mut drum = voice("membrane");
    set(drum.as_mut(), "membrane", "tension", 200.0);
    set(drum.as_mut(), "membrane", "bend", 0.0);
    drum.trigger(1.0, false);
    let out = render(drum.as_mut(), 0.5);
    let heard = dominant(&out, 100.0, 300.0);
    assert!(
        (heard / 200.0 - 1.0).abs() < 0.005,
        "membrane heard {heard}"
    );

    let mut conga = voice("conga");
    set(conga.as_mut(), "conga", "decay", 1.5);
    conga.trigger(1.0, false);
    let out = render(conga.as_mut(), 0.5);
    let tail = &out[(0.1 * RATE) as usize..];
    let heard = dominant(tail, 180.0, 280.0);
    assert!((heard / 230.0 - 1.0).abs() < 0.005, "conga heard {heard}");
}

#[test]
fn the_metal_voice_follows_its_tune() {
    let mut metal = voice("metal");
    set(metal.as_mut(), "metal", "index", 0.0);
    set(metal.as_mut(), "metal", "ring", 0.0);
    set(metal.as_mut(), "metal", "tune", 330.0);
    metal.trigger(1.0, false);
    let out = render(metal.as_mut(), 0.5);
    assert!((crossing_hz(&out) / 330.0 - 1.0).abs() < 0.002);
}

#[test]
fn shakes_build_up_and_every_model_rattles() {
    for model in 0..4 {
        let mut shaker = voice("shaker");
        set(shaker.as_mut(), "shaker", "model", model as f32);
        let mut out = Vec::new();
        for _ in 0..8 {
            shaker.trigger(0.8, false);
            out.extend(render(shaker.as_mut(), 0.06));
        }
        assert!(peak(&out) > 0.05, "model {model}: {}", peak(&out));
        assert!(peak(&out) < 2.0);
    }
}

#[test]
fn a_reset_silences_at_once() {
    for id in ids() {
        let mut voice = voice(id);
        voice.trigger(1.0, true);
        render(voice.as_mut(), 0.02);
        voice.reset();
        let out = render(voice.as_mut(), 0.01);
        assert!(out.iter().all(|&sample| sample == 0.0), "{id}");
    }
}
