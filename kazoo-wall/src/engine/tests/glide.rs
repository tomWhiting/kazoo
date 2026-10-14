use super::*;

#[test]
fn the_tempo_glides_a_sub_block_at_a_time() {
    let (mut engine, mut control) = fresh();
    let mut buffer = Vec::new();
    control.send(Command::TempoGlide {
        bpm: 90.0,
        frames: 48_000,
    });
    render(&mut engine, SUB_BLOCK, &mut buffer);
    assert!((control.shared().bpm() - 119.98).abs() < 1e-9);
    assert_eq!(engine.tempo_left, 48_000 - 32);
    render(&mut engine, 24_000 - SUB_BLOCK, &mut buffer);
    assert!((control.shared().bpm() - 105.0).abs() < 0.1);
    render(&mut engine, 24_000, &mut buffer);
    assert_eq!(control.shared().bpm().to_bits(), 90.0_f64.to_bits());
    assert_eq!(engine.tempo_left, 0);
    let integral = ((120.0 + 90.0) / 2.0) / 60.0;
    // SUB_BLOCK is 32: exact in f64.
    let one_block = 120.0 * SUB_BLOCK as f64 / (60.0 * f64::from(RATE));
    assert!((control.shared().beat() - integral).abs() < one_block);
    // A second command starts from the tempo reached by the first.
    control.send(Command::TempoGlide {
        bpm: 120.0,
        frames: 96,
    });
    render(&mut engine, SUB_BLOCK, &mut buffer);
    assert_eq!(control.shared().bpm().to_bits(), 100.0_f64.to_bits());
    control.send(Command::TempoGlide {
        bpm: 80.0,
        frames: 64,
    });
    render(&mut engine, SUB_BLOCK, &mut buffer);
    assert_eq!(control.shared().bpm().to_bits(), 90.0_f64.to_bits());
    render(&mut engine, SUB_BLOCK, &mut buffer);
    assert_eq!(control.shared().bpm().to_bits(), 80.0_f64.to_bits());
}

#[test]
fn a_tempo_or_the_desk_stops_a_tempo_glide() {
    let (mut engine, mut control) = fresh();
    let mut buffer = Vec::new();
    control.send(Command::TempoGlide {
        bpm: 90.0,
        frames: 48_000,
    });
    render(&mut engine, 24_000, &mut buffer);
    assert!((control.shared().bpm() - 105.0).abs() < 0.1);
    control.send(Command::Tempo(100.0));
    render(&mut engine, SUB_BLOCK, &mut buffer);
    assert_eq!(control.shared().bpm().to_bits(), 100.0_f64.to_bits());
    assert_eq!(engine.tempo_left, 0);
    render(&mut engine, 24_000, &mut buffer);
    assert_eq!(control.shared().bpm().to_bits(), 100.0_f64.to_bits());

    let (bpm, left) = glide_tempo(105.0, 90.0, -0.01, 1_000, true);
    assert_eq!(bpm.to_bits(), 105.0_f64.to_bits());
    assert_eq!(left, 0);
    let (bpm, left) = glide_tempo(105.0, 90.0, -0.01, 0, false);
    assert_eq!(bpm.to_bits(), 105.0_f64.to_bits());
    assert_eq!(left, 0);
    let (bpm, left) = glide_tempo(91.0, 90.0, -1.0 / 3.0, 3, false);
    assert_eq!(bpm.to_bits(), 90.0_f64.to_bits());
    assert_eq!(left, 0);
    let (bpm, left) = glide_tempo(299.0, MAX_BPM, 1.0, 64, false);
    assert_eq!(bpm.to_bits(), MAX_BPM.to_bits());
    assert_eq!(left, 32);

    // A stopped desk sets its tempo too, although it is not followed.
    let change = Some(TransportChange {
        bpm: 110.0,
        beat: None,
    });
    let (bpm, beat, following) = follow(105.0, 7.5, true, change, true);
    let (bpm, left) = glide_tempo(bpm, 90.0, -0.01, 1_000, following || change.is_some());
    assert_eq!(bpm.to_bits(), 110.0_f64.to_bits());
    assert_eq!(beat.to_bits(), 7.5_f64.to_bits());
    assert!(!following);
    assert_eq!(left, 0);
}

