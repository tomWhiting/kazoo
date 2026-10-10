//! Where an effect writes its wet signal alone, for
//! [`crate::Effect::process_wet`].
//!
//! An effect with a mix knob works its wet sound out on every sample before
//! mixing it with the dry, so it hands the wet sound to a [`WetOut`] at the
//! same point: one pass serves both [`crate::Effect::process`] (where the
//! wet side goes nowhere) and [`crate::Effect::process_wet`].

/// The wet side of one block: two slices, or nowhere.
#[derive(Debug)]
pub struct WetOut<'a> {
    sides: Option<[&'a mut [f32]; 2]>,
}

impl<'a> WetOut<'a> {
    /// No wet side: everything written is dropped.
    #[must_use]
    pub const fn none() -> Self {
        Self { sides: None }
    }

    /// The wet side goes to `sides`, left and right.
    #[must_use]
    pub const fn to(sides: [&'a mut [f32]; 2]) -> Self {
        Self { sides: Some(sides) }
    }

    /// `frames` cut to the wet slices' length, so a block never runs past
    /// the shortest of all its slices.
    #[must_use]
    pub fn limit(&self, frames: usize) -> usize {
        self.sides.as_ref().map_or(frames, |[left, right]| {
            frames.min(left.len()).min(right.len())
        })
    }

    /// Write `sample` as frame `frame` of side `side` (0 left, 1 right).
    /// Past the end, or with no wet side, it is dropped.
    pub fn put(&mut self, side: usize, frame: usize, sample: f32) {
        if let Some(slot) = self
            .sides
            .as_mut()
            .and_then(|sides| sides.get_mut(side))
            .and_then(|channel| channel.get_mut(frame))
        {
            *slot = sample;
        }
    }

    /// Write frame `frame` of both sides.
    pub fn frame(&mut self, frame: usize, [left, right]: [f32; 2]) {
        self.put(0, frame, left);
        self.put(1, frame, right);
    }

    /// Silence both sides from frame `from` on.
    pub fn silence_from(&mut self, from: usize) {
        if let Some(sides) = self.sides.as_mut() {
            for side in sides.iter_mut() {
                if let Some(rest) = side.get_mut(from..) {
                    rest.fill(0.0);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::WetOut;

    #[test]
    fn nowhere_drops_everything_and_limits_nothing() {
        let mut wet = WetOut::none();
        assert_eq!(wet.limit(64), 64);
        wet.frame(3, [1.0, 1.0]);
        wet.silence_from(0);
    }

    #[test]
    fn two_slices_take_frames_and_set_the_length() {
        let (mut left, mut right) = ([9.0f32; 4], [9.0f32; 3]);
        let mut wet = WetOut::to([&mut left, &mut right]);
        assert_eq!(wet.limit(64), 3);
        wet.frame(0, [0.5, -0.5]);
        // Past the end of the right side: dropped, never a panic.
        wet.frame(3, [0.25, 0.25]);
        wet.silence_from(1);
        assert_eq!(
            left.map(f32::to_bits),
            [0.5f32, 0.0, 0.0, 0.0].map(f32::to_bits)
        );
        assert_eq!(
            right.map(f32::to_bits),
            [-0.5f32, 0.0, 0.0].map(f32::to_bits)
        );
    }
}
