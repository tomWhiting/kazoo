//! Reproducible random streams, identified by the wall seed and module id.

use std::hash::{BuildHasher, Hash, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

/// Hash every byte with the stable 64-bit FNV-1a algorithm.
#[must_use]
pub const fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut index = 0;
    while index < bytes.len() {
        let byte = u64::from_le_bytes([bytes[index], 0, 0, 0, 0, 0, 0, 0]);
        hash = (hash ^ byte).wrapping_mul(0x0000_0100_0000_01b3);
        index += 1;
    }
    hash
}

/// Derive a module's seed from its stable id, independent of its placement.
#[must_use]
pub const fn module_seed(wall_seed: u64, id: &str) -> u64 {
    kazoo_fx::dsp::splitmix64(wall_seed ^ fnv1a64(id.as_bytes()))
}

/// Make a new wall's seed from an OS-keyed hasher on the control side.
#[must_use]
pub fn fresh_wall_seed() -> u64 {
    let random = std::collections::hash_map::RandomState::new();
    let mut hasher = random.build_hasher();
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
        .hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    kazoo_fx::dsp::splitmix64(hasher.finish())
}

#[cfg(test)]
mod tests;
