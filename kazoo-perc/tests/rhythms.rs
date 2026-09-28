//! Every rhythm generator, driven by real clock signals: patterns, sample
//! accurate edges, resets, extremes and bursts.

use kazoo_perc::{MAX_OUTPUTS, Rhythm, find_rhythm, rhythms};

const RATE: f32 = 48_000.0;
const BLOCK: usize = 37;

fn generator(id: &str) -> Box<dyn Rhythm> {
    let kind = find_rhythm(id).unwrap_or_else(|| panic!("no generator {id}"));
    let mut rhythm = (kind.build)();
    rhythm.prepare(RATE);
    rhythm
}

fn set(rhythm: &mut dyn Rhythm, id: &str, name: &str, value: f32) {
    let index = find_rhythm(id)
        .and_then(|kind| kind.params.iter().position(|spec| spec.name == name))
        .unwrap_or_else(|| panic!("{id} has no {name}"));
    rhythm.set_param(index, value);
}

/// A clock: `edges` pulses every `period` samples, each `width` long,
/// the first at `offset`.
fn clock(edges: usize, period: usize, width: usize, offset: usize) -> Vec<f32> {
    let mut signal = vec![0.0; offset + edges * period];
    for edge in 0..edges {
        let start = offset + edge * period;
        signal[start..start + width].fill(1.0);
    }
    signal
}

/// Run a clock (and optional reset) through in odd blocks; every output
/// comes back whole.
fn run(rhythm: &mut dyn Rhythm, clock: &[f32], reset: Option<&[f32]>) -> Vec<Vec<f32>> {
    let silent = vec![0.0; clock.len()];
    let reset = reset.unwrap_or(&silent);
    let mut outs = vec![vec![0.0f32; clock.len()]; MAX_OUTPUTS];
    let mut start = 0;
    while start < clock.len() {
        let end = (start + BLOCK).min(clock.len());
        let [a, b, c, d] = &mut outs[..] else {
            unreachable!("four outputs")
        };
        let mut slices: [&mut [f32]; MAX_OUTPUTS] = [
            &mut a[start..end],
            &mut b[start..end],
            &mut c[start..end],
            &mut d[start..end],
        ];
        rhythm.process(&clock[start..end], &reset[start..end], &mut slices);
        start = end;
    }
    outs
}

/// Which clock edges (by number) had output `out` high on their sample.
fn fired(output: &[f32], edges: usize, period: usize, offset: usize) -> Vec<usize> {
    (0..edges)
        .filter(|edge| output[offset + edge * period] > 0.5)
        .collect()
}

fn pattern(output: &[f32], edges: usize, period: usize) -> String {
    (0..edges)
        .map(|edge| {
            if output[edge * period] > 0.5 {
                'x'
            } else {
                '.'
            }
        })
        .collect()
}

#[test]
fn every_generator_is_listed() {
    let ids: Vec<&str> = rhythms().map(|kind| kind.id).collect();
    assert_eq!(ids, ["euclid", "prob", "grids", "poly", "burst"]);
}

#[test]
fn euclid_plays_bjorklund_and_rotates_later() {
    let mut euclid = generator("euclid");
    let signal = clock(32, 100, 10, 0);
    let outs = run(euclid.as_mut(), &signal, None);
    assert_eq!(pattern(&outs[0], 32, 100), "x..x..x..x..x...".repeat(2));
    assert_eq!(pattern(&outs[1], 32, 100), "x.....x.........".repeat(2));

    let mut rotated = generator("euclid");
    set(rotated.as_mut(), "euclid", "rotate", 3.0);
    let outs = run(rotated.as_mut(), &signal, None);
    assert_eq!(pattern(&outs[0], 16, 100), "...x..x..x..x..x");
}

#[test]
fn gates_rise_and_fall_with_the_clock_to_the_sample() {
    let mut euclid = generator("euclid");
    set(euclid.as_mut(), "euclid", "pulses", 16.0);
    let signal = clock(20, 113, 29, 7);
    let outs = run(euclid.as_mut(), &signal, None);
    for (n, (&gate, &clock)) in outs[0].iter().zip(&signal).enumerate() {
        assert!((gate > 0.5) == (clock > 0.5), "sample {n}");
        assert!([0.0, 1.0].contains(&gate));
    }
    // Outputs past a generator's own are held low.
    assert!(outs[2].iter().chain(&outs[3]).all(|&level| level == 0.0));
}

#[test]
fn a_reset_sends_the_next_edge_to_the_first_step() {
    let mut euclid = generator("euclid");
    let signal = clock(12, 100, 10, 0);
    let mut reset = vec![0.0; signal.len()];
    // Reset between edges 4 and 5, and on the sample of edge 8.
    reset[450..460].fill(1.0);
    reset[800..805].fill(1.0);
    let outs = run(euclid.as_mut(), &signal, Some(&reset));
    // Steps 0 to 4, then 0 to 2, then 0 to 3.
    assert_eq!(pattern(&outs[0], 12, 100), "x..x.x..x..x");
}

