//! Everything that runs on the audio thread, run under `assert_no_alloc`:
//! any allocation or free inside aborts this test binary.

use std::sync::Arc;
use std::time::Duration;

use assert_no_alloc::assert_no_alloc;
use kazoo_sampler::player::{PARAM_COUNT, index};
use kazoo_sampler::{
    PlayerInputs, Recorder, SampleData, SampleName, SamplePlayer, SampleStore, StoreLimits,
    TakeRequest,
};

#[global_allocator]
static A: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

const RATE: u32 = 48_000;
const BLOCK: usize = 128;

fn sample(name: &str, frames: usize) -> Arc<SampleData> {
    let left: Vec<f32> = (0..frames)
        .map(|n| (n as f32 * 0.031).sin() * 0.5 + if n % 9_000 < 40 { 0.4 } else { 0.0 })
        .collect();
    let right: Vec<f32> = left.iter().map(|x| x * 0.7).collect();
    Arc::new(SampleData::new(SampleName::new(name).unwrap(), RATE, left, right).unwrap())
}

#[test]
fn the_record_tap_never_allocates() {
    let dir = tempfile::tempdir().unwrap();
    let store = SampleStore::open(dir.path(), StoreLimits::default()).unwrap();
    let (mut recorder, mut tap) = Recorder::new(store, RATE, 0.1).unwrap();
    let left: Vec<f32> = (0..BLOCK).map(|n| n as f32 / BLOCK as f32).collect();
    let right = left.clone();
    let poison = [f32::NAN; BLOCK];
    let too_long = vec![0.25f32; RATE as usize];

    let id = recorder
        .start(TakeRequest::new(SampleName::new("rt").unwrap()))
        .unwrap();
    for block in 0..400 {
        assert_no_alloc(|| match block % 50 {
            0 => tap.process(&poison, &right),
            1 => tap.process(&too_long, &too_long),
            2 => tap.process(&left[..3], &right),
            _ => tap.process(&left, &right),
        });
        std::thread::sleep(Duration::from_micros(100));
    }
    recorder.stop().unwrap();
    assert_no_alloc(|| tap.process(&left, &right));
    let report = recorder.wait(Duration::from_secs(10)).expect("a report");
    assert_eq!(report.id, id);
    let take = report.result.unwrap();
    assert!(
        take.dropped_frames > 0,
        "the oversized blocks overran the ring"
    );
    assert!(!tap.is_recording());
}

#[test]
fn the_player_never_allocates() {
    let first = sample("first", 48_000);
    let second = sample("second", 20_000);
    let (mut player, mut reaper) = SamplePlayer::new();
    player.prepare(RATE as f32);
    let mut left = [0.0f32; BLOCK];
    let mut right = [0.0f32; BLOCK];
    let mut gate = [0.0f32; BLOCK];
    let pitch: Vec<f32> = (0..BLOCK)
        .map(|n| (n as f32 / BLOCK as f32) - 0.5)
        .collect();
    let velocity = [0.7f32; BLOCK];
    let bad = [f32::NAN, f32::INFINITY, -1e30, 1e30];

    let mut next = Some(Arc::clone(&first));
    let mut spare = Some(Arc::clone(&second));
    for round in 0..600usize {
        // Hand samples over and take them back as a host would: the Arcs are
        // made and dropped out here, moved in and out in there.
        let arriving = if round % 97 == 5 { next.take() } else { None };
        let unloading = round % 211 == 150;
        let mode = (round / 40) % 6;
        let returned = assert_no_alloc(|| {
            let returned = arriving.and_then(|data| player.load(data).err());
            if unloading {
                player.unload();
            }
            player.set_param(index::MODE, mode as f32);
            player.set_param(index::REVERSE, (round / 13 % 2) as f32);
            player.set_param(index::PITCH, (round % 50) as f32 - 25.0);
            player.set_param(index::LOOP_START, 0.2);
            player.set_param(index::LOOP_END, 0.6);
            player.set_param(index::SLICES, (round % 5) as f32);
            player.set_param(index::SLICE, (round % 7) as f32 / 7.0);
            player.set_param(index::FREEZE, (round % 2) as f32);
            player.set_param(index::SPREAD, 12.0);
            player.set_param(round % (PARAM_COUNT + 3), f32::NAN);
            player.set_param(round % (PARAM_COUNT + 3), 0.5);
            if round % 3 == 0 {
                player.note_on((40 + round % 40) as u8, 0.8);
            }
            if round % 5 == 0 {
                player.note_off((40 + round % 40) as u8);
            }
            if round % 101 == 0 {
                player.all_notes_off();
            }
            for (i, g) in gate.iter_mut().enumerate() {
                *g = if (round * BLOCK + i) % 3_000 < 2_000 {
                    1.0
                } else {
                    0.0
                };
            }
            let inputs = PlayerInputs {
                gate: &gate,
                pitch: &pitch,
                velocity: &velocity,
            };
            player.process(&inputs, [&mut left, &mut right]);
            player.process(
                &PlayerInputs {
                    gate: &bad,
                    pitch: &bad,
                    velocity: &bad,
                },
                [&mut left[..4], &mut right[..4]],
            );
            if round % 250 == 0 {
                player.reset();
            }
            returned
        });
        assert!(left.iter().chain(&right).all(|x| x.is_finite()));
        if let Some(back) = returned {
            next = Some(back);
        }
        if next.is_none() {
            next = spare.take();
        }
        reaper.collect();
    }
    assert!(player.sample().is_some() || next.is_some() || spare.is_none());
    drop(player);
    reaper.collect();
    assert_eq!(Arc::strong_count(&first), 1);
    assert_eq!(Arc::strong_count(&second), 1);
}

#[test]
fn reading_a_sample_never_allocates() {
    let data = sample("read", 10_000);
    let mut sum = 0.0f32;
    assert_no_alloc(|| {
        let mut position = -10.0f64;
        for speed in [0.25, 1.0, 1.5, 3.0, 17.0, 200.0, f64::NAN, f64::INFINITY] {
            for _ in 0..500 {
                let (l, r) = data.read(position, speed);
                sum += l + r;
                position += 13.7;
            }
        }
        sum += data.read(f64::NAN, 1.0).0;
    });
    assert!(sum.is_finite());
}
