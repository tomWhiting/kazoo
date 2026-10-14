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
pub mod ledger;
pub mod render;
#[cfg(test)]
mod test_support;

use std::io::{self, Stdout, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
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

use crate::app::{App, Grower, Grown, HELP, PANEL, Shown};
use crate::history::{HistoryError, Snapshot};
use crate::ledger::Ledger;
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
    let ledger = Ledger::of_tree(&dir);
    let mut app = App::open(dir, ledger).map_err(io::Error::other)?;
    let view = app.view(cols, rows);
    let scene = Scene {
        history: &app.history,
        growth: &app.growth,
        gestures: &app.gestures,
        days: &app.days,
        line: &app.line,
        selected: None,
        now: chrono::Utc::now().timestamp(),
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
    let mut grower = Grower::new(dir.clone(), Ledger::of_tree(&dir));
    let grown = grower.read(&snapshot).map_err(io::Error::other)?;
    let mut app = App::showing(dir.clone(), grown);
    let reader = Reader::start(dir, grower, snapshot)?;
    // A terminal closed, a `kill`, or ^C from outside the raw terminal:
    // each ends the walk the ordinary way, so the terminal is put back,
    // and then ends the process as that signal would have.
    let caught = Arc::new(AtomicUsize::new(0));
    let mut signals = Vec::new();
    for signal in [SIGTERM, SIGHUP, SIGINT] {
        let number = usize::try_from(signal).map_err(io::Error::other)?;
        signals.push(signal_hook::flag::register_usize(
            signal,
            Arc::clone(&caught),
            number,
        )?);
    }
    let walked = Screen::enter().and_then(|mut screen| {
        let walked = walk(&mut app, &mut screen.terminal, &reader, &caught);
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
        if !signal_hook::low_level::unregister(id) {
            eprintln!("kazoo tree: a signal's handler had already gone");
        }
    }
    let signal = caught.load(Ordering::Relaxed);
    if signal != 0 && walked.is_ok() {
        // As if the signal had come with nothing to catch it: the shell
        // sees the walk ended by it.
        signal_hook::low_level::emulate_default_handler(
            i32::try_from(signal).map_err(io::Error::other)?,
        )?;
    }
    walked
}

/// What the reader sends: a tree read and grown, or why it could not be.
type News = Result<Grown, HistoryError>;

/// Reads the tree off the screen's thread: looks every [`LOOK_AGAIN`], or
/// at once when asked, and sends a freshly grown tree whenever the refs or
/// HEAD have moved, or a look before failed. It reads and grows only what
/// is new since its last good read. At most one tree waits to be shown: the
/// reader waits for the screen to take it before reading on, so a screen
/// that is slow to draw is never sent a pile of trees.
struct Reader {
    asks: mpsc::Sender<()>,
    news: mpsc::Receiver<News>,
}

impl Reader {
    fn start(dir: PathBuf, grower: Grower, snapshot: Snapshot) -> io::Result<Self> {
        let (asks, asked) = mpsc::channel::<()>();
        let (tell, news) = mpsc::sync_channel::<News>(1);
        std::thread::Builder::new()
            .name(READER.to_owned())
            .spawn(move || read_on(&dir, grower, snapshot, &asked, &tell))?;
        Ok(Self { asks, news })
    }
}

/// The reader thread's name.
const READER: &str = "kazoo-tree-reader";

fn read_on(
    dir: &Path,
    mut grower: Grower,
    mut last: Snapshot,
    asked: &mpsc::Receiver<()>,
    tell: &mpsc::SyncSender<News>,
) {
    let mut failing = false;
    loop {
        let forced = match asked.recv_timeout(LOOK_AGAIN) {
            Ok(()) => true,
            Err(mpsc::RecvTimeoutError::Timeout) => false,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        let news = match Snapshot::read(dir) {
            Err(err) => {
                failing = true;
                Err(err)
            }
            Ok(snapshot) if snapshot == last && !forced && !failing => continue,
            Ok(snapshot) => {
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    grower.read(&snapshot)
                })) {
                    Ok(Ok(grown)) => {
                        last = snapshot;
                        failing = false;
                        Ok(grown)
                    }
                    Ok(Err(err)) => {
                        failing = true;
                        Err(err)
                    }
                    Err(panic) => {
                        // Whatever the grower held may be half changed:
                        // start again from nothing at the next look.
                        grower = Grower::new(dir.to_path_buf(), Ledger::of_tree(dir));
                        last = Snapshot::default();
                        failing = true;
                        Err(HistoryError::Broken(format!(
                            "reading the tree failed inside ({}); it is read again whole at the next look",
                            panic_words(panic.as_ref())
                        )))
                    }
                }
            }
        };
        if tell.send(news).is_err() {
            // The screen is gone: nothing is left to read for.
            return;
        }
    }
}

