//! The rack view's maths: where faceplates, knobs, jacks and cables sit
//! ([`geometry`]), how a knob is drawn as a dial ([`dial`]), and how a
//! scope's trace is drawn on its screen ([`screen`]). Drawing it is in
//! [`super::draw`]; what the mouse does, in [`super::app`].

pub mod dial;
pub mod fold;
pub mod geometry;
pub mod screen;
