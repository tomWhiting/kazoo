//! Clock stamps follow the song position published by the engine.

use super::*;
use crate::engine::{Engine, EngineConfig, engine};
use crate::protocol::ClockStamp;
use crate::seed::seed;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn wall(store: Option<Store>) -> Result<(Wall, Engine), WallError> {
    let (engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let mut patch = seed()?;
    patch.tempo = 120.0;
    Ok((Wall::new(patch, control, None, Vec::new(), store), engine))
}

fn turn(wall: &mut Wall, value: f64) -> Result<Change, WallError> {
    let outcome = wall.request(
        "Tom",
        &Request::turn("vcf1".to_string(), "cutoff".to_string(), value, Some(0.0)),
        Instant::now(),
    )?;
    outcome
        .change
        .ok_or_else(|| WallError::new(ErrorCode::Internal, "turn made no change"))
}

fn stamp(change: &Change) -> &ClockStamp {
    let Some(stamp) = &change.clock else {
        panic!("change {} has no clock stamp", change.seq);
    };
    stamp
}

fn finite_stamp(line: &mut ClockLine, beat: f64, tempo: f64, following: bool) -> ClockStamp {
    let Some(stamp) = line.stamp(beat, tempo, following) else {
        panic!("finite beat and tempo have no stamp");
    };
    stamp
}

#[test]
fn a_change_carries_the_beat_and_tempo_it_landed_on() -> TestResult {
    let (mut wall, mut engine) = wall(None)?;
    let mut buffer = vec![0.0; 48_000 * 2];
    engine.render(&mut buffer, 2);
    let before = wall.engine.shared().beat();
    assert!(before > 0.0);
    let change = turn(&mut wall, 800.0)?;
    let clock = stamp(&change);
    assert_eq!(clock.beat.to_bits(), before.to_bits());
    assert_eq!(clock.tempo.to_bits(), 120.0_f64.to_bits());
    let event = Event::Change {
        change: Box::new(change.clone()),
    };
    let value = serde_json::to_value(&event)?;
    assert_eq!(value["change"]["clock"], serde_json::to_value(clock)?);
    assert_eq!(serde_json::from_value::<Event>(value)?, event);
    Ok(())
}

#[test]
fn beats_on_one_line_only_go_forward() -> TestResult {
    let (mut wall, mut engine) = wall(None)?;
    let first = turn(&mut wall, 800.0)?;
    let mut buffer = vec![0.0; 24_000 * 2];
    engine.render(&mut buffer, 2);
    let second = turn(&mut wall, 900.0)?;
    let first = stamp(&first);
    let second = stamp(&second);
    assert_eq!(second.line, first.line);
    assert!((second.beat - first.beat - 1.0).abs() < 1e-9);
    Ok(())
}

#[test]
fn a_restart_starts_a_new_line() -> TestResult {
    let (mut first, first_engine) = wall(None)?;
    let first = turn(&mut first, 800.0)?;
    drop(first_engine);
    let (mut second, second_engine) = wall(None)?;
    let second = turn(&mut second, 800.0)?;
    let first = stamp(&first);
    let second = stamp(&second);
    let (first_start, first_n) = first.line.split_once('.').ok_or("missing count")?;
    let (second_start, second_n) = second.line.split_once('.').ok_or("missing count")?;
    assert_ne!(first_start, second_start);
    assert_eq!(first_start.len(), 16);
    assert_eq!(second_start.len(), 16);
    assert!(
        first_start
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );
    assert_eq!(first_n, "0");
    assert_eq!(second_n, "0");
    drop(second_engine);
    Ok(())
}

#[test]
fn a_load_starts_a_new_line_and_is_its_first_stamp() -> TestResult {
    let logs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/kz-logs");
    std::fs::create_dir_all(&logs)?;
    let dir = tempfile::tempdir_in(&logs)?;
    let store = Store::open(dir.path())?;
    let (mut wall, mut engine) = wall(Some(store))?;
    wall.request(
        "Tom",
        &Request::Save {
            name: "clock-test".to_string(),
            overwrite: false,
        },
        Instant::now(),
    )?;
    let mut buffer = vec![0.0; 48_000 * 2];
    engine.render(&mut buffer, 2);
    let first = turn(&mut wall, 800.0)?;
    let loaded = wall.request(
        "Tom",
        &Request::Load {
            name: "clock-test".to_string(),
        },
        Instant::now(),
    )?;
    let load = loaded.change.ok_or("load made no change")?;
    assert!(matches!(load.what, What::Load { .. }));
    let next = turn(&mut wall, 900.0)?;
    let (start, n) = stamp(&first).line.split_once('.').ok_or("missing count")?;
    assert_eq!(n, "0");
    assert_eq!(stamp(&load).line, format!("{start}.1"));
    assert_eq!(stamp(&next).line, stamp(&load).line);
    let logged = std::fs::read_to_string(dir.path().join("log.jsonl"))?;
    let disk_line = logged.lines().last().ok_or("empty change log")?;
    assert_eq!(serde_json::from_str::<Change>(disk_line)?, next);
    println!("STAMPED_LOG_LINE: {disk_line}");
    drop(wall);
    drop(engine);
    dir.close()?;
    Ok(())
}

#[test]
fn the_desk_taking_the_clock_breaks_the_line() {
    let mut line = ClockLine::new(1);
    assert_eq!(
        finite_stamp(&mut line, 10.0, 120.0, false).line,
        "0000000000000001.0"
    );
    line.check(10.5, true);
    assert_eq!(
        finite_stamp(&mut line, 10.5, 120.0, true).line,
        "0000000000000001.1"
    );
    assert_eq!(
        finite_stamp(&mut line, 11.0, 120.0, true).line,
        "0000000000000001.1"
    );
    line.check(11.5, false);
    assert_eq!(
        finite_stamp(&mut line, 11.5, 120.0, false).line,
        "0000000000000001.2"
    );
    // Housekeeping must retain a handover even if it ends before a change.
    line.check(12.0, true);
    line.check(12.5, false);
    assert_eq!(
        finite_stamp(&mut line, 13.0, 120.0, false).line,
        "0000000000000001.4"
    );
}

#[test]
fn a_beat_going_back_breaks_the_line() {
    let mut line = ClockLine::new(2);
    let first = finite_stamp(&mut line, 20.0, 120.0, false);
    let within_tolerance = finite_stamp(&mut line, 20.0 - 5e-10, 120.0, false);
    assert_eq!(first.line, within_tolerance.line);
    let second = finite_stamp(&mut line, 19.0, 120.0, false);
    assert_ne!(first.line, second.line);
}

#[test]
fn a_tempo_change_keeps_the_line() -> TestResult {
    let mut line = ClockLine::new(3);
    let first = finite_stamp(&mut line, 10.0, 120.0, false);
    let second = finite_stamp(&mut line, 11.0, 90.0, false);
    assert_eq!(first.line, second.line);
    assert_eq!(second.tempo.to_bits(), 90.0_f64.to_bits());
    let (mut wall, mut engine) = wall(None)?;
    let first = turn(&mut wall, 800.0)?;
    let changed = wall.request("Tom", &Request::tempo(90.0), Instant::now())?;
    let change = changed.change.ok_or("tempo made no change")?;
    assert_eq!(stamp(&change).tempo.to_bits(), 120.0_f64.to_bits());
    assert_eq!(stamp(&change).line, stamp(&first).line);
    let mut buffer = vec![0.0; 64];
    engine.render(&mut buffer, 2);
    let next = turn(&mut wall, 900.0)?;
    assert_eq!(stamp(&next).tempo.to_bits(), 90.0_f64.to_bits());
    assert_eq!(stamp(&next).line, stamp(&first).line);
    Ok(())
}

#[test]
fn no_beat_from_a_nan() {
    let mut line = ClockLine::new(4);
    let first = finite_stamp(&mut line, 10.0, 120.0, false);
    assert!(line.stamp(f64::NAN, 120.0, false).is_none());
    let next = finite_stamp(&mut line, 11.0, 120.0, false);
    assert_ne!(first.line, next.line);
    assert!(next.line.ends_with(".1"));
    for beat in [f64::INFINITY, f64::NEG_INFINITY] {
        assert!(line.stamp(beat, 120.0, false).is_none());
    }
    for tempo in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(line.stamp(12.0, tempo, false).is_none());
    }
}

