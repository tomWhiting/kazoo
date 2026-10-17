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

/// Whether `c` would break a line of text or turn its direction: a control
/// character, a line or paragraph separator, or a direction mark, embedding,
/// override or isolate. A note or comment holds none of them.
#[must_use]
pub fn breaks_line(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{2028}'
                | '\u{2029}'
                | '\u{061C}'
                | '\u{200E}'
                | '\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
        )
}

/// Check a comment (see [`super::RequestLine::comment`]): one line with no
/// control or direction characters (see [`breaks_line`]), 1 to 200
/// characters after trimming. Returned trimmed.
///
/// # Errors
///
/// `bad_request`, saying which rule it broke.
pub fn comment(comment: &str) -> Result<&str, WallError> {
    if comment.chars().any(breaks_line) {
        return Err(bad(
            "a comment must be one line with no control or direction characters",
        ));
    }
    let comment = comment.trim();
    if !(1..=200).contains(&comment.chars().count()) {
        return Err(bad(
            "a comment must contain 1 to 200 characters after trimming",
        ));
    }
    Ok(comment)
}

/// A batch's step comments as kept: none when no step has one, else one
/// per step, each trimmed.
///
/// # Errors
///
/// `bad_request` when there are some but not one per step, or one breaks
/// the rules of [`comment`] (with its step).
pub fn step_comments(
    comments: &[Option<String>],
    steps: usize,
) -> Result<Vec<Option<String>>, WallError> {
    if comments.is_empty() {
        return Ok(Vec::new());
    }
    if comments.len() != steps {
        return Err(bad(format!(
            "a batch's comments are one per step, null for a step without one: {} for {steps} \
             steps",
            comments.len()
        )));
    }
    let kept = comments
        .iter()
        .enumerate()
        .map(|(index, text)| {
            text.as_deref()
                .map(|text| comment(text).map(str::to_string))
                .transpose()
                .map_err(|err| at_step(err, index))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(if kept.iter().all(Option::is_none) {
        Vec::new()
    } else {
        kept
    })
}

/// Check a batch before anything is applied: its `note` (returned trimmed),
/// its step comments (returned as [`step_comments`] keeps them) and each
/// step's op and labels.
///
/// # Errors
///
/// `bad_request`, with the step's index when a step is refused.
pub fn validate<'a>(
    note: &'a str,
    changes: &[Request],
    comments: &[Option<String>],
) -> Result<(&'a str, Vec<Option<String>>), WallError> {
    if note.chars().any(breaks_line) {
        return Err(bad(
            "a batch note must be one line with no control or direction characters",
        ));
    }
    let note = note.trim();
    if !(1..=200).contains(&note.chars().count()) {
        return Err(bad(
            "a batch note must contain 1 to 200 characters after trimming",
        ));
    }
    if changes.len() > crate::tree::git::MOST_PARTS {
        return Err(bad("a batch takes at most 4096 steps"));
    }
    if changes.is_empty() {
        return Err(bad("a batch must contain at least one change"));
    }
    let comments = step_comments(comments, changes.len())?;
    let mut labels = BTreeSet::new();
    for (index, request) in changes.iter().enumerate() {
        validate_step(request, &mut labels).map_err(|err| at_step(err, index))?;
    }
    Ok((note, comments))
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
        Request::Turn { module, .. } | Request::Remove { module, .. } => reference(module, labels)?,
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
