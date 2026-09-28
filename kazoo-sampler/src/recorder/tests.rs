//! Recorder tests: exact takes, trimming, normalising, limits, overruns and
//! every refusal.

use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::StoreLimits;

const RATE: u32 = 48_000;
const WAIT: Duration = Duration::from_secs(10);

fn setup(ring_seconds: f64) -> (tempfile::TempDir, SampleStore, Recorder, RecordTap) {
    let dir = tempfile::tempdir().unwrap();
    let store = SampleStore::open(dir.path().join("samples"), StoreLimits::default()).unwrap();
    let (recorder, tap) = Recorder::new(store.clone(), RATE, ring_seconds).unwrap();
    (dir, store, recorder, tap)
}

fn request(name: &str) -> TakeRequest {
    TakeRequest::new(SampleName::new(name).unwrap())
}

/// A deterministic stereo signal full of values that do not survive any
/// conversion but a bit-exact one.
fn signal(frames: usize, seed: u32) -> (Vec<f32>, Vec<f32>) {
    let mut noise = kazoo_fx::dsp::Noise::new(seed);
    let left: Vec<f32> = (0..frames).map(|_| noise.sample() * 0.9).collect();
    let right: Vec<f32> = (0..frames).map(|_| noise.sample() * 0.3).collect();
    (left, right)
}

fn read_back(store: &SampleStore, name: &str) -> (Vec<f32>, Vec<f32>) {
    let path = store.dir().join(format!("{name}.wav"));
    let mut reader = hound::WavReader::open(path).unwrap();
    let samples: Vec<f32> = reader.samples::<f32>().map(|s| s.unwrap()).collect();
    let left = samples.iter().step_by(2).copied().collect();
    let right = samples.iter().skip(1).step_by(2).copied().collect();
    (left, right)
}

/// Feed `left`/`right` to the tap in blocks, from its own thread, as an
/// audio callback would; the tap comes back when done.
fn play(mut tap: RecordTap, left: Vec<f32>, right: Vec<f32>, block: usize) -> RecordTap {
    std::thread::spawn(move || {
        for (l, r) in left.chunks(block).zip(right.chunks(block)) {
            tap.process(l, r);
            std::thread::sleep(Duration::from_micros(200));
        }
        tap
    })
    .join()
    .unwrap()
}

fn finish(recorder: &mut Recorder) -> TakeReport {
    recorder.stop().unwrap();
    recorder.wait(WAIT).expect("a report")
}

#[test]
fn a_take_lands_bit_exactly() {
    let (_dir, store, mut recorder, tap) = setup(1.0);
    let id = recorder.start(request("exact")).unwrap();
    assert!(matches!(
        recorder.status(),
        RecorderStatus::Recording { .. }
    ));
    let (left, right) = signal(48_000, 7);
    let mut tap = play(tap, left.clone(), right.clone(), 256);
    let report = finish(&mut recorder);
    tap.process(&[1.0], &[1.0]); // After the stop: not recorded.
    assert_eq!(report.id, id);
    let take = report.result.unwrap();
    assert_eq!(take.info.frames, 48_000);
    assert_eq!(take.dropped_frames, 0);
    assert!(!take.hit_limit);
    let (got_left, got_right) = read_back(&store, "exact");
    let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&got_left), bits(&left));
    assert_eq!(bits(&got_right), bits(&right));
    assert_eq!(recorder.status(), RecorderStatus::Idle);
}

#[test]
fn takes_follow_one_another() {
    let (_dir, store, mut recorder, mut tap) = setup(1.0);
    for (i, name) in ["one", "two", "three"].into_iter().enumerate() {
        recorder.start(request(name)).unwrap();
        let (left, right) = signal(1_000 * (i + 1), i as u32 + 1);
        tap = play(tap, left, right, 128);
        let take = finish(&mut recorder).result.unwrap();
        assert_eq!(take.info.frames, 1_000 * (i as u64 + 1));
    }
    assert_eq!(store.list().unwrap().samples.len(), 3);
}

#[test]
fn leading_silence_is_trimmed_with_a_faded_preroll() {
    let (_dir, store, mut recorder, tap) = setup(1.0);
    let mut req = request("trimmed");
    req.trim_db = Some(-40.0);
    recorder.start(req).unwrap();
    let mut left = vec![0.001f32; 10_000];
    left.extend(std::iter::repeat_n(0.5f32, 5_000));
    let right = left.clone();
    let _tap = play(tap, left, right, 300);
    let take = finish(&mut recorder).result.unwrap();
    let preroll = (PREROLL_SECONDS * f64::from(RATE)).round() as u64;
    assert_eq!(take.trimmed_frames, 10_000 - preroll);
    assert_eq!(take.info.frames, 5_000 + preroll);
    let (got, _) = read_back(&store, "trimmed");
    assert!(got[0].abs() < 1e-6, "the pre-roll fades in from silence");
    assert!(got[..preroll as usize].windows(2).all(|w| w[1] >= w[0]));
    assert!((got[preroll as usize] - 0.5).abs() < f32::EPSILON);
}

