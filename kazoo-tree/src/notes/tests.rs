use std::fmt::Write as _;
use std::io::Write as _;
use std::process::{Command, Stdio};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::*;
use crate::history::{History, Note};

/// A tree planted in a temporary folder holding one module's change and
/// then a batch whose commit is `message`, with `files` beside its module.
fn tree_with_batch(message: &str, files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temporary folder");
    kazoo_wall::tree::git::plant(dir.path()).expect("planted");
    let first = "Tom added vco1\n\nKazoo-Kind: change\nKazoo-Seq: 1\n";
    let module = "kind = \"vco\"\nname = \"a\"\n\n[knobs]\n";
    let mut stream = format!(
        "commit refs/heads/trunk\nmark :1\nauthor Tom <tom@kazoo.invalid> 1000 +0000\ncommitter Tom <tom@kazoo.invalid> 1000 +0000\ndata {}\n{first}\nM 100644 inline modules/vco1.toml\ndata {}\n{module}\n\n",
        first.len(),
        module.len()
    );
    write!(
        stream,
        "commit refs/heads/trunk\nmark :2\nauthor Cally <cally@kazoo.invalid> 1002 +0000\ncommitter Cally <cally@kazoo.invalid> 1002 +0000\ndata {}\n{message}\nfrom :1\n",
        message.len()
    )
    .expect("a String takes it");
    for (path, body) in files {
        write!(
            stream,
            "M 100644 inline {path}\ndata {}\n{body}\n",
            body.len()
        )
        .expect("a String takes it");
    }
    stream.push('\n');
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["fast-import", "--quiet"])
        .stdin(Stdio::piped())
        .spawn()
        .expect("fast-import runs");
    child
        .stdin
        .take()
        .expect("its input")
        .write_all(stream.as_bytes())
        .expect("the stream is written");
    assert!(child.wait().expect("fast-import ends").success());
    dir
}

const PROOF: &str = "Theorem. The groove is unsettling.\n\tProof: by funk.\n\u{1b}[31mQ.E.D.";

fn batch_message(attachments: &str) -> String {
    format!(
        "Cally: proof of funk\n\nThe groove, proved.\n\nKazoo-Kind: batch\nKazoo-Seq: 2\nKazoo-Note: proof of funk\nKazoo-Step: turned vcf1 cutoff 400 Hz → 800 Hz\n{attachments}Kazoo-Change: {{\"seq\":2}}\n"
    )
}

#[test]
fn a_batch_reads_its_notes_names_and_sizes_and_the_card_reads_their_texts() {
    let message =
        batch_message("Kazoo-Attachment: description.md (19)\nKazoo-Attachment: proof.md (63)\n");
    let dir = tree_with_batch(
        &message,
        &[
            ("notes/2/description.md", "The groove, proved."),
            ("notes/2/proof.md", PROOF),
        ],
    );
    let history = History::read(dir.path()).expect("it reads");
    let moment = &history.moments[1];
    assert!(moment.problems.is_empty(), "{:?}", moment.problems);
    let batch = moment.batch.as_deref().expect("a batch");
    assert_eq!(
        batch.notes,
        [
            Note {
                name: "description.md".to_owned(),
                bytes: 19
            },
            Note {
                name: "proof.md".to_owned(),
                bytes: 63
            }
        ]
    );
    let notes = Notes::read(dir.path(), moment).expect("the notes read");
    assert_eq!(notes.title, "proof of funk");
    assert_eq!(notes.pages.len(), 2);
    assert_eq!(
        notes.pages[0].lines,
        Ok(vec!["The groove, proved.".to_owned()])
    );
    // A tab is four spaces, and an escape can never reach the terminal.
    assert_eq!(
        notes.pages[1].lines,
        Ok(vec![
            "Theorem. The groove is unsettling.".to_owned(),
            "    Proof: by funk.".to_owned(),
            "\u{fffd}[31mQ.E.D.".to_owned(),
        ])
    );
}

#[test]
fn a_note_missing_from_its_commit_says_so_on_its_page_and_the_others_still_read() {
    let message =
        batch_message("Kazoo-Attachment: description.md (19)\nKazoo-Attachment: proof.md (63)\n");
    let dir = tree_with_batch(
        &message,
        &[("notes/2/description.md", "The groove, proved.")],
    );
    let history = History::read(dir.path()).expect("it reads");
    let notes = Notes::read(dir.path(), &history.moments[1]).expect("the notes read");
    assert_eq!(
        notes.pages[0].lines,
        Ok(vec!["The groove, proved.".to_owned()])
    );
    let why = notes.pages[1]
        .lines
        .as_ref()
        .expect_err("proof.md is missing");
    assert!(why.starts_with("proof.md could not be read"), "{why}");
}

