use kazoo_wall::catalogue::Kind;

use super::Fixture;

#[must_use]
pub fn module_kind(kind: Kind) -> Fixture {
    let mut fixture = Fixture::new();
    let module = fixture.add(kind);
    if let Some(input) = kind.spec().inputs.first() {
        let vco = fixture.add(Kind::VCO);
        fixture.plug(
            vco,
            fixture.port(vco, "out"),
            module,
            fixture.input(module, input.name),
            1.0,
        );
    }
    if let Some(output) = kind.spec().outputs.first() {
        let out = fixture.add(Kind::OUT);
        fixture.plug(
            module,
            fixture.port(module, output.name),
            out,
            fixture.input(out, "left"),
            1.0,
        );
        if let Some(output) = kind.spec().outputs.get(1) {
            fixture.plug(
                module,
                fixture.port(module, output.name),
                out,
                fixture.input(out, "right"),
                1.0,
            );
        }
    }
    fixture.commit();
    fixture
}

#[must_use]
pub fn chains(count: usize) -> Fixture {
    let mut fixture = Fixture::new();
    for _ in 0..count {
        let vco = fixture.add(Kind::VCO);
        let vcf = fixture.add(Kind::VCF);
        let out = fixture.add(Kind::OUT);
        fixture.plug(
            vco,
            fixture.port(vco, "out"),
            vcf,
            fixture.input(vcf, "in"),
            1.0,
        );
        for input in ["left", "right"] {
            fixture.plug(
                vcf,
                fixture.port(vcf, "out"),
                out,
                fixture.input(out, input),
                1.0,
            );
        }
    }
    fixture.commit();
    fixture
}

#[must_use]
pub fn unplugged(count: usize) -> Fixture {
    let mut fixture = Fixture::new();
    for _ in 0..count {
        fixture.add(Kind::LFO);
    }
    fixture.commit();
    fixture
}

#[must_use]
pub fn cables(count: usize) -> Fixture {
    let mut fixture = Fixture::new();
    let vco = fixture.add(Kind::VCO);
    let mix_spec = Kind::MIX.spec();
    let jacks_per_mix = mix_spec.inputs.len() + mix_spec.knobs.len();
    let mixes: Vec<usize> = (0..count.div_ceil(jacks_per_mix))
        .map(|_| fixture.add(Kind::MIX))
        .collect();
    let mut level = mixes.clone();
    while level.len() > 1 {
        level = level
            .chunks(4)
            .map(|sources| {
                let mix = fixture.add(Kind::MIX);
                for (index, &source) in sources.iter().enumerate() {
                    let name = Kind::MIX.spec().inputs[index].name;
                    fixture.plug(
                        source,
                        fixture.port(source, "out"),
                        mix,
                        fixture.input(mix, name),
                        1.0,
                    );
                }
                mix
            })
            .collect();
    }
    let out = fixture.add(Kind::OUT);
    fixture.plug(
        level[0],
        fixture.port(level[0], "out"),
        out,
        fixture.input(out, "left"),
        1.0,
    );
    let mut remaining = count
        .checked_sub(fixture.cables.len())
        .expect("enough cables for summing");
    // Every leaf is heard even when the last fan-out row is partial.
    for &mix in &mixes {
        fixture.plug(
            vco,
            fixture.port(vco, "out"),
            mix,
            fixture.input(mix, mix_spec.inputs[0].name),
            1.0,
        );
        remaining = remaining
            .checked_sub(1)
            .expect("enough cables to feed each mix");
    }
    for mix in mixes {
        let spec = Kind::MIX.spec();
        let jacks = spec
            .inputs
            .iter()
            .map(|input| fixture.input(mix, input.name))
            .chain(spec.knobs.iter().map(|knob| fixture.knob(mix, knob.name)))
            .collect::<Vec<_>>();
        assert_eq!(jacks.len(), jacks_per_mix);
        for jack in jacks.into_iter().skip(1) {
            if remaining == 0 {
                break;
            }
            fixture.plug(vco, fixture.port(vco, "out"), mix, jack, 1.0);
            remaining -= 1;
        }
    }
    assert_eq!(remaining, 0);
    assert_eq!(fixture.cables.len(), count);
    fixture.commit();
    fixture
}

#[must_use]
pub fn culled(count: usize) -> Fixture {
    assert!(count >= 10);
    let mut fixture = Fixture::new();
    for index in 0..count {
        let lfo = fixture.add(Kind::LFO);
        let vca = fixture.add(Kind::VCA);
        fixture.plug(
            lfo,
            fixture.port(lfo, "out"),
            vca,
            fixture.input(vca, "cv"),
            1.0,
        );
        if index < 10 {
            let out = fixture.add(Kind::OUT);
            fixture.plug(
                vca,
                fixture.port(vca, "out"),
                out,
                fixture.input(out, "left"),
                1.0,
            );
        }
    }
    fixture.commit();
    fixture
}