#[test]
fn poly_counts_n_against_m() {
    let mut poly = generator("poly");
    let signal = clock(24, 50, 5, 0);
    let outs = run(poly.as_mut(), &signal, None);
    let every = |step: usize| (0..24).filter(|n| n % step == 0).collect::<Vec<_>>();
    assert_eq!(fired(&outs[0], 24, 50, 0), every(3));
    assert_eq!(fired(&outs[1], 24, 50, 0), every(4));
    assert_eq!(fired(&outs[2], 24, 50, 0), every(12));
    let either: Vec<usize> = (0..24).filter(|n| n % 3 == 0 || n % 4 == 0).collect();
    assert_eq!(fired(&outs[3], 24, 50, 0), either);

    let mut shifted = generator("poly");
    set(shifted.as_mut(), "poly", "shift", 1.0);
    let outs = run(shifted.as_mut(), &signal, None);
    assert_eq!(fired(&outs[1], 24, 50, 0), vec![1, 5, 9, 13, 17, 21]);
    assert_eq!(fired(&outs[2], 24, 50, 0), vec![9, 21]);
}

#[test]
fn prob_at_its_extremes() {
    let signal = clock(64, 40, 5, 0);
    let chances: Vec<String> = (1..=16).map(|step| format!("p{step}")).collect();
    let fires = |chance: f32, density: f32| {
        let mut prob = generator("prob");
        set(prob.as_mut(), "prob", "ratchet", 0.0);
        set(prob.as_mut(), "prob", "density", density);
        for name in &chances {
            set(prob.as_mut(), "prob", name, chance);
        }
        let outs = run(prob.as_mut(), &signal, None);
        fired(&outs[0], 64, 40, 0).len()
    };
    assert_eq!(fires(0.0, 0.5), 0);
    assert_eq!(fires(1.0, 0.5), 64);
    assert_eq!(fires(1.0, 0.0), 0);
    assert_eq!(fires(0.0, 1.0), 64);
    let some = fires(0.5, 0.5);
    assert!((16..48).contains(&some), "{some}");
}

#[test]
fn prob_ratchets_fill_the_step() {
    let mut prob = generator("prob");
    set(prob.as_mut(), "prob", "ratchet", 1.0);
    set(prob.as_mut(), "prob", "repeats", 4.0);
    set(prob.as_mut(), "prob", "density", 1.0);
    let signal = clock(4, 400, 20, 0);
    let outs = run(prob.as_mut(), &signal, None);
    // The first edge only measures the clock: one gate as wide as it.
    let first: usize = outs[0][..400].iter().filter(|&&level| level > 0.5).count();
    assert_eq!(first, 20);
    // After that, four gates of 50 samples every 100.
    for edge in 1..4 {
        let step = &outs[0][edge * 400..(edge + 1) * 400];
        let rises: Vec<usize> = (0..400)
            .filter(|&n| step[n] > 0.5 && (n == 0 || step[n - 1] <= 0.5))
            .collect();
        assert_eq!(rises, vec![0, 100, 200, 300], "edge {edge}");
        assert_eq!(step.iter().filter(|&&level| level > 0.5).count(), 200);
    }
}

#[test]
fn a_locked_prob_loop_repeats() {
    let mut prob = generator("prob");
    set(prob.as_mut(), "prob", "lock", 1.0);
    set(prob.as_mut(), "prob", "ratchet", 0.0);
    for step in 1..=16 {
        set(prob.as_mut(), "prob", &format!("p{step}"), 0.5);
    }
    let signal = clock(48, 30, 3, 0);
    let outs = run(prob.as_mut(), &signal, None);
    let loop_pattern = pattern(&outs[0], 48, 30);
    assert_eq!(loop_pattern[..16], loop_pattern[16..32]);
    assert_eq!(loop_pattern[..16], loop_pattern[32..]);
    assert!(loop_pattern.contains('x') && loop_pattern.contains('.'));
}

#[test]
fn grids_follows_density_and_accents_strong_beats() {
    let signal = clock(16, 60, 6, 0);
    let mut silent = generator("grids");
    for part in ["kick", "snare", "hat"] {
        set(silent.as_mut(), "grids", part, 0.0);
    }
    let outs = run(silent.as_mut(), &signal, None);
    assert!(outs.iter().flatten().all(|&level| level == 0.0));

    let mut house = generator("grids");
    set(house.as_mut(), "grids", "x", 0.0);
    set(house.as_mut(), "grids", "y", 0.0);
    for part in ["kick", "snare", "hat"] {
        set(house.as_mut(), "grids", part, 1.0);
    }
    let outs = run(house.as_mut(), &signal, None);
    assert_eq!(pattern(&outs[0], 16, 60), "x...x...x...x..x");
    assert_eq!(pattern(&outs[1], 16, 60), "....x.......x.x.");
    assert_eq!(pattern(&outs[3], 16, 60), "x...x...x...x...");

    // At half density only the essential hits play.
    let mut sparse = generator("grids");
    set(sparse.as_mut(), "grids", "x", 0.0);
    set(sparse.as_mut(), "grids", "y", 0.0);
    set(sparse.as_mut(), "grids", "kick", 0.5);
    let outs = run(sparse.as_mut(), &signal, None);
    assert_eq!(pattern(&outs[0], 16, 60), "x...x...x...x...");
}

