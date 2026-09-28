//! Finding where the hits are: slice points for the player's slice mode.
//!
//! The analyser runs once, off the audio thread, when a sample is built. It
//! cuts the mono sum into hops of about 5 ms and measures two things in
//! each: the energy, and the energy of the first difference (which weighs
//! high frequencies, so a hi-hat's attack stands out as well as a kick's).
//! An onset is a hop where either rises sharply in log terms, the rise is a
//! local peak, it clears the local median by a margin, and the hop is not
//! near-silent. Each onset is then moved back to the quietest sample just
//! before it, so a slice starts on a near-zero crossing and never clicks.

/// The most slice points one sample carries, the start included.
pub const MAX_ONSETS: usize = 128;

/// Onsets closer together than this are merged, in seconds.
const MIN_GAP_SECONDS: f64 = 0.05;

/// Hops quieter than this (mean square, about -60 dBFS) never start a slice.
const SILENCE: f64 = 1e-6;

/// How far above the local median a rise must be, in natural-log units of
/// energy (about 4.3 dB).
const MARGIN: f64 = 1.0;

/// Hops either side used for the local median.
const MEDIAN_REACH: usize = 8;

/// Hops either side a rise must beat to be a peak.
const PEAK_REACH: usize = 3;

/// Frame positions where new events start in the stereo audio `left` and
/// `right` at `rate`, sorted, always starting with 0 and holding at most
/// [`MAX_ONSETS`] points (the strongest are kept). Allocates.
#[must_use]
pub fn detect(left: &[f32], right: &[f32], rate: u32) -> Vec<usize> {
    let frames = left.len().min(right.len());
    let hop = (rate as usize / 200).max(16);
    let hops = frames / hop;
    if hops < 2 {
        return vec![0];
    }
    let mono = |n: usize| 0.5 * (f64::from(left[n]) + f64::from(right[n]));

    // Energy and difference energy per hop, as logs.
    let mut energy = Vec::with_capacity(hops);
    let mut bright = Vec::with_capacity(hops);
    let mut previous = 0.0;
    for h in 0..hops {
        let (mut e, mut d) = (0.0, 0.0);
        for n in h * hop..(h + 1) * hop {
            let x = mono(n);
            let x = if x.is_finite() { x } else { 0.0 };
            e += x * x;
            d += (x - previous) * (x - previous);
            previous = x;
        }
        energy.push(e / hop as f64);
        bright.push(d / hop as f64);
    }

    // The onset strength: how sharply either measure rises into each hop.
    let rise = |series: &[f64], h: usize| {
        (series[h] + SILENCE).ln() - (series[h.saturating_sub(1)] + SILENCE).ln()
    };
    let strength: Vec<f64> = (0..hops)
        .map(|h| {
            if h == 0 {
                0.0
            } else {
                rise(&energy, h).max(rise(&bright, h)).max(0.0)
            }
        })
        .collect();

    let min_gap = (MIN_GAP_SECONDS * f64::from(rate)) as usize;
    let mut window = Vec::with_capacity(2 * MEDIAN_REACH + 1);
    let mut found: Vec<(usize, f64)> = Vec::new();
    for h in 1..hops {
        let s = strength[h];
        if s <= 0.0 || energy[h] < SILENCE {
            continue;
        }
        let near = h.saturating_sub(PEAK_REACH)..(h + PEAK_REACH + 1).min(hops);
        if strength[near].iter().any(|&other| other > s) {
            continue;
        }
        window.clear();
        window.extend_from_slice(
            &strength[h.saturating_sub(MEDIAN_REACH)..(h + MEDIAN_REACH + 1).min(hops)],
        );
        window.sort_by(f64::total_cmp);
        if s < window[window.len() / 2] + MARGIN {
            continue;
        }
        let at = quietest_before(h * hop, hop, &mono);
        let too_close = found
            .last()
            .is_some_and(|&(last, _)| at.saturating_sub(last) < min_gap);
        if at >= min_gap && !too_close {
            found.push((at, s));
        }
    }

    // Keep the strongest if there are too many, then put them in order.
    if found.len() > MAX_ONSETS - 1 {
        found.sort_by(|a, b| b.1.total_cmp(&a.1));
        found.truncate(MAX_ONSETS - 1);
    }
    let mut points: Vec<usize> = std::iter::once(0)
        .chain(found.into_iter().map(|(at, _)| at))
        .collect();
    points.sort_unstable();
    points
}

/// The quietest frame in the `span` frames before `at` (the frame itself
/// included), the latest of equals: where a slice starting near `at`
/// should really start.
fn quietest_before(at: usize, span: usize, mono: &impl Fn(usize) -> f64) -> usize {
    (at.saturating_sub(span)..=at)
        .rev()
        .min_by(|&a, &b| mono(a).abs().total_cmp(&mono(b).abs()))
        .unwrap_or(at)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A decaying noise burst at every frame in `hits`.
    fn hits(frames: usize, hits: &[usize]) -> Vec<f32> {
        let mut out = vec![0.0f32; frames];
        let mut seed = 0x1234_5678u32;
        for &start in hits {
            for n in 0..4_000.min(frames - start) {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let noise = (seed >> 8) as f32 / 8_388_608.0 - 1.0;
                out[start + n] += noise * 0.8 * (-(n as f32) / 800.0).exp();
            }
        }
        out
    }

    #[test]
    fn finds_each_hit_close_to_where_it_is() {
        let at = [0, 12_000, 24_000, 30_000, 41_000];
        let audio = hits(48_000, &at);
        let onsets = detect(&audio, &audio, 48_000);
        assert_eq!(onsets.len(), at.len(), "{onsets:?}");
        for (&found, &wanted) in onsets.iter().zip(&at) {
            assert!(found.abs_diff(wanted) <= 300, "{found} vs {wanted}");
            assert!(found <= wanted, "a slice must not start after its hit");
        }
    }

    #[test]
    fn silence_and_steady_tones_have_one_slice() {
        let silence = vec![0.0f32; 48_000];
        assert_eq!(detect(&silence, &silence, 48_000), vec![0]);
        let tone: Vec<f32> = (0..48_000).map(|n| (n as f32 * 0.05).sin() * 0.5).collect();
        assert_eq!(detect(&tone, &tone, 48_000), vec![0]);
        assert_eq!(detect(&[], &[], 48_000), vec![0]);
    }

    #[test]
    fn poison_does_not_break_it() {
        let mut audio = hits(24_000, &[0, 12_000]);
        audio[5] = f32::NAN;
        audio[6] = f32::INFINITY;
        let onsets = detect(&audio, &audio, 48_000);
        assert_eq!(onsets[0], 0);
        assert!(onsets.len() <= MAX_ONSETS);
    }

    #[test]
    fn many_hits_are_capped() {
        let at: Vec<usize> = (0..300).map(|i| i * 4_000).collect();
        let audio = hits(1_200_000, &at);
        let onsets = detect(&audio, &audio, 48_000);
        assert_eq!(onsets.len(), MAX_ONSETS);
        assert!(onsets.windows(2).all(|pair| pair[0] < pair[1]));
    }
}
