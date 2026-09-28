//! Assertions shared by the unit tests.
//!
//! Float equality in tests goes through these instead of `assert_eq!`, so an
//! exact comparison is visibly deliberate: it uses IEEE equality (0.0 equals
//! −0.0, infinities equal themselves, NaN equals nothing).

use std::cmp::Ordering;

/// True when `a` and `b` are equal under IEEE comparison.
#[must_use]
pub fn floats_equal(a: f32, b: f32) -> bool {
    a.partial_cmp(&b) == Some(Ordering::Equal)
}

/// Assert `actual` equals `expected` exactly.
#[track_caller]
pub fn assert_float_eq(actual: f32, expected: f32) {
    assert!(
        floats_equal(actual, expected),
        "expected {expected:?}, got {actual:?}"
    );
}

/// Assert `actual` equals `expected` exactly, for `f64` values.
#[track_caller]
pub fn assert_f64_eq(actual: f64, expected: f64) {
    assert!(
        actual.partial_cmp(&expected) == Some(Ordering::Equal),
        "expected {expected:?}, got {actual:?}"
    );
}

/// Assert `actual` differs from `expected`.
#[track_caller]
pub fn assert_float_ne(actual: f32, unexpected: f32) {
    assert!(
        !floats_equal(actual, unexpected),
        "expected anything but {unexpected:?}"
    );
}

/// Assert two sample slices are equal, sample for sample.
#[track_caller]
pub fn assert_floats_eq(actual: &[f32], expected: &[f32]) {
    assert!(
        actual.len() == expected.len()
            && actual
                .iter()
                .zip(expected)
                .all(|(a, b)| floats_equal(*a, *b)),
        "expected {expected:?}, got {actual:?}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equality_follows_ieee() {
        assert!(floats_equal(0.0, -0.0));
        assert!(floats_equal(f32::NEG_INFINITY, f32::NEG_INFINITY));
        assert!(!floats_equal(f32::NAN, f32::NAN));
        assert!(!floats_equal(1.0, 1.0 + f32::EPSILON));
        assert_floats_eq(&[0.0, 1.0], &[-0.0, 1.0]);
        assert_float_ne(1.0, 2.0);
    }
}
