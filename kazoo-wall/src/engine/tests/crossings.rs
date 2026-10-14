use super::*;

#[derive(Debug)]
struct Position(Arc<AtomicU64>);
impl Module for Position {
    fn process(&mut self, tick: &Tick, _io: Io<'_>) {
        self.0.store(tick.beat.to_bits(), Ordering::Relaxed);
    }
    fn reset(&mut self) {}
}

fn held(control: &mut EngineControl, seen: &Arc<AtomicU64>) {
    control.begin_swap();
    control.send(Command::Insert {
        slot: 0,
        tag: 1,
        kind: Kind::CLOCK,
        module: Box::new(Position(Arc::clone(seen))),
        knobs: [0.0; MAX_KNOBS],
    });
    control.send(Command::Order(Box::new(Order::new(&[0], &[]))));
    control.end_cross();
}

fn render(engine: &mut Engine, frames: usize) {
    let mut buffer = vec![0.0; frames * 2];
    on_audio_thread(|| engine.render(&mut buffer, 2));
}

fn to_bar(engine: &mut Engine, control: &EngineControl, beat: f64) {
    loop {
        if control.shared().beat() >= beat - 1e-9 {
            break;
        }
        render(engine, 1);
    }
    render(engine, SUB_BLOCK + 1);
}

#[test]
fn a_cross_lands_with_relative_zero_and_keeps_the_master_gain() {
    let (mut engine, mut control) = engine(EngineConfig::new(8_000, 120.0, 37.3), None);
    let seen = Arc::new(AtomicU64::new(f64::NAN.to_bits()));
    held(&mut control, &seen);
    render(&mut engine, 10_800);
    assert_eq!(control.shared().unhushed(), 0);
    assert_eq!(engine.hush_gain.to_bits(), 1.0_f32.to_bits());
    render(&mut engine, 1);
    assert_eq!(seen.load(Ordering::Relaxed), 0.0_f64.to_bits());
    assert_eq!(control.shared().landing_frame(), 10_800);
    assert_eq!(control.shared().origin().to_bits(), 40.0_f64.to_bits());
    assert_eq!(engine.hush_gain.to_bits(), 1.0_f32.to_bits());
    assert!(!engine.hushed);
    assert_eq!(control.shared().leaked(), 0);
}

#[test]
fn a_cross_received_after_its_bar_reaims_to_the_next_bar() {
    let (mut engine, mut control) = engine(EngineConfig::new(8_000, 120.0, 37.3), None);
    let seen = Arc::new(AtomicU64::new(f64::NAN.to_bits()));
    held(&mut control, &seen);
    engine.beat = 40.5;
    render(&mut engine, SUB_BLOCK);
    assert_eq!(control.shared().unhushed(), 0);
    to_bar(&mut engine, &control, 44.0);
    assert_eq!(control.shared().origin().to_bits(), 44.0_f64.to_bits());
    assert_eq!(engine.hush_gain.to_bits(), 1.0_f32.to_bits());
}

#[test]
fn a_desk_jump_back_reaims_a_cross_to_the_bar_after_it() {
    let (mut engine, mut control) = engine(EngineConfig::new(8_000, 120.0, 37.3), None);
    let seen = Arc::new(AtomicU64::new(f64::NAN.to_bits()));
    held(&mut control, &seen);
    render(&mut engine, 400);
    engine.beat = 2.5;
    control
        .shared
        .beat
        .store(2.5_f64.to_bits(), Ordering::Relaxed);
    to_bar(&mut engine, &control, 4.0);
    assert_eq!(control.shared().unhushed(), 1);
    assert_eq!(control.shared().origin().to_bits(), 4.0_f64.to_bits());
    assert_eq!(engine.hush_gain.to_bits(), 1.0_f32.to_bits());
}

#[test]
fn a_second_held_cross_keeps_prepared_slots_and_intervening_edits() {
    let (mut engine, mut control) = engine(EngineConfig::new(8_000, 120.0, 37.3), None);
    let seen = Arc::new(AtomicU64::new(f64::NAN.to_bits()));
    held(&mut control, &seen);
    render(&mut engine, 400);
    control.send(Command::Knob {
        slot: 0,
        knob: 0,
        target: 2.0,
        glide_frames: 0,
    });
    control.begin_swap();
    control.send(Command::Knob {
        slot: 0,
        knob: 0,
        target: 3.0,
        glide_frames: 0,
    });
    control.end_cross();
    assert_eq!(control.landing_beat().to_bits(), 40.0_f64.to_bits());
    to_bar(&mut engine, &control, 40.0);
    assert_eq!(control.shared().unhushed(), 2);
    assert!(control.knob(0, 0).is_some());
    assert_eq!(engine.hush_gain.to_bits(), 1.0_f32.to_bits());
    assert_eq!(control.shared().leaked(), 0);
}
