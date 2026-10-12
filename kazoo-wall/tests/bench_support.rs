#[path = "../benches/support/mod.rs"]
pub mod support;

use support::{cables, chains, culled, unplugged};

#[test]
fn a_chain_fixture_is_heard() {
    let mut fixture = chains(1);
    let mut buffer = [0.0; 512 * 2];
    let mut peak = 0.0_f32;
    for _ in 0..64 {
        fixture.render(&mut buffer);
        assert!(buffer.iter().all(|sample| sample.is_finite()));
        peak = buffer
            .iter()
            .fold(peak, |value, sample| value.max(sample.abs()));
    }
    assert!(peak > 0.0, "the chain never reached the master");
}

#[test]
fn an_unplugged_fixture_is_silent() {
    let mut fixture = unplugged(10);
    let mut buffer = [0.0; 512 * 2];
    for _ in 0..64 {
        fixture.render(&mut buffer);
        assert!(buffer.iter().all(|sample| *sample == 0.0));
    }
}

#[test]
fn a_cable_fixture_places_every_cable() {
    let fixture = cables(100);
    assert_eq!(fixture.cables.len(), 100);
    assert_eq!(fixture.placed_routes, 100);
    assert_eq!(fixture.table_routes, 100);
}

#[test]
fn the_culled_fixture_has_ten_outs_fed() {
    let fixture = culled(100);
    assert_eq!(fixture.outs_fed_by("vca"), 10);
}