#[test]
fn a_moment_without_notes_says_why_there_is_nothing_to_read() {
    let dir = tree_with_batch(&batch_message(""), &[]);
    let history = History::read(dir.path()).expect("it reads");
    assert_eq!(
        Notes::read(dir.path(), &history.moments[1]),
        Err("this batch was sent without notes".to_owned())
    );
    assert_eq!(
        Notes::read(dir.path(), &history.moments[0]),
        Err("this moment is not a batch, so it has no notes".to_owned())
    );
}

#[test]
fn a_bad_or_misplaced_attachment_trailer_is_a_problem_said() {
    let message = batch_message(
        "Kazoo-Attachment: proof.md\nKazoo-Attachment: ../escape (4)\nKazoo-Attachment: Proof.md (3)\nKazoo-Attachment: proof.md (3)\n",
    );
    let dir = tree_with_batch(&message, &[]);
    let history = History::read(dir.path()).expect("it reads");
    let moment = &history.moments[1];
    let batch = moment.batch.as_deref().expect("a batch");
    // Only the first of a name, in any case, is read.
    assert_eq!(
        batch.notes,
        [Note {
            name: "Proof.md".to_owned(),
            bytes: 3
        }]
    );
    let problems = moment.problems.join("\n");
    assert!(
        problems.contains("does not read as name (bytes): proof.md"),
        "{problems}"
    );
    assert!(
        problems.contains("names a note no wall would: ../escape"),
        "{problems}"
    );
    assert!(
        problems.contains("the note proof.md is listed more than once"),
        "{problems}"
    );
    let single = "Tom turned vcf1 cutoff\n\nKazoo-Kind: change\nKazoo-Seq: 2\nKazoo-Attachment: proof.md (3)\n";
    let dir = tree_with_batch(single, &[]);
    let history = History::read(dir.path()).expect("it reads");
    assert!(
        history.moments[1]
            .problems
            .iter()
            .any(|problem| problem == "a note on a moment that is not a batch: proof.md"),
        "{:?}",
        history.moments[1].problems
    );
}

fn card(pages: usize) -> Notes {
    Notes::new(
        "proof of funk".to_owned(),
        (0..pages)
            .map(|i| Page {
                name: format!("page{i}.md"),
                bytes: 10,
                lines: Ok((0..40).map(|line| format!("line {line}")).collect()),
            })
            .collect(),
    )
}

#[test]
fn the_card_pages_round_scrolls_and_closes() {
    let mut notes = card(3);
    assert_eq!(notes.key(KeyCode::Down), Keyed::Open);
    assert_eq!(notes.key(KeyCode::PageDown), Keyed::Open);
    assert_eq!(notes.top, 11);
    assert_eq!(notes.key(KeyCode::Up), Keyed::Open);
    assert_eq!(notes.top, 10);
    // A new page starts at its top.
    notes.key(KeyCode::Right);
    assert_eq!((notes.at, notes.top), (1, 0));
    notes.key(KeyCode::Left);
    notes.key(KeyCode::Left);
    assert_eq!(notes.at, 2);
    notes.key(KeyCode::Tab);
    assert_eq!(notes.at, 0);
    for close in [KeyCode::Esc, KeyCode::Char('n'), KeyCode::Char('q')] {
        assert_eq!(card(1).key(close), Keyed::Closed);
    }
}

#[test]
fn the_card_shows_its_page_held_to_its_length() {
    let mut notes = card(2);
    notes.top = 1_000;
    let area = Rect::new(0, 0, 100, 20);
    let mut buf = Buffer::empty(area);
    draw(&mut notes, area, &mut buf);
    // Twenty rows: two borders and the page names leave seventeen lines.
    assert_eq!(notes.top, 40 - 17);
    let text: String = (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                + "\n"
        })
        .collect();
    assert!(
        text.contains("proof of funk │ page0.md (10 B) │ 24–40 of 40 │ ←→ ↑↓, n closes"),
        "{text}"
    );
    assert!(text.contains(" page1.md "), "{text}");
    assert!(text.contains("line 39"), "{text}");
    assert!(!text.contains("line 22"), "{text}");
}

#[test]
fn a_line_wraps_at_its_last_space_or_where_it_is_full() {
    assert_eq!(wrapped("", 10), [""]);
    assert_eq!(
        wrapped("the groove is proved", 10),
        ["the groove", "is proved"]
    );
    assert_eq!(wrapped("abcdefghijkl", 5), ["abcde", "fghij", "kl"]);
    // A wide character takes two columns, and is never split.
    assert_eq!(wrapped("音楽音楽音", 4), ["音楽", "音楽", "音"]);
    assert_eq!(wrapped("x", 0), ["x"]);
}

