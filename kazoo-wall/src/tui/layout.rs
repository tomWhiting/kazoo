//! Where the module panels hang: a grid that fills the width it is given,
//! with panels as tall as their knobs and jacks need, and a scroll that
//! keeps the selected knob in view.
//!
//! The grid is laid out on a virtual sheet as tall as the wall needs; the
//! screen shows a window onto it, `scroll` lines down.

use kazoo_wall::protocol::ModuleView;

/// Columns between panels (and rows between rows of panels).
pub const GAP: u16 = 1;

/// Narrowest panel.
pub const MIN_PANEL: u16 = 30;

/// Widest panel: wider panels only add empty cream.
pub const MAX_PANEL: u16 = 46;

/// Width of the `in ` / `out` label before a module's jacks.
pub const JACK_LABEL: u16 = 4;

/// A panel's place on the sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    /// Column, from the wall's left edge.
    pub x: u16,
    /// Line, from the top of the sheet.
    pub y: u32,
    /// Width, borders included.
    pub width: u16,
    /// Height, borders included.
    pub height: u16,
}

impl Placed {
    /// The sheet line of knob `knob`'s row.
    #[must_use]
    pub fn knob_line(&self, knob: usize) -> u32 {
        self.y + 1 + u32::try_from(knob).unwrap_or(u32::MAX - self.y - 1)
    }
}

/// The whole wall's grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grid {
    /// Panels across.
    pub columns: usize,
    /// One place per module, in the snapshot's order.
    pub panels: Vec<Placed>,
    /// Lines the sheet needs.
    pub height: u32,
}

/// Width of a jack's token: its glyph and its name.
fn token_width(name: &str) -> u16 {
    1 + u16::try_from(name.chars().count()).unwrap_or(u16::MAX - 1)
}

/// The jacks named `names`, broken into lines that fit a panel whose
/// inside is `inner` wide: each line holds the indices of its jacks. A
/// jack too long for a line gets a line of its own (and is truncated when
/// drawn).
#[must_use]
pub fn jack_lines(names: &[String], inner: u16) -> Vec<Vec<usize>> {
    let room = inner.saturating_sub(1 + JACK_LABEL).max(1);
    let mut lines: Vec<Vec<usize>> = Vec::new();
    let mut used = 0_u16;
    for (index, name) in names.iter().enumerate() {
        let width = token_width(name).min(room);
        match lines.last_mut() {
            Some(line) if used + 1 + width <= room => {
                line.push(index);
                used += 1 + width;
            }
            _ => {
                lines.push(vec![index]);
                used = width;
            }
        }
    }
    lines
}

/// Lines a module's panel needs at `width`: borders, a row per knob, and
/// its input and output jacks.
#[must_use]
pub fn panel_height(module: &ModuleView, width: u16) -> u16 {
    let inner = width.saturating_sub(2);
    let knobs = u16::try_from(module.knobs.len()).unwrap_or(u16::MAX / 2);
    let inputs = u16::try_from(jack_lines(&module.inputs, inner).len()).unwrap_or(0);
    let outputs = u16::try_from(jack_lines(&module.outputs, inner).len()).unwrap_or(0);
    2_u16
        .saturating_add(knobs)
        .saturating_add(inputs)
        .saturating_add(outputs)
}

/// Lay `modules` out in a wall `width` wide.
#[must_use]
pub fn grid(width: u16, modules: &[ModuleView]) -> Grid {
    let columns = usize::from(((width + GAP) / (MIN_PANEL + GAP)).max(1));
    let columns_u16 = u16::try_from(columns).unwrap_or(1);
    let panel_width = ((width.saturating_sub(GAP * (columns_u16 - 1))) / columns_u16)
        .clamp(1, MAX_PANEL)
        .min(width.max(1));
    let mut panels = Vec::with_capacity(modules.len());
    let mut y = 0_u32;
    for row in modules.chunks(columns) {
        let row_height = row
            .iter()
            .map(|module| panel_height(module, panel_width))
            .max()
            .unwrap_or(0);
        for (column, module) in row.iter().enumerate() {
            let column = u16::try_from(column).unwrap_or(0);
            panels.push(Placed {
                x: column * (panel_width + GAP),
                y,
                width: panel_width,
                height: panel_height(module, panel_width),
            });
        }
        y += u32::from(row_height) + u32::from(GAP);
    }
    Grid {
        columns,
        panels,
        height: y.saturating_sub(u32::from(GAP)),
    }
}

