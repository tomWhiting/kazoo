use crate::protocol::{LoopView, LoopsMeet};
use kazoo_core::ipc::follow::beats_in;

const MAX_MEET_BARS: u128 = 1_000_000_000;

const fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a
}

fn extended_gcd(mut a: i128, mut b: i128) -> Option<(i128, i128)> {
    let (mut old, mut next) = (1_i128, 0_i128);
    while b != 0 {
        let quotient = a / b;
        (a, b) = (b, a % b);
        (old, next) = (next, old.checked_sub(quotient.checked_mul(next)?)?);
    }
    Some((a, old))
}

/// First whole bar seam strictly after `after_beat`, or incompatible seams.
#[must_use]
pub fn meet(lengths: &[u64], anchors: &[i64], after_beat: f64) -> Option<u64> {
    if lengths.is_empty() || lengths.len() != anchors.len() || !after_beat.is_finite() {
        return None;
    }
    let (mut position, mut period) = (0_i128, 4_i128);
    for (&length, &anchor) in lengths.iter().zip(anchors) {
        if length == 0 {
            return None;
        }
        let length = i128::from(length);
        let (divisor, inverse) = extended_gcd(period, length)?;
        let difference = i128::from(anchor).checked_sub(position)?;
        if difference % divisor != 0 {
            return None;
        }
        let factor = (difference / divisor)
            .checked_mul(inverse)?
            .rem_euclid(length / divisor);
        let merged = period.checked_mul(length / divisor)?;
        position = position
            .checked_add(period.checked_mul(factor)?)?
            .rem_euclid(merged);
        period = merged;
    }
    // A bar number must fit u64; reject larger transport positions before casting.
    if after_beat >= (u64::MAX as f64) * 4.0 {
        return None;
    }
    let after = after_beat.floor() as i128;
    let cycles = after
        .checked_sub(position)?
        .div_euclid(period)
        .checked_add(1)?;
    let next = position.checked_add(cycles.checked_mul(period)?)?;
    u64::try_from(next / 4).map_or(None, |bar| bar.checked_add(1))
}

/// Meeting report for at least two loops holding tape.
#[must_use]
pub fn loops_meet<'a>(
    views: impl Iterator<Item = &'a LoopView>,
    tempo: f64,
    beat: f64,
) -> Option<LoopsMeet> {
    let tapes: Vec<_> = views
        .filter(|view| view.frames > 0 && view.beats.is_finite() && view.beats >= 1.0)
        .collect();
    if tapes.len() < 2 {
        return None;
    }
    let lengths: Vec<_> = tapes.iter().map(|view| view.beats.round() as u64).collect();
    let anchors: Vec<_> = tapes
        .iter()
        .map(|view| view.anchor.round() as i64)
        .collect();
    let period = lengths.iter().try_fold(4_u128, |period, &length| {
        (period / gcd(period, u128::from(length))).checked_mul(u128::from(length))
    });
    let bars = period
        .filter(|period| period / 4 <= MAX_MEET_BARS)
        .map(|period| (period / 4) as u64);
    Some(LoopsMeet {
        bars,
        seconds: bars
            .filter(|_| tempo.is_finite() && tempo > 0.0)
            .map(|bars| bars as f64 * 4.0 / beats_in(1.0, tempo, 1.0)),
        next_bar: bars.and_then(|_| meet(&lengths, &anchors, beat)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tapes(lengths: &[u64], anchors: &[i64]) -> Vec<LoopView> {
        lengths
            .iter()
            .zip(anchors)
            .map(|(&length, &anchor)| LoopView {
                beats: length as f64,
                frames: length * 24_000,
                anchor: anchor as f64,
                ..LoopView::default()
            })
            .collect()
    }

    #[test]
    fn seventeen_and_twenty_three_bars_meet_every_391_bars() {
        let views = tapes(&[68, 92], &[0, 0]);
        assert_eq!(
            loops_meet(views.iter(), 120.0, 10.0),
            Some(LoopsMeet {
                bars: Some(391),
                seconds: Some(782.0),
                next_bar: Some(392),
            })
        );
        let views = tapes(&[68, 92], &[0, 8]);
        let brute_force = (0_u64..10_000)
            .map(|bar| bar * 4)
            .find(|&beat| beat > 10 && beat % 68 == 0 && (i128::from(beat) - 8).rem_euclid(92) == 0)
            .map(|beat| beat / 4 + 1);
        assert_eq!(meet(&[68, 92], &[0, 8], 10.0), brute_force);
        assert_eq!(
            loops_meet(views.iter(), 120.0, 10.0).unwrap().next_bar,
            brute_force
        );
        assert_eq!(brute_force, Some(256));
        assert_eq!(meet(&[68, 92], &[0, 0], 1_564.0), Some(783));
    }

    #[test]
    fn meet_is_none_when_it_is_practically_never_or_never() {
        let lengths = [233, 239, 241, 251, 253, 255, 256, 257];
        let views = tapes(&lengths, &[0; 8]);
        assert_eq!(
            loops_meet(views.iter(), 120.0, 10.0),
            Some(LoopsMeet {
                bars: None,
                seconds: None,
                next_bar: None,
            })
        );
        let views = tapes(&[8, 16], &[0, 4]);
        assert_eq!(
            loops_meet(views.iter(), 120.0, 10.0),
            Some(LoopsMeet {
                bars: Some(4),
                seconds: Some(8.0),
                next_bar: None,
            })
        );
        assert_eq!(meet(&[8, 16], &[0, 4], 10.0), None);
        let views = tapes(&[3, 5], &[0, 0]);
        assert_eq!(loops_meet(views.iter(), 120.0, 0.0).unwrap().bars, Some(15));
        assert_eq!(meet(&[3, 5], &[0, 0], 0.0), Some(16));
        assert_eq!(meet(&[0], &[0], 0.0), None);
        assert_eq!(meet(&[4], &[], 0.0), None);
        assert_eq!(meet(&[4], &[0], f64::NAN), None);
        assert_eq!(meet(&[4, 8], &[-4, 4], 0.0), Some(2));
    }
}
