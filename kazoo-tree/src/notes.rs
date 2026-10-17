//! A batch's notes on a card over the tree (KZ-T13): its description and
//! attachments, read from its moment's commit (`notes/<seq>/<name>`) when
//! they are asked for, one page each.

use std::path::Path;

use crossterm::event::KeyCode;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Widget};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::history::{Batch, Moment, clean, file_at, file_size};

/// Spaces a tab in a note is shown as.
const TAB: &str = "    ";

/// The most bytes a note is read in: sixteen times the 64 KiB the wall
/// takes for one, so every note a wall wrote reads, and a commit holding
/// something far bigger under `notes/` can never fill the memory.
pub const MOST_BYTES: usize = 1 << 20;

/// The card's widest, in columns, borders included.
const CARD_WIDTH: u16 = 84;

/// What the card's title says of its keys, when there is room.
const HINT: &str = "←→ ↑↓, n closes";

/// The notes card: the batch's note as its title, and a page for each
/// note sent with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notes {
    /// The batch's note.
    pub title: String,
    /// Each note, in the order the wall wrote them (the description first).
    pub pages: Vec<Page>,
    /// The page shown.
    pub at: usize,
    /// The page's first line on screen, once wrapped.
    pub top: usize,
    /// The page shown, wrapped to the card as it was last drawn: worked
    /// out again only when the page or the card's width changes, never on
    /// every frame the fire draws.
    wrap: Option<Wrap>,
}

/// A page wrapped to a width.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Wrap {
    /// The page.
    at: usize,
    /// The columns inside the card.
    width: usize,
    /// Its rows.
    rows: Vec<Line<'static>>,
}

/// One note: its name, its size, and its lines, or why they could not be
/// read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// Its name, such as `proof.md`.
    pub name: String,
    /// Its size in bytes, as the wall wrote it.
    pub bytes: u64,
    /// Its lines as they may be shown ([`shown`]), or why it did not read.
    pub lines: Result<Vec<String>, String>,
}

/// What a key did to the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyed {
    /// The card stays open.
    Open,
    /// The card is closed.
    Closed,
}

impl Notes {
    /// A card of `pages` under `title`, at the first page's top.
    #[must_use]
    pub const fn new(title: String, pages: Vec<Page>) -> Self {
        Self {
            title,
            pages,
            at: 0,
            top: 0,
            wrap: None,
        }
    }

    /// The notes of `moment`, a batch's own moment, read from the tree in
    /// `dir`: each page that cannot be read says why on itself, and the
    /// others still show.
    ///
    /// # Errors
    ///
    /// Why there is nothing to read: the moment is not a batch, is one of a
    /// batch's parts (the notes are on its last change), carries no notes,
    /// or has no change number to find them under.
    pub fn read(dir: &Path, moment: &Moment) -> Result<Self, String> {
        let batch = moment
            .batch
            .as_deref()
            .ok_or_else(|| "this moment is not a batch, so it has no notes".to_owned())?;
        if let Some((part, count)) = batch.part.filter(|_| !batch.is_own()) {
            return Err(format!(
                "this is change {part} of the batch's {count}: its notes are on its last change"
            ));
        }
        if batch.notes.is_empty() {
            return Err("this batch was sent without notes".to_owned());
        }
        let seq = moment
            .seq
            .ok_or_else(|| "this batch's notes have no change number to be found by".to_owned())?;
        let pages = batch
            .notes
            .iter()
            .map(|note| Page {
                name: note.name.clone(),
                bytes: note.bytes,
                lines: page_text(dir, &moment.id, &format!("notes/{seq}/{}", note.name))
                    .map_err(|why| format!("{} {why}", note.name))
                    .map(|text| shown(&text)),
            })
            .collect();
        Ok(Self::new(batch.note.clone(), pages))
    }

    /// A key while the card is open.
    pub fn key(&mut self, code: KeyCode) -> Keyed {
        let pages = self.pages.len().max(1);
        match code {
            KeyCode::Esc | KeyCode::Char('n' | 'q') => return Keyed::Closed,
            KeyCode::Up | KeyCode::Char('k') => self.top = self.top.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.top = self.top.saturating_add(1),
            KeyCode::PageUp => self.top = self.top.saturating_sub(10),
            KeyCode::PageDown | KeyCode::Char(' ') => self.top = self.top.saturating_add(10),
            KeyCode::Home => self.top = 0,
            KeyCode::Right | KeyCode::Tab | KeyCode::Char('l') => {
                self.at = (self.at + 1) % pages;
                self.top = 0;
            }
            KeyCode::Left | KeyCode::BackTab | KeyCode::Char('h') => {
                self.at = (self.at + pages - 1) % pages;
                self.top = 0;
            }
            _ => {}
        }
        Keyed::Open
    }