#[test]
fn normalising_puts_the_peak_just_below_full_scale() {
    let (_dir, store, mut recorder, tap) = setup(1.0);
    let mut req = request("loud");
    req.normalise = true;
    recorder.start(req).unwrap();
    let left: Vec<f32> = (0..4_800).map(|n| (n as f32 * 0.02).sin() * 0.25).collect();
    let right: Vec<f32> = left.iter().map(|x| x * 0.5).collect();
    let _tap = play(tap, left, right, 256);
    let take = finish(&mut recorder).result.unwrap();
    let (got_left, got_right) = read_back(&store, "loud");
    let peak = got_left
        .iter()
        .chain(&got_right)
        .fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(
        (kazoo_fx::dsp::gain_to_db(peak) - NORMALISE_PEAK_DB).abs() < 0.01,
        "{peak}"
    );
    assert!(take.gain_db > 10.0);
    // Only the finished sample is left, no temporaries.
    assert_eq!(std::fs::read_dir(store.dir()).unwrap().count(), 1);
}

#[test]
fn a_take_stops_itself_at_its_limit() {
    let (_dir, _store, mut recorder, tap) = setup(1.0);
    let mut req = request("short");
    req.max_seconds = 0.01;
    recorder.start(req).unwrap();
    let (left, right) = signal(4_800, 3);
    let _tap = play(tap, left, right, 100);
    let report = recorder.wait(WAIT).expect("the take ends by itself");
    let take = report.result.unwrap();
    assert!(take.hit_limit);
    assert_eq!(take.info.frames, 480);
}

#[test]
fn an_overrun_is_counted_and_reported() {
    let (_dir, _store, mut recorder, mut tap) = setup(0.05);
    recorder.start(request("overrun")).unwrap();
    let (left, right) = signal(10_000, 9);
    // Claim the take, then offer far more than the ring holds at once.
    tap.process(&[], &[]);
    tap.process(&left, &right);
    let RecorderStatus::Recording { dropped_frames, .. } = recorder.status() else {
        panic!("still recording");
    };
    assert_eq!(dropped_frames, 10_000 - 2_400);
    let take = finish(&mut recorder).result.unwrap();
    assert_eq!(take.dropped_frames, 10_000 - 2_400);
    assert_eq!(take.info.frames, 2_400);
}

#[test]
fn refusals_are_clear() {
    let (_dir, store, mut recorder, mut tap) = setup(1.0);
    assert!(matches!(recorder.stop(), Err(Error::NotRecording)));
    recorder.start(request("busy")).unwrap();
    assert!(matches!(recorder.start(request("other")), Err(Error::Busy)));
    // Stopped before the tap ever saw it: an empty take.
    let report = finish(&mut recorder);
    assert!(
        matches!(report.result, Err(Error::EmptyTake { .. })),
        "{:?}",
        report.result
    );

    store
        .save(
            &SampleName::new("taken").unwrap(),
            RATE,
            &[0.1],
            &[0.1],
            Overwrite::Refuse,
        )
        .unwrap();
    assert!(matches!(
        recorder.start(request("taken")),
        Err(Error::Exists { .. })
    ));

    let mut bad = request("bad");
    bad.max_seconds = f64::NAN;
    assert!(recorder.start(bad).is_err());

    let mut quiet = request("quiet");
    quiet.trim_db = Some(-20.0);
    recorder.start(quiet).unwrap();
    tap.process(&[0.01; 512], &[0.01; 512]);
    let report = finish(&mut recorder);
    assert!(
        matches!(report.result, Err(Error::SilentTake { .. })),
        "{:?}",
        report.result
    );
    assert_eq!(store.list().unwrap().samples.len(), 1);
    assert!(Recorder::new(store.clone(), 0, 1.0).is_err());
    assert!(Recorder::new(store, RATE, 0.0).is_err());
}

#[test]
fn a_write_failure_is_reported_and_stops_the_tap() {
    let (_dir, store, mut recorder, mut tap) = setup(1.0);
    std::fs::set_permissions(store.dir(), std::fs::Permissions::from_mode(0o500)).unwrap();
    recorder.start(request("nowhere")).unwrap();
    for _ in 0..50 {
        tap.process(&[0.5; 256], &[0.5; 256]);
        std::thread::sleep(Duration::from_millis(1));
    }
    std::fs::set_permissions(store.dir(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let report = recorder.wait(WAIT).expect("the failure ends the take");
    assert!(
        matches!(report.result, Err(Error::Io { .. })),
        "{:?}",
        report.result
    );
    assert!(!tap.is_recording());
}

#[test]
fn a_take_survives_its_tap_or_recorder_going_away() {
    let (_dir, store, mut recorder, tap) = setup(1.0);
    recorder.start(request("orphan")).unwrap();
    let (left, right) = signal(2_000, 5);
    let tap = play(tap, left, right, 500);
    drop(tap);
    let take = recorder.wait(WAIT).expect("a report").result.unwrap();
    assert_eq!(take.info.frames, 2_000);

    let (mut recorder, tap) = Recorder::new(store.clone(), RATE, 1.0).unwrap();
    recorder.start(request("closing")).unwrap();
    let (left, right) = signal(1_000, 6);
    let _tap = play(tap, left, right, 250);
    drop(recorder);
    assert_eq!(
        store
            .info(&SampleName::new("closing").unwrap())
            .unwrap()
            .frames,
        1_000
    );
}
