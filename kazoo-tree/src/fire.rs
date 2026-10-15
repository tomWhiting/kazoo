//! The moment the wall is playing, set ablaze.
//!
//! A fire burns at the wall's place on the tree, its flames, wisps and
//! sparks rising into the night above it. It is a heat field at braille-dot
//! resolution, fed along the branch through the moment in tongues whose
//! roots wander, carried upward along drifting turbulence, and cooled by a
//! texture that rises with it, so the tongues tear into licks and wisps;
//! sparks are particles that rise, wander and fade. When the wall moves to
//! another moment, the old fire dies down where it was, sinking to embers,
//! while the new one catches from a spark. Each frame the dots whose heat
//! beats a dither threshold that turns every frame are lit (the eye blends
//! the turning into smooth gradients), coloured by their heat, and each
//! cell's background glows with its mean heat.
//!
//! The field is a fixed grid of dots around the wall's place, whatever the
//! tree's size: far out a bright ember with a few sparks, closer in flames
//! along the limb they burn on, right in a full blaze with finer detail. It is drawn over
//! the tree's own picture only: it never moves the tree, never changes what
//! is read, and never the music.

pub mod blaze;
pub mod field;
pub mod flicks;
pub mod fuel;
pub mod noise;
pub mod paint;
pub mod place;
pub mod shape;

pub use blaze::Blaze;
pub use field::Field;
pub use place::Placement;

#[cfg(test)]
mod flames_tests;
#[cfg(test)]
mod frame_tests;
#[cfg(test)]
mod tests;
