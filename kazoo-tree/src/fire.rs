//! The moment the wall is playing, set ablaze.
//!
//! A small fire burns at the wall's place on the tree, its flames and
//! sparks rising into the night above it. It is a heat field at braille-dot
//! resolution, carried upward along smooth drifting turbulence, cooling as
//! it climbs and fed from a flickering source at the burning point; sparks
//! are particles that rise, wander and fade. Each frame the dots whose heat
//! beats a dither threshold that turns every frame are lit (the eye blends
//! the turning into smooth gradients), coloured by their heat, and each
//! cell's background glows with its mean heat.
//!
//! The field is a fixed grid of dots around the wall's place, whatever the
//! tree's size: far out a bright ember with a few sparks, closer in flames
//! as wide as the limb they burn on, right in a full blaze. It is drawn over
//! the tree's own picture only: it never moves the tree, never changes what
//! is read, and never the music.

pub mod blaze;
pub mod field;
pub mod noise;
pub mod paint;
pub mod place;

pub use blaze::Blaze;
pub use field::Field;
pub use place::Placement;

#[cfg(test)]
mod tests;
