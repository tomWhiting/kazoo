//! The voices `say` offers, read from `say -v '?'`.
//!
//! Each line is a voice name (which may hold spaces and nested brackets:
//! `Eddy (English (UK))`), a locale such as `en_GB`, then `#` and a sample
//! sentence. The name is everything before the last word ahead of the `#`.

/// One voice `say` can speak with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Voice {
    /// The name `say -v` takes: `Daniel`, `Eddy (English (UK))`.
    pub name: String,
    /// The locale: `en_GB`.
    pub locale: String,
    /// The voice's own sample sentence.
    pub sample: String,
}

/// Every voice in `listing` (the output of `say -v '?'`), in order. Lines
/// that do not have the expected shape are skipped.
#[must_use]
pub fn parse_voices(listing: &str) -> Vec<Voice> {
    listing.lines().filter_map(parse_line).collect()
}

fn parse_line(line: &str) -> Option<Voice> {
    let marker = line.find(" #")?;
    let (head, tail) = line.split_at(marker);
    let sample = tail.trim_start_matches(" #").trim().to_string();
    let head = head.trim_end();
    let split = head.rfind(char::is_whitespace)?;
    let (name, locale) = head.split_at(split);
    let name = name.trim();
    let locale = locale.trim();
    let locale_ok = !locale.is_empty()
        && locale.contains(['_', '-'])
        && locale
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if name.is_empty() || !locale_ok || name.chars().any(char::is_control) {
        return None;
    }
    Some(Voice {
        name: name.to_string(),
        locale: locale.to_string(),
        sample,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from macOS 26's `say -v '?'`, with a few damaged lines.
    const SAMPLE: &str = "\
Albert              en_US    # Hello! My name is Albert.
Alva (Premium)      sv_SE    # Hej! Jag heter Alva.
Amélie              fr_CA    # Bonjour! Je m’appelle Amélie.
Bad News            en_US    # Hello! My name is Bad News.
Carmit              he_IL    # שלום, שמי כרמית.
Eddy (English (UK)) en_GB    # Hello! My name is Eddy.
Eddy (Chinese (China mainland)) zh_CN    # 你好！我叫Eddy。
Flo (English (UK))  en_GB    # Hello! My name is Flo.
Moira (Enhanced)    en_IE    # Hello! My name is Moira.
Fiona               en-scotland # Hello, my name is Fiona.

garbage without a marker
NoLocale # hello
   # nothing before
Broken              en US    # the locale has a space
";

    #[test]
    fn reads_every_well_formed_line() {
        let voices = parse_voices(SAMPLE);
        let names: Vec<&str> = voices.iter().map(|voice| voice.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Albert",
                "Alva (Premium)",
                "Amélie",
                "Bad News",
                "Carmit",
                "Eddy (English (UK))",
                "Eddy (Chinese (China mainland))",
                "Flo (English (UK))",
                "Moira (Enhanced)",
                "Fiona",
            ]
        );
        assert_eq!(voices[5].locale, "en_GB");
        assert_eq!(voices[6].locale, "zh_CN");
        assert_eq!(voices[9].locale, "en-scotland");
        assert_eq!(voices[0].sample, "Hello! My name is Albert.");
        assert_eq!(voices[4].sample, "שלום, שמי כרמית.");
    }

    #[test]
    fn empty_listing_is_no_voices() {
        assert!(parse_voices("").is_empty());
        assert!(parse_voices("\n\n").is_empty());
    }
}
