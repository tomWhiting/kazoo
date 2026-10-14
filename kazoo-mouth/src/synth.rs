use kazoo_core::effects::{BiquadFilter, FilterType};
use kazoo_core::synthesis::{OscillatorShape, PitchTrackedSynth};
use kazoo_core::voice::VoiceFrame;
use kazoo_core::{ParamResult, Processor, sanitize_sample};

mod voice;
use voice::VoiceBlend;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod voice_tests;

// The processor's parameter table defines these indices; controls remain Copy.
const PARAM_SHAPE: usize = 0;
const PARAM_DETUNE: usize = 1;
const PARAM_FILTER_CUTOFF: usize = 2;
const PARAM_FILTER_Q: usize = 3;
const PARAM_PORTAMENTO: usize = 4;
const PARAM_ENV_SENSITIVITY: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SynthSettings {
    pub shape: OscillatorShape,
    pub cutoff_hz: f32,
    pub air: f32,
    pub volume: f32,
    pub voice: f32,
    pub bands: u8,
    pub sibilance: f32,
}

impl Default for SynthSettings {
    fn default() -> Self {
        Self {
            shape: OscillatorShape::Saw,
            cutoff_hz: 2400.0,
            air: 0.25,
            volume: 0.7,
            voice: 0.0,
            bands: 16,
            sibilance: 0.3,
        }
    }
}

impl SynthSettings {
    #[must_use]
    pub const fn held(self) -> Self {
        Self {
            shape: self.shape,
            cutoff_hz: held(self.cutoff_hz, 100.0, 12_000.0, 2400.0),
            air: held(self.air, 0.0, 1.0, 0.25),
            volume: held(self.volume, 0.0, 1.0, 0.7),
            voice: held(self.voice, 0.0, 1.0, 0.0),
            bands: match self.bands {
                8 | 16 | 24 => self.bands,
                _ => 16,
            },
            sibilance: held(self.sibilance, 0.0, 1.0, 0.3),
        }
    }
}

const fn held(value: f32, min: f32, max: f32, default: f32) -> f32 {
    if value.is_nan() {
        default
    } else {
        value.clamp(min, max)
    }
}

#[derive(Debug)]
pub struct VoiceSynth {
    tone: PitchTrackedSynth,
    voice: VoiceBlend,
    carrier: Vec<f32>,
    air_filter: BiquadFilter,
    noise: Vec<f32>,
    filtered_noise: Vec<f32>,
    random: u32,
    settings: SynthSettings,
    air_gain: f32,
    gate: f32,
    gate_open: bool,
    attack: f32,
    release: f32,
}

impl VoiceSynth {
    pub fn new(rate: u32, block: usize) -> ParamResult<Self> {
        let rate = rate.max(1) as f32;
        let block = block.max(1);
        let mut tone = PitchTrackedSynth::new(rate);
        tone.prepare(block);
        tone.set_param(PARAM_DETUNE, 0.0)?;
        tone.set_param(PARAM_FILTER_Q, 0.9)?;
        tone.set_param(PARAM_PORTAMENTO, 3.0)?;
        tone.set_param(PARAM_ENV_SENSITIVITY, 0.5)?;
        let mut air_filter = BiquadFilter::new(FilterType::BandPass, rate);
        air_filter.set_frequency(200.0);
        air_filter.set_q(1.5);
        let mut synth = Self {
            tone,
            voice: VoiceBlend::new(rate, block),
            carrier: vec![0.0; block],
            air_filter,
            noise: vec![0.0; block],
            filtered_noise: vec![0.0; block],
            random: 0x6D2B_79F5,
            settings: SynthSettings::default(),
            air_gain: 0.0,
            gate: 0.0,
            gate_open: false,
            attack: (-1000.0 / (3.0 * rate)).exp(),
            release: (-1000.0 / (60.0 * rate)).exp(),
        };
        synth.set(SynthSettings::default())?;
        Ok(synth)
    }

    pub fn set(&mut self, settings: SynthSettings) -> ParamResult<()> {
        let settings = settings.held();
        let shape = match settings.shape {
            OscillatorShape::Sine => 0.0,
            OscillatorShape::Saw => 1.0,
            OscillatorShape::Square => 2.0,
            OscillatorShape::Triangle => 3.0,
        };
        self.tone.set_param(PARAM_SHAPE, shape)?;
        self.tone
            .set_param(PARAM_FILTER_CUTOFF, settings.cutoff_hz)?;
        self.voice.set(settings);
        self.settings = settings;
        Ok(())
    }

    pub fn control(&mut self, frame: &VoiceFrame) {
        if frame.voiced {
            self.tone.set_target_frequency(frame.snapped_hz);
        }
        self.air_filter
            .set_frequency((2.0 * sanitize_sample(frame.snapped_hz)).clamp(200.0, 8000.0));
        self.air_gain = sanitize_sample(frame.breath).clamp(0.0, 1.0)
            * sanitize_sample(frame.loudness).clamp(0.0, 1.0);
        self.gate_open = frame.gate;
    }

    pub fn render(&mut self, mic: &[f32], out: &mut [f32]) {
        let len = mic.len().min(out.len());
        out[len..].fill(0.0);
        for (mic, out) in mic[..len]
            .chunks(self.noise.len())
            .zip(out[..len].chunks_mut(self.noise.len()))
        {
            self.tone
                .process_with_carrier(mic, out, &mut self.carrier[..mic.len()]);
            self.voice.prepare(mic, &self.carrier[..mic.len()]);
            for sample in &mut self.noise[..mic.len()] {
                self.random ^= self.random << 13;
                self.random ^= self.random >> 17;
                self.random ^= self.random << 5;
                *sample = (self.random as f32 / u32::MAX as f32).mul_add(2.0, -1.0);
            }
            self.air_filter.process(
                &self.noise[..mic.len()],
                &mut self.filtered_noise[..mic.len()],
            );
            let target = if self.gate_open { 1.0 } else { 0.0 };
            let coefficient = if self.gate_open {
                self.attack
            } else {
                self.release
            };
            for (index, (tone, &air)) in out
                .iter_mut()
                .zip(&self.filtered_noise[..mic.len()])
                .enumerate()
            {
                self.gate = coefficient.mul_add(self.gate - target, target);
                let x = (self.air_gain * self.settings.air).mul_add(air, *tone)
                    * self.gate
                    * self.settings.volume;
                let plain = sanitize_sample(x / (1.0 + x.abs()));
                *tone = self
                    .voice
                    .mix(index, plain, self.gate * self.settings.volume);
            }
        }
    }
}
