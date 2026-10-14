use kazoo_fx::Curve;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::{BRIGHT, DIM, Display};
use crate::controls::{
    App, Control, Focus, SHAPES,
    panels::{Hit, Target},
};
use crate::engine::fx::{Slot, harmony};

pub fn draw(frame: &mut Frame<'_>, area: Rect, app: &App, display: &Display) {
    app.hits.borrow_mut().clear();
    let columns = if area.width < 60 {
        1
    } else if area.width < 100 || frame.area().height < 30 {
        2
    } else {
        4
    };
    let rows = 4_u16.div_ceil(columns);
    let panel_height = area.height / rows;
    let active_focus = if app.focus == Focus::Fx {
        Focus::Fx
    } else {
        app.selected.panel()
    };
    let active = Focus::ALL
        .iter()
        .position(|focus| *focus == active_focus)
        .unwrap_or(0) as u16;
    let single_visible = panel_height < 5 || columns == 1;
    for (index, focus) in Focus::ALL.into_iter().enumerate() {
        let index = index as u16;
        if single_visible && index != active {
            continue;
        }
        let rect = if single_visible {
            area
        } else {
            let column = index % columns;
            let row = index / columns;
            let x = area.x + area.width * column / columns;
            let right = area.x + area.width * (column + 1) / columns;
            let y = area.y + area.height * row / rows;
            let bottom = area.y + area.height * (row + 1) / rows;
            Rect::new(x, y, right - x, bottom - y)
        };
        let title = if focus == Focus::Gate {
            format!(
                "GATE {}",
                if display.latest.is_some_and(|voice| voice.gate) {
                    "●"
                } else {
                    "○"
                }
            )
        } else {
            focus.title().to_owned()
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(if focus == active_focus { BRIGHT } else { DIM }));
        let inner = block.inner(rect);
        frame.render_widget(block, rect);
        if focus == Focus::Fx {
            effects(frame, inner, app);
        } else {
            controls(frame, inner, app, focus);
        }
    }
}

fn selected_style(selected: bool) -> Style {
    if selected {
        Style::default()
            .fg(Color::Black)
            .bg(BRIGHT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    }
}

fn controls(frame: &mut Frame<'_>, area: Rect, app: &App, focus: Focus) {
    let controls = focus.controls();
    let selected = controls
        .iter()
        .position(|control| *control == app.selected)
        .unwrap_or(0);
    let selected_row = selected + usize::from(focus == Focus::Synth && selected > 0);
    let offset = selected_row.saturating_sub(usize::from(area.height.saturating_sub(1)));
    let mut row = 0_usize;
    for control in controls {
        if *control == Control::Shape {
            if row >= offset && row - offset < usize::from(area.height) {
                let rect = Rect::new(area.x, area.y + (row - offset) as u16, area.width, 1);
                frame.render_widget(
                    Paragraph::new("shape / waveform").style(selected_style(
                        app.selected == *control && app.focus == focus,
                    )),
                    rect,
                );
                app.hits.borrow_mut().push(Hit {
                    area: rect,
                    bar: Rect::default(),
                    target: Target::Control(*control),
                });
            }
            row += 1;
            if row >= offset && row - offset < usize::from(area.height) {
                shapes(
                    frame,
                    Rect::new(area.x, area.y + (row - offset) as u16, area.width, 1),
                    app,
                );
            }
        } else if row >= offset && row - offset < usize::from(area.height) {
            let rect = Rect::new(area.x, area.y + (row - offset) as u16, area.width, 1);
            let style = selected_style(app.selected == *control && app.focus == focus);
            if let Some((min, max, value)) = app.accepted.dial(*control) {
                let bar = dial(
                    frame,
                    rect,
                    control.name(),
                    &app.accepted.value(*control),
                    (value - min) / (max - min),
                    style,
                );
                app.hits.borrow_mut().push(Hit {
                    area: rect,
                    bar,
                    target: Target::Control(*control),
                });
            } else {
                frame.render_widget(
                    Paragraph::new(format!(
                        "{} [{}]",
                        control.name(),
                        app.accepted.value(*control)
                    ))
                    .style(style),
                    rect,
                );
                app.hits.borrow_mut().push(Hit {
                    area: rect,
                    bar: Rect::default(),
                    target: Target::Control(*control),
                });
            }
        }
        row += 1;
    }
}

fn shapes(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let mut x = area.x;
    for (index, shape) in SHAPES.into_iter().enumerate() {
        let name = crate::controls::settings::shape_name(shape);
        let width = (name.len() as u16 + 1).min(area.right().saturating_sub(x));
        if width == 0 {
            break;
        }
        let rect = Rect::new(x, area.y, width, 1);
        frame.render_widget(
            Paragraph::new(format!("{name} "))
                .style(selected_style(shape == app.accepted.synth.shape)),
            rect,
        );
        app.hits.borrow_mut().push(Hit {
            area: rect,
            bar: Rect::default(),
            target: Target::Shape(index),
        });
        x += width;
    }
}

fn dial(
    frame: &mut Frame<'_>,
    area: Rect,
    name: &str,
    value: &str,
    fraction: f32,
    style: Style,
) -> Rect {
    let name_width = name.chars().count() as u16;
    let value_width = value.chars().count() as u16;
    let width = area
        .width
        .saturating_sub(name_width + value_width + 5)
        .min(8);
    let filled = (fraction.clamp(0.0, 1.0) * f32::from(width)).round() as usize;
    let text = format!(
        "{name} ▕{}{}▏ {value}",
        "█".repeat(filled),
        "░".repeat(usize::from(width) - filled)
    );
    frame.render_widget(Paragraph::new(text).style(style), area);
    Rect::new(area.x + (name_width + 2).min(area.width), area.y, width, 1)
}

pub fn activity(input: f32, output: f32) -> &'static str {
    let delta = (output - input).abs();
    if !delta.is_finite() || delta < 0.000_01 {
        "▁"
    } else if delta < 0.01 {
        "▃"
    } else if delta < 0.1 {
        "▅"
    } else {
        "▇"
    }
}

