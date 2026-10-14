use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputPick {
    pub name: String,
    pub why: Option<String>,
}

#[must_use]
pub fn choose_input(default_input: Option<&str>, output: &str, inputs: &[String]) -> InputPick {
    let default = default_input
        .or_else(|| inputs.first().map(String::as_str))
        .unwrap_or(output);
    if default != output {
        return InputPick {
            name: default.to_owned(),
            why: None,
        };
    }
    let alternative = inputs
        .iter()
        .find(|name| {
            name.as_str() != output && (name.contains("MacBook") || name.contains("Built-in"))
        })
        .or_else(|| inputs.iter().find(|name| name.as_str() != output));
    alternative.map_or_else(
        || explicit_input(output, output),
        |name| InputPick {
            name: name.clone(),
            why: Some(format!(
                "not the {} mic: it would drop them to phone quality",
                possessive(output)
            )),
        },
    )
}

/// The owner of the mic as the header says it: an apostrophe after a final s, else apostrophe s.
fn possessive(name: &str) -> String {
    if name.ends_with(['s', 'S']) {
        format!("{name}'")
    } else {
        format!("{name}'s")
    }
}

/// The input to open: `explicit` as asked, else [`choose_input`]. With no
/// input device at all there is nothing to open, and the error says so
/// rather than naming the output as a mic.
///
/// # Errors
///
/// [`Error::Device`] when nothing was asked for and no input device exists.
pub fn input_for(
    explicit: Option<&str>,
    default_input: Option<&str>,
    output: &str,
    inputs: &[String],
) -> Result<InputPick> {
    if let Some(name) = explicit {
        return Ok(explicit_input(name, output));
    }
    if inputs.is_empty() {
        return Err(Error::Device(
            "no input device: kazoo mouth needs a microphone (kazoo mouth --list-inputs lists the inputs)"
                .into(),
        ));
    }
    Ok(choose_input(default_input, output, inputs))
}

#[must_use]
pub fn explicit_input(name: &str, output: &str) -> InputPick {
    InputPick {
        name: name.to_owned(),
        why: (name == output).then(|| "they now play at phone quality".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choose_input_contract() {
        let inputs = ["AirPods", "USB mic", "MacBook Pro Microphone"].map(str::to_owned);
        let picked = choose_input(Some("AirPods"), "AirPods", &inputs);
        assert_eq!(picked.name, "MacBook Pro Microphone");
        assert_eq!(
            picked.why.as_deref(),
            Some("not the AirPods' mic: it would drop them to phone quality")
        );
        let picked = choose_input(Some("USB mic"), "AirPods", &inputs);
        assert_eq!(picked.name, "USB mic");
        assert_eq!(picked.why, None);
        let picked = choose_input(Some("AirPods"), "AirPods", &inputs[..2]);
        assert_eq!(picked.name, "USB mic");
        let picked = choose_input(Some("AirPods"), "AirPods", &inputs[..1]);
        assert_eq!(picked.name, "AirPods");
        assert_eq!(
            picked.why.as_deref(),
            Some("they now play at phone quality")
        );
        for default in [None, Some("AirPods")] {
            let picked = choose_input(default, "AirPods", &[]);
            assert_eq!(picked.name, "AirPods");
            assert!(picked.why.is_some());
        }
        assert_eq!(choose_input(None, "Speakers", &inputs).name, "AirPods");
    }

    #[test]
    fn explicit_headset_input_keeps_the_phone_quality_note() {
        let picked = explicit_input("AirPods", "AirPods");
        assert_eq!(picked.name, "AirPods");
        assert_eq!(
            picked.why.as_deref(),
            Some("they now play at phone quality")
        );
        assert_eq!(explicit_input("USB mic", "AirPods").why, None);
    }

    #[test]
    fn the_note_names_a_singular_headset_properly() {
        let inputs = ["USB Headset", "MacBook Pro Microphone"].map(str::to_owned);
        let picked = choose_input(Some("USB Headset"), "USB Headset", &inputs);
        assert_eq!(
            picked.why.as_deref(),
            Some("not the USB Headset's mic: it would drop them to phone quality")
        );
    }

    #[test]
    fn no_input_device_is_said_and_never_names_the_output_as_a_mic() {
        let error = input_for(None, None, "MacBook Pro Speakers", &[]).unwrap_err();
        assert!(error.to_string().contains("no input device"), "{error}");
        let error = input_for(None, Some("ghost"), "AirPods", &[]).unwrap_err();
        assert!(error.to_string().contains("no input device"), "{error}");
        let asked = input_for(Some("AirPods"), None, "AirPods", &[]).unwrap();
        assert_eq!(asked, explicit_input("AirPods", "AirPods"));
        let inputs = ["AirPods", "MacBook Pro Microphone"].map(str::to_owned);
        assert_eq!(
            input_for(None, Some("AirPods"), "AirPods", &inputs).unwrap(),
            choose_input(Some("AirPods"), "AirPods", &inputs)
        );
    }
}