#[test]
fn sizes_are_said_as_people_say_them() {
    assert_eq!(size(0), "0 B");
    assert_eq!(size(1023), "1023 B");
    assert_eq!(size(1024), "1 KB");
    assert_eq!(size(12_000), "12 KB");
}

#[test]
fn n_opens_the_selected_batchs_notes_and_the_details_name_them() {
    let message = batch_message("Kazoo-Attachment: proof.md (63)\n");
    let dir = tree_with_batch(&message, &[("notes/2/proof.md", PROOF)]);
    let mut app = crate::app::App::open(dir.path().to_path_buf(), crate::ledger::Ledger::default())
        .expect("the tree opens");
    app.selected = Some(1);
    assert!(
        app.details()
            .contains(&("notes".to_owned(), "proof.md (63 B); n to read".to_owned())),
        "{:?}",
        app.details()
    );
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    app.key(key(KeyCode::Char('n')));
    assert_eq!(app.notes.as_ref().map(|notes| notes.pages.len()), Some(1));
    // While the card is open, the keys are its own: Down scrolls, never
    // walks the tree.
    app.key(key(KeyCode::Down));
    assert_eq!(app.selected, Some(1));
    app.key(key(KeyCode::Esc));
    assert_eq!(app.notes, None);
    assert!(!app.quit, "Esc closed the card, not the tree");
    app.selected = Some(0);
    app.key(key(KeyCode::Char('n')));
    assert_eq!(app.notes, None);
    assert_eq!(app.said, "this moment is not a batch, so it has no notes");
}

#[test]
fn a_run_of_spaces_or_an_indent_never_wraps_into_an_empty_row() {
    // Two spaces where the row is full: the second never makes a row of
    // its own.
    assert_eq!(wrapped("aaaa  bbbb", 4), ["aaaa", "bbbb"]);
    // An indent wider than the card is not a row of nothing.
    assert_eq!(wrapped("        longword", 6), ["longwo", "rd"]);
    // A line of spaces is one empty row, however wide.
    assert_eq!(wrapped("     ", 3), [""]);
    // A row broken where it is full keeps no space at its end.
    assert_eq!(wrapped("abc 音", 4), ["abc", "音"]);
}

#[test]
fn a_line_wraps_whole_characters_as_people_see_them() {
    // A singer (👩 + joiner + 🎤) is one character two columns wide.
    assert_eq!(wrapped("a👩\u{200d}🎤", 3), ["a👩\u{200d}🎤"]);
    // A flag is two letters that are one picture, never split.
    assert_eq!(wrapped("🇦🇺🇦🇺", 3), ["🇦🇺", "🇦🇺"]);
    // An accent stays on its letter.
    assert_eq!(wrapped("abe\u{301}", 2), ["ab", "e\u{301}"]);
}

#[test]
fn a_line_ending_in_a_carriage_return_shows_no_mark_for_it() {
    assert_eq!(shown("one\r\ntwo\r\n"), ["one", "two", ""]);
    // A carriage return inside a line is still never sent to the terminal.
    assert_eq!(shown("a\rb"), ["a\u{fffd}b"]);
}

fn text_of(buf: &Buffer) -> Vec<String> {
    let area = buf.area;
    (0..area.height)
        .map(|y| (0..area.width).map(|x| buf[(x, y)].symbol()).collect())
        .collect()
}

#[test]
fn a_long_note_is_cut_so_the_title_still_says_where_the_page_is() {
    let mut notes = card(1);
    notes.title = "a".repeat(200);
    let area = Rect::new(0, 0, 100, 20);
    let mut buf = Buffer::empty(area);
    draw(&mut notes, area, &mut buf);
    let top = &text_of(&buf)[0];
    assert!(
        top.contains("a… │ page0.md (10 B) │ 1–17 of 40 │ ←→ ↑↓, n closes "),
        "{top}"
    );
}

#[test]
fn a_card_with_no_room_for_a_line_says_none_is_shown() {
    let mut notes = card(1);
    let area = Rect::new(0, 0, 100, 3);
    let mut buf = Buffer::empty(area);
    draw(&mut notes, area, &mut buf);
    let top = &text_of(&buf)[0];
    assert!(top.contains("none of 40"), "{top}");
}

#[test]
fn the_page_shown_is_always_among_the_names_however_many() {
    let mut notes = card(9);
    for (i, page) in notes.pages.iter_mut().enumerate() {
        page.name = format!("{i}{}", "x".repeat(40));
    }
    notes.at = 8;
    let area = Rect::new(0, 0, 100, 20);
    let mut buf = Buffer::empty(area);
    draw(&mut notes, area, &mut buf);
    let names = &text_of(&buf)[1];
    assert!(names.contains(&format!(" 8{} ", "x".repeat(40))), "{names}");
    // A title that cannot hold all of it keeps the page and its place.
    let top = &text_of(&buf)[0];
    assert!(top.contains("1–17 of 40"), "{top}");
}

