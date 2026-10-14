/// The parts of the delay from the mic to the speaker, in milliseconds, each
/// as the host or the engine reports it; `None` where it is not known.
#[derive(Debug, Default, Clone, Copy)]
pub struct Latency {
    pub(crate) input: Option<f64>,
    pub(crate) resampler: Option<f64>,
    pub(crate) block: Option<f64>,
    pub(crate) effects: Option<f64>,
    pub(crate) output: Option<f64>,
    /// The host's in and out figures count the device buffers only
    /// (`CoreAudio` and WASAPI under cpal 0.17): converter, safety-offset and
    /// Bluetooth delay are missing, so those parts and the sum are floors.
    pub(crate) device_buffers_only: bool,
}

fn valid(value: Option<f64>) -> Option<f64> {
    value.filter(|ms| ms.is_finite() && *ms >= 0.0)
}

fn part(value: Option<f64>) -> String {
    valid(value).map_or_else(|| "?".into(), |ms| format!("{ms:.1}"))
}

impl Latency {
    /// A device part: marked `≥` when the host counts its buffer only.
    fn device(self, value: Option<f64>) -> String {
        match valid(value) {
            Some(ms) if self.device_buffers_only => format!("≥{ms:.1}"),
            known => part(known),
        }
    }

    pub(crate) fn header(self) -> String {
        let parts = [
            self.input,
            self.resampler,
            self.block,
            self.effects,
            self.output,
        ]
        .map(valid);
        let sum = parts.iter().flatten().fold(0.0, |total, ms| total + ms);
        let prefix = if self.device_buffers_only || parts.iter().any(Option::is_none) {
            "≥ "
        } else {
            ""
        };
        let dsp = parts[1]
            .zip(parts[2])
            .zip(parts[3])
            .map(|((a, b), c)| a + b + c);
        format!(
            "{prefix}{sum:.1} ms · in {} / dsp {} / out {}",
            self.device(parts[0]),
            part(dsp),
            self.device(parts[4])
        )
    }

    pub(crate) fn breakdown(self) -> String {
        let sum = format!(
            "in {} + resampler {} + block {} + fx {} + out {} ms",
            part(self.input),
            part(self.resampler),
            part(self.block),
            part(self.effects),
            part(self.output)
        );
        if self.device_buffers_only {
            format!(
                "{sum} · in and out are the device buffers only: this host reports no converter, safety-offset or Bluetooth delay"
            )
        } else {
            sum
        }
    }
}
