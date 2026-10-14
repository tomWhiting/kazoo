use super::{HubNotice, RefuseReason};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a refused instrument stays listed without trying again.
const KEPT_FOR: Duration = Duration::from_secs(60);
/// The most refusals kept: more than any desk can show, and a bound on what
/// a stream of differently named connections can make the hub hold.
const KEPT_MOST: usize = 32;

#[derive(Debug)]
struct Refusal {
    name: String,
    reason: RefuseReason,
    at: Instant,
}

#[derive(Debug, Default)]
pub(super) struct Refusals(Mutex<Vec<Refusal>>);

impl Refusals {
    pub(super) fn record(&self, notice: &HubNotice, now: Instant) {
        let mut entries = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.retain(|entry| now.saturating_duration_since(entry.at) < KEPT_FOR);
        match notice {
            HubNotice::Refused { name, reason, .. } => {
                entries.retain(|entry| entry.name != *name);
                entries.insert(
                    0,
                    Refusal {
                        name: name.clone(),
                        reason: reason.clone(),
                        at: now,
                    },
                );
                entries.truncate(KEPT_MOST);
            }
            HubNotice::Joined { name, .. } => entries.retain(|entry| entry.name != *name),
            HubNotice::Left { .. } | HubNotice::Fault(_) => {}
        }
    }

    pub(super) fn lines(&self, now: Instant) -> Vec<String> {
        let entries = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries
            .iter()
            .filter(|entry| now.saturating_duration_since(entry.at) < KEPT_FOR)
            .map(|entry| {
                let name = if entry.name.is_empty() {
                    "an instrument"
                } else {
                    &entry.name
                };
                match &entry.reason {
                    RefuseReason::SampleRate { instrument, desk } => {
                        format!("refused: {name} ({instrument} Hz, the desk runs at {desk} Hz)")
                    }
                    reason => format!("refused: {name} ({reason})"),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refused_instruments_stay_until_sixty_seconds_or_connection() {
        let refusals = Refusals::default();
        let now = Instant::now();
        let notice = HubNotice::Refused {
            name: "mouth".into(),
            reason: RefuseReason::SampleRate {
                instrument: 48_000,
                desk: 24_000,
            },
            told: true,
        };
        refusals.record(&notice, now);
        let expected = ["refused: mouth (48000 Hz, the desk runs at 24000 Hz)"];
        assert_eq!(refusals.lines(now), expected);
        assert_eq!(refusals.lines(now + Duration::from_secs(59)), expected);
        assert!(refusals.lines(now + Duration::from_secs(61)).is_empty());
        refusals.record(&notice, now);
        refusals.record(
            &HubNotice::Joined {
                name: "mouth".into(),
                slot: 0,
            },
            now,
        );
        assert!(refusals.lines(now).is_empty());
    }
    #[test]
    fn the_hub_keeps_a_bounded_list_newest_first() {
        let refusals = Refusals::default();
        let now = Instant::now();
        for n in 0..(KEPT_MOST + 8) {
            refusals.record(
                &HubNotice::Refused {
                    name: format!("probe {n}"),
                    reason: RefuseReason::DeskFull,
                    told: true,
                },
                now,
            );
        }
        let lines = refusals.lines(now);
        assert_eq!(lines.len(), KEPT_MOST);
        assert!(lines[0].starts_with(&format!("refused: probe {}", KEPT_MOST + 7)));
    }

    #[test]
    fn retries_refresh_expiry_and_newest_refusals_come_first() {
        let refusals = Refusals::default();
        let now = Instant::now();
        let refused = |name: &str| HubNotice::Refused {
            name: name.into(),
            reason: RefuseReason::DeskFull,
            told: true,
        };
        refusals.record(&refused("mouth"), now);
        refusals.record(&refused("wall"), now + Duration::from_secs(1));
        refusals.record(&refused("mouth"), now + Duration::from_secs(59));
        let lines = refusals.lines(now + Duration::from_secs(60));
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("refused: mouth"));
        assert!(lines[1].starts_with("refused: wall"));
    }
}
