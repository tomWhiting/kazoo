//! Saving the wall and the saves list: the keys, the requests they queue,
//! what the answers and events show, and the screen.

use std::time::{Duration, Instant};

use crossterm::event::KeyCode;

use crate::protocol::{
    BrokenSave, Change, DeleteSaveResult, ErrorCode, Event, LoadResult, Request, SaveInfo,
    SaveMark, SaveResult, SavesResult, What,
};

use super::super::app::{App, Mode, Tone};
use super::super::worker::{Failure, Job, Reply};
use super::render::{render, text};
use super::{answer, connected, press, snapshot, type_text};

fn calls(app: &mut App) -> Vec<Request> {
    app.take_jobs()
        .into_iter()
        .map(|job| match job {
            Job::Call(request) => request,
            Job::Launch => panic!("a key started the wall unasked"),
            Job::Wall(wall) => panic!("a key told the worker of wall {wall}"),
        })
        .collect()
}

fn status(app: &App) -> String {
    app.status(Instant::now())
        .map(|status| status.text.clone())
        .unwrap_or_default()
}

fn info(name: &str, saved_at: &str, auto: bool) -> SaveInfo {
    SaveInfo {
        name: name.to_string(),
        saved_at: saved_at.to_string(),
        tempo: 96.0,
        modules: 12,
        cables: 30,
        auto,
        version: 2,
    }
}

/// Two saves made by hand and one of the wall's own, newest first, and a
/// file that cannot be loaded.
fn listed() -> SavesResult {
    SavesResult {
        saves: vec![
            info("tom-fiddle-2026-09-29", "2026-09-29T02:00:00Z", false),
            info("before-load-20260927-201500", "2026-09-27T10:15:00Z", true),
            info("garage-korgy-2026-09-27", "2026-09-27T09:00:00Z", false),
        ],
        broken: vec![BrokenSave {
            name: "torn".to_string(),
            saved_at: "2026-09-26T12:00:00Z".to_string(),
            problem: "it is not JSON: EOF while parsing".to_string(),
        }],
        pending: Vec::new(),
    }
}

/// A console with the saves list open and filled.
fn with_the_list() -> App {
    let mut app = connected();
    press(&mut app, KeyCode::Char('o'));
    assert_eq!(calls(&mut app), vec![Request::Saves]);
    assert_eq!(*app.mode(), Mode::Saves { index: 0 });
    answer(&mut app, Request::Saves, &listed());
    app
}

/// The load of `save` (none: an empty wall) as change 80.
fn load_change(save: Option<&str>, undoes: Option<u64>) -> Change {
    Change {
        seq: 80,
        at: "2026-09-26T12:00:01Z".to_string(),
        seat: "Tom".to_string(),
        what: What::Load {
            save: save.map(str::to_string),
            backup: "before-load-20260926-220001".to_string(),
            modules: 12,
            cables: 30,
            tempo: 96.0,
            fingerprint: "0123456789abcdef-2048".to_string(),
            by_console: true,
        },
        summary: "Tom loaded the save".to_string(),
        undoes,
        clock: None,
    }
}

