//! The port against the `pyin` crate 1.2.0 it replaced: `tests/golden.txt`
//! holds what the crate gave for each signal in `tests/signals.rs` under four
//! settings (the voice judge, a scored take, pYIN's defaults, and
//! fine bins on a short frame), one line per frame:
//! `case signal start frame f0 voiced voiced_prob`.

mod signals;

use std::collections::BTreeMap;

use super::{Framing, Pyin, Settings};

const GOLDEN: &str = include_str!("tests/golden.txt");

/// What the crate gave for one run: (f0, voiced, `voiced_prob`) per frame.
type Expected = Vec<(f64, bool, f64)>;

fn golden() -> BTreeMap<(String, String, usize), Expected> {
    let mut runs: BTreeMap<(String, String, usize), Expected> = BTreeMap::new();
    for line in GOLDEN.lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        assert_eq!(fields.len(), 7, "{line}");
        let number = |index: usize| -> f64 {
            fields[index]
                .parse()
                .unwrap_or_else(|error| panic!("{line}: {error}"))
        };
        let start = fields[2]
            .parse()
            .unwrap_or_else(|error| panic!("{line}: {error}"));
        let frame: usize = fields[3]
            .parse()
            .unwrap_or_else(|error| panic!("{line}: {error}"));
        let run = runs
            .entry((fields[0].to_owned(), fields[1].to_owned(), start))
            .or_default();
        assert_eq!(run.len(), frame, "{line}: frames out of order");
        run.push((number(4), fields[5] == "1", number(6)));
    }
    runs
}

const fn settings(case: &str) -> Option<(Settings, Framing, f64)> {
    let rate = signals::RATE;
    let (settings, framing, fill) = match case.as_bytes() {
        b"judge" => (
            Settings {
                fmin: 50.0,
                fmax: 4000.0,
                rate,
                frame_length: 2048,
                win_length: 1024,
                hop_length: 512,
                resolution: 0.34,
            },
            Framing::Valid,
            f64::NAN,
        ),
        b"take" => (
            Settings {
                fmin: 50.0,
                fmax: 4000.0,
                rate,
                frame_length: 2048,
                win_length: 1024,
                hop_length: 256,
                resolution: 0.1,
            },
            Framing::Center(0.0),
            f64::NAN,
        ),
        b"plain" => (
            Settings::plain(60.0, 600.0, rate, 2048),
            Framing::Center(0.0),
            0.0,
        ),
        b"fine" => (
            Settings {
                fmin: 80.0,
                fmax: 1000.0,
                rate,
                frame_length: 1024,
                win_length: 400,
                hop_length: 128,
                resolution: 0.05,
            },
            Framing::Valid,
            f64::NAN,
        ),
        _ => return None,
    };
    Some((settings, framing, fill))
}

/// Every frame of every run: the same voicing, the same pitch and the same
/// voicing probability, bit for bit.
#[test]
fn the_port_gives_what_the_pyin_crate_gave() {
    let signals: BTreeMap<_, _> = signals::all().into_iter().collect();
    let mut trackers = BTreeMap::new();
    let mut differences = Vec::new();
    let mut compared = 0;
    for ((case, name, start), expected) in golden() {
        let (settings, framing, fill) =
            settings(&case).unwrap_or_else(|| panic!("no settings for {case}"));
        let tracker = trackers.entry(case.clone()).or_insert_with(|| {
            Pyin::new(settings).unwrap_or_else(|error| panic!("{case}: {error}"))
        });
        let wav = &signals[name.as_str()];
        let wav = if case == "judge" {
            &wav[start..start + 2048 + 2 * 512]
        } else {
            &wav[..]
        };
        let track = tracker
            .track(wav, fill, framing)
            .unwrap_or_else(|error| panic!("{case} {name}: {error}"));
        assert_eq!(track.f0.len(), expected.len(), "{case} {name} {start}");
        for (frame, &(f0, voiced, prob)) in expected.iter().enumerate() {
            compared += 1;
            let same_f0 = (f0.is_nan() && track.f0[frame].is_nan())
                || f0.to_bits() == track.f0[frame].to_bits();
            if track.voiced[frame] != voiced
                || !same_f0
                || track.voiced_prob[frame].to_bits() != prob.to_bits()
            {
                differences.push(format!(
                    "{case} {name} {start} frame {frame}: crate ({f0}, {voiced}, {prob}), \
                     port ({}, {}, {})",
                    track.f0[frame], track.voiced[frame], track.voiced_prob[frame]
                ));
            }
        }
    }
    assert_eq!(compared, GOLDEN.lines().count());
    assert!(
        differences.is_empty(),
        "{} of {compared} frames differ:\n{}",
        differences.len(),
        differences.join("\n")
    );
}

#[test]
fn settings_pyin_cannot_hold_are_refused_not_panicked_on() {
    let good = Settings::plain(60.0, 600.0, 48_000, 2048);
    assert!(Pyin::new(good).is_ok());
    let refused = [
        Settings { fmin: 0.0, ..good },
        Settings {
            fmin: f64::NAN,
            ..good
        },
        Settings { fmax: 60.0, ..good },
        Settings {
            fmax: 24_001.0,
            ..good
        },
        Settings {
            win_length: 0,
            ..good
        },
        Settings {
            win_length: 2048,
            ..good
        },
        Settings {
            hop_length: 0,
            ..good
        },
        Settings {
            resolution: 0.0,
            ..good
        },
        Settings {
            resolution: 1.0,
            ..good
        },
        Settings {
            resolution: f64::NAN,
            ..good
        },
        // The longest period, 2048 - 2000 - 1, is under the shortest, 80.
        Settings {
            win_length: 2000,
            ..good
        },
        // One bin: 600 Hz to 601 Hz is under a semitone.
        Settings {
            fmin: 600.0,
            fmax: 601.0,
            resolution: 0.9,
            ..good
        },
        // A pitch may move as many bins in one hop as the range holds.
        Settings {
            hop_length: 48_000,
            ..good
        },
    ];
    for settings in refused {
        assert!(
            matches!(Pyin::new(settings), Err(crate::Error::Config(_))),
            "{settings:?} must be refused"
        );
    }
}

