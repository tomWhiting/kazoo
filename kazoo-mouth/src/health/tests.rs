use super::*;

#[test]
fn nothing_lost_says_nothing() {
    let mut health = Health::default();
    assert_eq!(health.observe(Losses::default(), Some("ignored")), None);
    assert_eq!(health.summary(), None);
}

#[test]
fn a_rise_is_said_once_and_its_total_kept() {
    let mut health = Health::default();
    let first = Losses {
        mic_dropped: 28,
        ..Losses::default()
    };
    assert_eq!(
        health.observe(first, None).as_deref(),
        Some("lost: 28 mic samples dropped")
    );
    assert_eq!(health.observe(first, None), None, "unchanged: said once");
    assert_eq!(health.summary().as_deref(), Some("mic 28"));
    let second = Losses {
        mic_dropped: 30,
        ..first
    };
    assert_eq!(
        health.observe(second, None).as_deref(),
        Some("lost: 2 mic samples dropped")
    );
}

#[test]
fn the_worst_is_named_and_the_rest_counted_and_summed_worst_first() {
    let mut health = Health::default();
    let latest = Losses {
        display_dropped: 3,
        mic_skipped: 64,
        judge: JudgeStatsSnapshot {
            samples_dropped: 100,
            ..JudgeStatsSnapshot::default()
        },
        ..Losses::default()
    };
    assert_eq!(
        health.observe(latest, None).as_deref(),
        Some("lost: 100 samples the voice judge missed (+2 others, see below)")
    );
    assert_eq!(
        health.summary().as_deref(),
        Some("pitch in 100 · mic skip 64 · screen 3")
    );
}

#[test]
fn a_failed_pitch_run_is_said_with_its_reason_whenever_it_rises() {
    let mut health = Health::default();
    let alone = Losses {
        judge: JudgeStatsSnapshot {
            pitch_runs_failed: 1,
            ..JudgeStatsSnapshot::default()
        },
        ..Losses::default()
    };
    assert_eq!(
        health.observe(alone, Some("wrong length")).as_deref(),
        Some("lost: 1 pYIN runs that failed, read as unvoiced (engine fault): wrong length")
    );
    let with_mic = Losses {
        mic_dropped: 5,
        judge: JudgeStatsSnapshot {
            pitch_runs_failed: 2,
            ..JudgeStatsSnapshot::default()
        },
        ..alone
    };
    assert_eq!(
        health.observe(with_mic, Some("wrong length")).as_deref(),
        Some("lost: 5 mic samples dropped (+1 other, see below); pYIN failed: wrong length")
    );
    let worst = Losses {
        display_dropped: 1,
        judge: JudgeStatsSnapshot {
            pitch_runs_failed: 3,
            ..JudgeStatsSnapshot::default()
        },
        ..with_mic
    };
    assert_eq!(
        health.observe(worst, Some("wrong length")).as_deref(),
        Some(
            "lost: 1 pYIN runs that failed, read as unvoiced (engine fault): wrong length \
             (+1 other, see below)"
        )
    );
    let mic_only = Losses {
        mic_dropped: 6,
        ..with_mic
    };
    assert_eq!(
        health.observe(mic_only, Some("wrong length")).as_deref(),
        Some("lost: 1 mic samples dropped")
    );
}

/// A snapshot with every counter distinct: each is read by exactly one
/// entry, so a new counter cannot be missed.
#[test]
fn every_counter_is_covered() {
    let latest = Losses {
        mic_dropped: 1,
        mic_skipped: 2,
        display_dropped: 3,
        voice_refused: 4,
        audio_refused: 5,
        desk_lost: 6,
        fx_faults: 7,
        fx_refused: 8,
        judge: JudgeStatsSnapshot {
            samples_dropped: 9,
            results_dropped: 10,
            pitch_runs_failed: 11,
        },
    };
    let mut read: Vec<u64> = COUNTERS.iter().map(|c| (c.read)(&latest)).collect();
    read.sort_unstable();
    assert_eq!(read, (1..=11).collect::<Vec<u64>>());
    assert_eq!(COUNTERS.iter().filter(|c| c.says_pitch_failure).count(), 1);
}

/// The losses are read from the mouth's own counters and its judge's.
#[test]
fn losses_are_read_from_the_stats_and_the_judge() {
    let stats = Stats::default();
    stats.mic_dropped.store(7, Ordering::Relaxed);
    stats.desk_lost.store(2, Ordering::Relaxed);
    let read = Losses::read(&stats, None);
    assert_eq!(read.mic_dropped, 7);
    assert_eq!(read.desk_lost, 2);
    assert_eq!(read.judge, JudgeStatsSnapshot::default());

    let config = kazoo_core::analysis::PitchDetectorConfig::for_rate(48_000);
    let (mut feed, _results, judge) = kazoo_core::voice::judge::spawn(
        config,
        128,
        kazoo_core::voice::judge::OnsetConfig::default(),
    )
    .expect("a judge at 48 kHz");
    // Its ring holds a tenth of a second: 4800 of 4900 fit.
    feed.push(&vec![0.0; 4_900]);
    let counters = judge.counters();
    assert_eq!(
        Losses::read(&stats, Some(&counters)).judge.samples_dropped,
        100
    );
    drop(feed);
    judge.join().expect("the judge exits");
}