/// What a panic said, as far as it can be read.
fn panic_words(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|words| (*words).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "with no words".to_owned())
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
        let screen = std::thread::current().id();
        std::panic::set_hook(Box::new(move |info| {
            // Only the screen's own thread puts the terminal back: a panic
            // on the reader is caught there and said in the footer, and the
            // screen goes on. Its words are not written over the screen.
            if std::thread::current().id() == screen {
                restore();
                before(info);
            }
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
    stop: &AtomicUsize,
) -> io::Result<()> {
    let mut dirty = true;
    let mut reader_gone = false;
    while !app.quit && stop.load(Ordering::Relaxed) == 0 {
        if let Some(text) = app.yank.take() {
            // OSC 52: the terminal puts it on the clipboard.
            let backend = terminal.backend_mut();
            write!(backend, "\x1b]52;c;{}\x07", base64(text.as_bytes()))?;
            backend.flush()?;
        }
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
        now: chrono::Utc::now().timestamp(),
    };
    if app.shown == Shown::Lanes {
        lanes(app, tree, buf);
    } else {
        let picture = render::draw(&scene, &view, tree.width, tree.height);
        paint(&picture, tree, buf);
    }
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
        if app.shown == Shown::Lanes {
            "lanes: one row a moment"
        } else {
            level.name()
        }
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
        help(app, area, buf);
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

/// The lanes view: one row a moment, newest at the top, each limb a lane;
/// then a swatch of how it sounded, its words, who and when.
fn lanes(app: &mut App, area: Rect, buf: &mut Buffer) {
    let top = app.lanes_from(area.height);
    let bottom = (top + usize::from(area.height)).min(app.lanes.rows.len());
    let crossing = app.lanes.crossing(top, bottom);
    let graphs: Vec<String> = (top..bottom)
        .map(|r| app.lanes.graph(&app.history, &crossing, r))
        .collect();
    let widest = graphs
        .iter()
        .map(|graph| graph.chars().count())
        .max()
        .unwrap_or(0);
    let room = widest.min(usize::from(area.width / 2));
    // The graph scrolls sideways to keep the selected row's own lane in
    // view; a row cut on either side says so with ‹ or ›.
    let left = app
        .selected_row()
        .and_then(|at| app.lanes.rows.get(at))
        .map_or(0, |row| (row.lane * 2 + 2).saturating_sub(room));
    let lines: Vec<Line<'_>> = app.lanes.rows[top..bottom]
        .iter()
        .zip(&graphs)
        .map(|(row, full)| {
            let moment = &app.history.moments[row.moment];
            let (r, g, b) = render::colour_of(moment.sound);
            let length = full.chars().count();
            let mut graph: Vec<char> = full.chars().skip(left).take(room).collect();
            if left > 0
                && let Some(first) = graph.first_mut()
            {
                *first = '‹';
            }
            if length > left + room
                && let Some(last) = graph.last_mut()
            {
                *last = '›';
            }
            let graph: String = graph.into_iter().collect();
            let pad = room.saturating_sub(graph.chars().count());
            let on_line = app.line.get(row.moment).copied().unwrap_or(false);
            let mut graph_style = Style::new().fg(Color::Rgb(r, g, b));
            if on_line {
                graph_style = graph_style.add_modifier(Modifier::BOLD);
            }
            let when = chrono::DateTime::from_timestamp(moment.time, 0).map_or_else(
                || format!("(time {} does not read)", moment.time),
                |utc| {
                    utc.with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M")
                        .to_string()
                },
            );
            let mut style = Style::new();
            if app.selected == Some(row.moment) {
                style = style.add_modifier(Modifier::REVERSED);
            }
            Line::from(vec![
                Span::styled(format!("{graph}{} ", " ".repeat(pad)), graph_style),
                Span::styled("■ ", Style::new().fg(Color::Rgb(r, g, b))),
                Span::raw(moment.summary.clone()),
                Span::styled(
                    format!("  {} {when}", moment.seat),
                    Style::new().fg(Color::DarkGray),
                ),
            ])
            .style(style)
        })
        .collect();
    Paragraph::new(lines).render_into(area, buf);
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

fn help(app: &mut App, area: Rect, buf: &mut Buffer) {
    let mut lines = Vec::new();
    for (heading, keys) in HELP {
        lines.push(Line::styled(
            *heading,
            Style::new().add_modifier(Modifier::BOLD),
        ));
        for (key, what) in *keys {
            lines.push(Line::from(vec![
                Span::styled(format!("  {key:<18}"), Style::new().fg(Color::Yellow)),
                Span::raw(*what),
            ]));
        }
        lines.push(Line::raw(""));
    }
    let height = u16::try_from(lines.len() + 2)
        .unwrap_or(u16::MAX)
        .min(area.height);
    let inside = usize::from(height.saturating_sub(2));
    // Taller than the screen: it scrolls, and says so.
    app.help_top = app.help_top.min(lines.len().saturating_sub(inside));
    let cut = lines.len() > inside;
    let title = if cut {
        format!(
            " the tree: keys and marks ({}–{} of {}; up and down for more) ",
            app.help_top + 1,
            (app.help_top + inside).min(lines.len()),
            lines.len()
        )
    } else {
        " the tree: keys and marks ".to_owned()
    };
    let width = 62.min(area.width);
    let card = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    Clear.render_into(card, buf);
    Paragraph::new(lines.into_iter().skip(app.help_top).collect::<Vec<_>>())
        .block(Block::bordered().title(title))
        .render_into(card, buf);
}

/// `bytes` in base64 (RFC 4648, with padding): what OSC 52 carries.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}
