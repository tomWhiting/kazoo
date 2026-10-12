use std::sync::Arc;

use crate::player::index;
use crate::{PlayerInputs, SampleData, SampleName, SamplePlayer, resample};

#[test]
fn lint_arithmetic_preserves_sample_values() {
    let input = [0.25, -0.75, 0.5, 0.125, -0.375, 0.875, -0.5, 0.625];
    let mut actual = resample::resample(&input, 48_000, 32_000).unwrap();
    let left: Vec<f32> = (0..256).map(|n| (n as f32 * 0.31).sin()).collect();
    let right: Vec<f32> = left.iter().map(|x| x * 0.7).collect();
    let data = Arc::new(
        SampleData::new(SampleName::new("arithmetic").unwrap(), 48_000, left, right).unwrap(),
    );
    for mode in [0.0, 5.0] {
        let (mut player, mut reaper) = SamplePlayer::new();
        player.prepare(48_000.0);
        player.set_param(index::MODE, mode);
        player.set_param(index::VELOCITY, 0.63);
        player.set_param(index::START, 0.2);
        player.set_param(index::POSITION, 0.37);
        player.set_param(index::SIZE, 0.005);
        player.set_param(index::SPRAY, 0.0);
        player.set_param(index::FREEZE, 1.0);
        player.load(Arc::clone(&data)).unwrap();
        let (mut left, mut right) = ([0.0; 64], [0.0; 64]);
        player.process(
            &PlayerInputs {
                gate: &[1.0],
                pitch: &[],
                velocity: &[0.37],
            },
            [&mut left, &mut right],
        );
        for n in [8, 16, 32, 63] {
            actual.extend([left[n], right[n]]);
        }
        drop(player);
        reaper.collect();
    }
    let expected = [
        -0.125_327_83,
        -0.085_070_334,
        0.162_329_8,
        0.081_601_8,
        0.210_845_05,
        0.166_011_15,
        -0.021_605_616,
        -0.015_123_931,
        0.078_311_29,
        0.054_817_896,
        0.101_518_11,
        0.071_062_68,
        -0.240_353_33,
        -0.168_247_33,
        0.000_204_986_78,
        0.000_143_490_73,
        0.000_855_829_74,
        0.000_599_080_75,
        0.027_431_602,
        0.019_202_119,
        -0.169_999_73,
        -0.118_999_8,
    ];
    assert_eq!(actual.len(), expected.len());
    assert!(
        actual
            .iter()
            .zip(expected)
            .all(|(a, e)| (a - e).abs() < 1e-6),
        "{actual:?}"
    );
}
