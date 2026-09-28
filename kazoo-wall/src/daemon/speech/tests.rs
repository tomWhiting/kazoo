//! Speech tests: words checked before any render, and renders that finish
//! in any order, fail, or find the queue full all end where they should.

use std::sync::{Arc, Mutex};

use kazoo_speech::SpeechPlayer;
use kazoo_speech::tts::Render;

use super::*;

/// What the stand-in renderer has been asked, and what it has finished.
#[derive(Debug, Default)]
pub struct Script {
    /// How many renders may wait at once.
    pub room: usize,
    /// Renders asked for and not finished, by the renderer's ticket.
    pub queued: Vec<(u64, RenderRequest)>,
    done: VecDeque<Finished>,
    next: u64,
    /// The renderer has stopped.
    pub gone: bool,
    /// Renderers started.
    pub starts: usize,
}

impl Script {
    /// Finish the render of `text` (it must be queued), with a phrase
    /// 100 samples long per character, at the rate it was asked for.
    pub fn finish(&mut self, text: &str, cache_warning: Option<SpeechError>) {
        let at = self
            .queued
            .iter()
            .position(|(_, request)| request.text == text)
            .unwrap_or_else(|| panic!("{text:?} was never asked for"));
        let (ticket, request) = self.queued.remove(at);
        let phrase = Phrase::new(vec![0.1; text.len() * 100], request.sample_rate);
        self.done.push_back(Finished {
            ticket,
            request,
            result: Ok(Render {
                phrase,
                from_cache: false,
                cache_warning,
            }),
        });
    }

    /// Finish every queued render, oldest first.
    pub fn finish_all(&mut self) {
        let texts: Vec<String> = self
            .queued
            .iter()
            .map(|(_, request)| request.text.clone())
            .collect();
        for text in texts {
            self.finish(&text, None);
        }
    }

    /// The texts asked for and not finished, oldest first.
    #[must_use]
    pub fn texts(&self) -> Vec<&str> {
        self.queued
            .iter()
            .map(|(_, request)| request.text.as_str())
            .collect()
    }
}

/// A stand-in renderer run by a [`Script`].
#[derive(Debug)]
struct Fake(Arc<Mutex<Script>>);

impl Renderer for Fake {
    fn submit(&mut self, request: RenderRequest) -> Result<u64, SpeechError> {
        let mut script = self.0.lock().unwrap();
        if script.gone {
            return Err(SpeechError::WorkerGone);
        }
        if script.queued.len() >= script.room {
            return Err(SpeechError::Busy);
        }
        script.next += 1;
        let ticket = script.next;
        script.queued.push((ticket, request));
        drop(script);
        Ok(ticket)
    }

    fn try_finished(&mut self) -> Result<Option<Finished>, SpeechError> {
        let mut script = self.0.lock().unwrap();
        if script.gone && script.done.is_empty() {
            return Err(SpeechError::WorkerGone);
        }
        Ok(script.done.pop_front())
    }
}

/// Speech rendered by a stand-in with room for `room` renders. A new
/// stand-in, started after one stops, numbers its renders from 1 again as
/// a restarted speech thread does.
#[must_use]
pub fn scripted(room: usize) -> (Speech, Arc<Mutex<Script>>) {
    let script = Arc::new(Mutex::new(Script {
        room,
        ..Script::default()
    }));
    let shared = Arc::clone(&script);
    let speech = Speech::with_renderer(
        std::env::temp_dir(),
        Box::new(move |_| {
            let mut script = shared.lock().unwrap();
            script.starts += 1;
            script.gone = false;
            script.next = 0;
            drop(script);
            Ok(Box::new(Fake(Arc::clone(&shared))) as Box<dyn Renderer>)
        }),
    );
    (speech, script)
}

fn words(text: &str) -> Words {
    Words {
        text: text.to_string(),
        voice: None,
    }
}

/// A player and its feed; the player must be kept for the feed to work.
fn player() -> (SpeechPlayer, PhraseFeed) {
    SpeechPlayer::new(48_000.0)
}

fn said(speech: &Speech, module: &str) -> Option<usize> {
    speech
        .phrase(module)
        .map(|phrase| phrase.samples().len() / 100)
}

#[test]
fn bad_words_and_voices_are_refused_before_any_render() {
    let dir = tempfile::tempdir().unwrap();
    let mut speech = Speech::new(dir.path().to_path_buf());
    let refused = speech
        .submit("speak1", words("   "), 48_000, None)
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::BadRequest);
    let long = "a".repeat(kazoo_speech::tts::MAX_TEXT_CHARS + 1);
    let refused = speech
        .submit("speak1", words(&long), 48_000, None)
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::BadRequest);
    let evil = Words {
        text: "hello".to_string(),
        voice: Some("-v evil".to_string()),
    };
    let refused = speech.submit("speak1", evil, 48_000, None).unwrap_err();
    assert_eq!(refused.code, ErrorCode::BadRequest);
    assert_eq!(speech.in_flight(), 0);
    assert!(speech.finished().is_empty());
}