#[test]
fn a_trailers_size_is_digits_only() {
    let message = batch_message("Kazoo-Attachment: proof.md (+5)\n");
    let dir = tree_with_batch(&message, &[]);
    let history = History::read(dir.path()).expect("it reads");
    let moment = &history.moments[1];
    assert_eq!(moment.batch.as_deref().map(|b| b.notes.len()), Some(0));
    assert!(
        moment
            .problems
            .iter()
            .any(|p| p.contains("does not read as name (bytes): proof.md (+5)")),
        "{:?}",
        moment.problems
    );
}

#[test]
fn a_note_on_a_part_is_said_and_the_part_points_to_its_batch() {
    let message = "Cally: proof of funk\n\nKazoo-Kind: part\nKazoo-Seq: 2\nKazoo-Note: proof of funk\nKazoo-Part: 1/2\nKazoo-Step: turned vcf1 cutoff 400 Hz → 800 Hz\nKazoo-Attachment: proof.md (3)\n";
    let dir = tree_with_batch(message, &[("notes/2/proof.md", "abc")]);
    let history = History::read(dir.path()).expect("it reads");
    let moment = &history.moments[1];
    assert!(
        moment
            .problems
            .iter()
            .any(|p| p == "a note on a batch's part, not on the batch's own moment: proof.md"),
        "{:?}",
        moment.problems
    );
    assert_eq!(
        Notes::read(dir.path(), moment),
        Err("this is change 1 of the batch's 2: its notes are on its last change".to_owned())
    );
}

#[test]
fn a_note_too_big_to_be_one_the_wall_wrote_is_not_read() {
    let big = "x".repeat(MOST_BYTES + 1);
    let message = batch_message("Kazoo-Attachment: proof.md (3)\n");
    let dir = tree_with_batch(&message, &[("notes/2/proof.md", &big)]);
    let history = History::read(dir.path()).expect("it reads");
    let notes = Notes::read(dir.path(), &history.moments[1]).expect("the notes read");
    let why = notes.pages[0].lines.as_ref().expect_err("too big");
    assert_eq!(
        why,
        &format!(
            "proof.md is {} bytes, more than the {MOST_BYTES} a note may be",
            MOST_BYTES + 1
        )
    );
}

#[test]
fn ctrl_c_quits_with_the_card_open() {
    let message = batch_message("Kazoo-Attachment: proof.md (63)\n");
    let dir = tree_with_batch(&message, &[("notes/2/proof.md", PROOF)]);
    let mut app = crate::app::App::open(dir.path().to_path_buf(), crate::ledger::Ledger::default())
        .expect("the tree opens");
    app.selected = Some(1);
    app.key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    assert!(app.notes.is_some());
    app.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(app.quit, "Ctrl-C did not quit with the card open");
}

#[test]
fn the_wheel_scrolls_the_card_and_a_click_never_reaches_the_tree() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let message = batch_message("Kazoo-Attachment: proof.md (63)\n");
    let dir = tree_with_batch(&message, &[("notes/2/proof.md", PROOF)]);
    let mut app = crate::app::App::open(dir.path().to_path_buf(), crate::ledger::Ledger::default())
        .expect("the tree opens");
    app.selected = Some(1);
    app.key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    let at = |kind| MouseEvent {
        kind,
        column: 1,
        row: 1,
        modifiers: KeyModifiers::NONE,
    };
    app.mouse(at(MouseEventKind::ScrollDown));
    assert_eq!(app.notes.as_ref().map(|n| n.top), Some(3));
    let view = app.view;
    app.mouse(at(MouseEventKind::Down(MouseButton::Left)));
    assert_eq!((app.selected, app.view), (Some(1), view));
    assert!(app.notes.is_some());
}

#[test]
fn the_card_is_drawn_over_every_frame_the_fire_draws_too() {
    let message = batch_message("Kazoo-Attachment: proof.md (63)\n");
    let dir = tree_with_batch(&message, &[("notes/2/proof.md", PROOF)]);
    let mut app = crate::app::App::open(dir.path().to_path_buf(), crate::ledger::Ledger::default())
        .expect("the tree opens");
    app.selected = Some(1);
    app.key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    let area = Rect::new(0, 0, 140, 40);
    let mut drew_reused = false;
    for _ in 0..3 {
        let mut buf = Buffer::empty(area);
        crate::frame_of(&mut app, area, &mut buf);
        let text = text_of(&buf).join("\n");
        assert!(
            text.contains("Theorem. The groove is unsettling."),
            "{text}"
        );
        drew_reused |= app.fire.reused();
        app.fire.fire_only();
    }
    assert!(drew_reused, "no frame took the fire's kept chrome");
}