#[test]
fn chaos_changes_the_groove() {
    let signal = clock(64, 30, 3, 0);
    let mut calm = generator("grids");
    let mut wild = generator("grids");
    set(wild.as_mut(), "grids", "chaos", 1.0);
    let calm = run(calm.as_mut(), &signal, None);
    let wild = run(wild.as_mut(), &signal, None);
    assert_ne!(pattern(&calm[2], 64, 30), pattern(&wild[2], 64, 30));
}

#[test]
fn a_burst_fires_its_count_at_its_spacing() {
    let mut burst = generator("burst");
    set(burst.as_mut(), "burst", "spacing", 0.01);
    let signal = clock(1, 4_000, 10, 100);
    let outs = run(burst.as_mut(), &signal, None);
    let rises: Vec<usize> = (1..outs[0].len())
        .filter(|&n| outs[0][n] > 0.5 && outs[0][n - 1] <= 0.5)
        .collect();
    assert_eq!(rises, vec![100, 580, 1_060, 1_540]);
    for (hit, &at) in rises.iter().enumerate() {
        let want = 0.75f32.powf(hit as f32);
        assert!((outs[1][at] - want).abs() < 1.0e-6);
        // Each gate is high for half its gap.
        assert!(outs[0][at + 239] > 0.5 && outs[0][at + 240] < 0.5);
    }
}

#[test]
fn a_bouncing_burst_crowds_together() {
    let mut burst = generator("burst");
    set(burst.as_mut(), "burst", "spacing", 0.01);
    set(burst.as_mut(), "burst", "bounce", 0.5);
    let signal = clock(1, 3_000, 10, 0);
    let outs = run(burst.as_mut(), &signal, None);
    let rises: Vec<usize> = (0..outs[0].len())
        .filter(|&n| outs[0][n] > 0.5 && (n == 0 || outs[0][n - 1] <= 0.5))
        .collect();
    assert_eq!(rises, vec![0, 480, 720, 840]);
}

#[test]
fn a_synced_burst_fills_one_clock_step() {
    let mut burst = generator("burst");
    set(burst.as_mut(), "burst", "sync", 1.0);
    let signal = clock(3, 2_000, 10, 0);
    let outs = run(burst.as_mut(), &signal, None);
    let rises: Vec<usize> = (2_000..6_000)
        .filter(|&n| outs[0][n] > 0.5 && outs[0][n - 1] <= 0.5)
        .collect();
    assert_eq!(
        rises,
        vec![2_000, 2_500, 3_000, 3_500, 4_000, 4_500, 5_000, 5_500]
    );
}

#[test]
fn poison_and_odd_blocks_are_handled() {
    for kind in rhythms() {
        let mut rhythm = (kind.build)();
        rhythm.prepare(f32::NAN);
        for index in 0..kind.params.len() + 2 {
            rhythm.set_param(index, f32::NAN);
        }
        let nan_clock = vec![f32::NAN; 500];
        let outs = run(rhythm.as_mut(), &nan_clock, None);
        assert!(
            outs.iter().flatten().all(|&level| level == 0.0),
            "{}",
            kind.id
        );

        // Short outputs: only the shortest length runs; the rest is low.
        let clock_in = vec![1.0; 64];
        let reset_in = vec![0.0; 64];
        let mut long = vec![9.0; 64];
        let mut short = vec![9.0; 16];
        let mut extra: Vec<Vec<f32>> = vec![vec![9.0; 64]; MAX_OUTPUTS];
        {
            let [c, d, e, f] = &mut extra[..] else {
                unreachable!("four outputs")
            };
            let mut outs: [&mut [f32]; 6] = [&mut long, &mut short, c, d, e, f];
            rhythm.process(&clock_in, &reset_in, &mut outs);
        }
        assert!(long[16..].iter().all(|&level| level == 0.0), "{}", kind.id);
        assert!(long.iter().chain(&short).all(|level| level.is_finite()));
        let beyond = &extra[MAX_OUTPUTS - 2..];
        assert!(
            beyond.iter().flatten().all(|&level| level == 0.0),
            "{}",
            kind.id
        );
        rhythm.process(&[], &[], &mut []);
        rhythm.reset();
    }
}
