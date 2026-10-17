use super::*;

#[derive(Debug)]
struct SweepWatch {
    frames: Arc<AtomicU32>,
    step: Arc<AtomicU32>,
}

impl Module for SweepWatch {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        self.frames.store(io.sweeps[0].frames, Ordering::Relaxed);
        self.step
            .store(io.sweeps[0].step.to_bits(), Ordering::Relaxed);
    }

    fn reset(&mut self) {}
}

pub(super) fn watch_sweep(control: &mut EngineControl) -> [Arc<AtomicU32>; 2] {
    let frames = Arc::new(AtomicU32::new(0));
    let step = Arc::new(AtomicU32::new(0));
    control.send(Command::Insert {
        slot: 0,
        tag: 99,
        kind: Kind::VCA,
        module: Box::new(SweepWatch {
            frames: Arc::clone(&frames),
            step: Arc::clone(&step),
        }),
        knobs: [0.0; MAX_KNOBS],
    });
    control.send(Command::Knob {
        slot: 0,
        knob: 0,
        target: 1.0,
        glide_frames: 4_800,
        wait_frames: 0,
        curve: crate::protocol::GlideCurve::Linear,
    });
    [frames, step]
}

#[derive(Debug)]
struct JackProbe {
    mask: Arc<AtomicU32>,
    signal: Arc<AtomicU32>,
}

impl Module for JackProbe {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        let mut mask = 0;
        for (input, connected) in io.connected.iter().enumerate() {
            if *connected {
                mask |= 1 << input;
            } else {
                assert!(io.inputs[input].iter().all(|s| *s == 0.0));
            }
        }
        for (knob, patched) in io.knob_patched.iter().enumerate() {
            if *patched {
                mask |= 1 << (MAX_INPUTS + knob);
            } else {
                assert!(io.knob_cv[knob].iter().all(|s| *s == 0.0));
            }
        }
        self.mask.store(mask, Ordering::Relaxed);
        self.signal
            .store(io.inputs[0][0].to_bits(), Ordering::Relaxed);
    }

    fn reset(&mut self) {}
}

#[test]
fn an_unplugged_jack_reads_silence_and_the_table_knows_it() {
    let (mut engine, mut control) = fresh();
    let mut model = Model::default();
    model.add(&mut control, 0, Kind::LFO);
    let mask = Arc::new(AtomicU32::new(0));
    let signal = Arc::new(AtomicU32::new(0));
    control.send(Command::Insert {
        slot: 1,
        tag: 2,
        kind: Kind::LFO,
        module: Box::new(JackProbe {
            mask: Arc::clone(&mask),
            signal: Arc::clone(&signal),
        }),
        knobs: [0.0; MAX_KNOBS],
    });
    control.send(Command::Order(Box::new(Order::new(&[0, 1], &[]))));
    let mut table = CableTable::with_slots(2).unwrap();
    assert!(table.set(1, 0, Some(Route::new(0, 0, 1.0, 1))));
    assert_eq!(table.plugged(1), 1);
    assert_eq!(table.plugged(2), 0);
    control.send(Command::Cables(Box::new(table.clone())));
    render(&mut engine, SUB_BLOCK, &mut Vec::new());
    assert_eq!(mask.load(Ordering::Relaxed), 1);
    assert_ne!(signal.load(Ordering::Relaxed), 0.0_f32.to_bits());
    assert!(table.set(1, 0, None));
    assert_eq!(table.plugged(1), 0);
    control.send(Command::Cables(Box::new(table.clone())));
    render(&mut engine, SUB_BLOCK, &mut Vec::new());
    assert_eq!(mask.load(Ordering::Relaxed), 0);
    assert_eq!(signal.load(Ordering::Relaxed), 0.0_f32.to_bits());
    let jack = MAX_INPUTS + MAX_KNOBS - 1;
    assert!(table.set(1, jack, Some(Route::new(0, 0, 1.0, 2))));
    assert_eq!(table.plugged(1), 1 << jack);
    control.send(Command::Cables(Box::new(table.clone())));
    render(&mut engine, SUB_BLOCK, &mut Vec::new());
    assert_eq!(mask.load(Ordering::Relaxed), 1 << jack);
    assert!(engine.knob_cv[MAX_KNOBS - 1].iter().any(|s| *s != 0.0));
    assert!(table.set(1, jack, None));
    control.send(Command::Cables(Box::new(table)));
    render(&mut engine, SUB_BLOCK, &mut Vec::new());
    assert_eq!(mask.load(Ordering::Relaxed), 0);
}

#[test]
fn a_resting_knob_is_not_advanced() {
    let (mut engine, mut control) = fresh();
    let mut model = Model::default();
    model.add(&mut control, 3, Kind::VCF);
    model.send_tables(&mut control);
    on_audio_thread(|| engine.drain_commands());
    let (page, at) = engine.room.page(3).unwrap();
    assert_eq!(page.gliding[at], 0);
    let resting = control.knob(3, 0).unwrap();
    render(&mut engine, SUB_BLOCK, &mut Vec::new());
    assert_eq!(control.knob(3, 0).unwrap().to_bits(), resting.to_bits());
    control.send(Command::Knob {
        slot: 3,
        knob: 0,
        target: 200.0,
        glide_frames: 1_000,
        wait_frames: 0,
        curve: crate::protocol::GlideCurve::Linear,
    });
    on_audio_thread(|| engine.drain_commands());
    let (page, at) = engine.room.page(3).unwrap();
    assert_eq!(page.gliding[at], 1);
    render(&mut engine, SUB_BLOCK, &mut Vec::new());
    let moving = control.knob(3, 0).unwrap();
    assert_ne!(moving.to_bits(), resting.to_bits());
    assert_ne!(moving.to_bits(), 200.0_f32.to_bits());
    render(&mut engine, SUB_BLOCK * 32, &mut Vec::new());
    let (page, at) = engine.room.page(3).unwrap();
    assert_eq!(page.gliding[at], 0);
    assert_eq!(page.glides[at][0].remaining, 0);
    assert_eq!(control.knob(3, 0).unwrap().to_bits(), 200.0_f32.to_bits());
    render(&mut engine, SUB_BLOCK, &mut Vec::new());
    assert_eq!(control.knob(3, 0).unwrap().to_bits(), 200.0_f32.to_bits());
    control.send(Command::Knob {
        slot: 3,
        knob: 0,
        target: 400.0,
        glide_frames: 1_000,
        wait_frames: 0,
        curve: crate::protocol::GlideCurve::Linear,
    });
    model.add(&mut control, 3, Kind::VCF);
    on_audio_thread(|| engine.drain_commands());
    let (page, at) = engine.room.page(3).unwrap();
    assert_eq!(page.gliding[at], 0);
}

