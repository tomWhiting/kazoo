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
                "not the {output}' mic: it would drop them to phone quality"
            )),
        },
    )
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
}
