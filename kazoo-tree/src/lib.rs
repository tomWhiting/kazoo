//! `kazoo tree`: the wall's history grown as a tree you can walk.
//!
//! It reads the tree the wall keeps (`~/.kazoo/tree`, a bare git
//! repository; KZ-T02b) straight from git, so it works whether or not the
//! wall is playing, and it never changes the music: the wall does the
//! playing, the tree is for seeing where it has been and finding a moment.

pub mod app;
pub mod growth;
pub mod history;
pub mod lanes;
pub mod render;
#[cfg(test)]
mod test_support;

use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use crossterm::cursor::Show;
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
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};

use crate::app::{App, Grown, HELP, PANEL};
use crate::history::{History, HistoryError, Snapshot};
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

/// Refuse a folder with no tree in it, saying how one is planted.
fn planted(dir: &Path) -> io::Result<()> {
    if dir.join("HEAD").exists() {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "no tree at {}: the wall plants one with `kazoo wall tree import` (once, with the wall stopped)",
            dir.display()
        ),
    ))
}

/// The whole tree at `dir`, fitted to `cols` × `rows`, as braille text:
/// what a seat reads, and what `kazoo tree --print` writes.
///
/// # Errors
///
/// When there is no tree at `dir`, or it cannot be read.
pub fn print(dir: PathBuf, cols: u16, rows: u16) -> io::Result<String> {
    planted(&dir)?;
    let mut app = App::open(dir).map_err(io::Error::other)?;
    let view = app.view(cols, rows);
    let scene = Scene {
        history: &app.history,
        growth: &app.growth,
        gestures: &app.gestures,
        days: &app.days,
        line: &app.line,
        selected: None,
        newest: render::newest(&app.history),
    };
    Ok(render::draw(&scene, &view, cols, rows).text())
}

/// Open the tree at `dir` and walk it until quit.
///
/// # Errors
///
/// When there is no tree at `dir`, it cannot be read, or the terminal
/// cannot be used.
pub fn run(dir: PathBuf) -> io::Result<()> {
    planted(&dir)?;
    // Read once before the screen is taken, so a tree that cannot be read
    // says so plainly; the snapshot comes first, so the reader's first look
    // compares against exactly what was read.
    let snapshot = Snapshot::read(&dir).map_err(io::Error::other)?;
    let mut history = History::default();
    history.grow(&dir, &snapshot).map_err(io::Error::other)?;
    let mut app = App::showing(dir.clone(), Grown::of(history.clone()));
    let reader = Reader::start(dir, history, snapshot)?;
    // A terminal closed, a `kill`, or ^C from outside the raw terminal:
    // each ends the walk the ordinary way, so the terminal is put back.
    let stop = Arc::new(AtomicBool::new(false));
    let mut signals = Vec::new();
    for signal in [SIGTERM, SIGHUP, SIGINT] {
        signals.push(signal_hook::flag::register(signal, Arc::clone(&stop))?);
    }
    let walked = Screen::enter().and_then(|mut screen| {
        let walked = walk(&mut app, &mut screen.terminal, &reader, &stop);
        let left = screen.leave();
        match (walked, left) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(err), Ok(())) | (Ok(()), Err(err)) => Err(err),
            (Err(walk), Err(leave)) => Err(io::Error::other(format!(
                "{walk}; and the terminal could not be put back: {leave}"
            ))),
        }
    });
    for id in signals {
        signal_hook::low_level::unregister(id);
    }
    walked
}

/// What the reader sends: a tree read and grown, or why it could not be.
type News = Result<Grown, HistoryError>;

/// Reads the tree off the screen's thread: looks every [`LOOK_AGAIN`], or
/// at once when asked, and sends a freshly grown tree whenever the refs or
/// HEAD have moved. It reads only the commits new since its last good read,
/// and moves on from a snapshot only once the read from it has worked, so a
/// read that fails is tried again at the next look.
struct Reader {
    asks: mpsc::Sender<()>,
    news: mpsc::Receiver<News>,
}

impl Reader {
    fn start(dir: PathBuf, history: History, snapshot: Snapshot) -> io::Result<Self> {
        let (asks, asked) = mpsc::channel::<()>();
        let (tell, news) = mpsc::channel::<News>();
        std::thread::Builder::new()
            .name("kazoo-tree-reader".to_owned())
            .spawn(move || read_on(&dir, history, snapshot, &asked, &tell))?;
        Ok(Self { asks, news })
    }
}