#[test]
fn the_out_sum_follows_the_order_s_list() {
    let (mut engine, mut control) = fresh();
    let mut model = Model::default();
    model.add(&mut control, 0, Kind::VCO);
    model.add(&mut control, 70, Kind::OUT);
    model.plug(&mut control, (0, 0, 70, 0, 1.0));
    let order = Order::new(&[0, 70], &[70]);
    assert_eq!(order.outs(), &[70]);
    control.send(Command::Order(Box::new(order)));
    let mut buffer = Vec::new();
    render(&mut engine, 512, &mut buffer);
    assert!(peak(&buffer) > 0.05);
    let order = Order::new(&[0, 70], &[]);
    assert!(order.outs().is_empty());
    control.send(Command::Order(Box::new(order)));
    on_audio_thread(|| engine.master.reset());
    render(&mut engine, 512, &mut buffer);
    assert!(buffer.iter().all(|s| *s == 0.0));
    assert!(peak(engine.room.output(70, crate::dsp::FEED_LEFT).unwrap()) > 0.0);
    model.add(&mut control, 70, Kind::LFO);
    control.send(Command::Order(Box::new(Order::new(&[0, 70], &[70]))));
    on_audio_thread(|| engine.master.reset());
    render(&mut engine, 512, &mut buffer);
    assert!(buffer.iter().all(|s| *s == 0.0));
}

#[derive(Debug)]
struct RestingSweeps {
    calls: Arc<AtomicU32>,
}

impl Module for RestingSweeps {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        assert_eq!(io.sweeps[3].frames, 0);
        self.calls.fetch_add(1, Ordering::Relaxed);
    }

    fn reset(&mut self) {}
}

#[test]
fn a_sweep_never_leaks_to_the_next_module() {
    let (mut engine, mut control) = fresh();
    let mut model = Model::default();
    model.add(&mut control, 0, Kind::LFO);
    let calls = Arc::new(AtomicU32::new(0));
    control.send(Command::Insert {
        slot: 1,
        tag: 2,
        kind: Kind::LFO,
        module: Box::new(RestingSweeps {
            calls: Arc::clone(&calls),
        }),
        knobs: [0.0; MAX_KNOBS],
    });
    control.send(Command::Order(Box::new(Order::new(&[0, 1], &[]))));
    control.send(Command::Knob {
        slot: 0,
        knob: 3,
        target: 1.0,
        glide_frames: 1_000,
        wait_frames: 0,
        curve: crate::protocol::GlideCurve::Linear,
    });
    render(&mut engine, SUB_BLOCK, &mut Vec::new());
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[derive(Debug)]
struct BadOutput {
    port: usize,
    resets: Arc<AtomicU32>,
}

impl Module for BadOutput {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        for (port, output) in io.outputs.iter_mut().enumerate() {
            *output = [if port == self.port { f32::NAN } else { 0.5 }; SUB_BLOCK];
        }
    }

    fn reset(&mut self) {
        self.resets.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn an_unfed_output_is_not_scanned_and_a_fed_one_is() {
    for port in [3, 0] {
        let (mut engine, mut control) = fresh();
        let mut model = Model::default();
        model.add(&mut control, 0, Kind::VCO);
        model.add(&mut control, 1, Kind::OUT);
        let resets = Arc::new(AtomicU32::new(0));
        control.send(Command::Insert {
            slot: 0,
            tag: 99,
            kind: Kind::VCO,
            module: Box::new(BadOutput {
                port,
                resets: Arc::clone(&resets),
            }),
            knobs: [0.0; MAX_KNOBS],
        });
        model.plug(&mut control, (0, 0, 1, 0, 1.0));
        render(&mut engine, SUB_BLOCK, &mut Vec::new());
        if port == 3 {
            assert_eq!(control.shared().faults(), (0, 0));
            assert_eq!(resets.load(Ordering::Relaxed), 0);
            assert!(control.pump().is_empty());
            assert!(
                engine
                    .room
                    .output(0, 0)
                    .unwrap()
                    .iter()
                    .all(|s| s.to_bits() == 0.5_f32.to_bits())
            );
            assert!(engine.room.output(0, 3).unwrap().iter().all(|s| s.is_nan()));
        } else {
            assert_eq!(control.shared().faults(), (1, 0));
            assert_eq!(resets.load(Ordering::Relaxed), 1);
            assert_eq!(control.pump(), vec![Fault { slot: 0, tag: 99 }]);
            for output in 0..MAX_OUTPUTS {
                assert!(
                    engine
                        .room
                        .output(0, output)
                        .unwrap()
                        .iter()
                        .all(|s| *s == 0.0)
                );
            }
        }
    }
}