fn effects_area(frame: &mut Frame<'_>, area: Rect, focused: bool) -> Rect {
    if area.height == 0 {
        return area;
    }
    let content_height = area.height.saturating_sub(u16::from(focused));
    if focused {
        let hints = [
            "[ ] effect · ↑↓ row · ←→ change · b on/off · { } move · d default",
            "[ ] effect · ←→ change · b on/off",
            "[ ] effect",
        ];
        let hint = hints
            .into_iter()
            .find(|hint| hint.chars().count() <= usize::from(area.width))
            .unwrap_or("");
        frame.render_widget(
            Paragraph::new(hint).style(Style::default().fg(DIM)),
            Rect::new(area.x, area.y + content_height, area.width, 1),
        );
    }
    Rect::new(area.x, area.y, area.width, content_height)
}

/// A knob's value as its row reads it. A harmony level at the bottom is
/// off, and a harmony shift moves in whole semitones, so the rows say so.
fn value_label(slot: Slot, index: usize, spec: &kazoo_fx::ParamSpec, value: f32) -> String {
    match (slot, index, spec.curve) {
        (Slot::Harmony, 1 | 3 | 5 | 7, _) if value <= -60.0 => "off".to_owned(),
        (Slot::Harmony, 2 | 4 | 6, _) => format!("{value:+.0} st"),
        (_, _, Curve::Stepped { labels }) => labels
            .get((value - spec.min) as usize)
            .copied()
            .unwrap_or("?")
            .to_owned(),
        _ => format!("{value:.2} {}", spec.unit),
    }
}

pub(super) fn effects(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let area = effects_area(frame, area, app.focus == Focus::Fx);
    let saved = app.accepted.fx.slots[app.selected_slot as usize];
    let Some(kind) = app.selected_slot.kind(saved.active) else {
        return;
    };
    let focus_row = if app.focus == Focus::Fx {
        Slot::ALL.len() + app.selected_cell
    } else {
        0
    };
    let offset = focus_row.saturating_sub(usize::from(area.height.saturating_sub(1)));
    for (index, slot) in app.accepted.fx.order.into_iter().enumerate() {
        if index < offset || index - offset >= usize::from(area.height) {
            continue;
        }
        let saved = app.accepted.fx.slots[slot as usize];
        let rect = Rect::new(area.x, area.y + (index - offset) as u16, area.width, 1);
        let mix = slot
            .kind(saved.active)
            .and_then(|kind| {
                kind.params
                    .iter()
                    .enumerate()
                    .find(|(_, spec)| spec.name == "mix")
            })
            .map_or(1.0, |(index, spec)| {
                (saved.values[saved.active][index] - spec.min) / (spec.max - spec.min)
            });
        let meter = activity(app.fx_input[slot as usize], app.fx_output[slot as usize]);
        let light = if saved.on { "●" } else { "○" };
        frame.render_widget(
            Paragraph::new(if slot == Slot::Harmony {
                let preset = harmony::PRESETS[saved.values[0][0] as usize];
                format!("[{light}] harmony {preset} {meter}")
            } else {
                format!("[{light}] {} {meter} mix {mix:.2}", slot.name())
            })
            .style(selected_style(
                slot == app.selected_slot && app.focus == Focus::Fx,
            )),
            rect,
        );
        app.hits.borrow_mut().push(Hit {
            area: Rect::new(rect.x, rect.y, rect.width.min(3), 1),
            bar: Rect::default(),
            target: Target::FxOn(slot),
        });
        app.hits.borrow_mut().push(Hit {
            area: Rect::new(
                rect.x + rect.width.min(3),
                rect.y,
                rect.width.saturating_sub(3),
                1,
            ),
            bar: Rect::default(),
            target: Target::FxSlot(slot),
        });
    }
    let cells = (app.selected_slot != Slot::Harmony)
        .then(|| ("kind", kind.id.to_owned(), 0.0))
        .into_iter()
        .chain(kind.params.iter().enumerate().map(|(index, spec)| {
            let value = saved.values[saved.active][index];
            let fraction = match spec.curve {
                Curve::Log => (value / spec.min).log(spec.max / spec.min),
                _ => (value - spec.min) / (spec.max - spec.min),
            };
            (
                spec.name,
                value_label(app.selected_slot, index, spec, value),
                fraction,
            )
        }));
    for (cell, (name, value, fraction)) in cells.enumerate() {
        let row = Slot::ALL.len() + cell;
        if row < offset || row - offset >= usize::from(area.height) {
            continue;
        }
        let rect = Rect::new(area.x, area.y + (row - offset) as u16, area.width, 1);
        let style = selected_style(cell == app.selected_cell && app.focus == Focus::Fx);
        let bar = if cell == 0 {
            frame.render_widget(
                Paragraph::new(format!("{name} [{value}]")).style(style),
                rect,
            );
            Rect::default()
        } else {
            dial(frame, rect, name, &value, fraction, style)
        };
        app.hits.borrow_mut().push(Hit {
            area: rect,
            bar,
            target: Target::FxCell(cell),
        });
    }
}

