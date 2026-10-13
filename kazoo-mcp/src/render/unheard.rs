pub(super) fn line(out: &mut String, ids: &[String]) {
    if ids.is_empty() {
        return;
    }
    let shown = ids[..ids.len().min(20)].join(", ");
    let more = if ids.len() > 20 {
        format!(" and {} more", ids.len() - 20)
    } else {
        String::new()
    };
    super::line(
        out,
        format_args!(
            "{} modules reach no out, scope or readout and are not rendered: {shown}{more}.",
            ids.len()
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::super::{look, tests::snapshot};

    #[test]
    fn look_says_what_is_unheard() {
        let mut wall = snapshot();
        assert!(!look(&wall).contains("are not rendered:"));
        wall.timing.unheard = vec!["vco3".to_string(), "vcf2".to_string()];
        assert!(look(&wall).contains(
            "2 modules reach no out, scope or readout and are not rendered: vco3, vcf2."
        ));
        wall.timing.unheard = (1..=23).map(|index| format!("vco{index}")).collect();
        let text = look(&wall);
        assert!(text.contains("vco20 and 3 more."));
        assert!(!text.contains("vco21"));
    }
}
