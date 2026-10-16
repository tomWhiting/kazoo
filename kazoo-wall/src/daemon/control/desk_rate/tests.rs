use super::super::{AudioCounts, Running, backend, link_diagnostics};
use super::*;
use crate::daemon::audio::Headless;
use crate::engine::{EngineConfig, engine};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

#[test]
fn a_desk_refusing_another_rate_is_matched_and_a_gone_desk_gives_the_device_back() {
    // The wall at 48 kHz, refused by a desk at 44.1 kHz: match it.
    assert_eq!(
        follow(Look::Refused(44_100), 48_000, None, 0),
        Follow::Match(44_100)
    );
    // Up, or refused at the wall's own rate: nothing to do.
    assert_eq!(follow(Look::Up, 48_000, None, 0), Follow::Stay);
    assert_eq!(follow(Look::Refused(48_000), 48_000, None, 0), Follow::Stay);
    // Following a desk on the headless loop: stay after one look that
    // misses it; back to the device after two.
    assert_eq!(follow(Look::Gone, 44_100, Some(44_100), 1), Follow::Stay);
    assert_eq!(follow(Look::Gone, 44_100, Some(44_100), 2), Follow::Device);
    // No desk and not following one: nothing.
    assert_eq!(follow(Look::Gone, 48_000, None, 5), Follow::Stay);
    // A desk that comes back at yet another rate is matched again.
    assert_eq!(
        follow(Look::Refused(96_000), 44_100, Some(44_100), 0),
        Follow::Match(96_000)
    );
    // No link at all is no desk.
    assert_eq!(look(None), Look::Gone);
}

/// A look that must not be made.
fn no_look() -> Look {
    panic!("the desk was looked at when no look was due")
}

#[test]
fn the_desk_is_looked_at_once_a_retry_and_never_while_plugged_in() {
    let start = Instant::now();
    let mut watch = Watch::default();
    let mut looks = 0;
    let mut up = || {
        looks += 1;
        Look::Up
    };
    assert_eq!(watch.round(start, 48_000, false, &mut up), Follow::Stay);
    // Every 20 ms tick until the retry is up: no look.
    for tick in 1..100 {
        let now = start + Duration::from_millis(20) * tick;
        assert_eq!(watch.round(now, 48_000, false, no_look), Follow::Stay);
    }
    watch.round(start + DEVICE_RETRY, 48_000, false, &mut up);
    // Plugged in: never looked at, however long.
    for tick in 0..10 {
        let now = start + DEVICE_RETRY * (2 + tick);
        assert_eq!(watch.round(now, 48_000, true, no_look), Follow::Stay);
    }
    assert_eq!(looks, 2);
}

#[test]
fn a_followed_desk_is_gone_only_after_two_looks_a_retry_apart() {
    let start = Instant::now();
    let mut watch = Watch {
        following: Some(44_100),
        ..Watch::default()
    };
    // Plugged in, then unplugged: the first look misses it.
    watch.round(start, 44_100, true, no_look);
    assert_eq!(
        watch.round(start, 44_100, false, || Look::Gone),
        Follow::Stay
    );
    // Ticks before the retry: no look.
    assert_eq!(
        watch.round(start + DEVICE_RETRY / 2, 44_100, false, no_look),
        Follow::Stay
    );
    // The desk is up again (refusing for another reason): the count
    // starts again.
    assert_eq!(
        watch.round(start + DEVICE_RETRY, 44_100, false, || Look::Up),
        Follow::Stay
    );
    assert_eq!(
        watch.round(start + DEVICE_RETRY * 2, 44_100, false, || Look::Gone),
        Follow::Stay
    );
    // A second look in a row, a retry later, misses it: back to the device.
    assert_eq!(
        watch.round(start + DEVICE_RETRY * 3, 44_100, false, || Look::Gone),
        Follow::Device
    );
}

#[test]
fn a_follow_waits_for_recording_loops_and_says_so_once() {
    let start = Instant::now();
    let mut watch = Watch::default();
    watch.round(start, 48_000, false, || Look::Refused(44_100));
    let loops = |names: &[&str]| names.iter().map(ToString::to_string).collect::<Vec<_>>();
    // Nothing to do, or nothing recording: no wait.
    assert_eq!(
        watch.wait_for(Follow::Stay, loops(&["loop1"])),
        (false, None)
    );
    assert_eq!(
        watch.wait_for(Follow::Match(44_100), Vec::new()),
        (false, None)
    );
    // A loop recording: wait, said once.
    assert_eq!(
        watch.wait_for(Follow::Match(44_100), loops(&["loop1"])),
        (
            true,
            Some("loop1 recording; the wall follows the desk's rate once it is done".into())
        )
    );
    // Looked at again at the very next round.
    assert_eq!(
        watch.round(start, 48_000, false, || Look::Refused(44_100)),
        Follow::Match(44_100)
    );
    assert_eq!(
        watch.wait_for(Follow::Match(44_100), loops(&["loop1"])),
        (true, None)
    );
    assert_eq!(
        watch.wait_for(Follow::Match(44_100), loops(&["loop1", "loop2"])),
        (
            true,
            Some(
                "loop1, loop2 recording; the wall follows the desk's rate once they are done"
                    .into()
            )
        )
    );
}