#[test]
fn errors_map_to_protocol_codes() {
    assert_eq!(speech_error(&SpeechError::Busy).code, ErrorCode::SlowDown);
    assert_eq!(
        speech_error(&SpeechError::UnknownVoice("Nobody".to_string())).code,
        ErrorCode::BadRequest
    );
    assert_eq!(
        speech_error(&SpeechError::WorkerGone).code,
        ErrorCode::Internal
    );
}

#[test]
fn saved_words_put_back_never_replace_newer_words() {
    let (mut speech, script) = scripted(16);
    let (_player, feed) = player();
    speech.attach("speak1", feed);
    // A seat asks for new words; before they are ready the engine is
    // rebuilt and the saved (older) words would go back.
    speech
        .submit("speak1", words("newer words"), 48_000, Some("Tom".into()))
        .unwrap();
    speech.put_back("speak1", words("old"), 48_000);
    assert_eq!(script.lock().unwrap().texts(), ["newer words"]);
    script.lock().unwrap().finish_all();
    let spoken = speech.finished();
    assert_eq!(spoken.len(), 1);
    assert_eq!(said(&speech, "speak1"), Some("newer words".len()));

    // Put back first, then new words; the put-back finishing last must not
    // win.
    speech.put_back("speak1", words("old"), 48_000);
    speech
        .submit("speak1", words("newest"), 48_000, Some("Tom".into()))
        .unwrap();
    script.lock().unwrap().finish("newest", None);
    script.lock().unwrap().finish("old", None);
    let spoken = speech.finished();
    assert_eq!(spoken.len(), 2);
    assert!(spoken[0].result.is_ok());
    assert!(spoken[1].result.is_err(), "{:?}", spoken[1].result);
    assert_eq!(said(&speech, "speak1"), Some("newest".len()));

    // Two seats' words finishing out of order: the later asked wins.
    speech
        .submit("speak1", words("first"), 48_000, Some("Tom".into()))
        .unwrap();
    speech
        .submit("speak1", words("second one"), 48_000, Some("Ava".into()))
        .unwrap();
    script.lock().unwrap().finish("second one", None);
    script.lock().unwrap().finish("first", None);
    let spoken = speech.finished();
    assert!(spoken[0].result.is_ok());
    assert_eq!(
        spoken[1].result.as_ref().unwrap_err().code,
        ErrorCode::NotAllowed
    );
    assert_eq!(said(&speech, "speak1"), Some("second one".len()));
}

#[test]
fn a_speaker_keeps_its_phrase_through_a_rate_change() {
    let (mut speech, script) = scripted(16);
    let (_player, feed) = player();
    speech.attach("speak1", feed);
    speech
        .submit("speak1", words("hello"), 48_000, Some("Tom".into()))
        .unwrap();
    script.lock().unwrap().finish_all();
    speech.finished();
    // The engine is rebuilt at 44.1 kHz: the 48 kHz phrase goes straight
    // back (the player resamples), and nothing needs rendering.
    let (_player, feed) = SpeechPlayer::new(44_100.0);
    assert!(!speech.attach("speak1", feed));
    assert!(script.lock().unwrap().queued.is_empty());
    assert_eq!(said(&speech, "speak1"), Some("hello".len()));
}

#[test]
fn saved_words_go_back_one_at_a_time_and_all_arrive() {
    let (mut speech, script) = scripted(16);
    let mut players = Vec::new();
    for index in 1..=20 {
        let (player, feed) = player();
        players.push(player);
        speech.attach(&format!("speak{index}"), feed);
        speech.put_back(
            &format!("speak{index}"),
            words(&format!("words {index}")),
            48_000,
        );
    }
    assert_eq!(speech.in_flight(), 1);
    assert_eq!(speech.waiting(), 19);
    for _ in 0..20 {
        script.lock().unwrap().finish_all();
        speech.finished();
    }
    assert_eq!(speech.in_flight(), 0);
    assert_eq!(speech.waiting(), 0);
    // Every speaker is still attached to hear its words.
    assert_eq!(players.len(), 20);
    for index in 1..=20 {
        let module = format!("speak{index}");
        assert_eq!(said(&speech, &module), Some(format!("words {index}").len()));
    }
}

#[test]
fn a_seats_words_wait_behind_one_render_at_most() {
    let (mut speech, script) = scripted(16);
    let (_one, feed) = player();
    speech.attach("speak1", feed);
    let (_two, feed) = player();
    speech.attach("speak2", feed);
    // Saved words going back, and a seat's words behind them: fine.
    speech.put_back("speak1", words("saved"), 48_000);
    speech
        .submit("speak2", words("first"), 48_000, Some("Tom".into()))
        .unwrap();
    // A third would wait behind two.
    let refused = speech
        .submit("speak2", words("second"), 48_000, Some("Ava".into()))
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::SlowDown);
    // More saved words wait, and do not go ahead of the seat's.
    speech.put_back("speak2", words("saved too"), 48_000);
    assert_eq!(script.lock().unwrap().texts(), ["saved", "first"]);
}

