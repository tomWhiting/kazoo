use super::*;
use crate::audio::{COMMAND_CAPACITY, CommandSender};
use kazoo_core::ipc::link::LinkStatus;
use ratatui::backend::TestBackend;

#[test]
fn the_hub_badge_says_when_the_link_is_not_real_time() {
    let (tx, rx) = crossbeam_channel::bounded(COMMAND_CAPACITY);
    let mut app = App::new(48_000.0, CommandSender::new(tx));
    app.hub = Some(LinkStatus {
        normal_priority: true,
        connected: true,
        strip: Some(3),
        blocks_sent: 0,
        blocks_dropped: 0,
        messages_dropped: 0,
        connections: 1,
        last_refusal: None,
    });
    let mut terminal = ratatui::Terminal::new(TestBackend::new(120, 1)).unwrap();
    terminal
        .draw(|frame| draw_alert_line(frame, &app, frame.area()))
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect();
    assert!(text.contains("desk link at normal priority"), "{text}");
    drop(rx);
}
