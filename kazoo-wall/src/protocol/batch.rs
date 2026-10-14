//! A batch's rules: its note, the ops it takes, and its labels, checked
//! before anything is applied.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{Change, ErrorCode, Request, WallError};

/// The answer to a [`Request::Batch`]: the one change it made, and each
/// step's own answer in order (an add's with its `module` and its `as`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchResult {
    /// The batch, as logged.
    pub change: Change,
    /// Each step's answer, in order.
    pub steps: Vec<serde_json::Value>,
}

fn bad(message: impl Into<String>) -> WallError {
    WallError::new(ErrorCode::BadRequest, message)
}

/// `error` as step `index`'s: the step named in it and on it.
#[must_use]
pub fn at_step(mut error: WallError, index: usize) -> WallError {
    error.step = Some(index);
    error.message = format!("step {index}: {}", error.message);
    error
}

/// Check a batch before anything is applied: its `note` (returned trimmed)
/// and each step's op and labels.
///
/// # Errors
///
/// `bad_request`, with the step's index when a step is refused.
pub fn validate<'a>(note: &'a str, changes: &[Request]) -> Result<&'a str, WallError> {
    if note.chars().any(char::is_control) {
        return Err(bad(
            "a batch note must be one line with no control characters",
        ));
    }
    let note = note.trim();
    if !(1..=200).contains(&note.chars().count()) {
        return Err(bad(
            "a batch note must contain 1 to 200 characters after trimming",
        ));
    }
    if changes.is_empty() {
        return Err(bad("a batch must contain at least one change"));
    }
    let mut labels = BTreeSet::new();
    for (index, request) in changes.iter().enumerate() {
        validate_step(request, &mut labels).map_err(|err| at_step(err, index))?;
    }
    Ok(note)
}

fn reference(value: &str, labels: &BTreeSet<&str>) -> Result<(), WallError> {
    if let Some(value) = value.strip_prefix('@') {
        let label = value.split('.').next().unwrap_or(value);
        if !labels.contains(label) {
            return Err(bad(format!(
                "unknown label @{label}, or used before its add"
            )));
        }
    }
    Ok(())
}

fn validate_step<'a>(
    request: &'a Request,
    labels: &mut BTreeSet<&'a str>,
) -> Result<(), WallError> {
    match request {
        Request::Add { alias, place, .. } => {
            if let Some(before) = place.as_ref().and_then(|place| place.before.as_deref()) {
                reference(before, labels)?;
            }
            if let Some(label) = alias {
                if label.is_empty()
                    || label
                        .chars()
                        .any(|ch| ch.is_whitespace() || ch.is_control() || matches!(ch, '.' | '@'))
                {
                    return Err(bad(
                        "an add label must be a name without spaces, dots, controls or @",
                    ));
                }
                if !labels.insert(label) {
                    return Err(bad(format!("duplicate add label {label}")));
                }
            }
        }
        Request::Turn { module, .. } | Request::Remove { module } => reference(module, labels)?,
        Request::Patch { from, to, .. } => {
            reference(from, labels)?;
            reference(to, labels)?;
        }
        Request::Unpatch { to: Some(to), .. } => reference(to, labels)?,
        Request::Unpatch { .. } | Request::Tempo { .. } => {}
        other => return Err(bad(format!("{} is not allowed in a batch", other.op()))),
    }
    Ok(())
}
