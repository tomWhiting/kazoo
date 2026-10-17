use super::*;

#[derive(Debug)]
struct Counter {
    count: u32,
    resets: Arc<AtomicU32>,
}

impl Module for Counter {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        self.count += 1;
        io.outputs[0].fill(self.count as f32);
    }

    fn reset(&mut self) {
        self.count = 0;
        self.resets.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn an_unheard_module_keeps_its_state() {
    let (mut engine, mut control) = fresh();
    let resets = Arc::new(AtomicU32::new(0));
    let mut knobs = [0.0; MAX_KNOBS];
    for (target, value) in knobs.iter_mut().zip(Kind::LFO.spec().defaults()) {
        *target = value;
    }
    control.send(Command::Insert {
        slot: 0,
        tag: 1,
        kind: Kind::LFO,
        module: Box::new(Counter {
            count: 0,
            resets: Arc::clone(&resets),
        }),
        knobs,
    });
    control.send(Command::Order(Box::new(Order::new(&[0], &[]))));
    let mut buffer = Vec::new();
    render(&mut engine, SUB_BLOCK, &mut buffer);
    assert_eq!(
        engine.room.output(0, 0).unwrap()[0].to_bits(),
        1.0_f32.to_bits()
    );
    control.send(Command::Knob {
        slot: 0,
        knob: 0,
        target: 1.0,
        glide_frames: 1_000,
        wait_frames: 0,
        curve: crate::protocol::GlideCurve::Linear,
    });
    render(&mut engine, SUB_BLOCK, &mut buffer);
    let frozen = control.knob(0, 0).unwrap();
    let resets_before = resets.load(Ordering::Relaxed);
    control.send(Command::Order(Box::new(Order::new(&[], &[]))));
    for _ in 0..100 {
        render(&mut engine, SUB_BLOCK, &mut buffer);
    }
    assert_eq!(
        engine.room.output(0, 0).unwrap()[0].to_bits(),
        2.0_f32.to_bits()
    );
    assert_eq!(control.knob(0, 0).unwrap().to_bits(), frozen.to_bits());
    assert_eq!(resets.load(Ordering::Relaxed), resets_before);
    control.send(Command::Order(Box::new(Order::new(&[0], &[]))));
    render(&mut engine, SUB_BLOCK, &mut buffer);
    assert_eq!(
        engine.room.output(0, 0).unwrap()[0].to_bits(),
        3.0_f32.to_bits()
    );
    assert!(control.knob(0, 0).unwrap() > frozen);
    assert_eq!(resets.load(Ordering::Relaxed), resets_before);
}
