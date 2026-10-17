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
use unicode_width::UnicodeWidthChar;

use crate::history::{Batch, Moment, clean, file_at};

/// Spaces a tab in a note is shown as.
const TAB: &str = "    ";

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
    /// The notes of `moment`, a batch's own moment, read from the tree in
    /// `dir`: each page that cannot be read says why on itself, and the
    /// others still show.
    ///
    /// # Errors
    ///
    /// Why there is nothing to read: the moment is not a batch, carries no
    /// notes, or has no change number to find them under.
    pub fn read(dir: &Path, moment: &Moment) -> Result<Self, String> {
        let batch = moment
            .batch
            .as_deref()
            .ok_or_else(|| "this moment is not a batch, so it has no notes".to_owned())?;
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
                lines: file_at(dir, &moment.id, &format!("notes/{seq}/{}", note.name))
                    .map_err(|err| format!("{} could not be read: {err}", note.name))
                    .and_then(|bytes| {
                        String::from_utf8(bytes)
                            .map_err(|err| format!("{} is not text: {err}", note.name))
                    })
                    .map(|text| shown(&text)),
            })
            .collect();
        Ok(Self {
            title: batch.note.clone(),
            pages,
            at: 0,
            top: 0,
        })
    }

    /// A key while the card is open.
    pub fn key(&mut self, code: KeyCode) -> Keyed {
        let pages = self.pages.len().max(1);
        match code {
            KeyCode::Esc | KeyCode::Char('n' | 'q') => return Keyed::Closed,
            KeyCode::Up | KeyCode::Char('k') => self.top = self.top.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.top += 1,
            KeyCode::PageUp => self.top = self.top.saturating_sub(10),
            KeyCode::PageDown | KeyCode::Char(' ') => self.top += 10,
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
}

/// `text` as lines that may be shown: split at its line feeds, each tab as
/// four spaces, and each line [`clean`]ed, so nothing in a note can drive
/// the terminal or reorder the words.
#[must_use]
pub fn shown(text: &str) -> Vec<String> {
    text.split('\n')
        .map(|line| clean(&line.replace('\t', TAB)))
        .collect()
}

/// `line` broken into rows at most `width` columns wide: at the last space
/// that fits when there is one, else wherever the row is full. An empty
/// line is one empty row.
#[must_use]
pub fn wrapped(line: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut used = 0;
    // Where the row's last space ends, in bytes and in columns.
    let mut space: Option<(usize, usize)> = None;
    for c in line.chars() {
        let w = c.width().unwrap_or(0);
        // A space where the row is full ends the row, and goes.
        if c == ' ' && used + w > width {
            rows.push(row.trim_end().to_owned());
            row.clear();
            used = 0;
            space = None;
            continue;
        }
        if used + w > width && !row.is_empty() {
            if let Some((at, columns)) = space.filter(|&(at, _)| at < row.len()) {
                let rest = row.split_off(at);
                rows.push(row.trim_end().to_owned());
                row = rest;
                used -= columns;
            } else {
                rows.push(std::mem::take(&mut row));
                used = 0;
            }
            space = None;
        }
        row.push(c);
        used += w;
        if c == ' ' {
            space = Some((row.len(), used));
        }
    }
    rows.push(row.trim_end().to_owned());
    rows
}

/// Draw the card over `area`, its page wrapped to the card, scrolled to
/// `notes.top` (held to the page's length).
pub fn draw(notes: &mut Notes, area: Rect, buf: &mut Buffer) {
    let width = 84.min(area.width);
    let inside = usize::from(width.saturating_sub(2));
    let height = area.height;
    let rows_on_screen = usize::from(height.saturating_sub(3));
    let Some(page) = notes.pages.get(notes.at) else {
        return;
    };
    let rows: Vec<Line> = match &page.lines {
        Ok(lines) => lines
            .iter()
            .flat_map(|line| wrapped(line, inside))
            .map(Line::raw)
            .collect(),
        Err(why) => wrapped(why, inside)
            .into_iter()
            .map(|row| Line::styled(row, Style::new().fg(Color::Red)))
            .collect(),
    };
    notes.top = notes.top.min(rows.len().saturating_sub(rows_on_screen));
    let tabs: Vec<Span> = notes
        .pages
        .iter()
        .enumerate()
        .flat_map(|(i, page)| {
            let style = if i == notes.at {
                Style::new()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::new().fg(Color::Yellow)
            };
            [
                Span::styled(format!(" {} ", page.name), style),
                Span::raw(" "),
            ]
        })
        .collect();
    let mut lines = vec![Line::from(tabs)];
    lines.extend(rows.iter().skip(notes.top).take(rows_on_screen).cloned());
    let shown_to = (notes.top + rows_on_screen).min(rows.len());
    let title = format!(
        " {} │ {} ({}) │ {}–{} of {} │ ←→ ↑↓, n closes ",
        clean(&notes.title),
        page.name,
        size(page.bytes),
        (notes.top + 1).min(rows.len()),
        shown_to,
        rows.len()
    );
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
