#[derive(Debug, Default, Clone, Copy)]
pub struct Latency {
    pub(crate) input: Option<f64>,
    pub(crate) resampler: Option<f64>,
    pub(crate) block: Option<f64>,
    pub(crate) effects: Option<f64>,
    pub(crate) output: Option<f64>,
}

fn valid(value: Option<f64>) -> Option<f64> {
    value.filter(|ms| ms.is_finite() && *ms >= 0.0)
}

fn part(value: Option<f64>) -> String {
    valid(value).map_or_else(|| "?".into(), |ms| format!("{ms:.1}"))
}

impl Latency {
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
        let prefix = if parts.iter().any(Option::is_none) {
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
            part(parts[0]),
            part(dsp),
            part(parts[4])
        )
    }

    pub(crate) fn breakdown(self) -> String {
        format!(
            "in {} + resampler {} + block {} + fx {} + out {} ms",
            part(self.input),
            part(self.resampler),
            part(self.block),
            part(self.effects),
            part(self.output)
        )
    }
}
