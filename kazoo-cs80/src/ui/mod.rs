//! TUI rendering for the CS-80 pad synth.
//!
//! Layout (top to bottom):
//! ```text
//! +======================== YAMAHA CS-80 ========================+
//! | Oct: 0  |  Drift: 6.0c  |  LAYER I > Waveform: Saw         |
//! | [Layer I] [Layer II] [Ring] [LFO] [Mix]                     |
//! +--------- LAYER I ----------+--------- LAYER II ---------+---+
//! |  VCO                       |  VCO                        | R |
//! |    ...                     |    ...                       | . |
//! +----VOICES--1~C4+2.1c--2~E4-1.3c--3.....--4.....--------+---+
//! |  [waveform] | [spectrum]                                     |
//! | Piano: z x c v b n m   sharps: s d g h j   Shift+arrows     |
//! +--------------------------------------------------------------+
//! ```

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::{App, Section, ViewMode};

// ---------------------------------------------------------------------------
// CS-80 color palette — warm, analog, distinct from the 808's cyan
// ---------------------------------------------------------------------------

/// Primary accent: warm amber/orange.
const ACCENT: Color = Color::Rgb(255, 160, 40);
/// Secondary accent: soft gold for sub-headers.
const GOLD: Color = Color::Rgb(200, 170, 90);
/// Active voice indicator.
const VOICE_ACTIVE: Color = Color::Rgb(255, 100, 60);
/// Releasing voice indicator (dimmed).
const VOICE_RELEASING: Color = Color::Rgb(150, 80, 40);
/// Dimmed/inactive text.
const DIM: Color = Color::Rgb(80, 80, 80);
/// Selected param background.
const SEL_BG: Color = Color::Rgb(50, 35, 20);
/// Group header color.
const GROUP: Color = Color::Rgb(140, 120, 80);
/// Waveform display color.
const WAVE_COLOR: Color = Color::Rgb(255, 120, 40);
/// Spectrum display color.
const SPECTRUM_COLOR: Color = Color::Rgb(200, 100, 255);
/// Border color for inactive panels.
const BORDER_DIM: Color = Color::Rgb(60, 50, 40);
/// Border color for active panels.
const BORDER_ACTIVE: Color = Color::Rgb(200, 140, 40);
/// Hint text color for parameter descriptions.
const HINT: Color = Color::Rgb(120, 120, 90);

/// Draw the full CS-80 TUI.
#[allow(clippy::too_many_lines)]
pub fn draw(f: &mut Frame, app: &App) {
    match app.view_mode {
        ViewMode::Synth => draw_synth_view(f, app),
        ViewMode::Modular => draw_modular_view(f, app),
    }
}

/// Draw the main synth editing view.
fn draw_synth_view(f: &mut Frame, app: &App) {
    let size = f.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Length(1), // Section bar (issue 13)
            Constraint::Min(14),   // Main editor (layers + shared)
            Constraint::Length(3), // Voice monitor
            Constraint::Length(4), // Waveform + spectrum
            Constraint::Length(3), // Keyboard help + status
        ])
        .split(size);

    draw_header(f, chunks[0], app);
    draw_section_bar(f, chunks[1], app);
    draw_main_editor(f, chunks[2], app);
    draw_voice_monitor(f, chunks[3], app);
    draw_waveform_and_spectrum(f, chunks[4], app);
    draw_footer(f, chunks[5], app);
}

