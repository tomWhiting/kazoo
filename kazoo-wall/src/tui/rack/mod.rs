//! The rack view's maths: where faceplates, knobs, jacks and cables sit
//! ([`geometry`]), and how a knob is drawn as a dial ([`dial`]). Drawing it
//! is in [`super::draw`]; what the mouse does, in [`super::app`].

pub mod dial;
pub mod geometry;
