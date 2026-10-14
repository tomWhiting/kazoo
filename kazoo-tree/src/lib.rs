//! `kazoo tree`: the wall's history grown as a tree you can walk.
//!
//! It reads the tree the wall keeps (`~/.kazoo/tree`, a bare git
//! repository; KZ-T02b) straight from git, so it works whether or not the
//! wall is playing, and it never changes the music: the wall does the
//! playing, the tree is for seeing where it has been and finding a moment.

pub mod app;
pub mod growth;
pub mod history;
pub mod render;
#[cfg(test)]
mod test_support;

use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::{App, HELP, PANEL};
use crate::render::{Level, Picture, Scene};

/// How often the tree is checked for new growth.
const LOOK_AGAIN: Duration = Duration::from_secs(2);

/// Where the tree is: `$KAZOO_TREE_DIR`, else beside the wall's state.
///
/// # Errors
///
/// When neither the override nor `$HOME` is set.
pub fn tree_dir() -> io::Result<PathBuf> {
    let state = kazoo_wall::paths::state_dir()?;
    kazoo_wall::tree::git::tree_dir(&state).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "the wall's state folder {} has no parent to keep a tree beside",
                state.display()
            ),
        )
    })
}

/// The whole tree at `dir`, fitted to `cols` × `rows`, as braille text:
/// what a seat reads, and what `kazoo tree --print` writes.
///
/// # Errors
///
/// When the tree cannot be read.
pub fn print(dir: PathBuf, cols: u16, rows: u16) -> io::Result<String> {
    let mut app = App::open(dir).map_err(io::Error::other)?;
    let view = app.view(cols, rows);
    let scene = Scene {
        history: &app.history,
        growth: &app.growth,
        gestures: &app.gestures,
        line: &app.line,
        selected: None,
    };
    Ok(render::draw(&scene, &view, cols, rows).text())
}

/// Open the tree at `dir` and walk it until quit.
///
/// # Errors
///
/// When the tree cannot be read, or the terminal cannot be used.
pub fn run(dir: PathBuf) -> io::Result<()> {
    if !dir.join("HEAD").exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "no tree at {}: the wall plants one with `kazoo wall tree import` (once, with the wall stopped)",
                dir.display()
            ),
        ));
    }
    let mut app = App::open(dir).map_err(io::Error::other)?;
    let mut terminal = Screen::enter()?;
    let result = walk(&mut app, &mut terminal.0);
    terminal.leave()?;
    result
}

/// The terminal held for the tree: raw, on the alternate screen, with the
/// mouse. Left on every way out: `leave`, a drop after an error, or a panic.
struct Screen(Terminal<CrosstermBackend<Stdout>>, bool);

impl Screen {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            hook(info);
        }));
        let mut stdout = io::stdout();
        if let Err(err) = execute!(stdout, EnterAlternateScreen, EnableMouseCapture) {
            restore();
            return Err(err);
        }
        match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => Ok(Self(terminal, false)),
            Err(err) => {
                restore();
                Err(err)
            }
        }
    }

    fn leave(mut self) -> io::Result<()> {
        let shown = self.0.show_cursor();
        let left = execute!(
            self.0.backend_mut(),
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let raw = disable_raw_mode();
        // Left: Drop need not leave a second time.
        self.1 = true;
        shown.and(left).and(raw)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        if !self.1 {
            restore();
        }
    }
}

/// Put the terminal back as it was, from anywhere. Each step is tried even
/// when one before it failed; the failures are written to stderr, which is
/// all there is left to tell at that point.
fn restore() {
    let mut stdout = io::stdout();
    if let Err(err) = execute!(stdout, DisableMouseCapture, LeaveAlternateScreen) {
        eprintln!("kazoo tree: the terminal could not be put back: {err}");
    }
    if let Err(err) = disable_raw_mode() {
        eprintln!("kazoo tree: raw mode could not be left: {err}");
    }
}