/// Draw the modular node graph view (issue 8).
fn draw_modular_view(f: &mut Frame, app: &App) {
    let size = f.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(10),   // Node graph
            Constraint::Length(2), // Help
        ])
        .split(size);

    // Header
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(BORDER_ACTIVE))
        .title(Span::styled(
            " MODULAR NODE GRAPH ",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    let header_line = Line::from(vec![
        Span::styled(
            " Generative/modular synthesis engine ",
            Style::new().fg(GOLD),
        ),
        Span::styled("  F2:back to synth", Style::new().fg(DIM)),
    ]);
    let header = Paragraph::new(header_line).block(block);
    f.render_widget(header, chunks[0]);

    // Node list and connections
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(BORDER_DIM))
        .title(Span::styled(" NODES ", Style::new().fg(GOLD)));

    let inner = block.inner(chunks[1]);
    f.render_widget(block, chunks[1]);

    let node_list = app.modular_graph.node_list();
    let connections = app.modular_graph.connections();

    let mut lines: Vec<Line> = Vec::new();

    if node_list.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No nodes. This is the modular patching engine.",
            Style::new().fg(DIM),
        )));
        lines.push(Line::from(Span::styled(
            "  Available modules: VCO, VCF, VCA, ADSR, LFO, Ring Mod, Noise, Mixer",
            Style::new().fg(DIM),
        )));
        lines.push(Line::from(Span::styled(
            "  Nodes can be patched freely with type-checked audio/control/trigger ports.",
            Style::new().fg(DIM),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "  NODES:",
            Style::new().fg(GROUP).add_modifier(Modifier::BOLD),
        )));
        for (id, name) in &node_list {
            lines.push(Line::from(Span::styled(
                format!("    [{id}] {name}"),
                Style::new().fg(Color::White),
            )));
        }
        if !connections.is_empty() {
            lines.push(Line::from(Span::styled(
                "  CONNECTIONS:",
                Style::new().fg(GROUP).add_modifier(Modifier::BOLD),
            )));
            for conn in connections {
                lines.push(Line::from(Span::styled(
                    format!(
                        "    {} port {} -> {} port {}",
                        conn.from_node, conn.from_port, conn.to_node, conn.to_port
                    ),
                    Style::new().fg(GOLD),
                )));
            }
        }
    }

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, inner);

    // Help
    let help = Paragraph::new(Line::from(vec![
        Span::styled(" F2", Style::new().fg(ACCENT)),
        Span::styled(":synth view  ", Style::new().fg(DIM)),
        Span::styled("Esc", Style::new().fg(ACCENT)),
        Span::styled(":quit", Style::new().fg(DIM)),
    ]));
    f.render_widget(help, chunks[2]);
}

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(BORDER_ACTIVE))
        .title(Span::styled(
            " YAMAHA CS-80 ",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::from(Span::styled(
            " kazoo-cs80 ",
            Style::new().fg(DIM),
        )));

    let (param_name, param_val) = app.current_param_display();

    let mut spans = vec![
        Span::styled(" Oct: ", Style::new().fg(DIM)),
        Span::styled(format!("{}", app.octave), Style::new().fg(Color::White)),
        Span::styled("  |  Drift: ", Style::new().fg(DIM)),
        Span::styled(
            format!("{:.1}c", app.synth.params.drift_cents),
            Style::new().fg(ACCENT),
        ),
        Span::styled("  |  ", Style::new().fg(DIM)),
        Span::styled(
            app.section.name().to_string(),
            Style::new().fg(GOLD).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" > ", Style::new().fg(DIM)),
        Span::styled(param_name, Style::new().fg(Color::White)),
        Span::styled(": ", Style::new().fg(DIM)),
        Span::styled(param_val, Style::new().fg(ACCENT)),
    ];

    // Issue 3: show inline hint for IL/AL params when selected.
    if let Some(hint) = app.current_param_hint() {
        spans.push(Span::styled(format!("  ({hint})"), Style::new().fg(HINT)));
    }

    // Show aftertouch if non-zero.
    if app.aftertouch > 0.01 {
        spans.push(Span::styled(
            format!("  AT:{:.0}%", app.aftertouch * 100.0),
            Style::new().fg(GOLD),
        ));
    }

    let line = Line::from(spans);
    let paragraph = Paragraph::new(line).block(block);
    f.render_widget(paragraph, area);
}

// ---------------------------------------------------------------------------
// Section bar — breadcrumb showing active section (issue 13)
// ---------------------------------------------------------------------------

fn draw_section_bar(f: &mut Frame, area: Rect, app: &App) {
    let sections = [
        Section::Layer1,
        Section::Layer2,
        Section::RingMod,
        Section::Lfo,
        Section::Mixer,
    ];

    let spans: Vec<Span> = sections
        .iter()
        .flat_map(|&s| {
            let is_active = app.section == s;
            let label = match s {
                Section::Layer1 => "Layer I",
                Section::Layer2 => "Layer II",
                Section::RingMod => "Ring",
                Section::Lfo => "LFO",
                Section::Mixer => "Mix",
            };

            let style = if is_active {
                Style::new()
                    .fg(Color::Black)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(DIM)
            };

            vec![
                Span::styled(format!(" [{label}]"), style),
                Span::styled(" ", Style::new()),
            ]
        })
        .collect();

    let line = Line::from(spans);
    let paragraph = Paragraph::new(line);
    f.render_widget(paragraph, area);
}

