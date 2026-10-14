use super::{MATCH, Reading, SEARCH, Shifter, grain_of};
use crate::space::line::{Kernel, Line};

fn reading(rate: f32) -> Reading {
    let ratio = (7.0f32 / 12.0).exp2();
    let grain = grain_of(ratio, rate);
    Reading {
        base: 0.02 * rate,
        grain,
        step: (1.0 - ratio) / grain,
        search: (SEARCH * rate) as i32,
        span: (MATCH * rate) as usize,
    }
}

#[test]
fn the_search_budget_holds_at_every_rate() {
    for rate in [44_100.0, 48_000.0, 96_000.0, 192_000.0, 384_000.0] {
        let shape = reading(rate);
        assert_eq!(super::COARSE_STRIDE, 4, "fixed coarse stride at {rate}");
        let lags = 2 * shape.search as usize + 1;
        let scores = (lags - 1) / super::COARSE_STRIDE as usize
            + 1
            + super::CANDIDATES * (2 * super::FINE_RADIUS as usize + 1)
            + 1;
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
    let reading = reading(48_000.0);
    let kernel = Kernel::new(48_000.0);
    let mut source = Line::new(32_768, &kernel);
    let mut shifter = Shifter::default();
    let mut frame = 0;
    for _ in 0..32_768 {
        source.push(sample(frame));
        frame += 1;
    }
    let mut matches = 0;
    let mut farthest = 0.0f32;
    let mut splices = 0;
    while splices < 200 {
        source.push(sample(frame));
        frame += 1;
        let mut reference = shifter;
        shifter.read(&source, reading);
        if reference.alignments == shifter.alignments {
            continue;
        }
        reference.phase = shifter.phase;
        let tap = usize::from(shifter.phase < 0.5);
        reference.align_exhaustive(tap, &source, reading);
        let error = (shifter.slide[tap] - reference.slide[tap]).abs();
        matches += usize::from(error <= 1.0);
        farthest = farthest.max(error);
        splices += 1;
    }
    println!("{matches}/200 splices within one sample; farthest {farthest}");
    assert!(matches >= 198, "{matches}/200 within one sample");
    assert!(farthest <= 48_000.0 / 220.0);
}

#[test]
fn one_splice_per_block() {
    let mut effect = super::Shimmer::new();
    crate::Effect::prepare(&mut effect, 48_000.0);
    crate::Effect::set_param(&mut effect, super::SHIFT, 7.0);
    crate::Effect::reset(&mut effect);
    let mut shifters = effect.lines.map(|line| line.shifter);
    let ratio = super::ratio_of(7);
    let grain = grain_of(ratio, 48_000.0);
    let reading = Reading {
        grain,
        step: (1.0 - ratio) / grain,
        ..reading(48_000.0)
    };
    // Splice timing depends on the windows, so an empty source exercises
    // every search without rendering the tank's filters and feedback.
    let source = Line::default();
    let mut previous = usize::MAX;
    for frame in 0..480_000 {
        let before: usize = shifters.iter().map(|shifter| shifter.alignments).sum();
        for shifter in &mut shifters {
            shifter.read(&source, reading);
        }
        let after: usize = shifters.iter().map(|shifter| shifter.alignments).sum();
        if after != before {
            assert_eq!(after - before, 1, "simultaneous splices at {frame}");
            if previous != usize::MAX {
                assert!(frame - previous >= 32, "splices at {previous} and {frame}");
            }
            previous = frame;
        }
    }
    assert_ne!(previous, usize::MAX);
}
