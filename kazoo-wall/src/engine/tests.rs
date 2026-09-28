//! Engine tests: rendering under `assert_no_alloc`, feedback, retirement,
//! faults, glides and the clock.

use super::*;
use crate::dsp::{Rng, build};

const RATE: u32 = 48_000;

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

/// A cable between slots: source slot and output, destination slot and
/// jack, amount.
type Cable = (usize, usize, usize, usize, f32);

/// The control side's view of what is where, for building tables.
#[derive(Debug, Default)]
struct Model {
    slots: Vec<(usize, Kind)>,
    cables: Vec<Cable>,
    next_tag: u32,
}

impl Model {
    fn add(&mut self, control: &mut EngineControl, slot: usize, kind: Kind) {
        let mut knobs = [0.0; MAX_KNOBS];
        for (target, value) in knobs.iter_mut().zip(kind.spec().defaults()) {
            *target = value;
        }
        self.next_tag += 1;
        control.send(Command::Insert {
            slot,
            tag: self.next_tag,
            kind,
            module: build(kind, 48_000.0),
            knobs,
        });
        self.slots.push((slot, kind));
    }

    fn remove(&mut self, control: &mut EngineControl, slot: usize) {
        self.slots.retain(|(s, _)| *s != slot);
        self.cables.retain(|c| c.0 != slot && c.2 != slot);
        self.send_tables(control);
        control.send(Command::Remove { slot });
    }

    fn plug(&mut self, control: &mut EngineControl, cable: Cable) {
        self.cables.retain(|c| !(c.2 == cable.2 && c.3 == cable.3));
        self.cables.push(cable);
        self.send_tables(control);
    }

    fn send_tables(&self, control: &mut EngineControl) {
        let mut table = CableTable::new();
        for (line, &(from, port, to, input, amount)) in self.cables.iter().enumerate() {
            // A tag from the cable's ends: a line that changes hands is
            // started afresh by the engine.
            let tag = u32::try_from(((from * 7 + port) * 97 + to) * 31 + input + 1).unwrap();
            table.set(
                to,
                input,
                Some(Route::new(
                    from as u8,
                    port as u8,
                    amount,
                    u16::try_from(line).unwrap(),
                    tag,
                )),
            );
        }
        let slots: Vec<usize> = self.slots.iter().map(|(s, _)| *s).collect();
        let edges: Vec<(usize, usize)> = self.cables.iter().map(|c| (c.0, c.2)).collect();
        control.send(Command::Cables(Box::new(table)));
        control.send(Command::Order(Box::new(processing_order(&slots, &edges))));
    }
}

fn fresh() -> (Engine, EngineControl) {
    engine(EngineConfig::new(RATE, 120.0, 0.0), None)
}

/// Render `frames` stereo frames on the audio thread; returns the buffer.
fn render(engine: &mut Engine, frames: usize, buffer: &mut Vec<f32>) {
    buffer.clear();
    buffer.resize(frames * 2, 0.0);
    on_audio_thread(|| engine.render(buffer, 2));
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0_f32, |m, s| m.max(s.abs()))
}

