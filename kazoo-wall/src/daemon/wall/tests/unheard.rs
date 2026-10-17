use super::*;

#[derive(Debug)]
struct CountRenders(Arc<AtomicU32>);

impl DspModule for CountRenders {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        self.0.fetch_add(1, Ordering::Relaxed);
        io.outputs[0].copy_from_slice(&io.inputs[0]);
    }

    fn reset(&mut self) {}
}

#[test]
fn a_module_reaching_nothing_is_unheard_and_said_so() {
    let (mut wall, mut engine) = wall();
    let calls = Arc::new(AtomicU32::new(0));
    let slot = wall.rack.slot_of("vcf1").unwrap();
    let mut knobs = [0.0; MAX_KNOBS];
    for (target, value) in knobs
        .iter_mut()
        .zip(&wall.patch.module("vcf1").unwrap().knobs)
    {
        *target = *value;
    }
    wall.engine.send(Command::Insert {
        slot,
        tag: wall.rack.slots[slot].as_ref().unwrap().1,
        kind: Kind::VCF,
        module: Box::new(CountRenders(Arc::clone(&calls))),
        knobs,
    });
    let tail = wall
        .patch
        .cables()
        .iter()
        .find(|cable| cable.to_module == "out1")
        .map(|cable| {
            format!(
                "{}.{}",
                cable.from_module,
                wall.patch
                    .module(&cable.from_module)
                    .unwrap()
                    .kind
                    .spec()
                    .outputs[cable.from_port]
                    .name
            )
        })
        .unwrap();
    ask(&mut wall, &Request::remove("out1".to_string())).unwrap();
    let expected: Vec<String> = wall
        .patch
        .modules()
        .iter()
        .map(|module| module.id.clone())
        .collect();
    let look: Snapshot = serde_json::from_value(ask(&mut wall, &Request::Look).unwrap()).unwrap();
    assert_eq!(look.timing.unheard, expected);
    ask(
        &mut wall,
        &Request::turn("vcf1".to_string(), "cutoff".to_string(), 200.0, Some(1.0)),
    )
    .unwrap();
    let before = wall.engine.shared().frames();
    let mut buffer = vec![0.0; 48_000 * 2];
    assert_no_alloc::assert_no_alloc(|| engine.render(&mut buffer, 2));
    assert_eq!(wall.engine.shared().frames() - before, 48_000);
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert!(buffer.iter().all(|sample| *sample == 0.0));
    assert!(wall.snapshot().probes.is_empty());
    let frozen = wall
        .snapshot()
        .modules
        .into_iter()
        .find(|module| module.id == "vcf1")
        .unwrap();
    assert_eq!(frozen.knobs[0].target.to_bits(), 200.0_f64.to_bits());
    assert_ne!(frozen.knobs[0].value.to_bits(), 200.0_f64.to_bits());
    ask(&mut wall, &add("out")).unwrap();
    ask(&mut wall, &patch(&tail, "out2.left")).unwrap();
    ask(&mut wall, &add("readout")).unwrap();
    ask(&mut wall, &patch("vcf1.out", "readout1.in")).unwrap();
    assert!(wall.snapshot().timing.unheard.is_empty());
    assert_no_alloc::assert_no_alloc(|| engine.render(&mut buffer, 2));
    assert!(calls.load(Ordering::Relaxed) > 0);
    assert!(
        probed(&mut wall, Some("readout1")).probes[0]
            .readout
            .is_some()
    );
}

