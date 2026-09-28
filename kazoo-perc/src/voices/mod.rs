//! The voices. Each lives in its own file, documents the instrument or
//! circuit it models, and is listed in [`KINDS`].
//!
//! The analogue voices (`kick`, `snare`, `clap`, `hat`, `cymbal`, `tom`,
//! `conga`, `rim`, `cowbell`, `metal`, `noiseperc`) restart their circuit on
//! every strike and let [`Finish`] fade the old tail out, as a drum machine
//! does. The physical voices (`shaker`, `membrane`, `modal`) add each strike
//! to the object that is already ringing, as a real one does.

pub mod clap;
pub mod conga;
pub mod cowbell;
pub mod cymbal;
pub mod hat;
pub mod kick;
pub mod membrane;
pub mod metal;
pub mod modal;
pub mod noiseperc;
pub mod rim;
pub mod shaker;
pub mod snare;
pub mod tom;

use crate::VoiceKind;
use crate::parts::Finish;

/// Every voice, in the order a drum machine would list them.
pub static KINDS: &[VoiceKind] = &[
    kick::KIND,
    snare::KIND,
    clap::KIND,
    hat::KIND,
    cymbal::KIND,
    tom::KIND,
    conga::KIND,
    rim::KIND,
    cowbell::KIND,
    shaker::KIND,
    membrane::KIND,
    modal::KIND,
    metal::KIND,
    noiseperc::KIND,
];

/// What [`run`] needs from a voice to render it.
pub(crate) trait Circuit {
    /// Whether the voice has anything left to render besides its finish.
    fn is_active(&self) -> bool;
    /// The voice's last stage.
    fn finish(&mut self) -> &mut Finish;
    /// Advance every parameter glide by a sample.
    fn step_params(&mut self);
    /// Settle every parameter on its target.
    fn snap_params(&mut self);
    /// One dry sample. Only called while active; falls inactive itself when
    /// it has died away.
    fn render(&mut self) -> f32;
    /// Stop everything ringing and fall inactive.
    fn silence(&mut self);
}

/// Render `out` from `circuit`, as every voice's `process` does: exact
/// silence while idle, otherwise one sample at a time through the finish.
pub(crate) fn run<C: Circuit>(circuit: &mut C, out: &mut [f32]) {
    if !circuit.is_active() && circuit.finish().is_quiet() {
        circuit.snap_params();
        out.fill(0.0);
        return;
    }
    for sample in out {
        circuit.step_params();
        let dry = if circuit.is_active() {
            circuit.render()
        } else {
            0.0
        };
        let (wet, clear) = circuit.finish().next(dry);
        if clear {
            circuit.silence();
        }
        *sample = wet;
    }
}

/// Get ready to strike: a voice that was silent takes its knobs as they
/// are now, so a knob set just before a hit applies to all of that hit
/// instead of gliding in during it.
pub(crate) fn wake<C: Circuit>(circuit: &mut C) {
    if !circuit.is_active() {
        circuit.snap_params();
    }
}

/// The level of a strike: `velocity` raised by the accent.
#[must_use]
pub(crate) fn strike_level(velocity: f32, accent: bool) -> f32 {
    if accent { velocity * 1.35 } else { velocity }
}
