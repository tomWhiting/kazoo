use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::testing::Scratch;

/// A stand-in for `say`: a shell script in `dir` running `body`, with the
/// arguments `say` would get.
fn fake_say(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("fake-say");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write script");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("chmod");
    path
}

/// A 32-bit float mono WAVE of a 300 Hz tone at `rate`.
fn tone_wav(path: &Path, rate: u32) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("create wav");
    for n in 0..rate / 2 {
        let t = n as f32 / rate as f32;
        writer
            .write_sample(0.3 * (std::f32::consts::TAU * 300.0 * t).sin())
            .expect("write sample");
    }
    writer.finalize().expect("finalise wav");
}

fn tts_with(scratch: &Scratch, say: PathBuf) -> Tts {
    let mut config = TtsConfig::with_cache_dir(scratch.0.join("cache"));
    config.say_path = say;
    config.timeout = Duration::from_secs(10);
    Tts::new(config).expect("tts")
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|x| x * x).sum::<f32>() / samples.len().max(1) as f32).sqrt()
}

#[test]
fn text_is_cleaned_and_capped() {
    assert_eq!(
        clean_text("  hello\nthere\tyou\u{7}\u{1b}[31m  ").expect("clean"),
        "hello there you  [31m"
    );
    assert!(matches!(clean_text(""), Err(SpeechError::EmptyText)));
    assert!(matches!(
        clean_text(" \n\t\r "),
        Err(SpeechError::EmptyText)
    ));
    let most = "é".repeat(MAX_TEXT_CHARS);
    assert_eq!(clean_text(&most).expect("at the limit"), most);
    assert!(matches!(
        clean_text(&format!("{most}a")),
        Err(SpeechError::TextTooLong { chars }) if chars == MAX_TEXT_CHARS + 1
    ));
    // Leading dashes are just words: the text never becomes an argument.
    assert_eq!(clean_text("-v Evil -o /x").expect("clean"), "-v Evil -o /x");
}

#[test]
fn voices_that_could_be_options_are_refused() {
    assert!(check_voice("Daniel").is_ok());
    assert!(check_voice("Eddy (English (UK))").is_ok());
    for bad in [
        "",
        "   ",
        "-o",
        "--data-format=x",
        "Dan\niel",
        &"x".repeat(65),
    ] {
        assert!(
            matches!(check_voice(bad), Err(SpeechError::BadVoice(_))),
            "{bad:?}"
        );
    }
}

#[test]
fn arguments_never_carry_the_text() {
    let text_file = Path::new("/cache/tmp-1-1.txt");
    let out_file = Path::new("/cache/tmp-1-1.wav");
    let arguments = render_arguments(text_file, out_file, Some("Moira"), 44_100, Some(180));
    let expected: Vec<&OsStr> = [
        "-o",
        "/cache/tmp-1-1.wav",
        "--file-format=WAVE",
        "--data-format=LEF32@44100",
        "-f",
        "/cache/tmp-1-1.txt",
        "-v",
        "Moira",
        "-r",
        "180",
    ]
    .iter()
    .map(OsStr::new)
    .collect();
    assert_eq!(arguments, expected);
    let plain = render_arguments(text_file, out_file, None, 48_000, None);
    assert_eq!(plain.len(), 6);
}

#[test]
fn requests_are_checked_before_anything_runs() {
    let scratch = Scratch::new("tts-checks");
    // A `say` that would leave a mark if it ever ran.
    let mark = scratch.0.join("ran");
    let say = fake_say(&scratch.0, &format!("touch '{}'", mark.display()));
    let mut tts = tts_with(&scratch, say);
    let cases = [
        (RenderRequest::new("", 48_000), "empty"),
        (RenderRequest::new("x".repeat(501), 48_000), "long"),
        (RenderRequest::new("hi", 7_999), "rate"),
        (RenderRequest::new("hi", 192_001), "rate"),
        (
            RenderRequest::new("hi", 48_000).with_words_per_minute(10),
            "pace",
        ),
        (RenderRequest::new("hi", 48_000).with_voice("-o"), "voice"),
    ];
    for (request, why) in cases {
        assert!(tts.render(&request).is_err(), "{why}");
    }
    assert!(!mark.exists());
}

