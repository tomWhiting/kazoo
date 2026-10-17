use super::{Outcome, Wall, error};
use crate::protocol::{ErrorCode, Request, WallError};

pub(super) fn beats(value: f64, maximum: f64, name: &str) -> Result<(), WallError> {
    if !value.is_finite() || !(0.0..=maximum).contains(&value) {
        return Err(error(
            ErrorCode::BadRequest,
            format!("{name} must be a number from 0 to {maximum}"),
        ));
    }
    Ok(())
}

pub(super) fn validate(request: &Request) -> Result<(), WallError> {
    match request {
        Request::Add {
            after_beats: Some(_),
            ..
        }
        | Request::Remove {
            after_beats: Some(_),
            ..
        }
        | Request::Unpatch {
            after_beats: Some(_),
            ..
        } => Err(error(
            ErrorCode::BadRequest,
            "add, remove and unpatch happen at once; fade a level first with after_beats on a turn",
        )),
        Request::Patch { curve: Some(_), .. } => Err(error(
            ErrorCode::BadRequest,
            "a patch fades linearly; curve belongs on a turn",
        )),
        Request::Turn { after_beats, .. } => beats(*after_beats, 256.0, "after_beats"),
        Request::Patch {
            after_beats,
            fade_beats,
            ..
        } => {
            beats(*after_beats, 256.0, "after_beats")?;
            beats(*fade_beats, 64.0, "fade_beats")
        }
        Request::Tempo {
            after_beats,
            glide_beats,
            ..
        } => {
            beats(*after_beats, 256.0, "after_beats")?;
            beats(*glide_beats, 64.0, "glide_beats")
        }
        _ => Ok(()),
    }
}

impl Wall {
    pub(super) fn motion_route(
        &self,
        cable: &super::Cable,
        kinds: &std::collections::HashMap<&str, crate::catalogue::Kind>,
        source: (u32, u8, u32),
    ) -> crate::engine::Route {
        let (wait, glide) = self
            .cable_motion
            .get(&cable.id)
            .copied()
            .unwrap_or_default();
        crate::engine::Route {
            delay: self.timing.delays.get(&cable.id).copied().unwrap_or(0),
            gate: super::carries_gate(kinds, cable),
            wait,
            glide,
            ..crate::engine::Route::new(source.0, source.1, cable.amount, source.2)
        }
    }

    pub(super) fn scheduled_turn(
        &mut self,
        seat: &str,
        request: &Request,
    ) -> Result<Outcome, WallError> {
        let Request::Turn {
            module,
            knob,
            value,
            glide_beats,
            after_beats,
            curve,
        } = request
        else {
            return Err(error(
                ErrorCode::Internal,
                "scheduled turn requires a turn request",
            ));
        };
        let mut what = self.patch.turn(module, knob, *value, *glide_beats)?;
        let crate::protocol::What::Turn {
            after_beats: after,
            curve: shape,
            ..
        } = &mut what
        else {
            return Err(error(
                ErrorCode::Internal,
                "a turn did not produce a turn record",
            ));
        };
        *after = *after_beats;
        *shape = *curve;
        self.changed(seat, what, None, None, None)
    }

    pub(super) fn mirror_turn_motion(
        &mut self,
        module: &str,
        knob_name: &str,
        values: (f64, f64),
        timing: (f64, f64, crate::protocol::GlideCurve),
    ) {
        let slot = self.rack.slot_of(module);
        let index = self
            .patch
            .module(module)
            .and_then(|module| module.kind.spec().knob_index(knob_name));
        if let (Some(slot), Some(knob)) = (slot, index) {
            let frames = self.beat_frames(timing.0);
            let wait_frames = self.beat_frames(timing.1);
            self.engine.send(crate::engine::Command::Knob {
                slot,
                knob,
                target: values.1 as f32,
                glide_frames: frames,
                wait_frames,
                curve: timing.2,
            });
            let spec = self
                .patch
                .module(module)
                .filter(|module| {
                    matches!(
                        module.kind.spec().build,
                        crate::catalogue::Builder::Effect(_)
                    )
                })
                .and_then(|module| module.kind.spec().knobs.get(knob));
            if let Some(spec) = spec {
                let landing = super::landing(spec, values.0, values.1);
                let rate = f64::from(self.engine.sample_rate().max(1));
                let settles = std::time::Duration::from_secs_f64(
                    f64::from(frames).mul_add(landing, f64::from(wait_frames)) / rate,
                );
                self.turned.push((module.to_string(), knob, settles));
            }
        }
    }

    pub(super) fn mirror_patch_motion(
        &mut self,
        cable: &crate::protocol::CableRecord,
        replaced: Option<&crate::protocol::CableRecord>,
        timing: (f64, f64),
    ) {
        if let Some(old) = replaced {
            if old.from == cable.from && old.to == cable.to {
                if let Some(tag) = self.lined.remove(&old.id) {
                    self.lined.insert(cable.id, tag);
                }
                if let Some(place) = self.line_places.remove(&old.id) {
                    self.line_places.insert(cable.id, place);
                }
                if let Some(place) = self.sent_cables.remove(&old.id) {
                    self.sent_cables.insert(cable.id, place);
                }
            }
            self.cable_motion.remove(&old.id);
        }
        self.cable_motion.insert(
            cable.id,
            (self.beat_frames(timing.0), self.beat_frames(timing.1)),
        );
        self.send_tables();
    }

    pub(super) fn beat_frames(&self, beats: f64) -> u32 {
        let bpm = self
            .motion_bpm
            .unwrap_or_else(|| self.engine.shared().bpm());
        (beats * 60.0 * f64::from(self.engine.sample_rate().max(1)) / bpm)
            .max(0.0)
            .min(f64::from(u32::MAX))
            .round() as u32
    }

    /// Plug a cable for `seat`: checked, then the room for it, then
    /// plugged, its sound held `timing.0` beats and faded in over
    /// `timing.1` (both checked by [`validate`]).
    pub(super) fn scheduled_patch(
        &mut self,
        seat: &str,
        ends: (&str, &str),
        amount: Option<f64>,
        timing: (f64, f64),
    ) -> Result<Outcome, WallError> {
        let growth = super::Growth {
            cables: usize::from(self.patch.check_plug(ends.0, ends.1, amount)?),
            ..super::Growth::NONE
        };
        self.room_to_grow(growth)?;
        let mut what = self.patch.plug(ends.0, ends.1, amount, None)?;
        let crate::protocol::What::Patch {
            cable,
            after_beats,
            fade_beats,
            ..
        } = &mut what
        else {
            return Err(error(
                ErrorCode::Internal,
                "a patch did not produce a patch record",
            ));
        };
        *after_beats = timing.0;
        *fade_beats = timing.1;
        let cable = Some(cable.id);
        self.changed(seat, what, None, None, cable)
    }
}
