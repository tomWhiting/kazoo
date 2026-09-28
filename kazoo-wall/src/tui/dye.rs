//! Showing fingerprints as colour.
//!
//! The wall assigns no colours: a fingerprint is a seat's name and its
//! share. This view gives each name a hue worked out from the name itself
//! (the same name is always the same hue, in every run), and mixes the hues
//! of a cable's or module's fingerprint by their shares, as dyes mix in
//! water. Nothing is kept or assigned; a seat nobody has seen before gets
//! its hue the same way.

use ratatui::style::Color;

use kazoo_wall::fingerprints::Shares;

/// Saturation of a seat's dye.
const SATURATION: f64 = 0.62;

/// Brightness of a seat's dye: bright enough to read on the dark rack and
/// dark enough to read on a cream faceplate.
const VALUE: f64 = 0.78;

/// The hue of `seat`, 0 to 360, from its name (FNV-1a).
#[must_use]
pub fn hue(seat: &str) -> f64 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in seat.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    f64::from(hash % 360)
}

/// `seat`'s dye as red, green and blue, 0 to 1.
fn rgb(seat: &str) -> [f64; 3] {
    let hue = hue(seat) / 60.0;
    let chroma = VALUE * SATURATION;
    let second = chroma * (1.0 - (hue % 2.0 - 1.0).abs());
    let floor = VALUE - chroma;
    let (r, g, b) = match hue as u32 {
        0 => (chroma, second, 0.0),
        1 => (second, chroma, 0.0),
        2 => (0.0, chroma, second),
        3 => (0.0, second, chroma),
        4 => (second, 0.0, chroma),
        _ => (chroma, 0.0, second),
    };
    [r + floor, g + floor, b + floor]
}

/// The mix of the dyes in `shares`, weighted by share; `None` when no dye
/// is there.
#[must_use]
pub fn mix(shares: &Shares) -> Option<Color> {
    let mut total = 0.0;
    let mut sum = [0.0; 3];
    for (seat, share) in shares {
        if !share.is_finite() || *share <= 0.0 {
            continue;
        }
        let colour = rgb(seat);
        for (channel, value) in sum.iter_mut().zip(colour) {
            *channel = share.mul_add(value, *channel);
        }
        total += share;
    }
    if total <= 0.0 {
        return None;
    }
    let [r, g, b] = sum.map(|channel| ((channel / total).clamp(0.0, 1.0) * 255.0).round() as u8);
    Some(Color::Rgb(r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shares(pairs: &[(&str, f64)]) -> Shares {
        pairs
            .iter()
            .map(|(seat, share)| ((*seat).to_string(), *share))
            .collect()
    }

    #[test]
    fn a_name_always_has_the_same_hue() {
        assert!((hue("Tom") - hue("Tom")).abs() < f64::EPSILON);
        assert!(hue("Waffles") < 360.0);
        assert_eq!(mix(&shares(&[("Tom", 1.0)])), mix(&shares(&[("Tom", 0.4)])));
    }

    #[test]
    fn dyes_mix_by_share() {
        let tom = mix(&shares(&[("Tom", 1.0)])).expect("a dye");
        let waffles = mix(&shares(&[("Waffles", 1.0)])).expect("a dye");
        let (Color::Rgb(tr, tg, tb), Color::Rgb(wr, wg, wb)) = (tom, waffles) else {
            panic!("dyes are RGB");
        };
        let half = mix(&shares(&[("Tom", 0.5), ("Waffles", 0.5)])).expect("a dye");
        let expected = Color::Rgb(
            f64::midpoint(f64::from(tr), f64::from(wr)).round() as u8,
            f64::midpoint(f64::from(tg), f64::from(wg)).round() as u8,
            f64::midpoint(f64::from(tb), f64::from(wb)).round() as u8,
        );
        let (Color::Rgb(hr, hg, hb), Color::Rgb(er, eg, eb)) = (half, expected) else {
            panic!("dyes are RGB");
        };
        for (a, b) in [(hr, er), (hg, eg), (hb, eb)] {
            assert!(a.abs_diff(b) <= 1, "{half:?} vs {expected:?}");
        }
    }

    #[test]
    fn no_dye_is_no_colour() {
        assert_eq!(mix(&Shares::new()), None);
        assert_eq!(mix(&shares(&[("Tom", 0.0), ("Waffles", f64::NAN)])), None);
    }
}