#[test]
fn relative_paths_are_refused_and_the_cache_is_private() {
    let scratch = Scratch::new("tts-dirs");
    let relative = TtsConfig::with_cache_dir(PathBuf::from("speech"));
    assert!(matches!(
        Tts::new(relative),
        Err(SpeechError::NotAbsolute(_))
    ));
    let tts = tts_with(&scratch, PathBuf::from("/nonexistent/say"));
    let mode = fs::metadata(&tts.config().cache_dir)
        .expect("cache dir")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700);
}

#[test]
fn without_say_renders_fail_plainly_but_the_cache_still_serves() {
    let scratch = Scratch::new("tts-missing");
    let mut tts = tts_with(&scratch, PathBuf::from("/nonexistent/say"));
    assert!(!tts.available());
    let request = RenderRequest::new("hello", 48_000);
    let error = tts.render(&request).expect_err("no say");
    assert!(matches!(error, SpeechError::SayMissing(_)));
    assert!(error.to_string().contains("needs macOS `say`"), "{error}");
    assert!(matches!(tts.voices(), Err(SpeechError::SayMissing(_))));
    // A render already in the cache needs no `say`.
    let mut cache = Cache::new(scratch.0.join("cache"), DEFAULT_CACHE_CAP);
    cache
        .put(&CacheKey::new("hello", "", 48_000, 0), 48_000, &[0.1, 0.2])
        .expect("put");
    let render = tts.render(&request).expect("cached");
    assert!(render.from_cache);
    assert_eq!(render.phrase.samples(), [0.1, 0.2]);
}

#[test]
fn a_render_goes_through_the_file_and_into_the_cache() {
    let scratch = Scratch::new("tts-fake");
    let wav = scratch.0.join("tone.wav");
    tone_wav(&wav, 22_050);
    let log = scratch.0.join("args");
    // `say -o OUT ... -f TEXT`: copy the tone to OUT and log what came in.
    let say = fake_say(
        &scratch.0,
        &format!(
            "printf '%s\\n' \"$@\" > '{log}'\ncat \"$6\" >> '{log}'\ncp '{wav}' \"$2\"",
            log = log.display(),
            wav = wav.display()
        ),
    );
    let mut tts = tts_with(&scratch, say);
    let request = RenderRequest::new("-o /tmp/owned hello", 22_050);
    let render = tts.render(&request).expect("render");
    assert!(!render.from_cache);
    assert!(render.cache_warning.is_none());
    assert_eq!(render.phrase.sample_rate(), 22_050);
    assert_eq!(render.phrase.samples().len(), 11_025);
    assert!(rms(render.phrase.samples()) > 0.1);
    let logged = fs::read_to_string(&log).expect("log");
    let mut lines = logged.lines();
    assert_eq!(lines.next(), Some("-o"));
    assert!(
        lines
            .next()
            .is_some_and(|out| Path::new(out).extension() == Some(OsStr::new("wav")))
    );
    // The text arrived through the file, whole, and only there.
    assert_eq!(logged.matches("-o /tmp/owned hello").count(), 1);
    assert!(logged.ends_with("-o /tmp/owned hello"));
    // Again: from the cache, and the temporary files are gone.
    fs::remove_file(&log).expect("remove log");
    let again = tts.render(&request).expect("render");
    assert!(again.from_cache);
    assert!(!log.exists());
    assert_eq!(again.phrase, render.phrase);
    let leftovers = fs::read_dir(&tts.config().cache_dir).expect("list").count();
    assert_eq!(leftovers, 1);
}

#[test]
fn a_render_at_the_wrong_rate_is_refused() {
    let scratch = Scratch::new("tts-rate");
    let wav = scratch.0.join("tone.wav");
    tone_wav(&wav, 22_050);
    let say = fake_say(&scratch.0, &format!("cp '{}' \"$2\"", wav.display()));
    let mut tts = tts_with(&scratch, say);
    let error = tts
        .render(&RenderRequest::new("hello", 48_000))
        .expect_err("wrong rate");
    assert!(matches!(error, SpeechError::BadRender(_)), "{error}");
}

#[test]
fn a_failing_say_reports_its_words() {
    let scratch = Scratch::new("tts-fail");
    let say = fake_say(&scratch.0, "echo 'Voice not available' >&2\nexit 3");
    let mut tts = tts_with(&scratch, say);
    let error = tts
        .render(&RenderRequest::new("hello", 48_000))
        .expect_err("fails");
    assert!(matches!(error, SpeechError::SayFailed { .. }));
    assert!(error.to_string().contains("Voice not available"), "{error}");
}

