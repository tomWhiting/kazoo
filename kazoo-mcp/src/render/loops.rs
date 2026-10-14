use kazoo_core::ipc::follow::beats_in;
use kazoo_wall::format;
use kazoo_wall::protocol::{LoopState, LoopView, ModuleView, Snapshot};

use super::{line, round};

#[cfg(test)]
mod tests;

fn runs(selected: impl Iterator<Item = bool>) -> String {
    let mut ranges = Vec::new();
    let mut start = None;
    let mut end = 0;
    for (index, selected) in selected.chain(std::iter::once(false)).enumerate() {
        if selected {
            start.get_or_insert(index + 1);
            end = index + 1;
        } else if let Some(first) = start.take() {
            ranges.push(if first == end {
                first.to_string()
            } else {
                format!("{first}–{end}")
            });
        }
    }
    if ranges.is_empty() {
        "none".into()
    } else {
        ranges.join(", ")
    }
}

fn length(view: &LoopView) -> String {
    let bars = view.beats / 4.0;
    if bars.fract().abs() < f64::EPSILON {
        format!(
            "{} {}",
            round(bars, 2),
            if (bars - 1.0).abs() < f64::EPSILON {
                "bar"
            } else {
                "bars"
            }
        )
    } else {
        format::beats(view.beats)
    }
}

fn state(view: &LoopView, beat: f64) -> String {
    match view.state {
        LoopState::Blank => "blank".into(),
        LoopState::Armed => format!("recording, armed for bar {}", (beat / 4.0).floor() + 2.0),
        LoopState::Rolling => "recording".into(),
        LoopState::PostRoll => "recording, finishing punch".into(),
        LoopState::Holding => "holding".into(),
        LoopState::NoTape => "no tape".into(),
        LoopState::Loading => "loading reel".into(),
    }
}

pub(super) fn describe(view: &LoopView, wear: f64, tempo: f64, beat: f64) -> String {
    let mut parts = vec![state(view, beat)];
    if view.beats.is_finite() && view.beats > 0.0 {
        let seconds = view.beats / beats_in(1.0, tempo, 1.0);
        let splice = (view.anchor / 4.0).floor() + 1.0;
        parts.push(format!(
            "{} ({:.1} s at {}, spliced at bar {splice})",
            length(view),
            round(seconds, 1),
            format::bpm(tempo)
        ));
        let bar = ((beat - view.anchor).rem_euclid(view.beats) / 4.0).floor() + 1.0;
        parts.push(format!(
            "bar {bar} of {}, pass {}",
            (view.beats / 4.0).ceil(),
            view.pass
        ));
        parts.push(format!(
            "written bars {}",
            runs(view.written.iter().copied())
        ));
    }
    if wear.is_finite() && wear > 0.0 {
        let mut ages: Vec<_> = view.ages.iter().copied().filter(|age| *age > 0).collect();
        ages.sort_unstable();
        ages.dedup();
        let descriptions: Vec<_> = ages
            .iter()
            .map(|age| {
                let bars = runs(view.ages.iter().map(|value| value == age));
                format!(
                    "bars {bars} are {age} {} old",
                    if *age == 1 { "pass" } else { "passes" }
                )
            })
            .collect();
        let detail = if descriptions.is_empty() {
            String::new()
        } else {
            format!(": {}", descriptions.join("; "))
        };
        parts.push(format!("wear {:.2}{detail}", round(wear, 2)));
    }
    if let Some(from) = &view.from {
        parts.push(format!("from {from}"));
    }
    match (view.take, &view.reel) {
        (Some(take), Some(reel)) => parts.push(format!("take {take} (reel {reel})")),
        (Some(take), None) => parts.push(format!("take {take}")),
        (None, Some(reel)) => parts.push(format!("reel {reel}")),
        (None, None) => {}
    }
    if view.shortened {
        parts.push("shortened to tape capacity".into());
    }
    let mut text = parts.join(" · ");
    for note in &view.notes {
        text.push_str(". ");
        text.push_str(note.trim_end_matches('.'));
    }
    if !view.notes.is_empty() {
        text.push('.');
    }
    text
}

pub(super) fn module(module: &ModuleView, tempo: f64, beat: f64) -> String {
    module.tape.as_ref().map_or_else(String::new, |view| {
        let wear = module
            .knobs
            .iter()
            .find(|knob| knob.name == "wear")
            .map_or(0.0, |knob| knob.value);
        format!(": {}", describe(view, wear, tempo, beat))
    })
}

pub(super) fn footer(out: &mut String, snapshot: &Snapshot) {
    if let Some(meet) = &snapshot.loops_meet {
        if let Some(bars) = meet.bars {
            if let Some(next) = meet.next_bar {
                let lengths: Vec<_> = snapshot
                    .modules
                    .iter()
                    .filter_map(|module| module.tape.as_ref())
                    .filter(|view| view.frames > 0)
                    .map(|view| round(view.beats / 4.0, 2).to_string())
                    .collect();
                let duration = meet.seconds.map_or_else(String::new, |seconds| {
                    let seconds = seconds.round() as u64;
                    format!(
                        " ({} min {} s at {})",
                        seconds / 60,
                        seconds % 60,
                        format::bpm(snapshot.tempo)
                    )
                });
                line(
                    out,
                    format_args!(
                        "Loops {} bars long meet again every {bars} bars{duration}; next at bar {next}.",
                        lengths.join(" and ")
                    ),
                );
            } else {
                line(
                    out,
                    format_args!("These loops never all meet at their seams."),
                );
            }
        } else {
            line(
                out,
                format_args!("These loops meet again practically never."),
            );
        }
    }
    if snapshot.checkpoints_paused {
        line(
            out,
            format_args!("Checkpoints are paused: the reel store is full."),
        );
    }
}