/// A wall with a store in a directory under the workspace's target (never
/// the system's temporary directory), its engine, and a save "clock-test".
fn saving_wall() -> Result<(Wall, Engine, tempfile::TempDir), Box<dyn std::error::Error>> {
    let logs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/kz-logs");
    std::fs::create_dir_all(&logs)?;
    let dir = tempfile::tempdir_in(&logs)?;
    let store = Store::open(dir.path())?;
    let (mut wall, engine) = wall(Some(store))?;
    wall.request(
        "Tom",
        &Request::Save {
            name: "clock-test".to_string(),
            overwrite: false,
        },
        Instant::now(),
    )?;
    Ok((wall, engine, dir))
}

fn load(wall: &mut Wall) -> Result<Outcome, WallError> {
    wall.request(
        "Tom",
        &Request::Load {
            name: "clock-test".to_string(),
        },
        Instant::now(),
    )
}

fn fail_once(wall: &mut Wall, at: crate::store::FailAt) -> TestResult {
    wall.store_mut()
        .ok_or("the wall has no store")?
        .fail_once(at);
    Ok(())
}

#[test]
fn a_replacement_that_is_not_written_keeps_the_line() -> TestResult {
    let (mut wall, _engine, dir) = saving_wall()?;
    let first = turn(&mut wall, 800.0)?;
    // The disk refuses the batch before the load's line is written: the
    // wall is not replaced, so the music it plays stays on its line.
    fail_once(&mut wall, crate::store::FailAt::WriteTemp)?;
    let refused = load(&mut wall).err().ok_or("the load was not refused")?;
    assert!(
        refused.message.contains("nothing was replaced"),
        "{}",
        refused.message
    );
    fail_once(&mut wall, crate::store::FailAt::WriteTemp)?;
    let cleared = wall
        .request("Tom", &Request::New, Instant::now())
        .err()
        .ok_or("the new wall was not refused")?;
    assert!(
        cleared.message.contains("nothing was replaced"),
        "{}",
        cleared.message
    );
    let next = turn(&mut wall, 900.0)?;
    assert_eq!(stamp(&next).line, stamp(&first).line);
    // The load that is written after all starts the next line, the first
    // break this run has had.
    let loaded = load(&mut wall)?.change.ok_or("load made no change")?;
    let (start, _) = stamp(&first).line.split_once('.').ok_or("missing count")?;
    assert_eq!(stamp(&loaded).line, format!("{start}.1"));
    drop(wall);
    dir.close()?;
    Ok(())
}