#[test]
fn random_patching_never_allocates_on_the_audio_thread() {
    let (mut engine, mut control) = fresh();
    let mut model = Model::default();
    let mut rng = Rng::new(1234);
    let mut buffer = Vec::with_capacity(8_192);
    // The wall's own kinds and the test effect: every real effect gets its
    // own test below (they are heavy to run by the dozen in a debug build).
    let kinds: Vec<Kind> = Kind::all()
        .filter(|kind| kind.spec().family == "synth" || kind.name() == "testgain")
        .collect();
    let pick = |rng: &mut Rng, n: usize| (rng.unit() * n as f32) as usize % n.max(1);
    for round in 0..600 {
        match pick(&mut rng, 6) {
            0 | 1 if model.slots.len() < MAX_MODULES => {
                let free = (0..MAX_MODULES)
                    .find(|slot| model.slots.iter().all(|(s, _)| s != slot))
                    .unwrap();
                let kind = kinds[pick(&mut rng, kinds.len())];
                model.add(&mut control, free, kind);
                model.send_tables(&mut control);
            }
            2 if !model.slots.is_empty() => {
                let (slot, _) = model.slots[pick(&mut rng, model.slots.len())];
                model.remove(&mut control, slot);
            }
            3 if model.slots.len() >= 2 => {
                // Any output into any input, cycles and self-patches included.
                let (from, from_kind) = model.slots[pick(&mut rng, model.slots.len())];
                let (to, to_kind) = model.slots[pick(&mut rng, model.slots.len())];
                let outputs = from_kind.spec().outputs.len();
                let inputs = to_kind.spec().inputs.len();
                let jacks = inputs + to_kind.spec().knobs.len();
                if outputs > 0 {
                    // Inputs and knob jacks alike.
                    let jack = pick(&mut rng, jacks);
                    let jack = if jack < inputs {
                        jack
                    } else {
                        crate::MAX_INPUTS + jack - inputs
                    };
                    let cable = (from, pick(&mut rng, outputs), to, jack, rng.bipolar());
                    model.plug(&mut control, cable);
                }
            }
            4 if !model.slots.is_empty() => {
                let (slot, kind) = model.slots[pick(&mut rng, model.slots.len())];
                let spec = kind.spec();
                let knob = pick(&mut rng, spec.knobs.len());
                control.send(Command::Knob {
                    slot,
                    knob,
                    target: spec.knobs[knob].denormalise(rng.unit()),
                    glide_frames: pick(&mut rng, 20_000) as u32,
                });
            }
            _ => control.send(Command::Tempo(f64::from(rng.unit()) * 300.0)),
        }
        let frames = 1 + pick(&mut rng, 1_500);
        render(&mut engine, frames, &mut buffer);
        assert!(
            buffer.iter().all(|s| s.is_finite() && s.abs() <= CEILING),
            "round {round}"
        );
        let faults = control.pump();
        assert!(faults.len() <= FAULT_BACKLOG);
        assert_eq!(control.backlog(), 0);
    }
    assert_eq!(engine.shared.leaked(), 0);
    assert_eq!(engine.shared.ignored(), 0);
}

#[test]
fn feedback_cycles_render() {
    let (mut engine, mut control) = fresh();
    let mut model = Model::default();
    model.add(&mut control, 0, Kind::VCO);
    model.add(&mut control, 1, Kind::VCF);
    model.add(&mut control, 2, Kind::from_name("testgain").unwrap());
    model.add(&mut control, 3, Kind::OUT);
    // vco → vcf → effect → vco.fm, and the effect back into the filter's
    // cutoff jack: two cycles.
    model.plug(&mut control, (0, 0, 1, 0, 1.0));
    model.plug(&mut control, (1, 0, 2, 0, 1.0));
    model.plug(&mut control, (2, 0, 0, 1, 0.8));
    model.plug(&mut control, (2, 0, 1, crate::MAX_INPUTS, 0.3));
    model.plug(&mut control, (2, 0, 3, 0, 1.0));
    let mut buffer = Vec::new();
    render(&mut engine, 48_000, &mut buffer);
    assert!(buffer.iter().all(|s| s.is_finite()));
    assert!(peak(&buffer[48_000..]) > 0.05, "the cycle is silent");
}

#[test]
fn removed_modules_come_back_over_the_retire_ring() {
    let (mut engine, mut control) = fresh();
    let mut model = Model::default();
    model.add(&mut control, 5, Kind::NOISE);
    model.send_tables(&mut control);
    let mut buffer = Vec::new();
    render(&mut engine, 64, &mut buffer);
    // The empty tables the engine started with came back.
    assert_eq!(control.retired_waiting(), 2);
    control.pump();
    model.remove(&mut control, 5);
    render(&mut engine, 64, &mut buffer);
    // The module, and the tables its removal replaced.
    assert_eq!(control.retired_waiting(), 3);
    control.pump();
    assert_eq!(control.retired_waiting(), 0);
    assert!(engine.slots[5].is_none());
}