/// The scroll that keeps `panel` (or, when it is taller than the view,
/// its `line`) in a view `view` lines tall, moving as little as possible
/// from `current`.
#[must_use]
pub fn scroll_to(current: u32, view: u16, total: u32, panel: &Placed, line: u32) -> u32 {
    let view = u32::from(view.max(1));
    let (top, bottom) = if u32::from(panel.height) <= view {
        (panel.y, panel.y + u32::from(panel.height))
    } else {
        (line, line + 1)
    };
    let mut scroll = current;
    if top < scroll {
        scroll = top;
    }
    if bottom > scroll + view {
        scroll = bottom - view;
    }
    scroll.min(total.saturating_sub(view))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kazoo_wall::protocol::KnobView;

    fn module(knobs: usize, inputs: &[&str], outputs: &[&str]) -> ModuleView {
        ModuleView {
            id: "vcf1".to_string(),
            kind: "vcf".to_string(),
            name: None,
            knobs: (0..knobs)
                .map(|index| KnobView {
                    name: format!("k{index}"),
                    value: 0.0,
                    target: 0.0,
                    min: 0.0,
                    max: 1.0,
                    unit: String::new(),
                    stepped: false,
                    display: "0".to_string(),
                    target_display: "0".to_string(),
                })
                .collect(),
            inputs: inputs.iter().map(ToString::to_string).collect(),
            outputs: outputs.iter().map(ToString::to_string).collect(),
        }
    }

    #[test]
    fn jacks_wrap_to_the_panel() {
        let names: Vec<String> = ["pitch", "fm", "width", "sync", "reset"]
            .iter()
            .map(ToString::to_string)
            .collect();
        // Room for 30 - 2 - 1 - 4 = 23 columns of tokens.
        assert_eq!(jack_lines(&names, 28), vec![vec![0, 1, 2, 3], vec![4]]);
        assert!(jack_lines(&[], 28).is_empty());
        let long = vec!["an_extremely_long_input_name_indeed".to_string()];
        assert_eq!(jack_lines(&long, 28), vec![vec![0]]);
    }

    #[test]
    fn panels_fill_the_width_and_rows_take_the_tallest() {
        let modules = vec![
            module(5, &["pitch"], &["out"]),
            module(2, &[], &["out"]),
            module(19, &["clock", "reset"], &["pitch", "gate"]),
        ];
        let wide = grid(100, &modules);
        assert_eq!(wide.columns, 3);
        assert_eq!(wide.panels[0].width, 32);
        assert_eq!(wide.panels[1].x, 33);
        assert_eq!(wide.panels[0].height, 2 + 5 + 1 + 1);
        assert_eq!(wide.panels[1].height, 2 + 2 + 1);
        assert_eq!(wide.height, 2 + 19 + 1 + 1);

        let narrow = grid(62, &modules);
        assert_eq!(narrow.columns, 2);
        assert_eq!(narrow.panels[2].y, 10);
        assert_eq!(narrow.height, 10 + 23);

        // A wide terminal hangs more panels, not wider ones.
        let huge = grid(400, &modules);
        assert_eq!(huge.columns, 12);
        assert_eq!(huge.panels[0].width, 32);
        // One column never stretches past the widest panel.
        let single = grid(60, &modules);
        assert_eq!(single.columns, 1);
        assert_eq!(single.panels[0].width, MAX_PANEL);

        // A sliver still lays out, one column wide.
        let sliver = grid(10, &modules);
        assert_eq!(sliver.columns, 1);
        assert_eq!(sliver.panels[0].width, 10);
    }

    #[test]
    fn scrolling_keeps_the_panel_or_the_line_in_view() {
        let short = Placed {
            x: 0,
            y: 30,
            width: 30,
            height: 8,
        };
        assert_eq!(scroll_to(0, 20, 100, &short, 32), 18);
        assert_eq!(scroll_to(35, 20, 100, &short, 32), 30);
        assert_eq!(scroll_to(20, 20, 100, &short, 32), 20);
        let tall = Placed {
            x: 0,
            y: 0,
            width: 30,
            height: 40,
        };
        assert_eq!(scroll_to(0, 20, 40, &tall, 35), 16);
        assert_eq!(scroll_to(16, 20, 40, &tall, 2), 2);
        // Never past the end of the sheet.
        assert_eq!(scroll_to(90, 20, 40, &tall, 39), 20);
    }
}