fn read_on(
    dir: &Path,
    mut history: History,
    mut last: Snapshot,
    asked: &mpsc::Receiver<()>,
    tell: &mpsc::Sender<News>,
) {
    loop {
        let forced = match asked.recv_timeout(LOOK_AGAIN) {
            Ok(()) => true,
            Err(mpsc::RecvTimeoutError::Timeout) => false,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        let news = match Snapshot::read(dir) {
            Err(err) => Err(err),
            Ok(snapshot) if snapshot == last && !forced => continue,
            Ok(snapshot) => match history.grow(dir, &snapshot) {
                Ok(_) => {
                    last = snapshot;
                    Ok(Grown::of(history.clone()))
                }
                // A grow that failed leaves the tree's moments good but its
                // tips behind; reading it whole starts clean.
                Err(grow) => match History::read(dir) {
                    Ok(whole) => {
                        history = whole;
                        last = snapshot;
                        Ok(Grown::of(history.clone()))
                    }
                    Err(read) => Err(HistoryError::Git(format!("{grow}; and read whole: {read}"))),
                },
            },
        };
        if tell.send(news).is_err() {
            // The screen is gone: nothing is left to read for.
            return;
        }
    }
}

/// A panic hook, as the standard library keeps one.
type Hook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static>;

/// The terminal held for the tree: raw, on the alternate screen, with the
/// mouse, and a panic hook that puts it back. Left on every way out:
/// `leave`, a drop after an error, or a panic; the panic hook that was
/// there before is put back on leaving.
struct Screen {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    previous: Arc<Hook>,
    left: bool,
}

impl Screen {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let previous: Arc<Hook> = Arc::new(std::panic::take_hook());
        let before = Arc::clone(&previous);
        std::panic::set_hook(Box::new(move |info| {
            restore();
            before(info);
        }));
        let mut stdout = io::stdout();
        if let Err(err) = execute!(stdout, EnterAlternateScreen, EnableMouseCapture) {
            restore();
            put_back(&previous);
            return Err(err);
        }
        match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => Ok(Self {
                terminal,
                previous,
                left: false,
            }),
            Err(err) => {
                restore();
                put_back(&previous);
                Err(err)
            }
        }
    }

    fn leave(mut self) -> io::Result<()> {
        let shown = self.terminal.show_cursor();
        let left = execute!(
            self.terminal.backend_mut(),
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let raw = disable_raw_mode();
        put_back(&self.previous);
        // Left: Drop need not leave a second time.
        self.left = true;
        shown.and(left).and(raw)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        if !self.left {
            restore();
            put_back(&self.previous);
        }
    }
}

/// Put back the panic hook that was there before. Not while panicking: the
/// hooks cannot be changed then, and ours has already put the terminal
/// back.
fn put_back(previous: &Arc<Hook>) {
    if std::thread::panicking() {
        return;
    }
    let ours = std::panic::take_hook();
    drop(ours);
    let previous = Arc::clone(previous);
    std::panic::set_hook(Box::new(move |info| previous(info)));
}

/// Put the terminal back as it was, from anywhere: the cursor shown, the
/// mouse let go, the alternate screen left, raw mode off. Each step is
/// tried even when one before it failed; the failures are written to
/// stderr, which is all there is left to tell at that point.
fn restore() {
    let mut stdout = io::stdout();
    if let Err(err) = execute!(stdout, Show, DisableMouseCapture, LeaveAlternateScreen) {
        eprintln!("kazoo tree: the terminal could not be put back: {err}");
    }
    if let Err(err) = disable_raw_mode() {
        eprintln!("kazoo tree: raw mode could not be left: {err}");
    }
}

/// Walk the tree until quit or a signal: draw only when something
/// changed, take keys and the mouse, and show each tree the reader sends.
fn walk(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    reader: &Reader,
    stop: &AtomicBool,
) -> io::Result<()> {
    let mut dirty = true;
    let mut reader_gone = false;
    while !app.quit && !stop.load(Ordering::Relaxed) {
        if dirty {
            terminal.draw(|frame| frame_of(app, frame.area(), frame.buffer_mut()))?;
            dirty = false;
        }
        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => app.key(key),
                Event::Mouse(mouse) => app.mouse(mouse),
                Event::Resize(..) => {}
                _ => continue,
            }
            dirty = true;
        }
        if app.wants_reading {
            app.wants_reading = false;
            if reader.asks.send(()).is_err() {
                "the tree's reader has stopped: quit and open it again".clone_into(&mut app.said);
            }
            dirty = true;
        }
        while !reader_gone {
            match reader.news.try_recv() {
                Ok(Ok(grown)) => app.show(grown),
                Ok(Err(err)) => app.said = format!("the tree could not be read: {err}"),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    reader_gone = true;
                    "the tree's reader has stopped: quit and open it again"
                        .clone_into(&mut app.said);
                }
            }
            dirty = true;
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
        days: &app.days,
        line: &app.line,
        selected: app.selected,
        newest: render::newest(&app.history),
    };
    let picture = render::draw(&scene, &view, tree.width, tree.height);
    paint(&picture, tree, buf);
    if wide {
        details(app, panel, buf);
    }
    let level = Level::of(view.scale);
    let branches = app
        .history
        .limbs
        .iter()
        .filter(|limb| !limb.moments.is_empty())
        .count();
    let mut words = format!(
        " {} moments, {} branches │ {} │ ? help, q quit",
        app.history.moments.len(),
        branches,
        level.name()
    );
    let problems = app.history.problem_count();
    if problems > 0 {
        let noun = if problems == 1 { "problem" } else { "problems" };
        words = format!(" {problems} {noun} reading the tree, in the details │{words}");
    }
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
            if cell.tail {
                // The wide letter before it was set over both cells.
                continue;
            }
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
            let ch = cell.char();
            if unicode_width::UnicodeWidthChar::width(ch) == Some(2) {
                // A wide letter: set as a string, so the cell after it is
                // given over to it.
                let mut text = [0_u8; 4];
                buf.set_string(area.x + col, area.y + row, ch.encode_utf8(&mut text), style);
            } else if let Some(target) = buf.cell_mut((area.x + col, area.y + row)) {
                target.set_char(ch).set_style(style);
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
