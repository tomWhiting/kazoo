pub(super) const fn finite(value: f64) -> f64 {
    if value.is_finite() { value } else { 0.0 }
}
