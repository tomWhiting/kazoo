use kazoo_core::realtime::NotRealTime;

pub(super) fn report_refusal(
    told: &mut bool,
    outcome: Option<&Result<(), NotRealTime>>,
    mut log: impl FnMut(String),
) {
    if !*told {
        if let Some(Err(why)) = outcome {
            log(format!(
                "kazoo-wall: the desk link runs at normal priority ({why}); on a busy machine the desk may hear it jump"
            ));
            *told = true;
        }
    }
}

/// The process-wide desk link shutdown counts this daemon has logged.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct LinkShutdowns {
    /// Shutdowns that gave up and left the link thread running.
    pub stuck: u64,
    /// Link threads that finished after their shutdown had given up.
    pub late: u64,
}

impl LinkShutdowns {
    /// The counts as kazoo-core holds them now.
    pub(super) fn now() -> Self {
        Self {
            stuck: kazoo_core::ipc::link::stuck_shutdowns(),
            late: kazoo_core::ipc::link::undelivered_completions(),
        }
    }
}

/// Log each count that has grown since it was last logged.
pub(super) fn report_shutdowns(
    reported: &mut LinkShutdowns,
    now: LinkShutdowns,
    mut log: impl FnMut(String),
) {
    if now.stuck > reported.stuck {
        log(format!(
            "kazoo-wall: desk link shutdowns that left the link thread running: {}",
            now.stuck
        ));
        reported.stuck = now.stuck;
    }
    if now.late > reported.late {
        log(format!(
            "kazoo-wall: desk link threads that finished after their shutdown gave up: {}",
            now.late
        ));
        reported.late = now.late;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_time_refusal_is_logged_only_once() {
        let mut told = false;
        let mut lines = Vec::new();
        report_refusal(&mut told, None, |line| lines.push(line));
        report_refusal(&mut told, Some(&Ok(())), |line| lines.push(line));
        assert!(!told);
        let refusal =
            kazoo_core::realtime::RealTime::promote(std::time::Duration::ZERO).unwrap_err();
        let outcome = Err(refusal);
        for _ in 0..3 {
            report_refusal(&mut told, Some(&outcome), |line| lines.push(line));
        }
        assert!(told);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("desk link runs at normal priority"));
        assert!(lines[0].contains("not a period to schedule"));
    }

    #[test]
    fn shutdown_counts_are_logged_when_they_grow() {
        let mut reported = LinkShutdowns::default();
        let mut lines = Vec::new();
        for (stuck, late) in [(0, 0), (1, 0), (1, 0), (1, 1), (2, 1), (2, 1)] {
            report_shutdowns(&mut reported, LinkShutdowns { stuck, late }, |line| {
                lines.push(line);
            });
        }
        assert_eq!(
            lines,
            [
                "kazoo-wall: desk link shutdowns that left the link thread running: 1",
                "kazoo-wall: desk link threads that finished after their shutdown gave up: 1",
                "kazoo-wall: desk link shutdowns that left the link thread running: 2",
            ]
        );
        assert_eq!(reported, LinkShutdowns { stuck: 2, late: 1 });
    }
}

#[derive(Debug, Default)]
pub(super) struct DeskRefusals {
    last: Option<String>,
    connections: u64,
}

impl DeskRefusals {
    pub(super) fn report(
        &mut self,
        status: &kazoo_core::ipc::link::LinkStatus,
        mut log: impl FnMut(String),
    ) {
        if self.connections != status.connections || status.connected {
            self.connections = status.connections;
            self.last = None;
        }
        // Only the desk turning the wall away is a refusal: a desk that is
        // not running, or a link that came down, is not.
        if let Some(reason) = status.desk_refusal() {
            if status.connected || self.last.as_deref() == Some(reason) {
                return;
            }
            log(format!("kazoo-wall: the desk refused the wall: {reason}"));
            self.last = Some(reason.to_owned());
        }
    }
}

#[cfg(test)]
mod desk_tests {
    use super::*;
    #[test]
    fn desk_refusals_log_once_per_reason_and_after_connection() {
        let mut reported = DeskRefusals::default();
        let mut lines = Vec::new();
        let mut status = kazoo_core::ipc::link::LinkStatus {
            voice: kazoo_core::ipc::link::VoiceLink::default(),
            normal_priority: false,
            connected: false,
            strip: None,
            blocks_sent: 0,
            blocks_dropped: 0,
            messages_dropped: 0,
            connections: 0,
            last_refusal: Some("the desk refused wall: wrong rate".into()),
        };
        for _ in 0..2 {
            reported.report(&status, |line| lines.push(line));
        }
        assert_eq!(lines, ["kazoo-wall: the desk refused the wall: wrong rate"]);
        // A link that came down was not refused, and says nothing here.
        status.last_refusal = Some("the desk closed the connection".into());
        reported.report(&status, |line| lines.push(line));
        assert_eq!(lines.len(), 1);
        status.last_refusal = Some("the desk refused wall: desk full".into());
        reported.report(&status, |line| lines.push(line));
        assert_eq!(lines.len(), 2);
        status.connections = 1;
        reported.report(&status, |line| lines.push(line));
        assert_eq!(lines.len(), 3);
        status.connected = true;
        status.last_refusal = None;
        reported.report(&status, |line| lines.push(line));
        status.connected = false;
        status.last_refusal = Some("the desk refused wall: desk full".into());
        reported.report(&status, |line| lines.push(line));
        assert_eq!(lines.len(), 4);
    }
}
