//! Event detection, settle allowances, and unshifted trajectory arithmetic.

use super::{SCORE_STEP, Score, Take};

fn count_f64(count: usize) -> f64 {
    u32::try_from(count).map_or(f64::NAN, f64::from)
}

fn cents(estimate: f32, truth: f32) -> f32 {
    (1200.0 * (f64::from(estimate) / f64::from(truth)).log2()) as f32
}

fn valid(pitch: Option<f32>) -> Option<f32> {
    pitch.filter(|hz| hz.is_finite() && *hz > 0.0)
}

#[derive(Clone, Copy)]
struct Event {
    index: usize,
    hz: f32,
    start: bool,
}

fn events(truth: &[Option<f32>]) -> Vec<Event> {
    truth
        .iter()
        .enumerate()
        .filter_map(|(index, &pitch)| {
            let hz = valid(pitch)?;
            let previous = index.checked_sub(1).and_then(|i| valid(truth[i]));
            let start = previous.is_none();
            (start || previous.is_some_and(|previous| cents(hz, previous).abs() >= 50.0))
                .then_some(Event { index, hz, start })
        })
        .collect()
}

fn period_steps(rate: u32, hz: f32, extra_ms: f64) -> usize {
    ((3.0 * f64::from(rate) / f64::from(hz) + f64::from(rate) * extra_ms / 1000.0)
        / count_f64(SCORE_STEP))
    .ceil() as usize
}

fn within(estimate: Option<f32>, truth: Option<f32>, tolerance: f32) -> bool {
    match (valid(estimate), valid(truth)) {
        (Some(estimate), Some(truth)) => cents(estimate, truth).abs() <= tolerance,
        _ => false,
    }
}

/// The sample an event happens at. Steps are read at their centres, so an
/// event lies between the previous centre and this one; the step's first
/// sample stands for it unless the take knows the onset to the sample.
fn event_sample(event: Event, onsets: &[usize]) -> usize {
    let start = event.index * SCORE_STEP;
    let known = onsets
        .iter()
        .copied()
        .find(|&onset| onset + SCORE_STEP / 2 > start && onset <= start + SCORE_STEP / 2);
    match known {
        Some(onset) if event.start => onset,
        _ => start,
    }
}

/// Time from the event to the reading at which the tracker first lands and
/// stays. A reading is taken after the tracker has had all of its step, so
/// the figure counts every sample the tracker took and is never shorter than
/// the tracker's real latency (it may be up to one step longer).
fn latency(event: Event, end: usize, take: &Take, estimates: &[Option<f32>]) -> Option<f32> {
    let hold = period_steps(take.rate, event.hz, 0.0);
    let hold = if event.start {
        hold.max(3)
    } else {
        hold.max(1)
    };
    let tolerance = if event.start { 50.0 } else { 20.0 };
    let from = event_sample(event, &take.onsets);
    let mut run = 0;
    let readings = estimates[event.index..end]
        .iter()
        .zip(&take.truth[event.index..end]);
    for (index, (estimate, truth)) in (event.index..).zip(readings) {
        if within(*estimate, *truth, tolerance) {
            run += 1;
            if run >= hold {
                let landed = (index + 2 - hold) * SCORE_STEP;
                return Some(milliseconds(landed.saturating_sub(from), take.rate));
            }
        } else {
            run = 0;
        }
    }
    None
}

fn milliseconds(samples: usize, rate: u32) -> f32 {
    (count_f64(samples) * 1000.0 / f64::from(rate)) as f32
}

fn percent(numerator: usize, denominator: usize) -> f32 {
    if denominator == 0 {
        0.0
    } else {
        (100.0 * count_f64(numerator) / count_f64(denominator)) as f32
    }
}

fn median(values: &[f32]) -> f32 {
    if values.is_empty() {
        return f32::NAN;
    }
    let middle = values.len() / 2;
    if values.len() % 2 == 0 {
        values[middle - 1].midpoint(values[middle])
    } else {
        values[middle]
    }
}

fn maximum(values: &[f32]) -> f32 {
    values.last().copied().unwrap_or(f32::NAN)
}