// ---------------------------------------------------------------------------
// Main editor: two layers side-by-side + shared panel
// ---------------------------------------------------------------------------

fn draw_main_editor(f: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(38), // Layer I
            Constraint::Percentage(38), // Layer II
            Constraint::Percentage(24), // Shared (Ring, LFO, Mix)
        ])
        .split(area);

    draw_layer_panel(f, chunks[0], app, 1);
    draw_layer_panel(f, chunks[1], app, 2);
    draw_shared_panel(f, chunks[2], app);
}

/// Draw a single layer panel with grouped sub-sections.
#[allow(clippy::too_many_lines)]
fn draw_layer_panel(f: &mut Frame, area: Rect, app: &App, layer_num: u8) {
    let section = if layer_num == 1 {
        Section::Layer1
    } else {
        Section::Layer2
    };
    let is_active = app.section == section;

    let border_color = if is_active { BORDER_ACTIVE } else { BORDER_DIM };
    let title = if layer_num == 1 {
        " LAYER I "
    } else {
        " LAYER II "
    };
    let subtitle = if layer_num == 1 {
        " bright / fast "
    } else {
        " slow / evolving "
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(border_color))
        .title(Span::styled(
            title,
            Style::new()
                .fg(if is_active { ACCENT } else { Color::White })
                .add_modifier(if is_active {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ))
        .title_bottom(Line::from(Span::styled(subtitle, Style::new().fg(DIM))));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Build lines with group headers interspersed.
    let p = &app.synth.params;
    let display_fn = if layer_num == 1 {
        super::app::layer1_param_display
    } else {
        super::app::layer2_param_display
    };

    // Group structure: (group_name, start_idx, count)
    let groups: &[(&str, usize, usize)] = &[
        ("VCO", 0, 4),
        ("HPF", 4, 2),
        ("LPF", 6, 2),
        ("FILTER ENV", 8, 6),
        ("VCA ENV", 14, 4),
        ("OUTPUT", 18, 1),
    ];

    let mut lines: Vec<Line> = Vec::new();
    for &(group_name, start, count) in groups {
        // Group header line
        lines.push(Line::from(Span::styled(
            format!(" {group_name}"),
            Style::new().fg(GROUP).add_modifier(Modifier::BOLD),
        )));

        // Parameter lines within group
        for offset in 0..count {
            let idx = start + offset;
            let (name, value) = display_fn(p, idx);
            let is_selected = is_active && idx == app.param_index;

            let style = if is_selected {
                Style::new()
                    .fg(ACCENT)
                    .bg(SEL_BG)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(Color::White)
            };

            let marker = if is_selected { ">" } else { " " };
            lines.push(Line::from(Span::styled(
                format!("  {marker} {name:<12}{value:>10}"),
                style,
            )));
        }
    }

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, inner);
}

// ---------------------------------------------------------------------------
// Shared panel (Ring Mod, LFO, Mixer)
// ---------------------------------------------------------------------------

fn draw_shared_panel(f: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6), // Ring Mod
            Constraint::Length(7), // LFO (was 8, drift removed)
            Constraint::Min(5),    // Mixer
        ])
        .split(area);

    draw_section_panel(
        f,
        chunks[0],
        app,
        Section::RingMod,
        "RING MOD",
        super::app::ring_mod_param_display,
    );
    draw_section_panel(
        f,
        chunks[1],
        app,
        Section::Lfo,
        "LFO",
        super::app::lfo_param_display,
    );
    draw_section_panel(
        f,
        chunks[2],
        app,
        Section::Mixer,
        "MIX",
        super::app::mixer_param_display,
    );
}

