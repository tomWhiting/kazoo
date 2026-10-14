use super::*;
use kazoo_wall::protocol::LoopsMeet;

fn tape() -> LoopView {
    LoopView {
        state: LoopState::Armed,
        beats: 68.0,
        frames: 1_632_000,
        rate: 48_000,
        anchor: 8.0,
        pass: 3,
        written: (0..17).map(|bar| bar < 8 || bar == 11).collect(),
        ages: (0..17).map(|bar| if bar < 4 { 2 } else { 0 }).collect(),
        bins: vec![0; 64],
        peak_db: -12.0,
        from: Some("mouth/dry".into()),
        take: Some(812),
        reel: Some("9f3c01ab".into()),
        shortened: false,
        notes: vec!["Tape restored.".into()],
    }
}

#[test]
fn a_loop_is_described_in_words() {
    let mut wall = super::super::tests::snapshot();
    wall.tempo = 120.0;
    wall.beat = 30.0;
    let module = &mut wall.modules[0];
    module.id = "loop1".into();
    module.kind = "loop".into();
    module.name = None;
    module.knobs[0].name = "wear".into();
    module.knobs[0].value = 0.3;
    module.tape = Some(tape());
    let text = super::super::look(&wall);
    assert!(text.contains("loop1 (loop): recording, armed for bar 9 · 17 bars (34.0 s at 120 BPM, spliced at bar 3) · bar 6 of 17, pass 3 · written bars 1–8, 12 · wear 0.30: bars 1–4 are 2 passes old · from mouth/dry · take 812 (reel 9f3c01ab). Tape restored."), "{text}");
    assert!(!describe(&tape(), 0.0, 120.0, 30.0).contains("passes old"));
}

#[test]
fn the_meet_sentence_and_paused_checkpoints_are_said() {
    let mut wall = super::super::tests::snapshot();
    wall.tempo = 120.0;
    wall.modules[0].tape = Some(tape());
    let mut second = wall.modules[0].clone();
    second.id = "loop2".into();
    second.tape.as_mut().unwrap().beats = 92.0;
    wall.modules.push(second);
    wall.loops_meet = Some(LoopsMeet {
        bars: Some(391),
        seconds: Some(782.0),
        next_bar: Some(392),
    });
    wall.checkpoints_paused = true;
    let text = super::super::look(&wall);
    assert!(text.contains("Loops 17 and 23 bars long meet again every 391 bars (13 min 2 s at 120 BPM); next at bar 392."), "{text}");
    assert!(
        text.contains("Checkpoints are paused: the reel store is full."),
        "{text}"
    );
    wall.loops_meet = Some(LoopsMeet {
        bars: None,
        seconds: None,
        next_bar: None,
    });
    assert!(super::super::look(&wall).contains("practically never"));
    wall.loops_meet = Some(LoopsMeet {
        bars: Some(4),
        seconds: Some(8.0),
        next_bar: None,
    });
    assert!(super::super::look(&wall).contains("These loops never all meet at their seams."));
}