    /// The page shown, wrapped to `width` columns (kept until the page or
    /// the width changes), or `None` when there is no such page.
    fn rows(&mut self, width: usize) -> Option<&[Line<'static>]> {
        let page = self.pages.get(self.at)?;
        let fresh = self
            .wrap
            .as_ref()
            .is_some_and(|wrap| wrap.at == self.at && wrap.width == width);
        if !fresh {
            let rows = match &page.lines {
                Ok(lines) => lines
                    .iter()
                    .flat_map(|line| wrapped(line, width))
                    .map(Line::raw)
                    .collect(),
                Err(why) => wrapped(why, width)
                    .into_iter()
                    .map(|row| Line::styled(row, Style::new().fg(Color::Red)))
                    .collect(),
            };
            self.wrap = Some(Wrap {
                at: self.at,
                width,
                rows,
            });
        }
        self.wrap.as_ref().map(|wrap| wrap.rows.as_slice())
    }
}

/// The text of `path` in `commit`, or why it cannot be shown: git could
/// not read it, it is bigger than [`MOST_BYTES`], or it is not UTF-8. The
/// reason is [`clean`]ed, since it carries git's own words.
fn page_text(dir: &Path, commit: &str, path: &str) -> Result<String, String> {
    let bytes = file_size(dir, commit, path)
        .map_err(|err| format!("could not be read: {}", clean(&err.to_string())))?;
    if bytes > MOST_BYTES as u64 {
        return Err(format!(
            "is {bytes} bytes, more than the {MOST_BYTES} a note may be"
        ));
    }
    let bytes = file_at(dir, commit, path)
        .map_err(|err| format!("could not be read: {}", clean(&err.to_string())))?;
    String::from_utf8(bytes).map_err(|err| format!("is not text: {err}"))
}

/// `text` as lines that may be shown, so nothing in it drives the terminal.
///
/// Split at its line feeds, a carriage return ending a line left off (a
/// line end written `\r\n`), each tab as four spaces, and each line
/// [`clean`]ed, so nothing in it can reorder the words either.
#[must_use]
pub fn shown(text: &str) -> Vec<String> {
    text.split('\n')
        .map(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            clean(&line.replace('\t', TAB))
        })
        .collect()
}

/// `line` broken into rows at most `width` columns wide.
///
/// A character as the terminal draws it (a letter with its accents, an
/// emoji of several, a flag) is never split. A row breaks at the last
/// space that fits when there is one, else wherever the row is full. A row
/// a break starts never begins with spaces, no row ends in one, and spaces
/// alone never make a row; an empty line is one empty row. A character
/// wider than `width` (two columns in one) is a row of its own.
#[must_use]
pub fn wrapped(line: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut used = 0;
    // Whether a break has been made: the next row starts at a word.
    let mut broken = false;
    // Where the row's last space ends, in bytes and in columns, once a
    // word is before it.
    let mut space: Option<(usize, usize)> = None;
    for g in line.graphemes(true) {
        let w = g.width();
        let blank = g == " ";
        if used + w > width && !row.is_empty() {
            if let (false, Some((at, columns))) = (blank, space) {
                let rest = row.split_off(at);
                end_row(&mut rows, &mut row);
                row = rest;
                used -= columns;
            } else {
                end_row(&mut rows, &mut row);
                used = 0;
            }
            broken = true;
            space = None;
        }
        if blank && row.is_empty() && broken {
            continue;
        }
        row.push_str(g);
        used += w;
        if blank && row.bytes().any(|b| b != b' ') {
            space = Some((row.len(), used));
        }
    }
    let last = row.trim_end_matches(' ');
    if !last.is_empty() || rows.is_empty() {
        rows.push(last.to_owned());
    }
    rows
}

/// End `row`: kept without its trailing spaces when anything is left, and
/// emptied either way.
fn end_row(rows: &mut Vec<String>, row: &mut String) {
    let kept = row.trim_end_matches(' ');
    if !kept.is_empty() {
        rows.push(kept.to_owned());
    }
    row.clear();
}

