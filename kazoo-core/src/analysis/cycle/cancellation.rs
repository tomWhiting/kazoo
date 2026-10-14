use super::Peak;

#[derive(Debug, Clone, Copy)]
pub(super) struct Comb {
    pub(super) period: f32,
    pub(super) gain: f32,
}

#[derive(Debug)]
pub(super) struct Cancellation {
    pub(super) raw: Box<[f32]>,
    pub(super) combs: Vec<Comb>,
    stages: Vec<f32>,
    length: usize,
    mask: usize,
}

impl Cancellation {
    pub(super) fn new(length: usize, max_lag: usize) -> Self {
        Self {
            raw: vec![0.0; length].into_boxed_slice(),
            combs: Vec::with_capacity(max_lag),
            stages: vec![0.0; length * max_lag],
            length,
            mask: length - 1,
        }
    }

    pub(super) fn configure(&mut self, ringing: &mut Vec<Peak>, attack: usize) {
        ringing.retain(|peak| peak.clarity >= 0.60);
        ringing.sort_unstable_by(|a, b| b.clarity.total_cmp(&a.clarity));
        let mut count = 0;
        for index in 0..ringing.len() {
            let candidate = ringing[index];
            let redundant = ringing[..count].iter().any(|stronger| {
                if stronger.clarity <= candidate.clarity {
                    return false;
                }
                let ratio = candidate.period / stronger.period;
                let ratio = if ratio < 1.0 { ratio.recip() } else { ratio };
                (1200.0 * (ratio / ratio.round()).log2()).abs() <= 5.0
            });
            if !redundant {
                ringing[count] = candidate;
                count += 1;
            }
        }
        ringing.truncate(count);
        self.combs.clear();
        for peak in ringing {
            let window = peak.period.ceil() as usize;
            let mut product = 0.0_f64;
            let mut energy = 0.0_f64;
            for offset in 0..window {
                let at = attack.wrapping_sub(offset + 1);
                let newer = f64::from(self.raw[at & self.mask]);
                let older = f64::from(delayed(&self.raw, self.mask, at, peak.period));
                product = newer.mul_add(older, product);
                energy = older.mul_add(older, energy);
            }
            let gain = if energy > 1e-12 {
                (product / energy).clamp(0.9, 1.0) as f32
            } else {
                0.9
            };
            self.combs.push(Comb {
                period: peak.period,
                gain,
            });
        }
        self.stages[..self.combs.len() * self.length].fill(0.0);
    }

    pub(super) fn push(&mut self, mut sample: f32, at: usize) -> f32 {
        for (comb, history) in self.combs.iter().zip(self.stages.chunks_mut(self.length)) {
            let previous = delayed(history, self.mask, at, comb.period);
            history[at & self.mask] = sample;
            sample = crate::sanitize_sample((-comb.gain).mul_add(previous, sample));
        }
        sample
    }
}

fn delayed(history: &[f32], mask: usize, at: usize, period: f32) -> f32 {
    let lag = period.floor() as usize;
    let fraction = period - lag as f32;
    let younger = history[at.wrapping_sub(lag).wrapping_add(1) & mask];
    let current = history[at.wrapping_sub(lag) & mask];
    let older = history[at.wrapping_sub(lag + 1) & mask];
    let oldest = history[at.wrapping_sub(lag + 2) & mask];
    let a = -fraction * (fraction - 1.0) * (fraction - 2.0) / 6.0;
    let b = (fraction + 1.0) * (fraction - 1.0) * (fraction - 2.0) / 2.0;
    let c = -(fraction + 1.0) * fraction * (fraction - 2.0) / 2.0;
    let d = (fraction + 1.0) * fraction * (fraction - 1.0) / 6.0;
    a.mul_add(younger, b.mul_add(current, c.mul_add(older, d * oldest)))
}