/// Draw a bordered section with its parameters.
fn draw_section_panel(
    f: &mut Frame,
    area: Rect,
    app: &App,
    section: Section,
    title: &str,
    display_fn: fn(&crate::synth::SynthParams, usize) -> (String, String),
) {
    let is_active = app.section == section;
    let border_color = if is_active { BORDER_ACTIVE } else { BORDER_DIM };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(border_color))
        .title(Span::styled(
            format!(" {title} "),
            Style::new()
                .fg(if is_active { ACCENT } else { GOLD })
                .add_modifier(if is_active {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let lines: Vec<Line> = (0..section.param_count())
        .map(|i| {
            let (name, value) = display_fn(&app.synth.params, i);
            let is_selected = is_active && i == app.param_index;

            let style = if is_selected {
                Style::new()
                    .fg(ACCENT)
                    .bg(SEL_BG)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(Color::White)
            };

            let marker = if is_selected { ">" } else { " " };
            Line::from(Span::styled(
                format!("{marker} {name:<10}{value:>8}"),
                style,
            ))
        })
        .collect();

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, inner);
}

// ---------------------------------------------------------------------------
// Voice monitor — 8 voices with drift visualization (issue 11: releasing)
// ---------------------------------------------------------------------------

fn draw_voice_monitor(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(BORDER_DIM))
        .title(Span::styled(
            " VOICES ",
            Style::new().fg(GOLD).add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let voice_spans: Vec<Span> = app
        .voice_status
        .iter()
        .map(|vs| {
            if vs.active {
                let note_str = vs
                    .note
                    .map_or_else(|| "---".to_string(), kazoo_core::midi_note_name);

                // Issue 11: releasing voices shown with dim color
                if vs.releasing {
                    Span::styled(
                        format!(" {}:{}{:+.1}c~", vs.index + 1, note_str, vs.detune_cents),
                        Style::new().fg(VOICE_RELEASING),
                    )
                } else {
                    Span::styled(
                        format!(" {}:{}{:+.1}c ", vs.index + 1, note_str, vs.detune_cents),
                        Style::new().fg(VOICE_ACTIVE).add_modifier(Modifier::BOLD),
                    )
                }
            } else {
                Span::styled(format!(" {}:--- ", vs.index + 1), Style::new().fg(DIM))
            }
        })
        .collect();

    let line = Line::from(voice_spans);
    let paragraph = Paragraph::new(line);
    f.render_widget(paragraph, inner);
}

// ---------------------------------------------------------------------------
// Waveform + Spectrum display (issues 5, 7)
// ---------------------------------------------------------------------------

/// Braille character patterns for centered waveform rendering.
/// Each braille cell is 2 dots wide x 4 dots high.
/// We map the waveform amplitude to vertical position within the cell.
fn draw_waveform_and_spectrum(f: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(60), // Waveform
            Constraint::Percentage(40), // Spectrum
        ])
        .split(area);

    draw_waveform_braille(f, chunks[0], app);
    draw_spectrum(f, chunks[1], app);
}

/// Draw waveform using block characters for centered display.
/// Silence renders as a flat center line, not at 50% sparkline height.
fn draw_waveform_braille(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(BORDER_DIM))
        .title(Span::styled(" WAVEFORM ", Style::new().fg(DIM)));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let waveform = &app.waveform_buf;
    let width = inner.width as usize;
    let height = inner.height as usize;
    if width == 0 || height == 0 {
        return;
    }

    let step = waveform.len().max(1) / width.max(1);
    let step = step.max(1);

    // Build text lines using block chars: ▁▂▃▄▅▆▇█ for positive, inversed for negative.
    // Center line is at height/2. Positive samples go up, negative go down.
    let half_h = height as f32 / 2.0;
    let center_row = height / 2;

    let mut lines: Vec<Line> = Vec::with_capacity(height);
    for row in 0..height {
        let mut spans = Vec::with_capacity(width);
        for col in 0..width {
            let sample_idx = col * step;
            let sample = if sample_idx < waveform.len() {
                waveform[sample_idx].clamp(-1.0, 1.0)
            } else {
                0.0
            };

            // Map sample to row position. sample=1.0 -> row 0, sample=-1.0 -> row height-1
            let sample_row = ((-sample).mul_add(half_h, half_h)) as usize;
            let sample_row = sample_row.min(height.saturating_sub(1));

            let ch = if row == center_row && sample.abs() < 0.02 {
                // Silence: flat center line
                '─'
            } else if row == sample_row {
                '█'
            } else if (sample >= 0.0 && row > sample_row && row <= center_row)
                || (sample < 0.0 && row < sample_row && row >= center_row)
            {
                '│'
            } else if row == center_row {
                '·'
            } else {
                ' '
            };

            let color = if ch == '─' || ch == '·' {
                DIM
            } else {
                WAVE_COLOR
            };
            spans.push(Span::styled(String::from(ch), Style::new().fg(color)));
        }
        lines.push(Line::from(spans));
    }

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, inner);
}