pub fn help_lines(app: &App) -> Vec<String> {
    let lines = [
        "SOURCE",
        "w: next waveform",
        "s: silent switch",
        "PITCH",
        "Space: toggle focused switch",
        "v: keep vibrato",
        "GATE",
        "Tab: reach gate panel",
        "Left/Right: turn gate threshold or hold",
        "FX",
        "1..6: select effect slot",
        "b: toggle selected effect",
        "{ / }: move effect in chain",
        "[ / ]: previous / next effect",
        "LOOP",
        "r: record or overdub",
        "u: undo layer",
        "C: clear loop",
        "l: loop length",
        "o: play or stop",
        "Shift+w: save loop",
        "- / =: loop volume",
        "_ / +: big loop volume step",
        "KEYS",
        "Shift+o: voice+synth / voice / synth scope",
        "k: hide or show mic spectrum",
        "L: latency breakdown",
        "Tab / Shift+Tab: next / previous panel",
        "Up/Down: focus control",
        "Left/Right: turn; Shift: big step",
        "d: restore control default",
        ", / .: tempo; < / >: big tempo step",
        "/: filter help; Esc: clear or close",
        "Ctrl-C: quit",
        "Enter on a control: what it does",
    ];
    let filter = app.help_filter.as_deref().unwrap_or("").to_lowercase();
    lines
        .into_iter()
        .filter(|line| filter.is_empty() || line.to_lowercase().contains(&filter))
        .map(str::to_owned)
        .collect()
}

pub fn help(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let overlay = area;
    frame.render_widget(ratatui::widgets::Clear, overlay);
    let lines = help_lines(app);
    let maximum = lines
        .len()
        .saturating_sub(usize::from(overlay.height.saturating_sub(2))) as u16;
    let scroll = app.help_scroll.get().min(maximum);
    app.help_scroll.set(scroll);
    frame.render_widget(
        Paragraph::new(lines.into_iter().map(Line::from).collect::<Vec<_>>())
            .scroll((scroll, 0))
            .block(Block::default().borders(Borders::ALL).title(format!(
                "help /{}",
                app.help_filter.as_deref().unwrap_or("")
            )))
            .style(Style::default().fg(Color::White).bg(Color::Black)),
        overlay,
    );
}

pub fn meters(frame: &mut Frame<'_>, area: Rect, app: &App) {
    for (index, (name, rms)) in [("mic", app.mic_rms), ("output", app.output_rms)]
        .into_iter()
        .enumerate()
    {
        if index as u16 >= area.height {
            break;
        }
        let db = if rms.is_finite() && rms > 0.0 {
            (20.0 * rms.log10()).clamp(-80.0, 0.0)
        } else {
            -80.0
        };
        let width = area.width.saturating_sub(16).min(24);
        let level = ((db + 80.0) / 80.0 * f32::from(width)).round() as usize;
        let mut bar = vec!["░"; usize::from(width)];
        for cell in bar.iter_mut().take(level) {
            *cell = "█";
        }
        if index == 0 && width > 0 {
            for threshold in [app.accepted.gate.open_db, app.accepted.gate.close_db] {
                let position = ((threshold + 80.0) / 80.0 * f32::from(width - 1)).round() as usize;
                bar[position.min(usize::from(width - 1))] = "│";
            }
        }
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(format!("{name} ")),
                Span::styled(bar.concat(), Style::default().fg(BRIGHT)),
                Span::raw(format!(" {db:.0} dBFS")),
            ])),
            Rect::new(area.x, area.y + index as u16, area.width, 1),
        );
    }
}
