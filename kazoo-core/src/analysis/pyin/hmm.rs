//! pYIN's hidden Markov model: a voiced and an unvoiced state for each pitch
//! bin, a pitch that may move only so far each hop, and Viterbi decoding of
//! the likeliest path through the frames.
//!
//! The crate held every transition in a dense table, (2 × bins)² of them,
//! and searched all of it for every state of every frame. A pitch can only
//! move within its band, so all but a band of each state's ways in have
//! probability nil, which the log turns into one constant. Here only the band
//! is stored and searched. The best of the rest is the best-scoring state
//! outside the band, once the constant is added, found from the states sorted
//! once a frame by that score. The scores are the crate's, computed in its
//! order, and ties go to the lowest state, as its first maximum did.

use std::cmp::Ordering;

use super::Grid;
use super::prior::ndarray_sum;

/// The fastest a pitch may move, in octaves a second.
pub const MAX_TRANSITION_RATE: f64 = 35.92;

/// The chance of switching between voiced and unvoiced from one frame to
/// the next.
const SWITCH: f64 = 0.01;

/// The model's ways between states, built once for a grid.
pub struct Hmm {
    states: usize,
    /// Each state's ways in with a probability, `(from, ln(p + MIN_POSITIVE))`
    /// by `from`; state `to`'s are `ways[starts[to]..starts[to + 1]]`.
    ways: Vec<(usize, f64)>,
    starts: Vec<usize>,
    /// The log of the first frame's distribution: unvoiced, evenly.
    log_initial: Vec<f64>,
}

impl Hmm {
    pub fn new(grid: &Grid) -> Self {
        let bins = grid.pitch_bins;
        let states = grid.states();
        // By `from`, as built; turned around below to be by `to`.
        let local = local_rows(bins, grid.transition_width);
        let mut into: Vec<Vec<(usize, f64)>> = vec![Vec::new(); bins];
        for (from, row) in local.iter().enumerate() {
            for &(to, p) in row {
                into[to].push((from, p));
            }
        }
        let ways_into = |to: usize| {
            let voiced = to < bins;
            let near = &into[to % bins];
            [true, false].into_iter().flat_map(move |from_voiced| {
                let offset = if from_voiced { 0 } else { bins };
                let weight = if from_voiced == voiced {
                    1.0 - SWITCH
                } else {
                    SWITCH
                };
                near.iter()
                    .map(move |&(from, p)| (from + offset, p * weight))
            })
        };
        let initial = (0..states)
            .map(|state| if state < bins { 0.0 } else { 1.0 / bins as f64 })
            .collect();
        Self::from_ways(states, (0..states).map(ways_into), initial)
    }

    /// A model from each state's ways in, `(from, p)` by `from`, and the first
    /// frame's distribution, both as probabilities.
    fn from_ways<W, I>(states: usize, ways_in: W, initial: Vec<f64>) -> Self
    where
        W: Iterator<Item = I>,
        I: Iterator<Item = (usize, f64)>,
    {
        let mut ways = Vec::new();
        let mut starts = Vec::with_capacity(states + 1);
        for state in ways_in {
            starts.push(ways.len());
            ways.extend(state.map(|(from, p)| (from, (p + f64::MIN_POSITIVE).ln())));
        }
        starts.push(ways.len());
        let log_initial = initial
            .into_iter()
            .map(|p| (p + f64::MIN_POSITIVE).ln())
            .collect();
        Self {
            states,
            ways,
            starts,
            log_initial,
        }
    }

    /// The likeliest state at each of `frames` frames, from each frame's
    /// observation probabilities (frame by frame, `states` each).
    pub fn decode(&self, observed: &[f64], frames: usize) -> Vec<usize> {
        let states = self.states;
        if frames == 0 {
            return Vec::new();
        }
        let log = |frame: usize, out: &mut [f64]| {
            for (slot, &p) in out.iter_mut().zip(&observed[frame * states..]) {
                *slot = (p + f64::MIN_POSITIVE).ln();
            }
        };
        // Only the frame before and the frame now are kept; the pointers,
        // which the path is read back through, are kept for every frame.
        let mut seen = vec![0.0; states];
        let mut before = vec![0.0; states];
        let mut now = vec![0.0; states];
        let mut from_state = vec![0_usize; frames * states];
        let mut order: Vec<(f64, usize)> = Vec::with_capacity(states);

        log(0, &mut seen);
        for ((value, &initial), &seen) in before.iter_mut().zip(&self.log_initial).zip(&seen) {
            *value = initial + seen;
        }
        let never = f64::MIN_POSITIVE.ln();
        for frame in 1..frames {
            log(frame, &mut seen);
            // Every state as a way in at nil probability, best first and
            // lowest first among equals; a NaN score is never a way in.
            order.clear();
            order.extend(
                before
                    .iter()
                    .enumerate()
                    .map(|(state, &value)| (value + never, state))
                    .filter(|(score, _)| !score.is_nan()),
            );
            order.sort_unstable_by(|a, b| {
                b.0.partial_cmp(&a.0)
                    .unwrap_or(Ordering::Equal)
                    .then(a.1.cmp(&b.1))
            });

            let pointers = &mut from_state[frame * states..(frame + 1) * states];
            for (to, ((value, pointer), &seen)) in now
                .iter_mut()
                .zip(pointers.iter_mut())
                .zip(&seen)
                .enumerate()
            {
                let ways = &self.ways[self.starts[to]..self.starts[to + 1]];
                let (best, score) = best_way_in(&before, ways, &order);
                *pointer = best;
                *value = seen + score;
            }
            std::mem::swap(&mut before, &mut now);
        }

        let mut path = vec![0_usize; frames];
        path[frames - 1] = first_max(before.iter().copied()).0;
        for frame in (0..frames - 1).rev() {
            path[frame] = from_state[(frame + 1) * states + path[frame + 1]];
        }
        path
    }
}