#[test]
fn a_scope_makes_what_it_reads_heard() {
    let (engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let mut isolated = Patch::empty(120.0);
    isolated.add("lfo", None, None).unwrap();
    let mut wall = Wall::new(isolated, control, None, Vec::new(), None);
    assert_eq!(wall.snapshot().timing.unheard, ["lfo1"]);
    ask(&mut wall, &add("scope")).unwrap();
    assert_eq!(wall.snapshot().timing.unheard, ["lfo1"]);
    ask(&mut wall, &patch("lfo1.out", "scope1.a")).unwrap();
    assert!(wall.snapshot().timing.unheard.is_empty());
    drop(engine);
}

type Script = std::sync::Arc<std::sync::Mutex<crate::daemon::speech::tests::Script>>;

/// Gives `speak1` new words `times` times, rendering a block after each as
/// the audio thread would, and answers the last refusal, if any.
fn speak_often(
    wall: &mut Wall,
    engine: &mut Engine,
    script: &Script,
    times: usize,
) -> Option<WallError> {
    let mut buffer = vec![0.0; SUB_BLOCK * 2];
    let mut refused = None;
    for time in 0..times {
        let words = Request::Speak {
            module: "speak1".to_string(),
            text: format!("words number {time}"),
            voice: None,
        };
        ask(wall, &words).unwrap();
        script.lock().unwrap().finish_all();
        for (_, answer) in wall.spoken() {
            if let Err(err) = answer {
                refused = Some(err);
            }
        }
        engine.render(&mut buffer, 2);
    }
    refused
}

#[test]
fn an_unheard_speaker_given_too_many_words_says_why_it_takes_none() {
    let (mut engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let mut wall = Wall::new(Patch::empty(120.0), control, None, Vec::new(), None);
    let (speech, script) = crate::daemon::speech::tests::scripted(16);
    wall.speech = speech;
    ask(&mut wall, &add("speak")).unwrap();
    assert_eq!(wall.snapshot().timing.unheard, ["speak1"]);
    // Heard, its player takes every phrase as it comes.
    ask(&mut wall, &add("out")).unwrap();
    ask(&mut wall, &patch("speak1.out", "out1.left")).unwrap();
    assert!(speak_often(&mut wall, &mut engine, &script, 12).is_none());
    // Unheard, its player takes none: the refusal says why, and that
    // waiting will not help.
    ask(&mut wall, &Request::remove("out1".to_string())).unwrap();
    assert_eq!(wall.snapshot().timing.unheard, ["speak1"]);
    let refused = speak_often(&mut wall, &mut engine, &script, 12).unwrap();
    assert_eq!(refused.code, ErrorCode::NotAllowed, "{refused:?}");
    assert!(
        refused
            .message
            .contains("speak1 reaches no out, scope, readout or loop"),
        "{}",
        refused.message
    );
}

/// A mixer stand-in: the loudest of the first `first` frames its `b`
/// input heard in the block it last rendered.
#[derive(Debug)]
struct HearB {
    first: usize,
    loudest: Arc<AtomicU32>,
}

impl DspModule for HearB {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        let loudest = io.inputs[1][..self.first]
            .iter()
            .fold(0.0_f32, |most, sample| most.max(sample.abs()));
        self.loudest.store(loudest.to_bits(), Ordering::Relaxed);
        io.outputs[0].copy_from_slice(&io.inputs[0]);
    }

    fn reset(&mut self) {}
}

#[test]
fn a_held_back_cable_heard_again_plays_nothing_from_before_it_was_unheard() {
    let (mut engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let mut wall = Wall::new(Patch::empty(120.0), control, None, Vec::new(), None);
    for kind in ["vco", "klon", "mix", "readout"] {
        ask(&mut wall, &add(kind)).unwrap();
    }
    for (from, to) in [
        ("vco1.out", "klon1.left"),
        ("klon1.left", "mix1.a"),
        ("vco1.out", "mix1.b"),
        ("mix1.out", "readout1.in"),
    ] {
        ask(&mut wall, &patch(from, to)).unwrap();
    }
    // The dry cable into the mixer is held back to meet the drive.
    let dry = wall
        .patch
        .cables()
        .iter()
        .find(|cable| cable.from_module == "vco1" && cable.to_module == "mix1")
        .map(|cable| cable.id.to_string())
        .unwrap();
    let delay = wall.snapshot().timing.cables[&dry] as usize;
    assert!(delay > 0);
    let loudest = Arc::new(AtomicU32::new(0));
    let slot = wall.rack.slot_of("mix1").unwrap();
    let mut knobs = [0.0; MAX_KNOBS];
    for (target, value) in knobs
        .iter_mut()
        .zip(&wall.patch.module("mix1").unwrap().knobs)
    {
        *target = *value;
    }
    wall.engine.send(Command::Insert {
        slot,
        tag: wall.rack.slots[slot].as_ref().unwrap().1,
        kind: Kind::MIX,
        module: Box::new(HearB {
            first: delay.min(SUB_BLOCK),
            loudest: Arc::clone(&loudest),
        }),
        knobs,
    });
    // Heard: the dry cable's line fills with the oscillator.
    let mut buffer = vec![0.0; 4_800 * 2];
    engine.render(&mut buffer, 2);
    assert!(f32::from_bits(loudest.load(Ordering::Relaxed)) > 0.0);
    // Unheard, then heard again: the line starts as a new cable's does,
    // silent for its delay, not replaying what it held when it went unheard.
    ask(
        &mut wall,
        &Request::unpatch(None, Some("readout1.in".to_string())),
    )
    .unwrap();
    assert_eq!(wall.snapshot().timing.unheard, ["vco1", "klon1", "mix1"]);
    engine.render(&mut buffer, 2);
    ask(&mut wall, &patch("mix1.out", "readout1.in")).unwrap();
    assert!(wall.snapshot().timing.unheard.is_empty());
    assert_eq!(wall.snapshot().timing.cables[&dry] as usize, delay);
    let mut block = vec![0.0; SUB_BLOCK * 2];
    engine.render(&mut block, 2);
    assert_eq!(loudest.load(Ordering::Relaxed), 0.0_f32.to_bits());
}
