use super::*;

fn play(wall: &mut Wall, engine: &mut Engine) {
    let mut buffer = [0.0; SUB_BLOCK * 2];
    loop {
        engine.render(&mut buffer, 2);
        assert!(wall.engine.pump().is_empty());
        if wall.engine.backlog() == 0 {
            break;
        }
    }
}

#[test]
fn only_a_delayed_cable_gets_a_line_and_keeps_it() {
    let (mut wall, mut engine) = wall();
    play(&mut wall, &mut engine);
    let needed = wall
        .timing
        .delays
        .values()
        .filter(|delay| **delay > 0)
        .count();
    assert_eq!(wall.engine.shared().lines(), needed);
    let reverb = wall
        .patch
        .modules()
        .iter()
        .find(|module| crate::seed::SEED_REVERBS.contains(&module.kind.name()))
        .unwrap()
        .id
        .clone();
    ask(&mut wall, &add("klon")).unwrap();
    ask(&mut wall, &add("mix")).unwrap();
    ask(&mut wall, &patch("vco1.out", "klon1.left")).unwrap();
    ask(&mut wall, &patch("klon1.left", "mix2.a")).unwrap();
    ask(&mut wall, &patch(&format!("{reverb}.left"), "mix2.b")).unwrap();
    ask(&mut wall, &patch("mix2.out", "out1.left")).unwrap();
    play(&mut wall, &mut engine);
    let before = wall.engine.shared().lines();
    assert!(before > 0);
    let dry = wall
        .patch
        .cables()
        .iter()
        .find(|cable| cable.from_module == reverb && cable.to_module == "mix2")
        .unwrap()
        .id;
    let tag = wall.lined[&dry];
    let wet = wall
        .patch
        .cables()
        .iter()
        .find(|cable| cable.from_module == "klon1" && cable.to_module == "mix2")
        .unwrap()
        .id;
    ask(
        &mut wall,
        &Request::Unpatch {
            cable: Some(wet),
            to: None,
        },
    )
    .unwrap();
    play(&mut wall, &mut engine);
    assert!(!wall.timing.delays.contains_key(&dry));
    assert_eq!(wall.lined[&dry], tag);
    assert_eq!(wall.engine.shared().lines(), before);
    ask(&mut wall, &patch("klon1.left", "mix2.a")).unwrap();
    play(&mut wall, &mut engine);
    ask(&mut wall, &patch("lfo2.out", "vcf1.cutoff")).unwrap();
    play(&mut wall, &mut engine);
    assert_eq!(wall.engine.shared().lines(), before);
    ask(&mut wall, &Request::Remove { module: reverb }).unwrap();
    play(&mut wall, &mut engine);
    assert!(wall.engine.shared().lines() < before);
}
