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
    Notes {
        title: "proof of funk".to_owned(),
        pages: (0..pages)
            .map(|i| Page {
                name: format!("page{i}.md"),
                bytes: 10,
                lines: Ok((0..40).map(|line| format!("line {line}")).collect()),
            })
            .collect(),
        at: 0,
        top: 0,
    }
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
