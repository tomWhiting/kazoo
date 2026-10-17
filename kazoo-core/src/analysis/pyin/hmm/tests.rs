use super::*;
use std::cmp::Ordering;

/// The model's transitions as the crate's dense table, `[to][from]`, with
/// `ln(MIN_POSITIVE)` wherever there is no way.
fn dense(hmm: &Hmm) -> Vec<f64> {
    let states = hmm.states;
    let mut table = vec![f64::MIN_POSITIVE.ln(); states * states];
    for to in 0..states {
        for &(from, log_p) in &hmm.ways[hmm.starts[to]..hmm.starts[to + 1]] {
            table[to * states + from] = log_p;
        }
    }
    table
}

/// The crate's Viterbi: every state searched for every state.
fn dense_decode(hmm: &Hmm, observed: &[f64], frames: usize) -> Vec<usize> {
    let states = hmm.states;
    let table = dense(hmm);
    let log_observed: Vec<f64> = observed
        .iter()
        .map(|&p| (p + f64::MIN_POSITIVE).ln())
        .collect();
    let mut values = vec![0.0; frames * states];
    let mut pointers = vec![0; frames * states];
    for state in 0..states {
        values[state] = hmm.log_initial[state] + log_observed[state];
    }
    for frame in 1..frames {
        for to in 0..states {
            let (best, score) = first_max(
                (0..states)
                    .map(|from| values[(frame - 1) * states + from] + table[to * states + from]),
            );
            pointers[frame * states + to] = best;
            values[frame * states + to] = log_observed[frame * states + to] + score;
        }
    }
    let mut path = vec![0; frames];
    path[frames - 1] = first_max(values[(frames - 1) * states..].iter().copied()).0;
    for frame in (0..frames - 1).rev() {
        path[frame] = pointers[(frame + 1) * states + path[frame + 1]];
    }
    path
}

fn grid(pitch_bins: usize, transition_width: usize) -> Grid {
    Grid {
        min_period: 10,
        max_period: 40,
        bins_per_semitone: 1,
        pitch_bins,
        transition_width,
    }
}

/// The crate's construction: a dense local table, each row placed, rolled,
/// cut and normalised over the whole row, then the four voicing blocks,
/// then the log; `[to][from]`.
fn crate_table(bins: usize, width: usize) -> Vec<f64> {
    let window = triangle(width);
    let pad_left = (bins - width) / 2;
    let mut local = vec![0.0; bins * bins];
    for from in 0..bins {
        let mut placed = vec![0.0; bins];
        placed[pad_left..pad_left + width].copy_from_slice(&window);
        let shift = (bins / 2 + from + 1) % bins;
        let row = &mut local[from * bins..(from + 1) * bins];
        for (to, slot) in row.iter_mut().enumerate() {
            *slot = placed[(to + bins - shift) % bins];
        }
        row[(from + width / 2 + 1).min(bins)..].fill(0.0);
        row[..from.saturating_sub(width / 2)].fill(0.0);
        let total = ndarray_sum(row);
        for slot in row.iter_mut() {
            *slot /= total;
        }
    }
    let states = 2 * bins;
    let mut table = vec![0.0; states * states];
    for to in 0..states {
        for from in 0..states {
            let within = local[(from % bins) * bins + to % bins];
            let p = if (from < bins) == (to < bins) {
                within * (1.0 - SWITCH)
            } else {
                within * SWITCH
            };
            table[to * states + from] = (p + f64::MIN_POSITIVE).ln();
        }
    }
    table
}

/// Widths odd and even, the judge's own (16), and bands that cover the
/// whole range.
const SHAPES: [(usize, usize); 10] = [
    (2, 1),
    (3, 2),
    (4, 3),
    (5, 3),
    (9, 1),
    (12, 5),
    (30, 7),
    (40, 29),
    (40, 16),
    (228, 16),
];

#[test]
fn the_model_holds_the_crates_transitions_bit_for_bit() {
    for (bins, width) in SHAPES {
        let hmm = Hmm::new(&grid(bins, width));
        let ours = dense(&hmm);
        let theirs = crate_table(bins, width);
        for (index, (a, b)) in ours.iter().zip(&theirs).enumerate() {
            assert_eq!(
                a.to_bits(),
                b.to_bits(),
                "{bins} bins, width {width}, entry {index}"
            );
        }
        // Every bin keeps its own entry, so no row is empty.
        for to in 0..2 * bins {
            assert!(hmm.starts[to + 1] > hmm.starts[to], "{bins}, {width}: {to}");
        }
    }
}

