use kazoo_fx::{Curve, ParamSpec};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{BRIGHT, DIM};
use crate::controls::{App, Focus};
use crate::engine::fx::{MAX_PARAMS, Slot};

pub(crate) fn draw(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let mut chips = Vec::with_capacity(6);
    for (index, slot) in app.accepted.fx.order.into_iter().enumerate() {
        let saved = app.accepted.fx.slots[slot as usize];
        let symbol = if app.moving & (1 << slot as usize) != 0 {
            "◐"
        } else if saved.on {
            "●"
        } else {
            "○"
        };
        let name = chip_name(slot, saved.active, &saved.values[saved.active]);
        let times = if slot == Slot::Delay {
            delay_times(slot, saved.active, &saved.values[saved.active])
        } else {
            String::new()
        };
        let style = if slot == app.selected_slot && app.focus == Focus::Fx {
            Style::default().fg(BRIGHT).add_modifier(Modifier::REVERSED)
        } else {
            Style::default().fg(BRIGHT)
        };
        chips.push(Span::styled(
            format!(" {} {name} {symbol}{times}", index + 1),
            style,
        ));
    }
    let (bpm, source) = app
        .desk_bpm
        .map_or((app.accepted.own_bpm, "own"), |bpm| (bpm, "desk"));
    chips.push(Span::styled(
        format!(" ♩{bpm:.1} {source}"),
        Style::default().fg(DIM),
    ));
    let width: usize = chips.iter().map(Span::width).sum();
    if width > usize::from(area.width) {
        for chip in chips.iter_mut().take(5) {
            chip.content = chip
                .content
                .replace(" ●", "●")
                .replace(" ○", "○")
                .replace(" ◐", "◐")
                .into();
        }
    }
    frame.render_widget(
        Paragraph::new(Line::from(chips)),
        Rect::new(area.x, area.y, area.width, 1),
    );
    let slot = app.selected_slot;
    let saved = app.accepted.fx.slots[slot as usize];
    let Some(description) = slot.kind(saved.active) else {
        return;
    };
    let cells = std::iter::once(format!("kind  {} ◂▸", description.id)).chain(
        description
            .params
            .iter()
            .enumerate()
            .map(|(index, spec)| cell(spec, saved.values[saved.active][index])),
    );
    for (index, text) in cells.enumerate() {
        let column = (index / 5) as u16;
        let x = area.x + column * 26;
        let y = area.y + 1 + (index % 5) as u16;
        if x >= area.right() || y >= area.bottom() {
            continue;
        }
        let style = if index == app.selected_cell && app.focus == Focus::Fx {
            Style::default().fg(BRIGHT).add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        frame.render_widget(
            Paragraph::new(text).style(style),
            Rect::new(x, y, 26.min(area.right() - x), 1),
        );
    }
}

fn chip_name(slot: Slot, kind: usize, values: &[f32; MAX_PARAMS]) -> &'static str {
    if slot == Slot::Megaphone {
        let model = slot
            .kind(kind)
            .and_then(|description| {
                description
                    .params
                    .iter()
                    .position(|spec| spec.name == "model")
            })
            .map_or(3, |index| values[index] as usize);
        return match model {
            0 => "am radio",
            1 => "telephone",
            2 => "walkie-talkie",
            _ => "megaphone",
        };
    }
    match slot {
        Slot::Delay => "delay",
        Slot::Echo => "echo",
        Slot::Distortion | Slot::Reverb | Slot::Megaphone => {
            slot.ids().get(kind).copied().unwrap_or(slot.name())
        }
    }
}

fn delay_times(slot: Slot, kind: usize, values: &[f32; MAX_PARAMS]) -> String {
    let Some(description) = slot.kind(kind) else {
        return String::new();
    };
    let time = |sync: &str, free: &str| {
        if let Some(index) = description.params.iter().position(|spec| spec.name == sync) {
            let spec = &description.params[index];
            if values[index] > 0.0
                && let Curve::Stepped { labels } = spec.curve
            {
                return labels
                    .get((values[index] - spec.min) as usize)
                    .copied()
                    .unwrap_or("free")
                    .to_owned();
            }
        }
        description
            .params
            .iter()
            .position(|spec| spec.name == free)
            .map_or_else(String::new, |index| {
                format!("{:.0}ms", values[index] * 1000.0)
            })
    };
    if kind == 0 {
        format!(" {}·{}", time("lsync", "ltime"), time("rsync", "rtime"))
    } else {
        format!(" {}", time("sync", "time"))
    }
}

fn cell(spec: &ParamSpec, value: f32) -> String {
    if let Curve::Stepped { labels } = spec.curve {
        let label = labels
            .get((spec.clamp(value) - spec.min) as usize)
            .copied()
            .unwrap_or("?");
        return format!("{}  {label}", spec.name);
    }
    let travel = match spec.curve {
        Curve::Log => (value / spec.min).ln() / (spec.max / spec.min).ln(),
        Curve::Linear => (value - spec.min) / (spec.max - spec.min),
        Curve::Stepped { .. } => 0.0,
    }
    .clamp(0.0, 1.0);
    let value = value_label(spec, value);
    let bar = 26_usize
        .saturating_sub(spec.name.chars().count() + value.chars().count() + 4)
        .min(8);
    let filled = (travel * bar as f32).round() as usize;
    format!(
        "{} {} ▕{}{}▏",
        spec.name,
        value,
        "█".repeat(filled),
        "░".repeat(bar - filled)
    )
}

fn value_label(spec: &ParamSpec, value: f32) -> String {
    match spec.unit {
        "Hz" if value >= 1000.0 => format!("{:.2} kHz", value / 1000.0),
        "Hz" => format!("{value:.0} Hz"),
        "s" if value < 1.0 => format!("{:.0} ms", value * 1000.0),
        "s" => format!("{value:.2} s"),
        "dB" => format!("{value:.1} dB"),
        "%" => format!("{value:.0}%"),
        "" => format!("{value:.2}"),
        unit => format!("{value:.2} {unit}"),
    }
}