#[test]
fn a_tempo_glide_to_nonsense_is_ignored() {
    let (mut engine, mut control) = fresh();
    let mut buffer = Vec::new();
    control.send(Command::TempoGlide {
        bpm: f64::NAN,
        frames: 48_000,
    });
    render(&mut engine, SUB_BLOCK, &mut buffer);
    assert_eq!(control.shared().bpm().to_bits(), 120.0_f64.to_bits());
    assert_eq!(engine.tempo_left, 0);
    assert_eq!(control.shared().ignored(), 1);

    control.send(Command::TempoGlide {
        bpm: 1_000.0,
        frames: 64,
    });
    render(&mut engine, SUB_BLOCK, &mut buffer);
    assert_eq!(control.shared().bpm().to_bits(), 210.0_f64.to_bits());
    let state = (
        engine.bpm.to_bits(),
        engine.tempo_aim.to_bits(),
        engine.tempo_step.to_bits(),
        engine.tempo_left,
    );
    for bpm in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        control.send(Command::TempoGlide { bpm, frames: 0 });
    }
    on_audio_thread(|| engine.drain_commands());
    assert_eq!(
        (
            engine.bpm.to_bits(),
            engine.tempo_aim.to_bits(),
            engine.tempo_step.to_bits(),
            engine.tempo_left,
        ),
        state
    );
    assert_eq!(control.shared().ignored(), 4);
    render(&mut engine, SUB_BLOCK, &mut buffer);
    assert_eq!(control.shared().bpm().to_bits(), MAX_BPM.to_bits());
    assert_eq!(engine.tempo_left, 0);

    control.send(Command::TempoGlide {
        bpm: 0.0,
        frames: 33,
    });
    render(&mut engine, SUB_BLOCK, &mut buffer);
    assert!(control.shared().bpm() > MIN_BPM);
    assert_eq!(engine.tempo_left, 1);
    render(&mut engine, SUB_BLOCK, &mut buffer);
    assert_eq!(control.shared().bpm().to_bits(), MIN_BPM.to_bits());
    control.send(Command::TempoGlide {
        bpm: 90.0,
        frames: 1_000,
    });
    render(&mut engine, SUB_BLOCK, &mut buffer);
    control.send(Command::TempoGlide {
        bpm: 100.0,
        frames: 0,
    });
    render(&mut engine, SUB_BLOCK, &mut buffer);
    assert_eq!(control.shared().bpm().to_bits(), 100.0_f64.to_bits());
    assert_eq!(engine.tempo_left, 0);
    render(&mut engine, SUB_BLOCK, &mut buffer);
    assert_eq!(control.shared().bpm().to_bits(), 100.0_f64.to_bits());
}

#[derive(Debug)]
struct Constant;

impl Module for Constant {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        io.outputs[0].fill(1.0);
    }

    fn reset(&mut self) {}
}

#[derive(Debug)]
struct CableSink;

impl Module for CableSink {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        io.outputs[0] = io.inputs[0];
    }

    fn reset(&mut self) {}
}

#[test]
fn gliding_allocates_nothing() {
    let (mut engine, mut control) = fresh();
    control.send(Command::Insert {
        slot: 0,
        tag: 1,
        kind: Kind::LFO,
        module: Box::new(Constant),
        knobs: [0.0; MAX_KNOBS],
    });
    for slot in 1..=128 {
        control.send(Command::Insert {
            slot,
            tag: u32::try_from(slot + 1).unwrap(),
            kind: Kind::VCA,
            module: Box::new(CableSink),
            knobs: [0.0; MAX_KNOBS],
        });
    }
    let slots: Vec<usize> = (0..=128).collect();
    control.send(Command::Order(Box::new(Order::new(&slots, &[]))));
    let mut resting = CableTable::with_slots(129).unwrap();
    for line in 0..64 {
        let number = u32::try_from(line).unwrap();
        assert!(resting.set(line + 1, 0, Some(Route::new(0, 0, 1.0, number, number + 1))));
    }
    control.send(Command::Cables(Box::new(resting)));
    let mut buffer = Vec::new();
    super::render(&mut engine, SUB_BLOCK, &mut buffer);
    for slot in 1..=64 {
        assert!(
            engine
                .room
                .output(slot, 0)
                .unwrap()
                .iter()
                .all(|s| s.to_bits() == 1.0_f32.to_bits())
        );
    }

    let mut crossing = CableTable::with_slots(129).unwrap();
    for line in 0..128 {
        let number = u32::try_from(line).unwrap();
        assert!(crossing.set(
            line + 1,
            0,
            Some(Route {
                glide: 96_000,
                ..Route::new(0, 0, if line < 64 { 0.0 } else { 1.0 }, number, number + 1)
            }),
        ));
    }
    control.send(Command::Cables(Box::new(crossing)));
    control.send(Command::TempoGlide {
        bpm: 90.0,
        frames: 96_000,
    });
    super::render(&mut engine, 48_000, &mut buffer);
    for slot in 1..=128 {
        assert!(
            engine
                .room
                .output(slot, 0)
                .unwrap()
                .iter()
                .all(|s| (*s - 0.5).abs() < 0.001)
        );
    }
    super::render(&mut engine, 48_000, &mut buffer);
    for slot in 1..=128 {
        let amount: f32 = if slot <= 64 { 0.0 } else { 1.0 };
        let output = engine.room.output(slot, 0).unwrap();
        assert_eq!(output[SUB_BLOCK - 1].to_bits(), amount.to_bits());
    }
    assert_eq!(control.shared().bpm().to_bits(), 90.0_f64.to_bits());
    assert_eq!(engine.tempo_left, 0);
    assert_eq!(control.shared().ignored(), 0);
    assert_eq!(control.shared().leaked(), 0);
    assert!(control.pump().is_empty());
}