#[test]
fn w_names_a_save_and_asks_before_saving_over_one() {
    let mut app = connected();
    press(&mut app, KeyCode::Char('w'));
    let Mode::Save { text } = app.mode().clone() else {
        panic!("w did not ask for a name: {:?}", app.mode());
    };
    assert!(text.starts_with("wall-20"), "a dated name to start: {text}");
    for _ in 0..text.len() {
        press(&mut app, KeyCode::Backspace);
    }
    // Only what a save's name may hold is taken.
    type_text(&mut app, "jam/!");
    assert_eq!(
        *app.mode(),
        Mode::Save {
            text: "jam".to_string()
        }
    );
    let screen = text_of(&mut app);
    assert!(screen.contains("save the wall as: jam█"), "{screen}");
    press(&mut app, KeyCode::Enter);
    let asked = Request::Save {
        name: "jam".to_string(),
        overwrite: false,
    };
    assert_eq!(calls(&mut app), vec![asked.clone()]);
    assert_eq!(*app.mode(), Mode::Normal);
    // A save of that name is there: asked before it is replaced.
    app.on_reply(
        Reply::Answer {
            request: asked,
            outcome: Err(Failure {
                code: Some(ErrorCode::SaveExists),
                message: "a save named \"jam\" is there already".to_string(),
            }),
        },
        Instant::now(),
    );
    assert_eq!(
        *app.mode(),
        Mode::Overwrite {
            name: "jam".to_string()
        }
    );
    let screen = text_of(&mut app);
    assert!(
        screen.contains("A save named \"jam\" is there already."),
        "{screen}"
    );
    press(&mut app, KeyCode::Char('y'));
    let over = Request::Save {
        name: "jam".to_string(),
        overwrite: true,
    };
    assert_eq!(calls(&mut app), vec![over.clone()]);
    answer(
        &mut app,
        over,
        &SaveResult {
            save: info("jam", "2026-09-26T12:00:01Z", false),
            replaced: true,
            notes: Vec::new(),
        },
    );
    assert_eq!(status(&app), "saved over \"jam\" (o lists the saves)");
    assert_eq!(app.snapshot().unwrap().save.as_ref().unwrap().name, "jam");
    // Declining goes back to the name, to choose another.
    app.on_reply(
        Reply::Answer {
            request: Request::Save {
                name: "jam".to_string(),
                overwrite: false,
            },
            outcome: Err(Failure {
                code: Some(ErrorCode::SaveExists),
                message: "there already".to_string(),
            }),
        },
        Instant::now(),
    );
    press(&mut app, KeyCode::Char('n'));
    assert_eq!(
        *app.mode(),
        Mode::Save {
            text: "jam".to_string()
        }
    );
    assert!(calls(&mut app).is_empty());
    // The wall's own names are refused here, before they are sent.
    for _ in 0..3 {
        press(&mut app, KeyCode::Backspace);
    }
    type_text(&mut app, "before-load-1");
    press(&mut app, KeyCode::Enter);
    assert!(calls(&mut app).is_empty());
    assert!(matches!(app.mode(), Mode::Save { .. }));
    assert_eq!(app.status(Instant::now()).unwrap().tone, Tone::Trouble);
    press(&mut app, KeyCode::Esc);
    assert_eq!(*app.mode(), Mode::Normal);
    assert_eq!(status(&app), "nothing was saved");
}

#[test]
fn o_lists_the_saves_and_loads_one_after_asking() {
    let mut app = with_the_list();
    let screen = text_of(&mut app);
    for expected in [
        "the saves · 2",
        "tom-fiddle-2026-09-29",
        "garage-korgy-2026-09-27",
        "and 1 of the wall's own (the wall as it was before each load) · a shows them",
        "96 BPM",
        "12 · 30",
        "CANNOT BE LOADED",
        "torn — it is not JSON",
        "⏎ then y loads it",
    ] {
        assert!(screen.contains(expected), "{expected} missing:\n{screen}");
    }
    assert!(
        !screen.contains("before-load-20260927-201500"),
        "the wall's own are kept out of the way:\n{screen}"
    );
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    assert_eq!(*app.mode(), Mode::Saves { index: 1 }, "held to the saves");
    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::Enter);
    let asking = Mode::LoadSave {
        name: "tom-fiddle-2026-09-29".to_string(),
        index: 0,
    };
    assert_eq!(*app.mode(), asking);
    let screen = text_of(&mut app);
    assert!(
        screen.contains("Load \"tom-fiddle-2026-09-29\" onto the wall?"),
        "{screen}"
    );
    assert!(screen.contains("and z undoes the load."), "{screen}");
    // Any other key goes back to the list, loading nothing: Enter too,
    // so a second Enter never loads by accident.
    for other in [KeyCode::Char('n'), KeyCode::Enter] {
        press(&mut app, other);
        assert_eq!(*app.mode(), Mode::Saves { index: 0 });
        assert!(calls(&mut app).is_empty());
        press(&mut app, KeyCode::Enter);
    }
    press(&mut app, KeyCode::Char('y'));
    let load = Request::Load {
        name: "tom-fiddle-2026-09-29".to_string(),
    };
    assert_eq!(calls(&mut app), vec![load.clone()]);
    assert_eq!(*app.mode(), Mode::Normal);
    answer(
        &mut app,
        load,
        &LoadResult {
            change: load_change(Some("tom-fiddle-2026-09-29"), None),
            backup: "before-load-20260926-220001".to_string(),
            notes: vec!["the desk keeps the tempo at 120 BPM".to_string()],
        },
    );
    assert_eq!(
        status(&app),
        "loaded \"tom-fiddle-2026-09-29\" · the wall as it was is saved as \
         \"before-load-20260926-220001\" · z undoes it"
    );
    let log: Vec<String> = app.log().lines().map(|line| line.text.clone()).collect();
    assert!(log.contains(&"Tom loaded the save".to_string()), "{log:?}");
    assert!(
        log.contains(&"the desk keeps the tempo at 120 BPM".to_string()),
        "{log:?}"
    );
    // z undoes the load: it is the latest change.
    press(&mut app, KeyCode::Char('z'));
    assert_eq!(calls(&mut app), vec![Request::Undo { change: 80 }]);
}