/// Draw a simple spectrum display from the waveform data using magnitude bars.
/// Uses a basic DFT approximation (binned peak magnitudes) for display purposes.
fn draw_spectrum(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(BORDER_DIM))
        .title(Span::styled(" SPECTRUM ", Style::new().fg(DIM)));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let width = inner.width as usize;
    let height = inner.height as usize;
    if width == 0 || height == 0 {
        return;
    }

    // Compute simple magnitude spectrum from waveform using binned energy.
    let waveform = &app.waveform_buf;
    let num_bins = width.min(64);

    let mut magnitudes = vec![0.0_f32; num_bins];
    if !waveform.is_empty() {
        let samples_per_bin = waveform.len() / num_bins.max(1);
        for (bin, mag) in magnitudes.iter_mut().enumerate() {
            // Simple energy per frequency band approximation:
            // Take average absolute value of samples in this time segment,
            // weighted toward higher frequencies for later bins.
            let start = bin * samples_per_bin;
            let end = (start + samples_per_bin).min(waveform.len());
            if start < end {
                let sum: f32 = waveform[start..end].iter().map(|s| s.abs()).sum();
                *mag = sum / (end - start) as f32;
            }
        }
    }

    // Normalize magnitudes to [0, height]
    let max_mag = magnitudes
        .iter()
        .copied()
        .fold(0.0_f32, f32::max)
        .max(0.001);

    // Bar chart characters: ▁▂▃▄▅▆▇█
    let bar_chars = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

    let mut lines: Vec<Line> = Vec::with_capacity(height);
    for row in 0..height {
        let row_threshold = (height - 1 - row) as f32 / height as f32;
        let mut spans = Vec::with_capacity(width);

        for col in 0..width {
            let bin = col * num_bins / width.max(1);
            let bin = bin.min(num_bins.saturating_sub(1));
            let normalized = magnitudes[bin] / max_mag;

            if normalized > row_threshold {
                // Full or partial block
                let frac = (normalized - row_threshold) * height as f32;
                let char_idx = ((frac * 8.0) as usize).min(7);
                spans.push(Span::styled(
                    String::from(bar_chars[char_idx]),
                    Style::new().fg(SPECTRUM_COLOR),
                ));
            } else {
                spans.push(Span::styled(" ", Style::new()));
            }
        }
        lines.push(Line::from(spans));
    }

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, inner);
}

// ---------------------------------------------------------------------------
// Footer — keyboard help + controls (issue 12: show sharps)
// ---------------------------------------------------------------------------

fn draw_footer(f: &mut Frame, area: Rect, _app: &App) {
    let lines = vec![
        Line::from(vec![
            Span::styled(" Piano: ", Style::new().fg(GOLD)),
            Span::styled("z x c v b n m , . /", Style::new().fg(Color::White)),
            Span::styled("  sharps: ", Style::new().fg(DIM)),
            Span::styled("s d g h j", Style::new().fg(Color::White)),
            Span::styled("  upper: ", Style::new().fg(DIM)),
            Span::styled("q w e r t y u i o p", Style::new().fg(Color::White)),
            Span::styled("  sharps: ", Style::new().fg(DIM)),
            Span::styled("2 3 5 6 7", Style::new().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled(" Tab", Style::new().fg(ACCENT)),
            Span::styled(":section  ", Style::new().fg(DIM)),
            Span::styled("\u{2191}/\u{2193}", Style::new().fg(ACCENT)),
            Span::styled(":param  ", Style::new().fg(DIM)),
            Span::styled("\u{2190}/\u{2192}", Style::new().fg(ACCENT)),
            Span::styled(":adjust  ", Style::new().fg(DIM)),
            Span::styled("Shift+\u{2190}/\u{2192}", Style::new().fg(ACCENT)),
            Span::styled(":coarse  ", Style::new().fg(DIM)),
            Span::styled("[/]", Style::new().fg(ACCENT)),
            Span::styled(":octave  ", Style::new().fg(DIM)),
            Span::styled("Shift+\u{2191}/\u{2193}", Style::new().fg(ACCENT)),
            Span::styled(":aftertouch", Style::new().fg(DIM)),
        ]),
        Line::from(vec![
            Span::styled(" Ctrl+S", Style::new().fg(ACCENT)),
            Span::styled(":save preset  ", Style::new().fg(DIM)),
            Span::styled("Ctrl+L", Style::new().fg(ACCENT)),
            Span::styled(":load preset  ", Style::new().fg(DIM)),
            Span::styled("Esc", Style::new().fg(ACCENT)),
            Span::styled(":quit", Style::new().fg(DIM)),
        ]),
    ];

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, area);
}