/// The lag (ms) at which the tracker, shifted earlier, best agrees with the
/// reference over the reference's voiced steps: most steps within 50 cents,
/// less every step it misses or gets wrong. A step whose shifted reading
/// falls past the take's end cannot be judged and is left out, so a note
/// running to the end does not pull the lag either way; ties go to the
/// smaller lag, so a tracker that holds its last note through a release is
/// aligned by where its notes start. With no agreement at any lag, the lag is
/// NaN. The lag is read on the same terms as `latency`: a shift of `lag` steps
/// means the reading after `lag + 1` steps of samples.
fn aligned_lag(truth: &[Option<f32>], estimates: &[Option<f32>], rate: u32) -> f32 {
    let max_lag = (f64::from(rate) * 0.15 / count_f64(SCORE_STEP)).floor() as usize;
    let mut best: Option<(i64, usize)> = None;
    for lag in 0..=max_lag.min(estimates.len()) {
        let mut matches = 0_i64;
        let mut misses = 0_i64;
        for (index, reference) in truth.iter().enumerate() {
            if valid(*reference).is_none() {
                continue;
            }
            match estimates.get(index + lag) {
                Some(estimate) if within(*estimate, *reference, 50.0) => matches += 1,
                Some(_) => misses += 1,
                None => {}
            }
        }
        let agreement = matches - misses;
        if matches > 0 && best.is_none_or(|(most, _)| agreement > most) {
            best = Some((agreement, lag));
        }
    }
    best.map_or(f32::NAN, |(_, lag)| {
        milliseconds((lag + 1) * SCORE_STEP, rate)
    })
}

pub(super) fn measure(take: &Take, estimates: &[Option<f32>]) -> Score {
    let length = take.truth.len().min(estimates.len());
    let truth = &take.truth[..length];
    let estimates = &estimates[..length];
    let events = events(truth);
    let mut settled = vec![true; length];
    let mut onset_latencies = Vec::new();
    let mut step_latencies = Vec::new();
    let mut onsets_missed = 0;
    let mut steps_missed = 0;
    for (position, &event) in events.iter().enumerate() {
        let settle_end = event
            .index
            .saturating_add(period_steps(take.rate, event.hz, 2.0))
            .min(length);
        settled[event.index..settle_end].fill(false);
        let next_event = events.get(position + 1).map_or(length, |next| next.index);
        let note_end = (event.index..next_event)
            .find(|&index| valid(truth[index]).is_none())
            .unwrap_or(next_event);
        match latency(event, note_end, take, estimates) {
            Some(delay) if event.start => onset_latencies.push(delay),
            Some(delay) => step_latencies.push(delay),
            None if event.start => onsets_missed += 1,
            None => steps_missed += 1,
        }
    }
    let mut errors = Vec::new();
    let mut scored = 0;
    let mut octave = 0;
    let mut gross = 0;
    let mut missed = 0;
    let mut unvoiced = 0;
    let mut false_voiced = 0;
    let release_steps = (f64::from(take.rate) * 0.03 / count_f64(SCORE_STEP)).ceil() as usize;
    let mut release_end = 0;
    for index in 0..length {
        if let Some(hz) = valid(truth[index]) {
            if !settled[index] {
                continue;
            }
            scored += 1;
            if let Some(estimate) = valid(estimates[index]) {
                let error = cents(estimate, hz);
                let octaves = (error / 1200.0).round();
                if octaves != 0.0 && 1200.0_f32.mul_add(-octaves, error).abs() <= 100.0 {
                    octave += 1;
                } else if error.abs() > 100.0 {
                    gross += 1;
                } else {
                    errors.push(error.abs());
                }
            } else {
                missed += 1;
            }
        } else {
            if index > 0 && valid(truth[index - 1]).is_some() {
                release_end = index.saturating_add(release_steps);
            }
            if index >= release_end {
                unvoiced += 1;
                false_voiced += usize::from(valid(estimates[index]).is_some());
            }
        }
    }
    errors.sort_by(f32::total_cmp);
    onset_latencies.sort_by(f32::total_cmp);
    step_latencies.sort_by(f32::total_cmp);
    let p95 = errors
        .len()
        .saturating_mul(95)
        .div_ceil(100)
        .saturating_sub(1);
    Score {
        onset_ms_median: median(&onset_latencies),
        onset_ms_max: maximum(&onset_latencies),
        onsets_missed,
        step_ms_median: median(&step_latencies),
        step_ms_max: maximum(&step_latencies),
        steps: events.iter().filter(|event| !event.start).count(),
        steps_missed,
        cents_median: median(&errors),
        cents_p95: errors.get(p95).copied().unwrap_or(f32::NAN),
        octave_pct: percent(octave, scored),
        gross_pct: percent(gross, scored),
        missed_pct: percent(missed, scored),
        false_pct: percent(false_voiced, unvoiced),
        aligned_lag_ms: (!take.exact).then(|| aligned_lag(truth, estimates, take.rate)),
    }
}