/// The state this one is likeliest reached from and its score: the best of
/// its ways in, and of every other state at nil probability, which is the
/// first of `order` outside its ways; ties go to the lowest state. The first
/// state and NaN if every score is NaN.
fn best_way_in(before: &[f64], ways: &[(usize, f64)], order: &[(f64, usize)]) -> (usize, f64) {
    let mut best: Option<(usize, f64)> = None;
    for &(from, log_p) in ways {
        let score = before[from] + log_p;
        if score.is_nan() {
            continue;
        }
        match best {
            Some((_, most)) if most >= score => {}
            _ => best = Some((from, score)),
        }
    }
    let rest = order
        .iter()
        .find(|&&(_, from)| ways.binary_search_by_key(&from, |&(way, _)| way).is_err());
    match (best, rest) {
        (Some((from, most)), Some(&(top, other))) => {
            if most > top || (most.partial_cmp(&top) == Some(Ordering::Equal) && from < other) {
                (from, most)
            } else {
                (other, top)
            }
        }
        (Some(best), None) => best,
        (None, Some(&(top, other))) => (other, top),
        (None, None) => (0, f64::NAN),
    }
}

/// Where the first of the largest values is, and the value, skipping NaN;
/// the first place and NaN if every value is NaN.
fn first_max(values: impl Iterator<Item = f64>) -> (usize, f64) {
    let mut best: Option<(usize, f64)> = None;
    for (index, value) in values.enumerate() {
        if value.is_nan() {
            continue;
        }
        match best {
            Some((_, most)) if most >= value => {}
            _ => best = Some((index, value)),
        }
    }
    best.unwrap_or((0, f64::NAN))
}

/// librosa's `transition_local` with a triangular window and no wrapping:
/// from each bin, a triangle `width` bins wide centred on it, cut at the
/// ends of the range and normalised by the row's sum (in ndarray's order,
/// over the whole row, as the crate summed it). Each bin's row holds only
/// the bins it can move to, `(to, p)` by `to`. The bin's own entry is the
/// triangle's peak and is never cut, so no row sums to nothing.
fn local_rows(bins: usize, width: usize) -> Vec<Vec<(usize, f64)>> {
    let window = triangle(width);
    let pad_left = (bins - width) / 2;
    let mut placed = vec![0.0; bins];
    placed[pad_left..pad_left + width].copy_from_slice(&window);
    let mut row = vec![0.0; bins];
    (0..bins)
        .map(|from| {
            // The window placed in the middle of the range, rolled round to
            // this bin, then cut to the bins within half a width of it.
            let shift = (bins / 2 + from + 1) % bins;
            for (to, slot) in row.iter_mut().enumerate() {
                *slot = placed[(to + bins - shift) % bins];
            }
            row[(from + width / 2 + 1).min(bins)..].fill(0.0);
            row[..from.saturating_sub(width / 2)].fill(0.0);
            let total = ndarray_sum(&row);
            row.iter()
                .enumerate()
                .filter(|&(_, &p)| p != 0.0)
                .map(|(to, &p)| (to, p / total))
                .collect()
        })
        .collect()
}

/// A symmetric triangular window of `size` points (scipy's `triang`).
fn triangle(size: usize) -> Vec<f64> {
    let half: Vec<f64> = (1..=size.div_ceil(2)).map(|n| n as f64).collect();
    if size.is_multiple_of(2) {
        let half: Vec<f64> = half.iter().map(|n| (n * 2.0 - 1.0) / size as f64).collect();
        half.iter().chain(half.iter().rev()).copied().collect()
    } else {
        let half: Vec<f64> = half.iter().map(|n| n * 2.0 / (size + 1) as f64).collect();
        half.iter()
            .chain(half[..half.len() - 1].iter().rev())
            .copied()
            .collect()
    }
}

#[cfg(test)]
mod tests;
