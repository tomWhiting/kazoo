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

pub(super) fn report_shutdowns(reported: &mut u64, count: u64, mut log: impl FnMut(String)) {
    if count > *reported {
        log(format!(
            "kazoo-wall: {count} desk link shutdown failures so far (timeout or undelivered completion)"
        ));
        *reported = count;
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
    fn shutdown_failures_are_logged_when_the_process_count_grows() {
        let mut reported = 0;
        let mut lines = Vec::new();
        for count in [0, 1, 1, 2, 2] {
            report_shutdowns(&mut reported, count, |line| lines.push(line));
        }
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("1 desk link shutdown failures"));
        assert!(lines[1].contains("2 desk link shutdown failures"));
    }
}