#[test]
fn replacing_a_module_retires_the_old_one() {
    let (mut engine, mut control) = fresh();
    let mut model = Model::default();
    model.add(&mut control, 0, Kind::VCO);
    model.add(&mut control, 0, Kind::LFO);
    let mut buffer = Vec::new();
    render(&mut engine, 32, &mut buffer);
    assert_eq!(control.retired_waiting(), 1);
}

/// A module that turns out NaN after a few sub-blocks, until reset.
#[derive(Debug)]
struct Broken {
    blocks: u32,
    resets: Arc<AtomicU32>,
}

impl Module for Broken {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        self.blocks += 1;
        let value = if self.blocks > 3 { f32::NAN } else { 0.5 };
        io.outputs[0] = [value; SUB_BLOCK];
    }

    fn reset(&mut self) {
        self.blocks = 0;
        self.resets.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn a_module_producing_nan_is_silenced_reset_and_reported() {
    let (mut engine, mut control) = fresh();
    let resets = Arc::new(AtomicU32::new(0));
    control.send(Command::Insert {
        slot: 7,
        tag: 99,
        kind: Kind::VCO,
        module: Box::new(Broken {
            blocks: 0,
            resets: Arc::clone(&resets),
        }),
        knobs: [0.0; MAX_KNOBS],
    });
    control.send(Command::Insert {
        slot: 8,
        tag: 100,
        kind: Kind::OUT,
        module: build(Kind::OUT, 48_000.0),
        knobs: [1.0; MAX_KNOBS],
    });
    let mut table = CableTable::new();
    table.set(8, 0, Some(Route::new(7, 0, 1.0, 0, 1)));
    control.send(Command::Cables(Box::new(table)));
    control.send(Command::Order(Box::new(Order::new(&[7, 8]))));
    let mut buffer = Vec::new();
    render(&mut engine, SUB_BLOCK * 4, &mut buffer);
    assert!(buffer.iter().all(|s| s.is_finite()));
    assert_eq!(resets.load(Ordering::Relaxed), 1);
    let faults = control.pump();
    assert_eq!(faults, vec![Fault { slot: 7, tag: 99 }]);
    assert_eq!(control.shared().faults(), (1, 0));
    assert!(engine.outputs[7][0].iter().all(|s| *s == 0.0));
}

#[test]
fn knobs_glide_evenly_along_their_travel() {
    let (mut engine, mut control) = fresh();
    let mut model = Model::default();
    model.add(&mut control, 3, Kind::VCF);
    model.send_tables(&mut control);
    control.send(Command::Knob {
        slot: 3,
        knob: 0,
        target: 200.0,
        glide_frames: 0,
    });
    let mut buffer = Vec::new();
    render(&mut engine, 480, &mut buffer);
    control.send(Command::Knob {
        slot: 3,
        knob: 0,
        target: 3_200.0,
        glide_frames: 4_800,
    });
    render(&mut engine, 2_400, &mut buffer);
    // Half way along a logarithmic cutoff is the geometric middle, 800 Hz,
    // give or take a sub-block (the carry renders ahead).
    let middle = control.shared().knob(3, 0).unwrap();
    assert!((middle - 800.0).abs() < 15.0, "{middle}");
    render(&mut engine, 4_800, &mut buffer);
    assert!((control.shared().knob(3, 0).unwrap() - 3_200.0).abs() < f32::EPSILON);
    // A glide of zero still takes a millisecond, so it cannot click, and
    // then lands exactly.
    control.send(Command::Knob {
        slot: 3,
        knob: 0,
        target: 300.0,
        glide_frames: 0,
    });
    render(&mut engine, 1, &mut buffer);
    let moving = control.shared().knob(3, 0).unwrap();
    assert!(moving > 300.0 && moving < 3_200.0, "{moving}");
    render(&mut engine, 96, &mut buffer);
    assert!((control.shared().knob(3, 0).unwrap() - 300.0).abs() < f32::EPSILON);
}

#[test]
fn a_glide_reaches_the_module_frame_by_frame() {
    // A gain glide is handed to the VCA as a sweep through the whole
    // sub-block, so it moves frame by frame rather than once per sub-block.
    let (mut engine, mut control) = fresh();
    let mut model = Model::default();
    model.add(&mut control, 0, Kind::VCA);
    model.send_tables(&mut control);
    control.send(Command::Knob {
        slot: 0,
        knob: 0,
        target: 0.0,
        glide_frames: 0,
    });
    let mut buffer = Vec::new();
    render(&mut engine, 480, &mut buffer);
    control.send(Command::Knob {
        slot: 0,
        knob: 0,
        target: 1.0,
        glide_frames: 4_800,
    });
    render(&mut engine, 64, &mut buffer);
    let sweep = engine.sweeps[0];
    assert!(
        sweep.frames == SUB_BLOCK as u32 && sweep.step > 0.0,
        "{sweep:?}"
    );
}

#[test]
fn the_beat_keeps_time_and_tempo_changes_apply() {
    let (mut engine, mut control) = fresh();
    let mut buffer = Vec::new();
    render(&mut engine, 48_000, &mut buffer);
    // 120 BPM for a second, rendered a sub-block ahead: two beats.
    let beat = control.shared().beat();
    assert!((beat - 2.0).abs() < 0.01, "{beat}");
    control.send(Command::Tempo(60.0));
    render(&mut engine, 48_000, &mut buffer);
    let beat = control.shared().beat();
    assert!((beat - 3.0).abs() < 0.01, "{beat}");
    assert!((control.shared().bpm() - 60.0).abs() < f64::EPSILON);
    control.send(Command::Tempo(f64::NAN));
    control.send(Command::Tempo(1_000.0));
    render(&mut engine, 32, &mut buffer);
    assert!((control.shared().bpm() - MAX_BPM).abs() < f64::EPSILON);
    assert!(!control.shared().following_desk());
}

#[test]
fn a_knob_jack_moves_the_knob() {
    let (mut engine, mut control) = fresh();
    let mut model = Model::default();
    model.add(&mut control, 0, Kind::LFO);
    // Depth 0 and offset 0.25: a steady 0.25, fed back into the lfo's own
    // offset jack (a legal self-patch).
    control.send(Command::Knob {
        slot: 0,
        knob: 2,
        target: 0.0,
        glide_frames: 0,
    });
    control.send(Command::Knob {
        slot: 0,
        knob: 3,
        target: 0.25,
        glide_frames: 0,
    });
    model.plug(&mut control, (0, 0, 0, crate::MAX_INPUTS + 3, 1.0));
    let mut buffer = Vec::new();
    render(&mut engine, 4_800, &mut buffer);
    // Each sub-block adds the last one's output times half the offset
    // range (1.0): it climbs to the top of the range and stays there.
    assert!((engine.outputs[0][0][0] - 1.0).abs() < 1e-6);
}

#[test]
fn outs_feed_the_device_channels() {
    let (mut engine, mut control) = fresh();
    let mut model = Model::default();
    model.add(&mut control, 0, Kind::VCO);
    model.add(&mut control, 1, Kind::OUT);
    model.plug(&mut control, (0, 0, 1, 0, 1.0));
    // Hard left.
    control.send(Command::Knob {
        slot: 1,
        knob: 1,
        target: -1.0,
        glide_frames: 0,
    });
    // The pan takes a millisecond to get there.
    let mut settle = vec![0.0; 480 * 3];
    on_audio_thread(|| engine.render(&mut settle, 3));
    // Clear the meters of the settling, so the peaks below are the steady
    // state's.
    let (settling_left, settling_right) = control.shared().take_peaks();
    assert!(settling_left.is_finite() && settling_right.is_finite());
    let mut buffer = vec![0.0; 4_800 * 3];
    on_audio_thread(|| engine.render(&mut buffer, 3));
    let left: Vec<f32> = buffer.chunks(3).map(|f| f[0]).collect();
    let right: Vec<f32> = buffer.chunks(3).map(|f| f[1]).collect();
    let third: Vec<f32> = buffer.chunks(3).map(|f| f[2]).collect();
    assert!(peak(&left[2_400..]) > 0.3);
    assert!(peak(&right[2_400..]) < 0.01);
    assert!(third.iter().all(|s| *s == 0.0));
    let (peak_left, peak_right) = control.shared().take_peaks();
    assert!(peak_left > 0.3 && peak_right < 0.01);
    assert_eq!(control.shared().take_peaks(), (0.0, 0.0));

    // A mono device gets both sides averaged.
    let mut mono = vec![0.0; 4_800];
    on_audio_thread(|| engine.render(&mut mono, 1));
    assert!(peak(&mono) > 0.15);
}

#[test]
fn without_an_out_the_wall_is_silent_and_listening_hears_it() {
    let (mut engine, mut control) = fresh();
    let mut listen = control.take_listen().unwrap();
    assert!(control.take_listen().is_none());
    let mut model = Model::default();
    model.add(&mut control, 0, Kind::NOISE);
    model.send_tables(&mut control);
    let mut buffer = Vec::new();
    render(&mut engine, 4_800, &mut buffer);
    assert!(buffer.iter().all(|s| *s == 0.0));
    let mut heard = 0;
    while listen.try_pop().is_some() {
        heard += 1;
    }
    // Rendered a sub-block ahead.
    assert!((4_800..=4_800 + SUB_BLOCK).contains(&heard), "{heard}");
}

#[test]
fn nonsense_commands_are_counted_not_obeyed() {
    let (mut engine, mut control) = fresh();
    control.send(Command::Remove { slot: 99 });
    control.send(Command::Knob {
        slot: 0,
        knob: 99,
        target: 1.0,
        glide_frames: 0,
    });
    control.send(Command::Knob {
        slot: 0,
        knob: 0,
        target: f32::NAN,
        glide_frames: 0,
    });
    control.send(Command::Insert {
        slot: 99,
        tag: 1,
        kind: Kind::VCO,
        module: build(Kind::VCO, 48_000.0),
        knobs: [0.0; MAX_KNOBS],
    });
    let mut buffer = Vec::new();
    render(&mut engine, 32, &mut buffer);
    assert_eq!(control.shared().ignored(), 4);
    // The refused module still comes back to be freed.
    assert_eq!(control.retired_waiting(), 1);
}

#[test]
fn a_full_ring_waits_on_the_control_side_in_order() {
    let (mut engine, mut control) = fresh();
    for step in 0..COMMAND_BACKLOG + 10 {
        control.send(Command::Tempo((step as f64).mul_add(0.1, 40.0)));
    }
    assert_eq!(control.backlog(), 10);
    let mut buffer = Vec::new();
    render(&mut engine, 32, &mut buffer);
    control.pump();
    assert_eq!(control.backlog(), 0);
    render(&mut engine, 32, &mut buffer);
    let last = ((COMMAND_BACKLOG + 9) as f64).mul_add(0.1, 40.0);
    assert!((control.shared().bpm() - last).abs() < 1e-9);
}

#[test]
fn the_wall_follows_a_playing_desk_and_keeps_its_own_beat_otherwise() {
    let playing = |bpm, beat| {
        Some(kazoo_core::ipc::follow::TransportChange {
            bpm,
            beat: Some(beat),
        })
    };
    let stopped = |bpm| Some(kazoo_core::ipc::follow::TransportChange { bpm, beat: None });
    // Nothing due: nothing changes.
    assert_eq!(follow(96.0, 7.5, false, None, true), (96.0, 7.5, false));
    // The desk plays: its tempo and its beat.
    assert_eq!(
        follow(96.0, 7.5, false, playing(120.0, 16.0), true),
        (120.0, 16.0, true)
    );
    // The desk stops: its tempo, the wall's own beat carries on.
    assert_eq!(
        follow(120.0, 17.25, true, stopped(110.0), true),
        (110.0, 17.25, false)
    );
    // The desk goes away: the last tempo and beat, on the wall's own clock.
    assert_eq!(
        follow(120.0, 17.25, true, None, false),
        (120.0, 17.25, false)
    );
    // Nonsense from the desk is held to the wall's range.
    assert_eq!(
        follow(96.0, 1.0, false, playing(1_000.0, f64::NAN), true),
        (MAX_BPM, 1.0, false)
    );
}

#[test]
fn every_effect_renders_in_the_engine_without_allocating() {
    let mut model = Model::default();
    let (mut engine, mut control) = fresh();
    let mut buffer = Vec::new();
    let effects: Vec<Kind> = Kind::all().filter(|k| k.spec().family == "fx").collect();
    for chunk in effects.chunks(8) {
        for (slot, _) in model.slots.clone() {
            model.remove(&mut control, slot);
        }
        control.pump();
        model.add(&mut control, 0, Kind::VCO);
        model.add(&mut control, 1, Kind::OUT);
        let mut previous = 0;
        for (index, kind) in chunk.iter().enumerate() {
            let slot = 2 + index;
            model.add(&mut control, slot, *kind);
            model.plug(&mut control, (previous, 0, slot, 0, 1.0));
            // Every knob jack of the effect moved by the oscillator too.
            for knob in 0..kind.spec().knobs.len() {
                model.plug(&mut control, (0, 0, slot, crate::MAX_INPUTS + knob, 0.3));
            }
            previous = slot;
        }
        model.plug(&mut control, (previous, 0, 1, 0, 1.0));
        render(&mut engine, 4_800, &mut buffer);
        assert!(
            buffer.iter().all(|s| s.is_finite() && s.abs() <= CEILING),
            "{chunk:?}"
        );
        control.pump();
    }
    assert_eq!(engine.shared.leaked(), 0);
}

/// An oscillator into an out, in an engine heard or not from the start.
fn tone(audible: bool) -> (Engine, EngineControl) {
    let (mut engine, mut control) = engine(
        EngineConfig {
            audible,
            ..EngineConfig::new(RATE, 120.0, 0.0)
        },
        None,
    );
    let mut model = Model::default();
    model.add(&mut control, 0, Kind::VCO);
    model.add(&mut control, 1, Kind::OUT);
    model.plug(&mut control, (0, 0, 1, 0, 1.0));
    let mut settle = Vec::new();
    render(&mut engine, 480, &mut settle);
    (engine, control)
}

fn left(buffer: &[f32]) -> Vec<f32> {
    buffer.chunks(2).map(|frame| frame[0]).collect()
}

#[test]
fn a_silent_wall_plays_on_for_the_meters_and_the_ears_but_sends_nothing() {
    let (mut engine, mut control) = tone(false);
    let mut listen = control.take_listen().unwrap();
    while listen.try_pop().is_some() {}
    // Clear the meters of the settling.
    let (settling, _) = control.shared().take_peaks();
    assert!(settling.is_finite());
    let mut buffer = Vec::new();
    render(&mut engine, 4_800, &mut buffer);
    // From its very first frame: a rebuilt engine starts where it should,
    // not fading down from heard.
    assert!(buffer.iter().all(|s| *s == 0.0), "silence, exactly");
    let (peak_left, _) = control.shared().take_peaks();
    assert!(peak_left > 0.3, "the meters still see it: {peak_left}");
    let mut heard = 0.0_f32;
    while let Some(sample) = listen.try_pop() {
        heard = heard.max(sample.abs());
    }
    assert!(heard > 0.1, "listening still hears it: {heard}");
    assert!(!control.shared().audible());
}

#[test]
fn the_monitor_fades_in_and_out_and_lands_on_its_level() {
    let (mut heard_engine, _heard_control) = tone(true);
    let (mut engine, control) = tone(false);
    let mut reference = Vec::new();
    let mut buffer = Vec::new();

    control.shared().set_audible(true);
    render(&mut heard_engine, 4_800, &mut reference);
    render(&mut engine, 4_800, &mut buffer);
    let (reference, faded) = (left(&reference), left(&buffer));
    // The same wall, faded in over 50 ms (2 400 frames): never louder than
    // the fade allows (to within the rounding of a gain stepped in f32), and the same as a wall heard all along after it.
    for (frame, (want, got)) in reference.iter().zip(&faded).enumerate() {
        let gain = ((frame + 1) as f32 / 2_400.0).min(1.0);
        assert!(
            got.abs() <= (want.abs() * gain).mul_add(1.0 + 1e-3, 1e-6),
            "{frame}: {got} vs {want}"
        );
        if frame >= 2_400 {
            assert_eq!(got.to_bits(), want.to_bits(), "{frame}");
        }
    }

    control.shared().set_audible(false);
    render(&mut engine, 2_400, &mut buffer);
    assert!(peak(&buffer[..48]) > 0.0, "fades rather than cuts");
    render(&mut engine, 4_800, &mut buffer);
    assert!(buffer.iter().all(|s| *s == 0.0), "silence, exactly");
}

#[test]
fn the_record_ring_takes_the_stereo_master_only_while_recording() {
    let (mut engine, mut control) = tone(false);
    let mut record = control.take_record().unwrap();
    assert!(control.take_record().is_none(), "taken once");
    let mut buffer = Vec::new();
    // Not recording: nothing goes in.
    render(&mut engine, 4_800, &mut buffer);
    assert_eq!(record.occupied_len(), 0);
    // Recording: every frame, stereo interleaved, whole sub-blocks, even
    // while the wall is silent. The master is the same whether it is
    // heard or not, so a wall heard all along shows what went in.
    let (mut heard, _heard_control) = tone(true);
    let mut reference = Vec::new();
    render(&mut heard, 4_800, &mut reference);
    control.shared().set_recording(true);
    render(&mut engine, 4_800, &mut buffer);
    render(&mut heard, 4_800, &mut reference);
    assert!(buffer.iter().all(|s| *s == 0.0), "the device hears silence");
    assert_eq!(record.occupied_len(), 4_800 * 2);
    let mut recorded = vec![0.0; 4_800 * 2];
    assert_eq!(record.pop_slice(&mut recorded), 4_800 * 2);
    assert!(peak(&recorded) > 0.3, "the recording hears the wall");
    for (index, (got, want)) in recorded.iter().zip(&reference).enumerate() {
        assert_eq!(got.to_bits(), want.to_bits(), "sample {index}");
    }
    // Left and right each have their own place.
    assert_eq!(recorded[0].to_bits(), reference[0].to_bits());
    assert_eq!(recorded[1].to_bits(), reference[1].to_bits());
    // Stopped: nothing more goes in.
    control.shared().set_recording(false);
    render(&mut engine, 4_800, &mut buffer);
    assert_eq!(record.occupied_len(), 0);
    assert_eq!(control.shared().record_dropped(), 0);
}

#[test]
fn a_full_record_ring_drops_whole_sub_blocks_and_counts_them() {
    let (mut engine, mut control) = engine(
        EngineConfig {
            record_frames: SUB_BLOCK * 4,
            ..EngineConfig::new(RATE, 120.0, 0.0)
        },
        None,
    );
    let record = control.take_record().unwrap();
    control.shared().set_recording(true);
    let mut buffer = Vec::new();
    // Ten sub-blocks into a ring of four.
    render(&mut engine, SUB_BLOCK * 10, &mut buffer);
    assert_eq!(record.occupied_len(), SUB_BLOCK * 4 * 2);
    assert_eq!(
        control.shared().record_dropped(),
        (SUB_BLOCK * 6 * 2) as u64,
        "six sub-blocks of stereo"
    );
    assert!(control.shared().recording());
}

#[test]
fn recording_never_allocates_on_the_audio_thread() {
    let (mut engine, mut control) = tone(true);
    let mut record = control.take_record().unwrap();
    control.shared().set_recording(true);
    let mut buffer = Vec::with_capacity(8_192);
    let mut drained = vec![0.0; RATE as usize * 2];
    for _ in 0..50 {
        // `render` runs the engine under assert_no_alloc.
        render(&mut engine, 4_096, &mut buffer);
        assert_eq!(record.pop_slice(&mut drained), 4_096 * 2);
    }
    // Full, it drops without allocating too.
    for _ in 0..(RATE as usize * RECORD_SECONDS / 4_096 + 2) {
        render(&mut engine, 4_096, &mut buffer);
    }
    assert!(control.shared().record_dropped() > 0);
    assert_eq!(engine.shared.leaked(), 0);
}