/// Two different scores that round to the same one once `ln(MIN_POSITIVE)` is
/// added: the lower state must win, as the dense search's first maximum.
#[test]
fn scores_that_round_together_go_to_the_lower_state() {
    let never = f64::MIN_POSITIVE.ln();
    let high = -1.0_f64;
    let low = f64::from_bits(high.to_bits() + 1);
    assert_ne!(high.to_bits(), low.to_bits());
    assert_eq!((high + never).to_bits(), (low + never).to_bits());
    // State 2 (the higher score before) and state 1 (lower) tie after it.
    let before = [-900.0, low, high, -900.0];
    let order = {
        let mut order: Vec<(f64, usize)> = before
            .iter()
            .enumerate()
            .map(|(state, &value)| (value + never, state))
            .collect();
        order.sort_unstable_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(Ordering::Equal)
                .then(a.1.cmp(&b.1))
        });
        order
    };
    // Reached only from state 3 by a way of its own.
    let ways = [(3, (0.5_f64).ln())];
    let (best, score) = best_way_in(&before, &ways, &order);
    let dense =
        first_max((0..4).map(|from| before[from] + if from == 3 { (0.5_f64).ln() } else { never }));
    assert_eq!((best, score.to_bits()), (dense.0, dense.1.to_bits()));
    assert_eq!(best, 1);
}

#[test]
fn a_triangle_is_scipys() {
    assert_eq!(triangle(3), vec![0.5, 1.0, 0.5]);
    assert_eq!(triangle(4), vec![0.25, 0.75, 0.75, 0.25]);
    assert_eq!(triangle(1), vec![1.0]);
}

/// The crate's own check, from librosa.
#[test]
fn local_transitions_are_librosas() {
    let expected = [
        [2.0 / 3.0, 1.0 / 3.0, 0.0, 0.0, 0.0],
        [0.25, 0.5, 0.25, 0.0, 0.0],
        [0.0, 0.25, 0.5, 0.25, 0.0],
        [0.0, 0.0, 0.25, 0.5, 0.25],
        [0.0, 0.0, 0.0, 1.0 / 3.0, 2.0 / 3.0],
    ];
    for (row, want) in local_rows(5, 3).iter().zip(expected) {
        let mut dense = [0.0; 5];
        for &(to, p) in row {
            dense[to] = p;
        }
        for (got, want) in dense.iter().zip(want) {
            assert!((got - want).abs() < 1e-15, "{row:?} against {want:?}");
        }
    }
}

/// The crate's Viterbi check, from Wikipedia's healthy and fever example.
#[test]
fn viterbi_finds_wikipedias_path() {
    let transition = [[0.7, 0.3], [0.4, 0.6]];
    let hmm = Hmm::from_ways(
        2,
        (0..2).map(|to| (0..2).map(move |from| (from, transition[from][to]))),
        vec![0.6, 0.4],
    );
    let observed = [0.5, 0.1, 0.4, 0.3, 0.1, 0.6];
    assert_eq!(hmm.decode(&observed, 3), vec![0, 0, 1]);
}

#[test]
fn no_frames_decode_to_no_path() {
    assert!(Hmm::new(&grid(5, 3)).decode(&[], 0).is_empty());
}

#[test]
fn ties_and_nan_go_to_the_first() {
    assert_eq!(first_max([1.0, 3.0, 3.0].into_iter()).0, 1);
    assert_eq!(first_max([f64::NAN, 2.0, 2.0].into_iter()).0, 1);
    assert_eq!(first_max([f64::NAN].into_iter()).0, 0);
}

/// The banded search decides as the dense one did, over observations that
/// make ties, NaN, nil and certainty, at widths up to most of the range.
#[test]
fn the_banded_search_decodes_as_the_dense_one() {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        state >> 33
    };
    for (bins, width) in SHAPES {
        let hmm = Hmm::new(&grid(bins, width));
        for _ in 0..40 {
            let frames = 1 + (next() % 6) as usize;
            let observed: Vec<f64> = (0..frames * 2 * bins)
                .map(|_| match next() % 8 {
                    0 => 0.0,
                    1 => 1.0,
                    2 => 0.5,
                    3 => f64::NAN,
                    _ => (next() % 1000) as f64 / 1000.0,
                })
                .collect();
            assert_eq!(
                hmm.decode(&observed, frames),
                dense_decode(&hmm, &observed, frames),
                "{bins} bins, width {width}: {observed:?}"
            );
        }
    }
}
