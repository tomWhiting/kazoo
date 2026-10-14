use std::sync::atomic::Ordering;

use kazoo_core::sanitize_sample;
use kazoo_fx::{Context, Effect, EffectKind, ParamSpec};

use super::Stats;

#[cfg(test)]
mod tests;

pub mod harmony;

pub const MAX_PARAMS: usize = 16;
pub const RAMP_SECONDS: f32 = 0.010;
const SEED: u64 = 0x6D6F_7574_6800;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Harmony,
    Megaphone,
    Distortion,
    Delay,
    Echo,
    Reverb,
}

impl Slot {
    pub const ALL: [Self; 6] = [
        Self::Harmony,
        Self::Megaphone,
        Self::Distortion,
        Self::Delay,
        Self::Echo,
        Self::Reverb,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Harmony => "harmony",
            Self::Megaphone => "megaphone",
            Self::Distortion => "distortion",
            Self::Delay => "delay",
            Self::Echo => "echo",
            Self::Reverb => "reverb",
        }
    }

    pub const fn ids(self) -> &'static [&'static str] {
        match self {
            Self::Harmony => &["pitch"],
            Self::Megaphone => &["radio"],
            Self::Distortion => &["fuzz", "screamer", "amp", "crush"],
            Self::Delay => &["digital", "bbd"],
            Self::Echo => &["tape"],
            Self::Reverb => &["plate", "hall", "spring"],
        }
    }

    pub fn kind(self, index: usize) -> Option<&'static EffectKind> {
        if self == Self::Harmony && index == 0 {
            Some(&harmony::KIND)
        } else {
            self.ids().get(index).and_then(|id| kazoo_fx::find(id))
        }
    }

    const fn trail(self) -> bool {
        matches!(self, Self::Delay | Self::Echo | Self::Reverb)
    }

    pub fn seed(self, kind: usize) -> u64 {
        let before: usize = Self::ALL[..self as usize]
            .iter()
            .map(|slot| slot.ids().len())
            .sum();
        if self == Self::Harmony {
            SEED + Self::ALL
                .iter()
                .filter(|slot| **slot != Self::Harmony)
                .map(|slot| slot.ids().len())
                .sum::<usize>() as u64
        } else {
            SEED + (before.saturating_sub(Self::Harmony.ids().len()) + kind) as u64
        }
    }
}

/// Model 3 selects the horn; drive 50 gives it a pronounced voice before the
/// separate distortion. Noise 0 prevents hiss while the synth gate is silent.
/// Left sync 7 is a dotted eighth against right sync 9's quarter note;
/// routing 1 spreads those repeats across stereo with ping-pong.
pub fn mouth_default(slot: Slot, kind: usize, spec: &ParamSpec) -> f32 {
    match (slot, kind, spec.name) {
        (Slot::Megaphone, 0, "model") => 3.0,
        (Slot::Megaphone, 0, "drive") => 50.0,
        (Slot::Megaphone, 0, "noise") => 0.0,
        (Slot::Delay, 0, "lsync") => 7.0,
        (Slot::Delay, 0, "rsync") => 9.0,
        (Slot::Delay, 0, "routing") => 1.0,
        _ => spec.default,
    }
}