#[test]
fn a_hung_say_is_killed() {
    let scratch = Scratch::new("tts-hang");
    let say = fake_say(&scratch.0, "exec sleep 30");
    let mut config = TtsConfig::with_cache_dir(scratch.0.join("cache"));
    config.say_path = say;
    config.timeout = Duration::from_millis(200);
    let mut tts = Tts::new(config).expect("tts");
    let started = Instant::now();
    let error = tts
        .render(&RenderRequest::new("hello", 48_000))
        .expect_err("hangs");
    assert!(matches!(error, SpeechError::TimedOut(_)), "{error}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn stale_temporary_files_are_swept() {
    let scratch = Scratch::new("tts-sweep");
    let cache = scratch.0.join("cache");
    fs::create_dir_all(&cache).expect("dir");
    let stale = cache.join("tmp-1-1.wav");
    let fresh = cache.join("tmp-1-2.wav");
    let kept = cache.join("0000000000000000.kzs");
    for path in [&stale, &fresh, &kept] {
        fs::write(path, b"x").expect("write");
    }
    let old = SystemTime::now() - Duration::from_secs(7_200);
    fs::File::options()
        .write(true)
        .open(&stale)
        .expect("open")
        .set_modified(old)
        .expect("age");
    fs::File::options()
        .write(true)
        .open(&kept)
        .expect("open")
        .set_modified(old)
        .expect("age");
    tts_with(&scratch, PathBuf::from("/nonexistent/say"));
    assert!(!stale.exists());
    assert!(fresh.exists());
    assert!(kept.exists());
}

#[test]
fn the_worker_renders_in_order_without_blocking() {
    let scratch = Scratch::new("tts-worker");
    let wav = scratch.0.join("tone.wav");
    tone_wav(&wav, 22_050);
    let say = fake_say(
        &scratch.0,
        &format!("sleep 0.05\ncp '{}' \"$2\"", wav.display()),
    );
    let mut worker = TtsWorker::spawn(tts_with(&scratch, say)).expect("spawn");
    let first = worker
        .submit(RenderRequest::new("one", 22_050))
        .expect("submit");
    let second = worker
        .submit(RenderRequest::new("", 22_050))
        .expect("submit");
    assert!(worker.try_finished().expect("alive").is_none());
    let done = worker
        .wait_finished(Duration::from_secs(10))
        .expect("alive")
        .expect("finished");
    assert_eq!(done.ticket, first);
    assert!(done.result.is_ok());
    let done = worker
        .wait_finished(Duration::from_secs(10))
        .expect("alive")
        .expect("finished");
    assert_eq!(done.ticket, second);
    assert!(matches!(done.result, Err(SpeechError::EmptyText)));
    worker.shutdown().expect("shutdown");
}

/// The real thing, where there is a real `say`.
#[test]
fn say_really_speaks() {
    if !Path::new(SAY_PATH).is_file() {
        eprintln!("skipping say_really_speaks: {SAY_PATH} is not on this machine");
        return;
    }
    let scratch = Scratch::new("tts-real");
    let mut tts = tts_with(&scratch, PathBuf::from(SAY_PATH));
    let voices = tts.voices().expect("voices").to_vec();
    assert!(!voices.is_empty());
    let request = RenderRequest::new("-v nobody. Hello from the wall.", 22_050);
    let render = tts.render(&request).expect("render");
    let phrase = &render.phrase;
    assert_eq!(phrase.sample_rate(), 22_050);
    assert!(
        (0.5..10.0).contains(&phrase.seconds()),
        "{}",
        phrase.seconds()
    );
    assert!(rms(phrase.samples()) > 0.01, "{}", rms(phrase.samples()));
    assert!(tts.render(&request).expect("again").from_cache);
    // A listed voice in the wrong case finds its proper name.
    let named = &voices[0].name;
    let shouted = RenderRequest::new("Hello.", 16_000).with_voice(named.to_uppercase());
    if voices
        .iter()
        .filter(|voice| voice.name.to_lowercase() == named.to_lowercase())
        .count()
        == 1
    {
        assert!(tts.render(&shouted).is_ok());
    }
    let unknown = RenderRequest::new("Hello.", 16_000).with_voice("No Such Voice 123");
    assert!(matches!(
        tts.render(&unknown),
        Err(SpeechError::UnknownVoice(_))
    ));
}
