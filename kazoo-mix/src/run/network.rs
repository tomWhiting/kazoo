use std::net::SocketAddr;

pub(super) fn note(note: String, address: Option<SocketAddr>) -> String {
    match address {
        Some(address) => format!("listening {address}; {note}"),
        None => note,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn listening_address_is_visible_in_the_header_and_absent_by_default() {
        assert_eq!(note("ready".to_owned(), None), "ready");
        let address = "0.0.0.0:7700".parse().unwrap();
        let status = crate::ui::StatusInfo {
            sample_rate: 48_000,
            channels: 2,
            buffer_frames: Some(256),
            note: note("ready".to_owned(), Some(address)),
            note_alert: false,
            stream_errors: 0,
            engine_faults: 0,
            mouse: true,
            hub_priority: crate::ui::HubPriority::RealTime,
        };
        let mut terminal = Terminal::new(TestBackend::new(200, 34)).unwrap();
        let mut desk = crate::desk::Desk::new();
        let shared = crate::shared::SharedState::new();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut desk, &shared, &status))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(text.contains("listening 0.0.0.0:7700; ready"), "{text}");
    }
}