#[test]
fn the_list_deletes_after_asking_and_starts_an_empty_wall() {
    let mut app = with_the_list();
    press(&mut app, KeyCode::Char('d'));
    assert_eq!(
        *app.mode(),
        Mode::DeleteSave {
            name: "tom-fiddle-2026-09-29".to_string(),
            index: 0
        }
    );
    let screen = text_of(&mut app);
    assert!(
        screen.contains("Delete the save \"tom-fiddle-2026-09-29\" for good?"),
        "{screen}"
    );
    // Enter is not enough to delete: only y.
    press(&mut app, KeyCode::Enter);
    assert!(calls(&mut app).is_empty());
    assert_eq!(*app.mode(), Mode::Saves { index: 0 });
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Char('y'));
    let delete = Request::DeleteSave {
        name: "tom-fiddle-2026-09-29".to_string(),
    };
    assert_eq!(calls(&mut app), vec![delete.clone()]);
    assert_eq!(*app.mode(), Mode::Saves { index: 0 });
    answer(
        &mut app,
        delete,
        &DeleteSaveResult {
            name: "tom-fiddle-2026-09-29".to_string(),
            notes: Vec::new(),
        },
    );
    assert_eq!(status(&app), "deleted the save \"tom-fiddle-2026-09-29\"");
    // The open list is read again, a moment after it was last read.
    assert!(calls(&mut app).is_empty());
    app.tick(Instant::now() + Duration::from_secs(2));
    assert_eq!(calls(&mut app), vec![Request::Saves]);
    press(&mut app, KeyCode::Char('n'));
    assert_eq!(*app.mode(), Mode::Clear { index: 0 });
    press(&mut app, KeyCode::Char('y'));
    assert_eq!(calls(&mut app), vec![Request::New]);
    answer(
        &mut app,
        Request::New,
        &LoadResult {
            change: load_change(None, None),
            backup: "before-new-20260926-220001".to_string(),
            notes: Vec::new(),
        },
    );
    assert!(
        status(&app).starts_with("cleared the wall"),
        "{}",
        status(&app)
    );
    // Esc closes the list.
    press(&mut app, KeyCode::Char('o'));
    assert_eq!(calls(&mut app), vec![Request::Saves]);
    press(&mut app, KeyCode::Esc);
    assert_eq!(*app.mode(), Mode::Normal);
}

#[test]
fn another_seats_load_is_told_and_the_old_wall_let_go() {
    let mut app = connected();
    // A turn gathered on the old wall...
    press(&mut app, KeyCode::Char('='));
    // ...and a patch under way.
    press(&mut app, KeyCode::Char('p'));
    assert!(matches!(app.mode(), Mode::Patch(_)));
    app.on_reply(
        Reply::Event(Event::Loaded {
            seq: 80,
            seat: "Waffles".to_string(),
            save: Some("garage-korgy-2026-09-29".to_string()),
            backup: "before-load-20260926-220001".to_string(),
        }),
        Instant::now(),
    );
    assert_eq!(*app.mode(), Mode::Normal);
    // The turn is not sent to the new wall.
    app.tick(Instant::now() + std::time::Duration::from_secs(1));
    assert!(calls(&mut app).is_empty());
    assert_eq!(
        status(&app),
        "Waffles loaded \"garage-korgy-2026-09-29\"; the wall as it was is saved as \
         \"before-load-20260926-220001\""
    );
    // Saves made and deleted by others are told in the log.
    app.on_reply(
        Reply::Event(Event::Saved {
            seat: "Waffles".to_string(),
            name: "wobbly".to_string(),
            replaced: false,
        }),
        Instant::now(),
    );
    let latest = app.log().lines().next().unwrap().text.clone();
    assert_eq!(latest, "Waffles saved the wall as \"wobbly\"");
}

#[test]
fn a_shows_the_walls_own_saves_after_toms() {
    let mut app = with_the_list();
    press(&mut app, KeyCode::Char('a'));
    let screen = text_of(&mut app);
    assert!(
        screen.contains("THE WALL'S OWN: THE WALL AS IT WAS BEFORE EACH LOAD"),
        "{screen}"
    );
    assert!(screen.contains("before-load-20260927-201500"), "{screen}");
    // Tom's first, then the wall's own: the third is the wall's.
    press(&mut app, KeyCode::End);
    assert_eq!(*app.mode(), Mode::Saves { index: 2 });
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        *app.mode(),
        Mode::LoadSave {
            name: "before-load-20260927-201500".to_string(),
            index: 2
        }
    );
    // Hidden again, the cursor is held to what is shown.
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('a'));
    assert_eq!(*app.mode(), Mode::Saves { index: 1 });
    assert!(!text_of(&mut app).contains("before-load-20260927-201500"));
}

