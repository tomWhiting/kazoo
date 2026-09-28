//! Every real-time path, run under `assert_no_alloc`: any allocation or
//! free inside the guarded closures aborts the test.

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use kazoo_speech::player::{self, Phrase, SpeechPlayer};
use kazoo_speech::vocoder::{self, Vocoder};

#[global_allocator]
static A: AllocDisabler = AllocDisabler;

const RATE: f32 = 48_000.0;

fn signal(len: usize, hz: f32) -> Vec<f32> {
    (0..len)
        .map(|n| 0.4 * (std::f32::consts::TAU * hz * n as f32 / RATE).sin())
        .collect()
}

#[test]
fn the_vocoder_never_allocates() {
    let mut vocoder = Vocoder::new(RATE);
    let carrier = signal(4_096, 110.0);
    let voice = signal(4_096, 700.0);
    let mut hiss: Vec<f32> = (0..4_096u32)
        .map(|n| ((n.wrapping_mul(2_654_435_761) >> 8) as f32 / 8_388_608.0) - 1.0)
        .collect();
    hiss[17] = f32::NAN;
    hiss[99] = f32::INFINITY;
    let mut left = vec![0.0; 4_096];
    let mut right = vec![0.0; 4_096];
    assert_no_alloc(|| {
        for bands in [8.0, 40.0, 13.0, 27.0, 20.0] {
            vocoder.set_param(vocoder::param::BANDS, bands);
            for index in 1..vocoder::param::COUNT {
                let spec = &vocoder::PARAMS[index];
                vocoder.set_param(index, (spec.min + spec.max) * 0.5);
            }
            vocoder.set_param(vocoder::param::SHIFT, bands - 20.0);
            for start in (0..4_096).step_by(96) {
                let end = (start + 96).min(4_096);
                vocoder.process(
                    [&carrier[start..end], &carrier[start..end]],
                    &voice[start..end],
                    [&mut left[start..end], &mut right[start..end]],
                );
            }
            vocoder.process([&carrier, &hiss], &hiss, [&mut left, &mut right]);
        }
        vocoder.set_param(vocoder::param::BANDS, 36.0);
        vocoder.reset();
        vocoder.process([&carrier, &carrier], &voice, [&mut left, &mut right]);
        vocoder.set_param(vocoder::param::HOLD, 1.0);
        vocoder.process([&carrier, &carrier], &voice[..100], [&mut left, &mut right]);
    });
    assert_eq!(vocoder.active_bands(), 36);
    assert!(left.iter().chain(&right).all(|x| x.is_finite()));
}

#[test]
fn the_player_never_allocates_or_frees() {
    let (mut player, mut feed) = SpeechPlayer::new(RATE);
    let first = Phrase::new(signal(24_000, 330.0), 48_000);
    let second = Phrase::new(signal(12_000, 440.0), 24_000);
    let mut out = vec![0.0; 256];
    let mut gate = vec![0.0; 256];
    gate[64..192].fill(1.0);

    assert!(feed.load(first, true).is_ok());
    assert_no_alloc(|| {
        for mode in 0..3 {
            player.set_param(player::param::MODE, mode as f32);
            for timing in 0..2 {
                player.set_param(player::param::TIMING, timing as f32);
                player.set_param(player::param::RATE, (timing as f32).mul_add(3.0, 0.25));
                player.set_param(player::param::START, 0.3);
                for _ in 0..40 {
                    player.process(&gate, &mut out);
                }
                player.trigger();
                player.process(&[], &mut out);
                player.trigger();
                player.release();
                player.process(&[1.0, f32::NAN], &mut out);
            }
        }
        player.reset();
    });

    // A new phrase while the old one plays: the old goes back to the feed
    // rather than being freed here.
    assert!(feed.load(second, true).is_ok());
    assert_no_alloc(|| {
        player.process(&[], &mut out);
        player.process(&[], &mut out);
    });
    assert!(player.is_playing());
    assert_eq!(feed.collect(), 1);

    assert!(feed.unload());
    assert_no_alloc(|| {
        player.process(&[], &mut out);
    });
    assert!(player.phrase().is_none());
    assert_eq!(feed.collect(), 1);
    assert!(out.iter().all(|x| x.is_finite()));
}
