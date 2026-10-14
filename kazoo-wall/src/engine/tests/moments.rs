use super::*;

const MOMENT_RATE: u32 = 8_000;

fn start(beat: f64) -> (Engine, EngineControl) {
    engine(EngineConfig::new(MOMENT_RATE, 120.0, beat), None)
}

fn begin(control: &mut EngineControl) {
    control.begin_swap();
    control.send(Command::Hush);
}

fn end(control: &mut EngineControl) {
    control.send(Command::Unhush);
    control.end_swap();
}

fn insert(control: &mut EngineControl, slot: usize, kind: Kind, values: &[(&str, f32)]) {
    let mut knobs = [0.0; MAX_KNOBS];
    for (at, spec) in kind.spec().knobs.iter().enumerate() {
        knobs[at] = spec.default;
        for &(name, value) in values {
            if spec.name == name {
                knobs[at] = value;
            }
        }
    }
    control.send(Command::Insert {
        slot,
        tag: 1,
        kind,
        module: build(kind, MOMENT_RATE as f32, slot as u64 + 97),
        knobs,
    });
}

fn moment(control: &mut EngineControl) {
    begin(control);
    for slot in 0..9 {
        control.send(Command::Remove { slot });
    }
    insert(control, 0, Kind::CLOCK, &[]);
    insert(
        control,
        1,
        Kind::SEQ,
        &[
            ("steps", 12.0),
            ("chance", 0.5),
            ("step2", 7.0),
            ("step3", 12.0),
        ],
    );
    insert(control, 2, Kind::from_name("euclid").unwrap(), &[]);
    insert(control, 3, Kind::LFO, &[("rate", 0.013), ("sync", 0.0)]);
    insert(control, 4, Kind::LFO, &[("shape", 4.0), ("sync", 4.0)]);
    insert(control, 5, Kind::VCO, &[]);
    insert(control, 6, Kind::VCA, &[]);
    insert(control, 7, Kind::OUT, &[]);
    let mut table = CableTable::new();
    let cables = [
        (0, 0, 1, 0, 1.0),
        (0, 0, 2, 0, 1.0),
        (1, 0, 5, 0, 1.0),
        (3, 0, 5, MAX_INPUTS, 0.01),
        (4, 0, 5, MAX_INPUTS + 1, 0.1),
        (5, 0, 6, 0, 0.2),
        (1, 1, 6, 1, 1.0),
        (6, 0, 7, 0, 1.0),
        (2, 0, 7, 1, 0.02),
    ];
    for (tag, &(from, port, to, jack, amount)) in cables.iter().enumerate() {
        assert!(table.set(
            to,
            jack,
            Some(Route::new(from, port, amount, tag as u32 + 1))
        ));
    }
    control.send(Command::Cables(Box::new(table)));
    control.send(Command::Order(Box::new(Order::new(
        &[0, 1, 2, 3, 4, 5, 6, 7],
        &[7],
    ))));
    end(control);
}

fn render(engine: &mut Engine, frames: usize) -> Vec<f32> {
    let mut data = vec![0.0; frames * 2];
    on_audio_thread(|| engine.render(&mut data, 2));
    data
}

#[derive(Debug)]
struct BeatProbe(Arc<AtomicU64>);
impl Module for BeatProbe {
    fn process(&mut self, tick: &Tick, _io: Io<'_>) {
        self.0.store(tick.beat.to_bits(), Ordering::Relaxed);
    }
    fn reset(&mut self) {}
}

fn probe(control: &mut EngineControl, seen: &Arc<AtomicU64>) {
    begin(control);
    control.send(Command::Insert {
        slot: 0,
        tag: 1,
        kind: Kind::CLOCK,
        module: Box::new(BeatProbe(Arc::clone(seen))),
        knobs: [0.0; MAX_KNOBS],
    });
    control.send(Command::Order(Box::new(Order::new(&[0], &[]))));
    end(control);
}

#[test]
fn moment_go_lands_on_bar_with_relative_zero_and_absolute_console() {
    let (mut engine, mut control) = start(37.3);
    let seen = Arc::new(AtomicU64::new(f64::NAN.to_bits()));
    probe(&mut control, &seen);
    render(&mut engine, 10_800);
    assert_eq!(control.shared().unhushed(), 0, "go sounded before beat 40");
    render(&mut engine, 1);
    assert_eq!(control.shared().unhushed(), 1);
    assert_eq!(seen.load(Ordering::Relaxed), 0.0_f64.to_bits());
    assert!(
        control.shared().beat() >= 40.0,
        "console lost absolute beat"
    );
}

#[test]
fn moment_second_go_replaces_the_first_before_landing() {
    let (mut engine, mut control) = start(37.3);
    let first = Arc::new(AtomicU64::new(f64::NAN.to_bits()));
    let second = Arc::new(AtomicU64::new(f64::NAN.to_bits()));
    probe(&mut control, &first);
    render(&mut engine, 800);
    probe(&mut control, &second);
    render(&mut engine, 10_001);
    assert!(
        f64::from_bits(first.load(Ordering::Relaxed)).is_nan(),
        "first go sounded"
    );
    assert_eq!(second.load(Ordering::Relaxed), 0.0_f64.to_bits());
}

#[test]
fn moment_held_and_landing_allocate_nothing() {
    let (mut engine, mut control) = start(39.999);
    moment(&mut control);
    let mut data = [0.0; SUB_BLOCK * 2];
    on_audio_thread(|| engine.render(&mut data, 2));
    assert_eq!(control.shared().unhushed(), 1);
    assert!(control.shared().beat() >= 40.0);
    assert_eq!(control.shared().leaked(), 0);
}

#[test]
fn moment_revisits_render_eight_sample_identical_bars() {
    let (mut engine, mut control) = start(37.3);
    moment(&mut control);
    render(&mut engine, 10_800);
    let first = render(&mut engine, 128_000);
    engine.beat = 1_200.0;
    control
        .shared
        .beat
        .store(1_200.0_f64.to_bits(), Ordering::Relaxed);
    begin(&mut control);
    for slot in 0..9 {
        control.send(Command::Remove { slot });
    }
    end(&mut control);
    render(&mut engine, 16_032);
    engine.beat = 1_201.9;
    control
        .shared
        .beat
        .store(1_201.9_f64.to_bits(), Ordering::Relaxed);
    moment(&mut control);
    render(&mut engine, 8_400);
    let second = render(&mut engine, 128_000);
    assert!(first.iter().any(|sample| sample.abs() > 0.001));
    assert_eq!(
        first
            .iter()
            .zip(&second)
            .position(|(a, b)| a.to_bits() != b.to_bits()),
        None,
        "moment changed samples between visits"
    );
}

#[test]
fn moment_origin_zero_keeps_an_unreplaced_wall_unchanged() {
    let (mut engine, mut control) = start(37.3);
    let seen = Arc::new(AtomicU64::new(f64::NAN.to_bits()));
    control.send(Command::Insert {
        slot: 0,
        tag: 1,
        kind: Kind::CLOCK,
        module: Box::new(BeatProbe(Arc::clone(&seen))),
        knobs: [0.0; MAX_KNOBS],
    });
    control.send(Command::Order(Box::new(Order::new(&[0], &[]))));
    render(&mut engine, 1);
    assert_eq!(seen.load(Ordering::Relaxed), 37.3_f64.to_bits());
    assert_eq!(engine.origin.to_bits(), 0.0_f64.to_bits());
}
