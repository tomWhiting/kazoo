use super::{
    super::{SPRING_COUNT, Spring},
    RATE,
};

pub(super) fn two_springs_run_two_cascades_and_the_third_comes_back_clean() {
    use crate::Effect;
    let mut spring = Spring::new();
    spring.prepare(RATE);
    spring.set_param(SPRING_COUNT, 2.0);
    let fixture = crate::bench::fixture(RATE);
    for n in 0..4096 {
        spring.frame(fixture[n % fixture.len()], fixture[n % fixture.len()]);
    }
    assert_eq!(spring.lows.live, 2);
    assert_eq!(spring.highs.live, 2);
    let reads = spring.coils[2].reads;
    for &input in fixture.iter().take(128) {
        spring.frame(input, input);
    }
    assert_eq!(spring.coils[2].reads, reads, "the silent coil still reads");
    for delay in 1..8192 {
        assert_eq!(spring.coils[2].line.tap(delay).to_bits(), 0.0f32.to_bits());
        assert_eq!(
            spring.coils[2].high_line.tap(delay).to_bits(),
            0.0f32.to_bits()
        );
    }
    for chains in [&spring.lows, &spring.highs] {
        assert!(
            chains.history[2]
                .iter()
                .chain(&chains.thiran_out[2])
                .all(|x| *x == 0.0)
        );
    }
    let mut reference = spring.clone();
    let mut error = 0.0f64;
    let mut power = 0.0f64;
    for n in 0..8192 {
        reference.countdown = 1;
        reference.lows.live = 3;
        reference.highs.live = 3;
        let input = fixture[n % fixture.len()];
        let actual = spring.frame(input, input)[1];
        let expected = reference.frame(input, input)[1];
        for (actual, expected) in actual.into_iter().zip(expected) {
            error += f64::from(actual - expected).powi(2);
            power += f64::from(expected).powi(2);
        }
    }
    assert!(
        error < power * 1e-8,
        "two cascade error {error}, reference power {power}"
    );
    let mut before = 0.0f32;
    let mut previous = 0.0;
    for n in 0..(0.5 * RATE) as usize {
        let input =
            10.0f32.powf(-12.0 / 20.0) * (std::f32::consts::TAU * 440.0 * n as f32 / RATE).sin();
        let output = spring.frame(input, input)[1][0];
        if n >= (0.4 * RATE) as usize {
            before = before.max((output - previous).abs());
        }
        previous = output;
    }
    spring.set_param(SPRING_COUNT, 3.0);
    let mut after = 0.0f32;
    for n in (0.5 * RATE) as usize..(0.56 * RATE) as usize {
        let input =
            10.0f32.powf(-12.0 / 20.0) * (std::f32::consts::TAU * 440.0 * n as f32 / RATE).sin();
        let output = spring.frame(input, input)[1][0];
        after = after.max((output - previous).abs());
        previous = output;
    }
    assert_eq!(spring.lows.live, 3);
    assert_eq!(spring.highs.live, 3);
    assert!(
        after <= 3.0 * before,
        "third return step {after}, before {before}"
    );
}