#[test]
fn saved_words_waiting_when_the_renderer_is_busy_go_later() {
    let (mut speech, script) = scripted(0);
    let (_player, feed) = player();
    speech.attach("speak1", feed);
    speech.put_back("speak1", words("saved"), 48_000);
    assert_eq!(speech.waiting(), 1);
    script.lock().unwrap().room = 16;
    speech.finished();
    assert_eq!(script.lock().unwrap().texts(), ["saved"]);
}

#[test]
fn a_waiting_put_back_for_a_removed_speaker_is_dropped() {
    let (mut speech, script) = scripted(16);
    let (_one, feed) = player();
    speech.attach("speak1", feed);
    let (_two, feed) = player();
    speech.attach("speak2", feed);
    speech.put_back("speak1", words("one"), 48_000);
    speech.put_back("speak2", words("two"), 48_000);
    assert_eq!(speech.waiting(), 1);
    speech.forget("speak2");
    assert_eq!(speech.waiting(), 0);
    script.lock().unwrap().finish_all();
    speech.finished();
    assert!(script.lock().unwrap().queued.is_empty());
}

#[test]
fn any_engine_rate_can_speak() {
    let (mut speech, script) = scripted(16);
    let (_player, feed) = player();
    speech.attach("speak1", feed);
    speech
        .submit("speak1", words("high"), 384_000, None)
        .unwrap();
    speech.submit("speak1", words("low"), 4_000, None).unwrap();
    let rates: Vec<u32> = script
        .lock()
        .unwrap()
        .queued
        .iter()
        .map(|(_, request)| request.sample_rate)
        .collect();
    assert_eq!(rates, [MAX_SAMPLE_RATE, MIN_SAMPLE_RATE]);
}

#[test]
fn each_cache_warning_is_told_once() {
    let (mut speech, script) = scripted(16);
    let (_player, feed) = player();
    speech.attach("speak1", feed);
    let full = || SpeechError::Io {
        doing: "writing the speech cache",
        source: std::io::Error::other("disk full"),
    };
    for text in ["one", "two"] {
        speech.submit("speak1", words(text), 48_000, None).unwrap();
        script.lock().unwrap().finish(text, Some(full()));
    }
    speech.finished();
    assert_eq!(speech.warned.len(), 1);
    speech
        .submit("speak1", words("three"), 48_000, None)
        .unwrap();
    script
        .lock()
        .unwrap()
        .finish("three", Some(SpeechError::NoHome));
    speech.finished();
    assert_eq!(speech.warned.len(), 2);
}

#[test]
fn renders_lost_with_the_renderer_fail_and_tickets_stay_unique() {
    let (mut speech, script) = scripted(16);
    let (_player, feed) = player();
    speech.attach("speak1", feed);
    let first = speech
        .submit("speak1", words("lost"), 48_000, Some("Tom".into()))
        .unwrap();
    script.lock().unwrap().gone = true;
    let refused = speech
        .submit("speak1", words("refused"), 48_000, Some("Tom".into()))
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::Internal);
    // The render that was in flight is answered, as a failure.
    let spoken = speech.finished();
    assert_eq!(spoken.len(), 1);
    assert_eq!(spoken[0].ticket, first);
    assert!(spoken[0].result.is_err());
    assert_eq!(speech.in_flight(), 0);
    // A new renderer numbers its renders from 1 again; the tickets given
    // out never repeat.
    let second = speech
        .submit("speak1", words("again"), 48_000, Some("Tom".into()))
        .unwrap();
    assert_ne!(second, first);
    assert_eq!(script.lock().unwrap().starts, 2);
    script.lock().unwrap().finish_all();
    let spoken = speech.finished();
    assert_eq!(spoken.len(), 1);
    assert_eq!(spoken[0].ticket, second);
    assert!(spoken[0].result.is_ok());
}

#[test]
fn a_renderer_that_stops_while_rendering_fails_everything_in_flight() {
    let (mut speech, script) = scripted(16);
    let (_player, feed) = player();
    speech.attach("speak1", feed);
    let one = speech.submit("speak1", words("one"), 48_000, None).unwrap();
    let two = speech
        .submit("speak1", words("two"), 48_000, Some("Tom".into()))
        .unwrap();
    script.lock().unwrap().gone = true;
    let spoken = speech.finished();
    let tickets: Vec<u64> = spoken.iter().map(|spoken| spoken.ticket).collect();
    assert_eq!(tickets, [one, two]);
    assert!(spoken.iter().all(|spoken| spoken.result.is_err()));
}