#[test]
fn a_refused_listing_is_asked_again_a_moment_later() {
    let mut app = connected();
    let opened = Instant::now();
    press(&mut app, KeyCode::Char('o'));
    assert_eq!(calls(&mut app), vec![Request::Saves]);
    app.on_reply(
        Reply::Answer {
            request: Request::Saves,
            outcome: Err(Failure {
                code: Some(ErrorCode::SlowDown),
                message: "Tom has asked for the saves too often; wait a moment".to_string(),
            }),
        },
        opened,
    );
    let screen = text_of(&mut app);
    assert!(
        screen.contains("the saves could not be read: Tom has asked for the saves too often"),
        "{screen}"
    );
    // Not at once (that is what was refused), but a moment later.
    app.tick(opened);
    assert!(calls(&mut app).is_empty());
    app.tick(opened + Duration::from_secs(2));
    assert_eq!(calls(&mut app), vec![Request::Saves]);
    answer(&mut app, Request::Saves, &listed());
    assert!(app.saves().problem.is_none());
}

#[test]
fn a_storm_of_saves_is_read_once_a_second_and_a_load_reads_the_list_again() {
    let mut app = with_the_list();
    let opened = Instant::now();
    let saved = |n: usize| {
        Reply::Event(Event::Saved {
            seat: "Waffles".to_string(),
            name: format!("take {n}"),
            replaced: false,
        })
    };
    for n in 0..30 {
        app.on_reply(saved(n), opened);
        app.tick(opened);
    }
    assert!(calls(&mut app).is_empty(), "not before a second has passed");
    let later = opened + Duration::from_millis(1_100);
    app.tick(later);
    app.tick(later);
    assert_eq!(calls(&mut app), vec![Request::Saves], "one, for all thirty");
    // While that is on its way, nothing more is asked.
    app.on_reply(saved(31), later);
    app.tick(later + Duration::from_secs(5));
    assert!(calls(&mut app).is_empty());
    answer(&mut app, Request::Saves, &listed());
    app.tick(later + Duration::from_secs(5));
    assert_eq!(calls(&mut app), vec![Request::Saves], "what came meanwhile");
    answer(&mut app, Request::Saves, &listed());
    // Another seat's load wrote the wall as it was to a save: the open
    // list is read again.
    app.on_reply(
        Reply::Event(Event::Loaded {
            seq: 90,
            seat: "Waffles".to_string(),
            save: Some("garage-korgy-2026-09-27".to_string()),
            backup: "before-load-20261009-143005".to_string(),
        }),
        later,
    );
    app.tick(later + Duration::from_secs(10));
    assert_eq!(calls(&mut app), vec![Request::Saves]);
    // A closed list is not read at all.
    answer(&mut app, Request::Saves, &listed());
    press(&mut app, KeyCode::Esc);
    app.on_reply(saved(40), later);
    app.tick(later + Duration::from_secs(20));
    assert!(calls(&mut app).is_empty());
}

#[test]
fn the_header_says_which_save_the_wall_came_from() {
    let mut app = connected();
    let mut marked = snapshot();
    marked.save = Some(SaveMark {
        name: "tom-fiddle-2026-09-29".to_string(),
        revision: marked.revision,
        changed: false,
    });
    app.on_reply(Reply::Snapshot(Box::new(marked.clone())), Instant::now());
    let screen = text_of(&mut app);
    assert!(screen.contains("save: tom-fiddle-2026-09-29"), "{screen}");
    assert!(!screen.contains("changed since"));
    // A later revision alone (the wall's own notes) is not a change.
    marked.revision += 1;
    app.on_reply(Reply::Snapshot(Box::new(marked.clone())), Instant::now());
    assert!(!text_of(&mut app).contains("changed since"));
    // The wall says when it changed.
    if let Some(mark) = marked.save.as_mut() {
        mark.changed = true;
    }
    app.on_reply(Reply::Snapshot(Box::new(marked)), Instant::now());
    let screen = text_of(&mut app);
    assert!(
        screen.contains("save: tom-fiddle-2026-09-29 · changed since"),
        "{screen}"
    );
}

#[test]
fn the_keys_are_in_the_help() {
    let mut app = connected();
    press(&mut app, KeyCode::Char('?'));
    let mut screen = String::new();
    for _ in 0..8 {
        screen.push_str(&text_of(&mut app));
        press(&mut app, KeyCode::PageDown);
    }
    assert!(
        screen.contains("save the wall as it is under a name"),
        "{screen}"
    );
    assert!(screen.contains("the saves: load one"), "{screen}");
}

/// The screen, as text, at a size that fits the saves list.
fn text_of(app: &mut App) -> String {
    text(&render(app, 140, 40))
}
