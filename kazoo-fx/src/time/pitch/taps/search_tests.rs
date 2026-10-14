use super::{Geometry, Onsets, Taps};
use crate::dsp::DelayLine;

#[test]
fn the_search_budget_holds_at_every_rate() {
    for rate in [44_100.0, 48_000.0, 96_000.0, 192_000.0, 384_000.0] {
        let shape = Geometry::new(rate);
        assert_eq!(super::COARSE_STRIDE, 4, "fixed coarse stride at {rate}");
        let lags = 2 * shape.search as usize + 1;
        let scores = (lags - 1) / super::COARSE_STRIDE as usize
            + 1
            + super::CANDIDATES * (2 * super::FINE_RADIUS as usize + 1)
            + 2;
        let exhaustive = lags * shape.span;
        let pairs = scores * shape.span;
        println!(
            "{rate} Hz: {scores} scores, {pairs} pairs, {} reads",
            2 * pairs
        );
        assert!(pairs * 100 <= exhaustive * 36);
    }
}

fn sample(frame: usize) -> f32 {
    let time = frame as f32 / 48_000.0;
    10.0f32.powf(-24.0 / 20.0).mul_add(
        (std::f32::consts::TAU * 1_000.0 * time).sin(),
        10.0f32.powf(-12.0 / 20.0) * 2.0f32.mul_add((220.0 * time).fract(), -1.0),
    )
}

#[test]
fn the_bounded_search_finds_the_period() {
    let shape = Geometry::new(48_000.0);
    let mut guide = DelayLine::new(shape.longest as usize + shape.span + 32);
    let mut taps = Taps::default();
    taps.settle(shape.start());
    let mut onsets = Onsets::default();
    onsets.prepare(48_000.0);
    let drift = 1.0 - (7.0f32 / 12.0).exp2();
    let mut frame = 0;
    for _ in 0..guide.max_delay() {
        let input = sample(frame);
        guide.push(input);
        onsets.push(input);
        frame += 1;
    }
    let mut matches = 0;
    let mut farthest = 0.0f32;
    let mut splices = 0;
    while splices < 200 {
        let input = sample(frame);
        guide.push(input);
        onsets.push(input);
        frame += 1;
        if !taps.advance(drift, &shape, &guide, &onsets) {
            continue;
        }
        let mut reference = taps;
        reference.heard = taps.fading;
        let (target, low, high) = Taps::rising(taps.fading, shape.jump, &shape, &onsets);
        let (expected, _) = reference.align_exhaustive(target, low, high, &shape, &guide);
        let error = (taps.heard - expected).abs();
        matches += usize::from(error <= 1.0);
        farthest = farthest.max(error);
        splices += 1;
    }
    println!("{matches}/200 splices within one sample; farthest {farthest}");
    assert!(matches >= 198, "{matches}/200 within one sample");
    assert!(farthest <= 48_000.0 / 220.0);
}

#[test]
fn silence_keeps_the_planned_jump() {
    let shape = Geometry::new(48_000.0);
    let guide = DelayLine::new(shape.longest as usize + shape.span + 32);
    let mut taps = Taps::default();
    taps.settle(shape.start());
    let target = shape.start() + shape.jump;
    let (at, alike) = taps.align(target, -shape.search, shape.search, &shape, &guide);
    assert_eq!(at.to_bits(), target.round().to_bits());
    assert_eq!(alike.to_bits(), 0.0f32.to_bits());
}