fn walk(app: &mut App, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    let mut stamp = history::stamp(&app.dir).map_err(io::Error::other)?;
    let mut looked = Instant::now();
    while !app.quit {
        terminal.draw(|frame| frame_of(app, frame.area(), frame.buffer_mut()))?;
        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => app.key(key),
                Event::Mouse(mouse) => app.mouse(mouse),
                _ => {}
            }
        }
        if looked.elapsed() >= LOOK_AGAIN {
            looked = Instant::now();
            match history::stamp(&app.dir) {
                Ok(now) if now != stamp => {
                    stamp = now;
                    app.reload();
                }
                Ok(_) => {}
                Err(err) => app.said = format!("the tree could not be checked: {err}"),
            }
        }
    }
    Ok(())
}

/// Draw the whole frame: the tree, the details panel when there is room,
/// the footer, and the help card over them when open.
pub fn frame_of(app: &mut App, area: Rect, buf: &mut Buffer) {
    let [body, footer] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    let wide = body.width >= PANEL + 40;
    let [tree, panel] = if wide {
        Layout::horizontal([Constraint::Min(1), Constraint::Length(PANEL)]).areas(body)
    } else {
        [body, Rect::default()]
    };
    app.area = (tree.width, tree.height);
    let view = app.view(tree.width, tree.height);
    let scene = Scene {
        history: &app.history,
        growth: &app.growth,
        gestures: &app.gestures,
        line: &app.line,
        selected: app.selected,
    };
    let picture = render::draw(&scene, &view, tree.width, tree.height);
    paint(&picture, tree, buf);
    if wide {
        details(app, panel, buf);
    }
    let level = Level::of(view.scale);
    let mut words = format!(
        " {} moments, {} limbs │ {} │ ? help, q quit",
        app.history.moments.len(),
        app.history.limbs.len(),
        level.name()
    );
    if !app.said.is_empty() {
        words = format!(" {} │{words}", app.said);
    }
    Paragraph::new(words)
        .style(Style::new().fg(Color::Gray))
        .render_into(footer, buf);
    if app.help {
        help(area, buf);
    }
}

/// `Widget::render` by value, without importing the trait at every call.
trait RenderInto {
    fn render_into(self, area: Rect, buf: &mut Buffer);
}

impl<W: ratatui::widgets::Widget> RenderInto for W {
    fn render_into(self, area: Rect, buf: &mut Buffer) {
        self.render(area, buf);
    }
}

fn paint(picture: &Picture, area: Rect, buf: &mut Buffer) {
    for row in 0..picture.rows.min(area.height) {
        for col in 0..picture.cols.min(area.width) {
            let Some(cell) = picture.at(col, row) else {
                continue;
            };
            let mut style = Style::new();
            if let Some((r, g, b)) = cell.colour {
                style = style.fg(Color::Rgb(r, g, b));
            }
            if cell.bright {
                style = style.add_modifier(Modifier::BOLD);
            }
            if cell.selected {
                style = style.add_modifier(Modifier::REVERSED);
            }
            if let Some(target) = buf.cell_mut((area.x + col, area.y + row)) {
                target.set_char(cell.char()).set_style(style);
            }
        }
    }
}

fn details(app: &App, area: Rect, buf: &mut Buffer) {
    let lines: Vec<Line<'_>> = app
        .details()
        .into_iter()
        .map(|(key, value)| {
            if key.is_empty() {
                Line::from(value)
            } else {
                Line::from(vec![
                    Span::styled(format!("{key:>7} "), Style::new().fg(Color::DarkGray)),
                    Span::raw(value),
                ])
            }
        })
        .collect();
    Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(Block::new().borders(Borders::LEFT).title(" moment "))
        .render_into(area, buf);
}

fn help(area: Rect, buf: &mut Buffer) {
    let mut lines = Vec::new();
    for (heading, keys) in HELP {
        lines.push(Line::styled(
            *heading,
            Style::new().add_modifier(Modifier::BOLD),
        ));
        for (key, what) in *keys {
            lines.push(Line::from(vec![
                Span::styled(format!("  {key:<16}"), Style::new().fg(Color::Yellow)),
                Span::raw(*what),
            ]));
        }
        lines.push(Line::raw(""));
    }
    let height = u16::try_from(lines.len() + 2)
        .unwrap_or(u16::MAX)
        .min(area.height);
    let width = 56.min(area.width);
    let card = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    Clear.render_into(card, buf);
    Paragraph::new(lines)
        .block(Block::bordered().title(" the tree: keys and marks "))
        .render_into(card, buf);
}
