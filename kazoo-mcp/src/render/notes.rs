#[must_use]
pub fn notes(notes: &kazoo_wall::protocol::NotesResult) -> String {
    let mut out = format!("Change #{}: {}\n", notes.seq, notes.note);
    if let Some(description) = &notes.description {
        out.push_str(description);
        out.push('\n');
    }
    for attachment in &notes.attachments {
        out.push_str("\n## ");
        out.push_str(&attachment.name);
        out.push('\n');
        out.push_str(&attachment.text);
        out.push('\n');
    }
    for name in &notes.missing {
        out.push_str("\nMissing note: ");
        out.push_str(name);
        out.push('\n');
    }
    out
}