pub fn permutation(order: [Slot; Slot::ALL.len()]) -> bool {
    Slot::ALL
        .iter()
        .all(|slot| order.iter().filter(|entry| *entry == slot).count() == 1)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlotSettings {
    pub on: bool,
    pub active: usize,
    pub values: [[f32; MAX_PARAMS]; 4],
    pub custom: [f32; 7],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FxSettings {
    pub order: [Slot; Slot::ALL.len()],
    pub slots: [SlotSettings; Slot::ALL.len()],
}

impl Default for FxSettings {
    fn default() -> Self {
        let slots = Slot::ALL.map(|slot| {
            let mut settings = SlotSettings {
                on: false,
                active: 0,
                values: [[0.0; MAX_PARAMS]; 4],
                custom: harmony::KNOBS[harmony::DEFAULT],
            };
            for kind in 0..slot.ids().len() {
                if let Some(description) = slot.kind(kind) {
                    for (index, spec) in description.params.iter().take(MAX_PARAMS).enumerate() {
                        settings.values[kind][index] = mouth_default(slot, kind, spec);
                    }
                }
            }
            settings
        });
        Self {
            order: Slot::ALL,
            slots,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Ramp {
    value: f32,
    target: f32,
    step: f32,
    frames: usize,
    remaining: usize,
}

impl Ramp {
    pub const fn new(value: f32, frames: usize) -> Self {
        Self {
            value,
            target: value,
            step: 0.0,
            frames,
            remaining: 0,
        }
    }

    pub fn aim(&mut self, target: f32) {
        if target.to_bits() == self.value.to_bits() {
            self.snap(target);
            return;
        }
        self.target = target;
        self.remaining = self.frames.max(1);
        self.step = (target - self.value) / self.remaining as f32;
    }

    pub const fn snap(&mut self, value: f32) {
        self.value = value;
        self.target = value;
        self.remaining = 0;
        self.step = 0.0;
    }

    pub const fn next(&mut self) -> f32 {
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

    pub const fn moving(self) -> bool {
        self.remaining > 0
    }
}

#[derive(Debug)]
struct Kind {
    effect: Box<dyn Effect>,
    values: [f32; MAX_PARAMS],
    description: &'static EffectKind,
    custom: [f32; 7],
}

#[derive(Debug)]
struct Buffers<'a> {
    input: [&'a [f32]; 2],
    output: [&'a mut [f32]; 2],
    spare: [&'a mut [f32]; 2],
    gains: &'a mut [f32],
}

#[derive(Debug)]
struct Pedal {
    kinds: Box<[Kind]>,
    active: usize,
    on: bool,
    gain: Ramp,
    presence: Ramp,
    fading_from: Option<usize>,
    fade: Ramp,
    asleep: bool,
    quiet: usize,
    trail: bool,
}

impl Pedal {
    fn turn(&mut self, on: bool) {
        if on == self.on {
            return;
        }
        self.on = on;
        self.gain.aim(if on { 1.0 } else { 0.0 });
        if on {
            if !self.trail {
                self.kinds[self.active].effect.reset();
            }
            self.asleep = false;
            self.quiet = 0;
        } else if !self.trail && !self.gain.moving() && self.fading_from.is_none() {
            self.asleep = true;
        }
    }

    fn switch(&mut self, kind: usize) {
        if let Some(old) = self.fading_from.take() {
            self.kinds[old].effect.reset();
            self.fade.snap(1.0);
        }
        if kind == self.active {
            return;
        }
        if self.asleep {
            self.kinds[self.active].effect.reset();
        } else {
            self.fading_from = Some(self.active);
            self.fade.snap(0.0);
            self.fade.aim(1.0);
        }
        self.active = kind;
    }

    fn fault(&mut self, stats: &Stats) {
        self.kinds[self.active].effect.reset();
        if let Some(old) = self.fading_from {
            self.kinds[old].effect.reset();
        }
        stats.fx_faults.fetch_add(1, Ordering::Relaxed);
    }

    fn finish_sample(&mut self, wet: [f32; 2], gain: f32, second: usize) {
        if self.on || gain > 0.0 || self.fading_from.is_some() {
            self.quiet = 0;
            return;
        }
        if !self.trail {
            self.asleep = true;
        } else if wet.iter().all(|value| value.abs() < 1e-6) {
            self.quiet = self.quiet.saturating_add(1);
            if self.quiet >= second {
                self.asleep = true;
            }
        } else {
            self.quiet = 0;
        }
    }

    fn fading_sample(
        &mut self,
        context: Context,
        input: [f32; 2],
        stats: &Stats,
        second: usize,
    ) -> [f32; 2] {
        let g = self.gain.next();
        let fed = input.map(|x| if self.trail { g * x } else { x });
        let mut new_l = [0.0];
        let mut new_r = [0.0];
        self.kinds[self.active].effect.process(
            &context,
            [&[fed[0]], &[fed[1]]],
            [&mut new_l, &mut new_r],
        );
        let mut old_l = [0.0];
        let mut old_r = [0.0];
        if let Some(old) = self.fading_from {
            self.kinds[old].effect.process(
                &context,
                [&[fed[0]], &[fed[1]]],
                [&mut old_l, &mut old_r],
            );
        }
        if [new_l[0], new_r[0], old_l[0], old_r[0]]
            .iter()
            .any(|x| !x.is_finite())
        {
            self.fault(stats);
        }
        let f = self.fade.next();
        let wet = [0, 1].map(|side| {
            let new = if side == 0 { new_l[0] } else { new_r[0] };
            let old = if side == 0 { old_l[0] } else { old_r[0] };
            (1.0 - f).mul_add(sanitize_sample(old), f * sanitize_sample(new))
        });
        if !self.fade.moving()
            && let Some(old) = self.fading_from.take()
        {
            self.kinds[old].effect.reset();
        }
        self.finish_sample(wet, g, second);
        let p = self.presence.next();
        [0, 1].map(|side| {
            let slot = (1.0 - g).mul_add(
                input[side],
                if self.trail { wet[side] } else { g * wet[side] },
            );
            sanitize_sample((1.0 - p).mul_add(input[side], p * slot))
        })
    }

    fn split_process(
        &mut self,
        context: Context,
        buffers: Buffers<'_>,
        stats: &Stats,
        second: usize,
        split: usize,
    ) {
        let Buffers {
            input,
            output: [left, right],
            spare: [spare_l, spare_r],
            gains,
        } = buffers;
        let (left_before, left_after) = left.split_at_mut(split);
        let (right_before, right_after) = right.split_at_mut(split);
        let (spare_left_prefix, spare_left_suffix) = spare_l.split_at_mut(split);
        let (scratch_right_prefix, scratch_right_suffix) = spare_r.split_at_mut(split);
        let (gains_before, gains_after) = gains.split_at_mut(split);
        self.process(
            context,
            Buffers {
                input: [&input[0][..split], &input[1][..split]],
                output: [left_before, right_before],
                spare: [spare_left_prefix, scratch_right_prefix],
                gains: gains_before,
            },
            stats,
            second,
        );
        self.process(
            context,
            Buffers {
                input: [&input[0][split..], &input[1][split..]],
                output: [left_after, right_after],
                spare: [spare_left_suffix, scratch_right_suffix],
                gains: gains_after,
            },
            stats,
            second,
        );
    }

    fn process(&mut self, context: Context, buffers: Buffers<'_>, stats: &Stats, second: usize) {
        if !self.trail
            && !self.on
            && self.fading_from.is_none()
            && self.gain.remaining > 0
            && self.gain.remaining < buffers.input[0].len()
        {
            self.split_process(context, buffers, stats, second, self.gain.remaining);
            return;
        }
        let Buffers {
            input,
            output,
            spare,
            gains,
        } = buffers;
        let [left, right] = output;
        if self.asleep {
            left.copy_from_slice(input[0]);
            right.copy_from_slice(input[1]);
            for _ in 0..left.len() {
                self.presence.next();
            }
            return;
        }
        if self.fading_from.is_some() {
            for i in 0..left.len() {
                let wet = if self.fading_from.is_some() {
                    self.fading_sample(context, [input[0][i], input[1][i]], stats, second)
                } else {
                    // The block path also works with one frame after a fade ends.
                    let mut l = [0.0];
                    let mut r = [0.0];
                    let mut sl = [0.0];
                    let mut sr = [0.0];
                    self.process(
                        context,
                        Buffers {
                            input: [&input[0][i..=i], &input[1][i..=i]],
                            output: [&mut l, &mut r],
                            spare: [&mut sl, &mut sr],
                            gains: &mut gains[..1],
                        },
                        stats,
                        second,
                    );
                    [l[0], r[0]]
                };
                left[i] = wet[0];
                right[i] = wet[1];
            }
            return;
        }
        let [spare_l, spare_r] = spare;
        for (i, gain) in gains.iter_mut().enumerate() {
            *gain = self.gain.next();
            spare_l[i] = input[0][i] * *gain;
            spare_r[i] = input[1][i] * *gain;
        }
        let fed = if self.trail {
            [&*spare_l, &*spare_r]
        } else {
            input
        };
        self.kinds[self.active]
            .effect
            .process(&context, fed, [&mut *left, &mut *right]);
        if left.iter().chain(right.iter()).any(|x| !x.is_finite()) {
            self.fault(stats);
        }
        for i in 0..left.len() {
            let wet = [sanitize_sample(left[i]), sanitize_sample(right[i])];
            let g = gains[i];
            self.finish_sample(wet, g, second);
            let p = self.presence.next();
            for (side, to) in [&mut left[i], &mut right[i]].into_iter().enumerate() {
                let slot = if g == 0.0 && !self.trail {
                    input[side][i]
                } else {
                    (1.0 - g).mul_add(
                        input[side][i],
                        if self.trail { wet[side] } else { g * wet[side] },
                    )
                };
                *to = if p.to_bits() == 1.0_f32.to_bits() {
                    slot
                } else {
                    (1.0 - p).mul_add(input[side][i], p * slot)
                };
                *to = sanitize_sample(*to);
            }
        }
    }
}

#[derive(Debug)]
pub struct FxChain {
    pub order: [Slot; Slot::ALL.len()],
    pedals: [Pedal; Slot::ALL.len()],
    pending: Option<[Slot; Slot::ALL.len()]>,
    a: [Box<[f32]>; 2],
    b: [Box<[f32]>; 2],
    spare: [Box<[f32]>; 2],
    gains: Box<[f32]>,
    second: usize,
    input_meters: [super::meters::Rms; Slot::ALL.len()],
    output_meters: [super::meters::Rms; Slot::ALL.len()],
}

impl FxChain {
    pub fn latency(&self) -> usize {
        self.pedals
            .iter()
            .filter(|pedal| pedal.on || pedal.gain.moving())
            .map(|pedal| {
                let current = pedal.kinds[pedal.active].effect.latency();
                pedal.fading_from.map_or(current, |old| {
                    current.max(pedal.kinds[old].effect.latency())
                })
            })
            .sum()
    }
    pub fn new(rate: u32, block: usize, settings: &FxSettings) -> kazoo_core::Result<Self> {
        if rate == 0 || block == 0 {
            return Err(kazoo_core::Error::Config(
                "mouth effects need a positive rate and block".into(),
            ));
        }
        let ramp = (rate as f32 * RAMP_SECONDS).round() as usize;
        let mut pedals = Vec::with_capacity(Slot::ALL.len());
        for slot in Slot::ALL {
            let saved = settings.slots[slot as usize];
            let mut kinds = Vec::with_capacity(slot.ids().len());
            for (kind, id) in slot.ids().iter().enumerate() {
                let description = slot.kind(kind).ok_or_else(|| {
                    kazoo_core::Error::Config(format!("mouth effect {id} is missing"))
                })?;
                if description.params.len() > MAX_PARAMS {
                    return Err(kazoo_core::Error::Config(format!(
                        "mouth effect {} exceeds MAX_PARAMS",
                        description.id
                    )));
                }
                let mut effect: Box<dyn Effect> = if slot == Slot::Harmony {
                    Box::new(harmony::Harmony::new(block))
                } else {
                    (description.build)()
                };
                effect.prepare(rate as f32);
                effect.set_seed(slot.seed(kind));
                let mut values = saved.values[kind];
                for (index, spec) in description.params.iter().enumerate() {
                    values[index] = spec.clamp(if values[index].is_finite() {
                        values[index]
                    } else {
                        mouth_default(slot, kind, spec)
                    });
                    if slot == Slot::Harmony && matches!(index, 2 | 4 | 6) {
                        values[index] = values[index].round();
                    }
                    effect.set_param(index, values[index]);
                }
                effect.reset();
                kinds.push(Kind {
                    effect,
                    values,
                    description,
                    custom: saved.custom,
                });
            }
            if saved.active >= kinds.len() {
                return Err(kazoo_core::Error::Config(format!(
                    "mouth {} kind index is invalid",
                    slot.name()
                )));
            }
            pedals.push(Pedal {
                kinds: kinds.into_boxed_slice(),
                active: saved.active,
                on: saved.on,
                gain: Ramp::new(if saved.on { 1.0 } else { 0.0 }, ramp),
                presence: Ramp::new(1.0, ramp),
                fading_from: None,
                fade: Ramp::new(1.0, ramp),
                asleep: !saved.on,
                quiet: 0,
                trail: slot.trail(),
            });
        }
        let pedals = pedals
            .try_into()
            .map_err(|_| kazoo_core::Error::Config("mouth requires six slots".into()))?;
        if !permutation(settings.order) {
            return Err(kazoo_core::Error::Config(
                "mouth effect order is invalid".into(),
            ));
        }
        Ok(Self {
            order: settings.order,
            pedals,
            pending: None,
            a: std::array::from_fn(|_| vec![0.0; block].into_boxed_slice()),
            b: std::array::from_fn(|_| vec![0.0; block].into_boxed_slice()),
            spare: std::array::from_fn(|_| vec![0.0; block].into_boxed_slice()),
            gains: vec![0.0; block].into_boxed_slice(),
            second: rate as usize,
            input_meters: [super::meters::Rms::default(); Slot::ALL.len()],
            output_meters: [super::meters::Rms::default(); Slot::ALL.len()],
        })
    }

    pub fn on(&mut self, slot: Slot, on: bool) {
        self.pedals[slot as usize].turn(on);
    }

    pub fn kind(&mut self, slot: Slot, kind: u8, stats: &Stats) {
        let pedal = &mut self.pedals[slot as usize];
        if usize::from(kind) < pedal.kinds.len() {
            pedal.switch(usize::from(kind));
        } else {
            stats.fx_refused.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn param(&mut self, slot: Slot, kind: u8, index: u8, value: f32, stats: &Stats) {
        let pedal = &mut self.pedals[slot as usize];
        if let Some(kind) = pedal.kinds.get_mut(usize::from(kind))
            && let Some(spec) = kind.description.params.get(usize::from(index))
        {
            if value.is_finite() {
                let value = spec.clamp(value);
                if slot == Slot::Harmony {
                    let value = if matches!(index, 2 | 4 | 6) {
                        value.round()
                    } else {
                        value
                    };
                    harmony::adjust(
                        &mut kind.values,
                        &mut kind.custom,
                        usize::from(index),
                        value,
                    );
                    for (index, value) in kind
                        .values
                        .iter()
                        .take(harmony::KIND.params.len())
                        .enumerate()
                    {
                        kind.effect.set_param(index, *value);
                    }
                } else {
                    kind.values[usize::from(index)] = value;
                    kind.effect.set_param(usize::from(index), value);
                }
            }
        } else {
            stats.fx_refused.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn reorder(&mut self, order: [Slot; Slot::ALL.len()], stats: &Stats) {
        if !permutation(order) {
            stats.fx_refused.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if self.pending.is_none() && order == self.order {
            return;
        }
        self.pending = Some(order);
        for (position, slot) in self.order.iter().enumerate() {
            self.pedals[*slot as usize]
                .presence
                .aim(if order[position] == *slot { 1.0 } else { 0.0 });
        }
    }

    pub fn process(
        &mut self,
        context: Context,
        mono: &[f32],
        output: [&mut [f32]; 2],
        stats: &Stats,
    ) {
        let [left, right] = output;
        left.fill(0.0);
        right.fill(0.0);
        let count = mono.len().min(left.len()).min(right.len());
        let mut start = 0;
        while start < count {
            if let Some(order) = self.pending
                && self.pedals.iter().all(|pedal| !pedal.presence.moving())
            {
                self.order = order;
                self.pending = None;
                for pedal in &mut self.pedals {
                    pedal.presence.aim(1.0);
                }
            }
            let moving = self
                .pedals
                .iter()
                .map(|p| p.presence.remaining)
                .max()
                .unwrap_or(0);
            let size = (count - start).min(self.gains.len()).min(
                if self.pending.is_some() && moving > 0 {
                    moving
                } else {
                    usize::MAX
                },
            );
            for side in &mut self.a {
                for (to, from) in side[..size].iter_mut().zip(&mono[start..start + size]) {
                    *to = sanitize_sample(*from);
                }
            }
            for slot in self.order {
                let [a_l, a_r] = &self.a;
                let [b_l, b_r] = &mut self.b;
                let [s_l, s_r] = &mut self.spare;
                self.input_meters[slot as usize].stereo(
                    &a_l[..size],
                    &a_r[..size],
                    self.second / 10,
                    &stats.fx_input[slot as usize],
                );
                self.pedals[slot as usize].process(
                    context,
                    Buffers {
                        input: [&a_l[..size], &a_r[..size]],
                        output: [&mut b_l[..size], &mut b_r[..size]],
                        spare: [&mut s_l[..size], &mut s_r[..size]],
                        gains: &mut self.gains[..size],
                    },
                    stats,
                    self.second,
                );
                self.output_meters[slot as usize].stereo(
                    &b_l[..size],
                    &b_r[..size],
                    self.second / 10,
                    &stats.fx_output[slot as usize],
                );
                std::mem::swap(&mut self.a, &mut self.b);
            }
            for i in 0..size {
                let finite = mono[start + i].is_finite();
                left[start + i] = if finite { self.a[0][i] } else { 0.0 };
                right[start + i] = if finite { self.a[1][i] } else { 0.0 };
            }
            start += size;
        }
        if let Some(order) = self.pending
            && self.pedals.iter().all(|p| !p.presence.moving())
        {
            self.order = order;
            self.pending = None;
            for pedal in &mut self.pedals {
                pedal.presence.aim(1.0);
            }
        }
        let moving = self
            .pedals
            .iter()
            .enumerate()
            .fold(0, |bits, (index, pedal)| {
                bits | (u64::from(
                    pedal.gain.moving() || pedal.presence.moving() || pedal.fade.moving(),
                ) << index)
            });
        stats.fx_moving.store(moving, Ordering::Relaxed);
    }
}

pub fn ceiling(sample: f32) -> f32 {
    let sample = sanitize_sample(sample);
    if sample.abs() <= 0.8 {
        sample
    } else {
        // Single precision rounds tanh's asymptote to one; keep the bound strict.
        sample.signum()
            * 0.2_f32
                .mul_add(((sample.abs() - 0.8) / 0.2).tanh(), 0.8)
                .min(f32::from_bits(1.0_f32.to_bits() - 1))
    }
}
