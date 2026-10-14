use kazoo_wall::protocol::Snapshot;

use super::line;

pub(super) fn block(out: &mut String, snapshot: &Snapshot) {
    line(out, format_args!("\nCables ({}):", snapshot.cables.len()));
    if snapshot.cables.is_empty() {
        line(out, format_args!("  none"));
    }
    for cable in &snapshot.cables {
        line(
            out,
            format_args!(
                "  #{} {} → {} amount {}",
                cable.id, cable.from, cable.to, cable.amount
            ),
        );
    }
}