#[test]
fn a_signal_shorter_than_a_frame_is_refused() {
    let mut tracker = Pyin::new(Settings::plain(60.0, 600.0, 48_000, 2048))
        .unwrap_or_else(|error| panic!("{error}"));
    let short = vec![0.0; 2047];
    assert!(tracker.track(&short, 0.0, Framing::Valid).is_err());
    // Padded with zeros, one sample is a whole frame.
    let one = tracker
        .track(&[0.5], 0.0, Framing::Center(0.0))
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(one.f0.len(), 1);
    // An odd frame is one sample longer than its two halves of padding.
    let mut odd = Pyin::new(Settings::plain(60.0, 600.0, 48_000, 2049))
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(odd.track(&[], 0.0, Framing::Center(0.0)).is_err());
    assert!(odd.track(&[0.0], 0.0, Framing::Center(0.0)).is_ok());
}

#[test]
fn non_finite_samples_are_silence() {
    let mut tracker = Pyin::new(Settings::plain(60.0, 600.0, 48_000, 2048))
        .unwrap_or_else(|error| panic!("{error}"));
    let tone: Vec<f64> = (0..8192)
        .map(|i| (2.0 * std::f64::consts::PI * 220.0 * f64::from(i) / 48_000.0).sin())
        .collect();
    let mut spoiled = tone.clone();
    for (index, sample) in spoiled.iter_mut().enumerate() {
        if index % 997 == 0 {
            *sample = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY][index % 3];
        }
    }
    let mut silenced = tone;
    for (index, sample) in silenced.iter_mut().enumerate() {
        if index % 997 == 0 {
            *sample = 0.0;
        }
    }
    let spoiled = tracker
        .track(&spoiled, 0.0, Framing::Valid)
        .unwrap_or_else(|error| panic!("{error}"));
    let silenced = tracker
        .track(&silenced, 0.0, Framing::Valid)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(spoiled, silenced);
    assert!(spoiled.voiced_prob.iter().all(|p| p.is_finite()));
}

/// The ceiling sits exactly where the crate's dense table passed 64 MiB:
/// 2896 states of `f64`s squared is just under it, 2898 just over.
#[test]
fn the_state_ceiling_is_the_old_table_ceiling() {
    const {
        assert!(super::MAX_STATES * super::MAX_STATES * 8 <= 64 << 20);
        assert!((super::MAX_STATES + 2) * (super::MAX_STATES + 2) * 8 > 64 << 20);
    }
    // Cent bins from 100 Hz: the range holds floor(1200 log2(fmax / 100)) + 1
    // bins, so fmax half a bin past 1447 bins up makes 1448, 2896 states.
    let at = |bins: f64| Settings {
        fmax: 100.0 * (bins / 1200.0).exp2(),
        resolution: 0.01,
        ..Settings::plain(100.0, 4000.0, 48_000, 2048)
    };
    let grid = at(1447.5).grid().unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(grid.states(), super::MAX_STATES);
    let refused = at(1448.5).grid();
    assert!(
        matches!(refused, Err(crate::Error::Config(ref message)) if message.contains("2898 pyin states")),
        "{refused:?}"
    );
}

/// A candidate below the lowest bin, which panicked in the crate, goes to
/// the lowest bin as librosa clips it. At 1900 to 2400 Hz and 48 kHz the
/// longest period, 26 samples, refined by up to a sample, reads under
/// 1900 Hz.
#[test]
fn a_pitch_under_the_lowest_bin_reads_as_the_lowest() {
    // A hop of 64 keeps the pitch's band (11 bins) inside the range's 41.
    let settings = Settings {
        hop_length: 64,
        ..Settings::plain(1900.0, 2400.0, 48_000, 2048)
    };
    let mut tracker = Pyin::new(settings).unwrap_or_else(|error| panic!("{error}"));
    let tone: Vec<f64> = (0..8192)
        .map(|i| (2.0 * std::f64::consts::PI * 1840.0 * f64::from(i) / 48_000.0).sin())
        .collect();
    let track = tracker
        .track(&tone, f64::NAN, Framing::Valid)
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(track.voiced_prob.iter().all(|p| (0.0..=1.0).contains(p)));
    for (&f0, &voiced) in track.f0.iter().zip(&track.voiced) {
        assert!(!voiced || f0.to_bits() == 1900.0_f64.to_bits(), "{f0}");
    }
    assert!(track.voiced.iter().any(|&voiced| voiced), "{track:?}");
}

#[test]
fn a_non_finite_padding_value_is_silence() {
    let mut tracker = Pyin::new(Settings::plain(60.0, 600.0, 48_000, 2048))
        .unwrap_or_else(|error| panic!("{error}"));
    let tone: Vec<f64> = (0..4096)
        .map(|i| (2.0 * std::f64::consts::PI * 220.0 * f64::from(i) / 48_000.0).sin())
        .collect();
    let silent = tracker
        .track(&tone, 0.0, Framing::Center(0.0))
        .unwrap_or_else(|error| panic!("{error}"));
    for pad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let padded = tracker
            .track(&tone, 0.0, Framing::Center(pad))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(padded, silent, "{pad}");
    }
}
