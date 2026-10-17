use super::*;

/// The CDF of Beta(2, 18) in closed form: the chance that at least 2 of 19
/// trials succeed at `x`, summed term by term, or for the upper part one less
/// the chance of fewer.
fn beta_cdf(x: f64) -> f64 {
    let (a, trials) = (2, 19);
    let upper = x >= 3.0 / 22.0;
    let mut choose = 1.0;
    let mut total = 0.0;
    for k in 0..=trials {
        if (k >= a) != upper {
            total = (choose * x.powi(k)).mul_add((1.0 - x).powi(trials - k), total);
        }
        choose = choose * f64::from(trials - k) / f64::from(k + 1);
    }
    if upper { 1.0 - total } else { total }
}

/// The crate's own check of the Boltzmann prior, from librosa, at lambda 1.4.
#[test]
fn the_boltzmann_prior_matches_librosa() {
    let lambda_14: Vec<f64> = (0..19)
        .map(|k| {
            let (lambda, n) = (1.4_f64, 19.0_f64);
            let fact = (1.0 - (-lambda).exp()) / (1.0 - (-lambda * n).exp());
            fact * (-lambda * f64::from(k)).exp()
        })
        .collect();
    let expected = [
        7.534_030_36e-01,
        1.857_869_01e-01,
        4.581_448_58e-02,
        1.129_771_31e-02,
        2.785_981_75e-03,
        6.870_146_41e-04,
        1.694_157_25e-04,
        4.177_740_34e-05,
        1.030_218_08e-05,
        2.540_486_52e-06,
        6.264_762_62e-07,
        1.544_871_44e-07,
        3.809_606_07e-08,
        9.394_372_91e-09,
        2.316_623_84e-09,
        5.712_724_05e-10,
        1.408_740_41e-10,
        3.473_911_07e-11,
        8.566_559_23e-12,
    ];
    for (got, want) in lambda_14.iter().zip(expected) {
        assert!((got - want).abs() < 1e-6, "{got} against {want}");
    }
    // The prior used here is the same formula at lambda 2.
    let total: f64 = (0..7).map(|k| boltzmann(k, 7)).sum();
    assert!((total - 1.0).abs() < 1e-12, "{total}");
}

/// The table is Beta(2, 18): it agrees with the closed form, rises and
/// falls once, and sums to one.
#[test]
fn the_beta_prior_is_beta_2_18() {
    for (index, &p) in BETA_PROBS.iter().enumerate() {
        let closed =
            beta_cdf((index + 1) as f64 / THRESHOLDS as f64) - beta_cdf(index as f64 / 100.0);
        assert!((p - closed).abs() < 1e-13, "{index}: {p} against {closed}");
        assert!(p >= 0.0, "{index}: {p}");
    }
    assert!((ndarray_sum(&BETA_PROBS) - 1.0).abs() < 1e-12);
    // Its mode is 1/18: the heaviest bin is the one holding it.
    let heaviest = BETA_PROBS
        .iter()
        .enumerate()
        .fold(
            (0, 0.0),
            |best, (index, &p)| if p > best.1 { (index, p) } else { best },
        )
        .0;
    assert_eq!(heaviest, 5);
}

/// ndarray's eight-lane order, worked by hand on values whose sum depends
/// on it: 2^53 swallows a 1 added to it alone (the tie rounds to even), but
/// not a 2.
#[test]
fn sums_go_in_ndarrays_order() {
    const BIG: f64 = 9_007_199_254_740_992.0;
    assert_eq!(ndarray_sum(&[]).to_bits(), 0.0_f64.to_bits());
    assert_eq!(ndarray_sum(&[1.0, 2.0, 3.0]).to_bits(), 6.0_f64.to_bits());
    // Lane 0 is BIG + 1 = BIG, lane 4 is 1, so (0 + 4) is BIG; lanes 1 and
    // 5 are 1 each, so adding (1 + 5) makes BIG + 2. In order, every 1 is
    // swallowed alone.
    let mut values = [0.0; 16];
    values[0] = BIG;
    values[1] = 1.0;
    values[4] = 1.0;
    values[5] = 1.0;
    values[8] = 1.0;
    let in_order: f64 = values.iter().sum();
    assert_eq!(ndarray_sum(&values).to_bits(), (BIG + 2.0).to_bits());
    assert_eq!(in_order.to_bits(), BIG.to_bits());
}

#[test]
fn thresholds_are_ndarrays_linspace() {
    for index in 0..THRESHOLDS {
        // ndarray's `start + step * i`, with start 0 and step (1 - 0) / 100.
        let step = (1.0 - 0.0) / 100.0;
        let along = step * (index + 1) as f64;
        let linspace = 0.0 + along;
        assert_eq!(threshold(index).to_bits(), linspace.to_bits());
    }
}

#[test]
fn troughs_are_found_at_the_ends_and_between() {
    assert_eq!(troughs(&[0.1, 0.5, 0.2, 0.2, 0.9, 0.3]), vec![0, 2, 5]);
    assert_eq!(troughs(&[0.5, 0.5, 0.5]), Vec::<usize>::new());
    assert_eq!(first_min([0.4, f64::NAN, 0.2, 0.2].into_iter()), Some(2));
    assert_eq!(first_min([f64::NAN].into_iter()), None);
}

#[test]
fn a_frame_without_troughs_has_no_probability() {
    let mut out = [1.0; 3];
    trough_probs(&[0.5, 0.5, 0.5], &mut out);
    assert_eq!(out.map(f64::to_bits), [0; 3]);
}

#[test]
fn one_trough_under_every_threshold_takes_everything() {
    let mut out = [0.0; 5];
    trough_probs(&[0.9, 0.8, 0.0, 0.8, 0.9], &mut out);
    assert_eq!(out[0].to_bits(), 0);
    assert!((out[2] - 1.0).abs() < 1e-12, "{out:?}");
}

/// A frame whose troughs are all over every threshold gives the global
/// minimum the no-trough share alone, 0.01, and the others nothing.
#[test]
fn troughs_over_every_threshold_leave_the_minimum_the_no_trough_share() {
    let mut out = [0.0; 7];
    trough_probs(&[1.5, 1.2, 1.6, 1.1, 1.7, 1.3, 1.8], &mut out);
    assert_eq!(out[1].to_bits(), 0);
    assert_eq!(out[5].to_bits(), 0);
    assert!((out[3] - 0.01).abs() < 1e-15, "{out:?}");
}
