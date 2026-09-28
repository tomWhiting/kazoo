//! Sample names: the only thing that ever becomes part of a path.

use std::fmt;
use std::str::FromStr;

use crate::Error;

/// The longest a sample name may be, in characters.
pub const MAX_NAME_LEN: usize = 32;

/// A checked sample name: 1 to 32 characters from `A-Z a-z 0-9`, space,
/// `_`, `.` and `-`, not starting with `.` or a space and not ending with
/// `.` or a space.
///
/// The rules leave no way to spell a separator, a parent directory, a hidden
/// file or the store's own temporary files, so a name can never lead a path
/// out of the store.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SampleName(String);

impl SampleName {
    /// Check `name` against the rules.
    pub fn new(name: &str) -> Result<Self, Error> {
        if name.is_empty() {
            return Err(Error::bad_name(name, "a name needs at least one character"));
        }
        if name.chars().count() > MAX_NAME_LEN {
            return Err(Error::bad_name(name, "a name may be at most 32 characters"));
        }
        if !name.chars().all(allowed) {
            return Err(Error::bad_name(
                name,
                "only letters, digits, space, '_', '.' and '-' are allowed",
            ));
        }
        if name.starts_with(['.', ' ']) {
            return Err(Error::bad_name(
                name,
                "a name may not start with '.' or a space",
            ));
        }
        if name.ends_with(['.', ' ']) {
            return Err(Error::bad_name(
                name,
                "a name may not end with '.' or a space",
            ));
        }
        Ok(Self(name.to_owned()))
    }

    /// The name as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The file this sample lives in, inside the store directory.
    pub(crate) fn file_name(&self) -> String {
        format!("{}.wav", self.0)
    }
}

/// Whether `c` may appear in a sample name.
const fn allowed(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '.' | '-')
}

impl FromStr for SampleName {
    type Err = Error;

    fn from_str(name: &str) -> Result<Self, Error> {
        Self::new(name)
    }
}

impl fmt::Display for SampleName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for SampleName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn good_names_pass() {
        for name in [
            "kick",
            "Vox take 2",
            "a",
            "snare_01.v2",
            "x-y",
            &"z".repeat(32),
        ] {
            let checked = SampleName::new(name).unwrap();
            assert_eq!(checked.as_str(), name);
            assert_eq!(checked.file_name(), format!("{name}.wav"));
        }
    }

    #[test]
    fn bad_names_are_refused() {
        for name in [
            "",
            ".",
            "..",
            "../etc",
            "a/b",
            "a\\b",
            ".hidden",
            " lead",
            "trail ",
            "dot.",
            "tab\there",
            "new\nline",
            "nul\0",
            "émigré",
            &"z".repeat(33),
        ] {
            assert!(
                matches!(SampleName::new(name), Err(Error::BadName { .. })),
                "{name:?} should be refused"
            );
        }
    }
}