/// A control thread's state with no backend yet, its audio `audio`,
/// joining `desk`, the wall's engine at 48 kHz.
fn control(audio: AudioMode, desk: DeskMode) -> Control {
    let (_engine, engine_control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let wall = super::super::Wall::new(
        crate::seed::seed().unwrap(),
        engine_control,
        None,
        Vec::new(),
        None,
    );
    let (listen_tx, listen_rx) = std::sync::mpsc::channel();
    let now = Instant::now();
    Control {
        wall,
        conns: HashMap::new(),
        audio,
        desk,
        backend: None,
        listen_tx,
        listen_rx,
        stop: Arc::new(AtomicBool::new(false)),
        stopping: false,
        heard: false,
        last_tick: now,
        awaiting: HashMap::new(),
        counts: AudioCounts::new(now),
        told_link_refused: false,
        desk_refusals: link_diagnostics::DeskRefusals::default(),
        reported_link_shutdowns: link_diagnostics::LinkShutdowns::default(),
        desk_rate: Watch::default(),
    }
}

fn renders_headless_at(control: &mut Control, rate: u32) {
    assert_eq!(control.wall.engine_mut().sample_rate(), rate);
    assert!(matches!(
        control.backend.as_ref().map(|backend| &backend.running),
        Some(Running::Headless(_))
    ));
}

#[test]
fn a_desk_at_another_rate_than_the_device_is_rendered_for_on_the_headless_loop() {
    let mut control = control(AudioMode::Device { sample_rate: None }, DeskMode::Discover);
    control.follow_with(Follow::Match(44_100), Instant::now(), || {
        Err("no device in a test".to_string())
    });
    renders_headless_at(&mut control, 44_100);
    assert_eq!(control.desk_rate.following(), Some(44_100));
    // Nothing to retry: the desk plays the wall while it is up.
    assert_eq!(
        control
            .backend
            .as_ref()
            .and_then(|backend| backend.retry_at),
        None
    );
}

#[test]
fn a_render_loop_rendering_for_a_desk_starts_again_at_the_desks_rate() {
    let mut control = control(AudioMode::Device { sample_rate: None }, DeskMode::Off);
    control.desk_rate.following = Some(44_100);
    control.rebuild(Instant::now());
    // Never back on the device while the desk is up.
    renders_headless_at(&mut control, 44_100);
    assert_eq!(
        control
            .backend
            .as_ref()
            .and_then(|backend| backend.retry_at),
        None
    );
    assert_eq!(control.desk_rate.following(), Some(44_100));
}

#[test]
fn a_render_loop_that_could_not_start_is_tried_again_after_the_retry_not_every_round() {
    let mut control = control(
        AudioMode::Headless {
            sample_rate: 48_000,
        },
        DeskMode::Off,
    );
    let now = Instant::now();
    let (_engine, mut engine_control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let retry = now + DEVICE_RETRY;
    control.backend = Some(backend(
        Running::Headless(Headless::stopped()),
        &mut engine_control,
        &control.listen_tx.clone(),
        Some(retry),
    ));
    for tick in 0..50 {
        control.watch_audio(now + Duration::from_millis(20) * tick);
        assert_eq!(
            control
                .backend
                .as_ref()
                .and_then(|backend| backend.retry_at),
            Some(retry),
            "rebuilt before the retry"
        );
    }
    control.watch_audio(retry);
    assert_eq!(
        control
            .backend
            .as_ref()
            .and_then(|backend| backend.retry_at),
        None
    );
    renders_headless_at(&mut control, 48_000);
}

#[test]
fn only_a_wall_on_its_device_as_set_joining_a_desk_looks_at_it() {
    let now = Instant::now();
    for (audio, desk) in [
        (AudioMode::Device { sample_rate: None }, DeskMode::Off),
        (
            AudioMode::Device {
                sample_rate: Some(48_000),
            },
            DeskMode::Discover,
        ),
        (
            AudioMode::Headless {
                sample_rate: 48_000,
            },
            DeskMode::Discover,
        ),
    ] {
        let mut control = control(audio, desk);
        control.follow_desk_rate(now);
        assert_eq!(control.desk_rate.due, None, "{audio:?}");
        assert!(control.backend.is_none());
    }
    // A wall on its device as set, joining a desk, looks (no link here:
    // no desk), and nothing changes.
    let mut control = control(AudioMode::Device { sample_rate: None }, DeskMode::Discover);
    control.follow_desk_rate(now);
    assert_eq!(control.desk_rate.due, Some(now + DEVICE_RETRY));
    assert_eq!(control.desk_rate.missed, 1);
    assert!(control.backend.is_none());
}