#[test]
fn a_load_that_may_be_logged_keeps_its_line_broken() -> TestResult {
    let (mut wall, _engine, dir) = saving_wall()?;
    let first = turn(&mut wall, 800.0)?;
    // The load's line is written but not synced: it may be in the log, on
    // the line it broke, so that line is never taken again by another load.
    fail_once(&mut wall, crate::store::FailAt::SyncLog)?;
    let refused = load(&mut wall).err().ok_or("the load was not refused")?;
    assert!(
        refused.message.contains("nothing was replaced"),
        "{}",
        refused.message
    );
    let (start, n) = stamp(&first).line.split_once('.').ok_or("missing count")?;
    assert_eq!(n, "0");
    let logged = Store::peek(dir.path())?.load_log()?.changes;
    let [.., maybe, abandoned] = logged.as_slice() else {
        return Err(format!("{logged:?}").into());
    };
    assert!(matches!(maybe.what, What::Load { .. }));
    assert_eq!(stamp(maybe).line, format!("{start}.1"));
    assert!(matches!(abandoned.what, What::Abandoned { .. }));
    assert_eq!(stamp(abandoned).line, format!("{start}.1"));
    let next = turn(&mut wall, 900.0)?;
    assert_eq!(stamp(&next).line, format!("{start}.1"));
    let loaded = load(&mut wall)?.change.ok_or("load made no change")?;
    assert_eq!(stamp(&loaded).line, format!("{start}.2"));
    drop(wall);
    dir.close()?;
    Ok(())
}

#[test]
fn a_new_wall_and_an_undone_load_each_start_a_line() -> TestResult {
    let (mut wall, mut engine, dir) = saving_wall()?;
    let mut buffer = vec![0.0; 4_800 * 2];
    let first = turn(&mut wall, 800.0)?;
    let (start, _) = stamp(&first).line.split_once('.').ok_or("missing count")?;
    engine.render(&mut buffer, 2);
    let cleared = wall
        .request("Tom", &Request::New, Instant::now())?
        .change
        .ok_or("the new wall made no change")?;
    assert!(matches!(cleared.what, What::Load { save: None, .. }));
    assert_eq!(stamp(&cleared).line, format!("{start}.1"));
    engine.render(&mut buffer, 2);
    let loaded = load(&mut wall)?.change.ok_or("load made no change")?;
    assert_eq!(stamp(&loaded).line, format!("{start}.2"));
    engine.render(&mut buffer, 2);
    let undone = wall
        .request("Tom", &Request::Undo { change: loaded.seq }, Instant::now())?
        .change
        .ok_or("the undo made no change")?;
    assert!(matches!(undone.what, What::Load { .. }));
    assert_eq!(undone.undoes, Some(loaded.seq));
    assert_eq!(stamp(&undone).line, format!("{start}.3"));
    // The wall brought back is the empty one: a tempo change rides on it.
    let next = wall
        .request("Tom", &Request::tempo(100.0), Instant::now())?
        .change
        .ok_or("the tempo made no change")?;
    assert_eq!(stamp(&next).line, format!("{start}.3"));
    assert!(stamp(&next).beat >= stamp(&undone).beat);
    drop(wall);
    dir.close()?;
    Ok(())
}
