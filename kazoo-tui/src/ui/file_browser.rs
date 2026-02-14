//! Modal file browser overlay for loading audio files onto tracks.
//!
//! The file browser appears as a centered popup when the user presses `o`.
//! It lists directories and audio files in the current directory, allows
//! navigation with j/k, entering directories with Enter, going up with
//! Backspace, and loading audio files with Enter.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};

use crate::app::{App, AppMode};
use crate::theme;

/// Draw the file browser overlay if the app is in `FileBrowser` mode.
///
/// Renders a centered popup over the existing UI with directory contents.
pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let AppMode::FileBrowser {
        ref directory,
        ref entries,
        selected,
    } = app.mode
    else {
        return;
    };

    let popup = super::centered_rect(70, 80, area);
    frame.render_widget(Clear, popup);

    let dir_display = directory.display().to_string();
    let title = format!(" Load Audio -- {dir_display} ");

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_style(theme::style_help_key())
        .style(theme::style_help_bg());

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if entries.is_empty() {
        let empty = Paragraph::new("  (empty directory)").style(theme::style_text_dimmed());
        frame.render_widget(empty, inner);
        return;
    }

    // Calculate scroll offset to keep selection visible.
    let visible_rows = inner.height as usize;
    let scroll_offset = if visible_rows == 0 {
        0
    } else if selected >= visible_rows {
        selected - visible_rows + 1
    } else {
        0
    };

    // Build list items (only the visible slice to avoid needless allocation).
    let visible_items: Vec<ListItem<'_>> = entries
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(visible_rows)
        .map(|(i, entry)| {
            let (icon, style) = if entry.is_dir {
                ("\u{1F4C1} ", theme::style_text())
            } else {
                ("\u{266B} ", Style::new().fg(theme::ACCENT_PLAY))
            };

            let line = if i == selected {
                Line::from(vec![
                    Span::styled("> ", theme::style_help_key()),
                    Span::styled(icon, style),
                    Span::styled(&entry.name, style.add_modifier(Modifier::BOLD)),
                ])
            } else {
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(icon, style),
                    Span::styled(&entry.name, style),
                ])
            };

            ListItem::new(line)
        })
        .collect();

    let list = List::new(visible_items);
    frame.render_widget(list, inner);

    // Draw footer with navigation hints.
    if popup.height > 2 {
        let footer_area = Rect::new(
            popup.x + 1,
            popup.y + popup.height - 1,
            popup.width.saturating_sub(2),
            1,
        );
        let footer = Paragraph::new(" j/k:nav  Enter:open/load  Backspace:up  Esc:close ")
            .style(theme::style_text_dimmed());
        frame.render_widget(footer, footer_area);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::FileBrowserEntry;

    #[test]
    fn centered_rect_produces_valid_rect() {
        let area = Rect::new(0, 0, 100, 50);
        let popup = crate::ui::centered_rect(60, 70, area);
        assert!(popup.width > 0);
        assert!(popup.height > 0);
        assert!(popup.x > 0);
        assert!(popup.y > 0);
        assert!(popup.right() <= area.right());
        assert!(popup.bottom() <= area.bottom());
    }

    #[test]
    fn centered_rect_full_size() {
        let area = Rect::new(0, 0, 80, 40);
        let popup = crate::ui::centered_rect(100, 100, area);
        assert_eq!(popup.width, area.width);
        assert_eq!(popup.height, area.height);
    }

    #[test]
    fn file_browser_entry_debug() {
        let entry = FileBrowserEntry {
            name: "dir".into(),
            path: std::path::PathBuf::from("/tmp/dir"),
            is_dir: true,
        };
        let dbg = format!("{entry:?}");
        assert!(dbg.contains("dir"));
    }
}
