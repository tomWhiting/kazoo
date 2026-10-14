//! Randomness for the fire: a small seeded source for sparks, and smooth
//! value noise for the turbulence the flames ride on.

/// A small, fast, seeded random source (xorshift64), so the fire burns the
/// same way on every run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rng(u64);

impl Rng {
    /// A source started at `seed`; a zero seed (which xorshift never
    /// leaves) is moved off zero.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9e37_79b9_7f4a_7c15
        } else {
            seed
        })
    }

    /// The next 64 random bits.
    pub const fn next_bits(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A number in `0.0..1.0`.
    pub fn unit(&mut self) -> f32 {
        // The top 24 bits: exactly representable in an f32.
        (self.next_bits() >> 40) as f32 / 16_777_216.0
    }
}

/// A lattice point's value, in `-1.0..=1.0`.
fn hash(x: i32, y: i32, z: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ (z as u32).wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    f32::from((h & 0xffff) as u16).mul_add(2.0 / 65_535.0, -1.0)
}

/// Where a coordinate falls: its lattice point, clamped far inside `i32`,
/// and the smoothed fraction past it. A coordinate that is not finite
/// reads as zero, so noise is always finite.
fn lattice(v: f32) -> (i32, f32) {
    if !v.is_finite() {
        return (0, 0.0);
    }
    let floor = v.floor().clamp(-1.0e9, 1.0e9);
    let t = (v - floor).clamp(0.0, 1.0);
    (floor as i32, t * t * 2.0f32.mul_add(-t, 3.0))
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    (b - a).mul_add(t, a)
}

/// Smooth value noise at `(x, y, z)`, in `-1.0..=1.0`: neighbours alike,
/// one lattice step apart unlike. Always finite.
#[must_use]
pub fn noise(x: f32, y: f32, z: f32) -> f32 {
    let (xi, fx) = lattice(x);
    let (yi, fy) = lattice(y);
    let (zi, fz) = lattice(z);
    let plane = |dz: i32| {
        let z = zi.wrapping_add(dz);
        let x1 = xi.wrapping_add(1);
        let y1 = yi.wrapping_add(1);
        lerp(
            lerp(hash(xi, yi, z), hash(x1, yi, z), fx),
            lerp(hash(xi, y1, z), hash(x1, y1, z), fx),
            fy,
        )
    };
    lerp(plane(0), plane(1), fz)
}