/// `text` held to `cols` columns: whole, or cut at a character's edge and
/// ending in `…`.
fn fit(text: &str, cols: usize) -> String {
    if text.width() <= cols {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for g in text.graphemes(true) {
        let w = g.width();
        if used + w + 1 > cols {
            break;
        }
        out.push_str(g);
        used += w;
    }
    if cols > 0 {
        out.push('…');
    }
    out
}

/// The card's title in `cols` columns: the page and where it is always,
/// then the keys when they fit, then the batch's note, cut to what is
/// left.
fn title_of(note: &str, page: &str, place: &str, cols: usize) -> String {
    let with_hint = format!(" {page} │ {place} │ {HINT} ");
    let base = if with_hint.width() <= cols {
        with_hint
    } else {
        format!(" {page} │ {place} ")
    };
    if base.width() > cols {
        return fit(&base, cols);
    }
    // ` <note> │` before it, with a column of the note at least.
    let room = cols - base.width();
    if room < 4 || note.is_empty() {
        return base;
    }
    format!(" {} │{base}", fit(note, room - 3))
}

/// The page names in `cols` columns, from the first when they fit, else
/// from as far back as fits with the page shown, after `‹`.
fn tabs(notes: &Notes, cols: usize) -> Line<'static> {
    let widths: Vec<usize> = notes
        .pages
        .iter()
        .map(|page| clean(&page.name).width() + 3)
        .collect();
    let from = |budget: usize| {
        let mut from = notes.at.min(widths.len().saturating_sub(1));
        let mut used = widths.get(from).copied().unwrap_or(0);
        while from > 0 && used + widths[from - 1] <= budget {
            from -= 1;
            used += widths[from];
        }
        from
    };
    let mut start = from(cols);
    let mut spans = Vec::new();
    if start > 0 {
        start = from(cols.saturating_sub(2));
        spans.push(Span::styled("‹ ", Style::new().fg(Color::Yellow)));
    }
    for (i, page) in notes.pages.iter().enumerate().skip(start) {
        let style = if i == notes.at {
            Style::new()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::new().fg(Color::Yellow)
        };
        spans.push(Span::styled(format!(" {} ", clean(&page.name)), style));
        spans.push(Span::raw(" "));
    }
    Line::from(spans)
}

/// Draw the card over `area`, its page wrapped to the card, scrolled to
/// `notes.top` (held to the page's length).
pub fn draw(notes: &mut Notes, area: Rect, buf: &mut Buffer) {
    let width = CARD_WIDTH.min(area.width);
    let inside = usize::from(width.saturating_sub(2));
    let height = area.height;
    let rows_on_screen = usize::from(height.saturating_sub(3));
    let asked = notes.top;
    let Some(rows) = notes.rows(inside) else {
        return;
    };
    let count = rows.len();
    let top = asked.min(count.saturating_sub(rows_on_screen));
    let page_rows: Vec<Line<'static>> = rows
        .iter()
        .skip(top)
        .take(rows_on_screen)
        .cloned()
        .collect();
    notes.top = top;
    let Some(page) = notes.pages.get(notes.at) else {
        return;
    };
    let shown_to = (top + rows_on_screen).min(count);
    let place = if shown_to > top {
        format!("{}–{shown_to} of {count}", top + 1)
    } else {
        format!("none of {count}")
    };
    let title = title_of(
        &clean(&notes.title),
        &format!("{} ({})", clean(&page.name), size(page.bytes)),
        &place,
        inside,
    );
    let mut lines = vec![tabs(notes, inside)];
    lines.extend(page_rows);
    let card = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y,
        width,
        height,
    };
    Clear.render(card, buf);
    Paragraph::new(lines)
        .block(Block::bordered().title(title))
        .render(card, buf);
}

/// The details panel's line naming `batch`'s notes, when it has any.
#[must_use]
pub fn listed(batch: &Batch) -> Option<(String, String)> {
    if batch.notes.is_empty() {
        return None;
    }
    let names: Vec<String> = batch
        .notes
        .iter()
        .map(|note| format!("{} ({})", note.name, size(note.bytes)))
        .collect();
    Some((
        "notes".to_owned(),
        format!("{}; n to read", names.join(", ")),
    ))
}

/// A note's size as people say it: bytes below a kilobyte, else KB.
#[must_use]
pub fn size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

#[cfg(test)]
mod tests;
