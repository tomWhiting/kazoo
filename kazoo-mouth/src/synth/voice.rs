use std::f32::consts::FRAC_PI_2;

use kazoo_core::effects::{BiquadFilter, FilterType};
use kazoo_core::synthesis::{Vocoder, VocoderBands, VocoderCarrierMode};
use kazoo_core::{Processor, sanitize_sample};

use super::SynthSettings;

#[derive(Debug)]
struct Ramp {
    value: f32,
    target: f32,
    step: f32,
    remaining: u32,
    length: u32,
}

impl Ramp {
    fn new(value: f32, rate: f32) -> Self {
        Self {
            value,
            target: value,
            step: 0.0,
            remaining: 0,
            length: (rate * 0.01).round().max(1.0) as u32,
        }
    }

    fn set(&mut self, target: f32) {
        if target.to_bits() != self.target.to_bits() {
            self.target = target;
            self.step = (target - self.value) / self.length as f32;
            self.remaining = self.length;
        }
    }

    /// Silent now and staying silent: whatever it scales can rest.
    fn silent(&self) -> bool {
        self.value == 0.0 && self.target == 0.0
    }

    fn next(&mut self) -> f32 {
        if self.remaining > 0 {
            self.remaining -= 1;
            self.value = if self.remaining == 0 {
                self.target
            } else {
                self.value + self.step
            };
        }
        self.value
    }
}

#[derive(Debug)]
pub(super) struct VoiceBlend {
    /// Every band count runs while the voice is up, so a change of bands
    /// crossfades between two settled banks and never clicks.
    banks: [Vocoder; 3],
    /// The blend rests while the voice is at 0, and starts again from
    /// silence, never from the voice it heard before it rested.
    resting: bool,
    outputs: [Vec<f32>; 3],
    high_pass: BiquadFilter,
    consonants: Vec<f32>,
    voice: Ramp,
    bands: [Ramp; 3],
    sibilance: Ramp,
}

impl VoiceBlend {
    pub(super) fn new(rate: f32, block: usize) -> Self {
        let banks = [
            VocoderBands::Eight,
            VocoderBands::Sixteen,
            VocoderBands::TwentyFour,
        ]
        .map(|bands| {
            let mut vocoder = Vocoder::with_bands(rate, bands);
            vocoder.set_carrier_mode(VocoderCarrierMode::ExternalInput);
            vocoder.prepare(block);
            vocoder
        });
        let mut high_pass = BiquadFilter::new(FilterType::HighPass, rate);
        high_pass.set_frequency(5000.0);
        Self {
            banks,
            resting: true,
            outputs: std::array::from_fn(|_| vec![0.0; block]),
            high_pass,
            consonants: vec![0.0; block],
            voice: Ramp::new(0.0, rate),
            bands: [
                Ramp::new(0.0, rate),
                Ramp::new(1.0, rate),
                Ramp::new(0.0, rate),
            ],
            sibilance: Ramp::new(0.3, rate),
        }
    }

    pub(super) fn set(&mut self, settings: SynthSettings) {
        self.voice.set(settings.voice);
        self.sibilance.set(settings.sibilance);
        for (index, gain) in self.bands.iter_mut().enumerate() {
            gain.set(f32::from(usize::from(settings.bands / 8 - 1) == index));
        }
    }

    pub(super) fn prepare(&mut self, mic: &[f32], carrier: &[f32]) {
        if self.voice.silent() {
            self.resting = true;
            return;
        }
        if self.resting {
            for bank in &mut self.banks {
                bank.reset();
            }
            self.high_pass.reset();
            self.resting = false;
        }
        for (bank, output) in self.banks.iter_mut().zip(&mut self.outputs) {
            bank.process_with_carrier(mic, carrier, &mut output[..mic.len()]);
        }
        self.high_pass
            .process(mic, &mut self.consonants[..mic.len()]);
    }

    pub(super) fn mix(&mut self, index: usize, plain: f32, level: f32) -> f32 {
        let voice = self.voice.next();
        let mut wet = 0.0;
        for (gain, output) in self.bands.iter_mut().zip(&self.outputs) {
            wet = gain.next().mul_add(output[index], wet);
        }
        wet = self.sibilance.next().mul_add(self.consonants[index], wet) * level;
        if voice == 0.0 {
            return plain;
        }
        wet = sanitize_sample(wet / (1.0 + wet.abs()));
        if voice >= 1.0 {
            return wet;
        }
        let angle = voice * FRAC_PI_2;
        sanitize_sample(angle.cos().mul_add(plain, angle.sin() * wet))
    }
}
