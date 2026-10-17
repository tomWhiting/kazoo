use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{ErrorCode, WallError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    pub name: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteRef {
    pub name: String,
    pub bytes: usize,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotesResult {
    pub seq: u64,
    pub note: String,
    pub description: Option<String>,
    pub attachments: Vec<Attachment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
}

#[must_use]
pub fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

pub fn validate(description: Option<&str>, attachments: &[Attachment]) -> Result<(), WallError> {
    let bad = |why| WallError::new(ErrorCode::BadRequest, why);
    if attachments.len() > 8 {
        return Err(bad("a batch takes at most 8 attachments"));
    }
    let mut total = 0;
    if let Some(text) = description {
        validate_text(text, 8 * 1024, "description")?;
        total += text.len();
    }
    let mut names = BTreeSet::from(["description.md".to_string()]);
    for attachment in attachments {
        if !valid_name(&attachment.name) {
            return Err(bad(
                "attachment name must be 1 to 64 of A-Z a-z 0-9 . _ -, and cannot start with a dot",
            ));
        }
        if !names.insert(attachment.name.to_ascii_lowercase()) {
            return Err(bad(
                "attachment names must be unique ignoring case; description.md is reserved",
            ));
        }
        validate_text(&attachment.text, 64 * 1024, &attachment.name)?;
        total += attachment.text.len();
    }
    if total > 128 * 1024 {
        return Err(bad("description and attachments together exceed 128 KiB"));
    }
    Ok(())
}

fn validate_text(text: &str, limit: usize, name: &str) -> Result<(), WallError> {
    let bad = |why| WallError::new(ErrorCode::BadRequest, why);
    if text.is_empty() || text.len() > limit {
        return Err(bad(format!("{name} must contain 1 to {limit} UTF-8 bytes")));
    }
    if let Some(control) = text
        .chars()
        .find(|c| c.is_control() && !matches!(c, '\n' | '\t'))
    {
        return Err(bad(format!(
            "{name} contains forbidden control {control:?} (U+{:04X})",
            u32::from(control)
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_accept_each_exact_limit_and_refuse_one_past_it() {
        let a = |name: &str, text: String| Attachment {
            name: name.into(),
            text,
        };
        assert!(validate(Some(&"é".repeat(4096)), &[]).is_ok());
        assert!(validate(Some(&("é".repeat(4096) + "x")), &[]).is_err());
        assert!(validate(None, &[a(&"a".repeat(64), "x".repeat(65536))]).is_ok());
        assert!(validate(None, &[a(&"a".repeat(65), "x".into())]).is_err());
        assert!(
            validate(
                None,
                &[a("a", "é".repeat(32768)), a("b", "x".repeat(65536))]
            )
            .is_ok()
        );
        assert!(
            validate(
                Some("x"),
                &[a("a", "x".repeat(65536)), a("b", "x".repeat(65536))]
            )
            .is_err()
        );
        let eight: Vec<_> = (0..8).map(|n| a(&format!("n{n}"), "x".into())).collect();
        assert!(validate(None, &eight).is_ok());
        let nine: Vec<_> = (0..9).map(|n| a(&format!("n{n}"), "x".into())).collect();
        assert!(validate(None, &nine).is_err());
        assert!(validate(Some("\n\t"), &[a("a-Z_0.9", "\n\t".into())]).is_ok());
        for c in (0..=159)
            .filter_map(char::from_u32)
            .filter(|c| c.is_control() && !matches!(c, '\n' | '\t'))
        {
            let text = format!("a{c}");
            assert!(validate(Some(&text), &[]).is_err(), "{c:?}");
            assert!(validate(None, &[a("a", text)]).is_err(), "{c:?}");
        }
        assert!(
            validate(Some("\r"), &[])
                .unwrap_err()
                .message
                .contains("\\r")
        );
    }
}
