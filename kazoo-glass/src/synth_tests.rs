use super::*;

fn energy(synth: &mut GlassSynth, count: usize) -> f32 {
    (0..count)
        .map(|_| synth.process().iter().map(|s| s * s).sum::<f32>())
        .sum()
}

#[test]
fn silence_until_struck_and_every_patch_sounds() {
    for patch in PATCHES {
        let mut synth = GlassSynth::new(48_000.0);
        assert!(energy(&mut synth, 128) < f32::EPSILON);
        synth.set_patch(patch);
        synth.note_on(60, 100);
        assert!(energy(&mut synth, 4_800) > 0.1, "{}", patch.name);
    }
}

#[test]
fn all_notes_and_rates_are_finite_and_bounded() {
    for rate in [
        8_000.0,
        44_100.0,
        48_000.0,
        96_000.0,
        f32::NAN,
        f32::INFINITY,
        0.0,
    ] {
        let mut synth = GlassSynth::new(rate);
        for note in 0..=127 {
            synth.note_on(note, 127);
            for _ in 0..64 {
                assert!(
                    synth
                        .process()
                        .iter()
                        .all(|s| s.is_finite() && s.abs() <= 1.0)
                );
            }
        }
        synth.set_master(f32::NAN);
        assert!(energy(&mut synth, 128).is_finite());
    }
}

#[test]
fn note_ownership_is_independent() {
    let mut synth = GlassSynth::new(48_000.0);
    for source in [Source::Player, Source::Phrase, Source::Hub] {
        synth.note_on_from(source, 60, 100);
    }
    synth.release_source(Source::Phrase);
    synth.note_off(60);
    assert_eq!(synth.voices.iter().filter(|v| v.gate).count(), 1);
    assert!(
        synth
            .voices
            .iter()
            .any(|v| v.gate && v.source == Source::Hub)
    );
    synth.note_on_from(Source::Hub, 60, 0);
    assert_eq!(synth.voices.iter().filter(|v| v.gate).count(), 0);
}

#[test]
fn panic_damps_every_tail() {
    let mut synth = GlassSynth::new(48_000.0);
    for note in 50..70 {
        synth.note_on(note, 127);
    }
    synth.all_notes_off();
    for _ in 0..9_600 {
        synth.process();
    }
    assert!(energy(&mut synth, 1_024) < 1.0e-9);
    assert!(synth.voice_states().iter().all(Option::is_none));
}

#[test]
fn width_zero_is_mono_and_width_one_spreads_modes() {
    let mut synth = GlassSynth::new(48_000.0);
    let mut patch = PATCHES[4];
    patch.width = 0.0;
    synth.set_patch(patch);
    synth.note_on(60, 100);
    for _ in 0..256 {
        let [l, r] = synth.process();
        assert!((l - r).abs() < 1.0e-6);
    }
    patch.width = 1.0;
    let mut synth = GlassSynth::new(48_000.0);
    synth.set_patch(patch);
    synth.note_on(60, 100);
    assert!((0..256).any(|_| {
        let [l, r] = synth.process();
        (l - r).abs() > 0.01
    }));
}

#[test]
fn velocity_changes_energy_and_release_accelerates_decay() {
    let mut soft = GlassSynth::new(48_000.0);
    let mut loud = GlassSynth::new(48_000.0);
    soft.note_on(60, 30);
    loud.note_on(60, 127);
    assert!(energy(&mut loud, 4_800) > energy(&mut soft, 4_800) * 4.0);
    let mut patch = PATCHES[2];
    patch.release = 0.0;
    loud.set_patch(patch);
    loud.note_off(60);
    for _ in 0..9_600 {
        loud.process();
    }
    assert!(energy(&mut loud, 256) < 1.0e-9);
}

#[test]
fn ultrasonic_modes_are_omitted() {
    let mut synth = GlassSynth::new(8_000.0);
    synth.note_on(127, 100);
    assert!(energy(&mut synth, 256) < f32::EPSILON);
    synth.note_on(60, 100);
    assert!(
        synth
            .voices
            .iter()
            .flat_map(|v| v.modes)
            .filter(|m| m.sounding())
            .count()
            < MODES
    );
}

#[test]
fn rendered_fundamental_is_in_tune() {
    for rate in [44_100.0, 48_000.0, 96_000.0] {
        for note in [28, 36, 48, 60, 69, 84, 96] {
            let mut synth = GlassSynth::new(rate);
            let mut patch = PATCHES[0];
            patch.decay = 1.0;
            synth.set_patch(patch);
            synth.note_on(note, 100);
            let mut fundamental = synth.voices[0].modes[0];
            let mut previous = 0.0;
            let mut crossings = Vec::new();
            for frame in 0..rate as usize {
                let sample = fundamental.process();
                if previous < 0.0 && sample >= 0.0 {
                    let fraction = f64::from(-previous / (sample - previous));
                    crossings.push(frame as f64 - 1.0 + fraction);
                }
                previous = sample;
            }
            assert!(crossings.len() > 2);
            let duration = crossings.last().unwrap() - crossings[0];
            let measured = (crossings.len() - 1) as f64 * f64::from(rate) / duration;
            let cents = 1_200.0 * (measured / f64::from(midi_to_hz(note))).log2();
            assert!(cents.abs() < 0.2, "note {note} at {rate}: {cents} cents");
        }
    }
}

#[test]
fn repeated_strikes_and_voice_stealing_do_not_introduce_large_steps() {
    let mut synth = GlassSynth::new(48_000.0);
    synth.set_patch(PATCHES[2]);
    let mut last = [0.0; 2];
    let mut max_step = 0.0_f32;
    for frame in 0..24_000 {
        if frame % 480 == 0 {
            synth.note_on(48 + ((frame / 480) % 24) as u8, 127);
        }
        let next = synth.process();
        for channel in 0..2 {
            max_step = max_step.max((next[channel] - last[channel]).abs());
        }
        last = next;
    }
    assert!(max_step < 0.4, "largest adjacent-sample step: {max_step}");
}

#[test]
fn strikes_edits_stealing_and_rendering_do_not_allocate() {
    let mut synth = GlassSynth::new(48_000.0);
    let before = assert_no_alloc::violation_count();
    assert_no_alloc::assert_no_alloc(|| {
        for patch in PATCHES {
            synth.set_patch(patch);
            for note in 48..72 {
                synth.note_on(note, 100);
                for _ in 0..128 {
                    synth.process();
                }
                synth.note_off(note);
            }
            synth.all_notes_off();
        }
    });
    assert_eq!(assert_no_alloc::violation_count(), before);
}
