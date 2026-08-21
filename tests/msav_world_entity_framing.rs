//! JAR-less CI regression for MSAV world-entity chunk framing.
//!
//! Save4–Save9 (`readLegacyShortChunk`) prefix each world entity with a u16
//! length. Save10+ (`readChunk`) use i32. The old reader always consumed i32,
//! so official short-chunk maps such as archipelago.msav failed with
//! `UnexpectedEof: truncated entity chunk`. GitHub Actions does not have the
//! sibling `../core` tree; these tracked 4×4 fixtures are the always-on gate.
//!
//! Fixtures are frozen bytes (not rebuilt at test time) so a matching writer
//! bug cannot hide a reader regression. They do not depend on `../core`,
//! `desktop.jar`, the network, or a full official map.

use oxide::engine::msav_roundtrip::{apply_msav_entities, load_msav_world};
use oxide::engine::world_stream::{
    embedded_template, msav_world_entity_section, replace_map_from_msav,
};
use oxide::network::listener::fresh_world_from_template;
use oxide::state::game_state::GameState;

const V4: &[u8] = include_bytes!("fixtures/msav-world-entities/v4.msav");
const V5: &[u8] = include_bytes!("fixtures/msav-world-entities/v5.msav");
const V6: &[u8] = include_bytes!("fixtures/msav-world-entities/v6.msav");
const V7: &[u8] = include_bytes!("fixtures/msav-world-entities/v7.msav");
const V8: &[u8] = include_bytes!("fixtures/msav-world-entities/v8.msav");
const V9: &[u8] = include_bytes!("fixtures/msav-world-entities/v9.msav");
const V10: &[u8] = include_bytes!("fixtures/msav-world-entities/v10.msav");
const V11: &[u8] = include_bytes!("fixtures/msav-world-entities/v11.msav");
const V5_ARCHIPELAGO_POLY: &[u8] =
    include_bytes!("fixtures/msav-world-entities/v5-archipelago-poly.msav");

const VERSIONED_UNIT_FIXTURES: &[(i32, &[u8])] = &[
    (4, V4),
    (5, V5),
    (6, V6),
    (7, V7),
    (8, V8),
    (9, V9),
    (10, V10),
    (11, V11),
];

fn assert_chunk_prefix_reproduces_old_bug(msav: &[u8], expect_short: bool) {
    let section = msav_world_entity_section(msav).expect("world-entity section");
    let bytes = section.bytes.as_slice();
    assert!(
        bytes.len() >= 8,
        "v{} world-entity payload too small",
        section.save_version
    );
    let count = i32::from_be_bytes(bytes[0..4].try_into().unwrap());
    assert_eq!(count, 1, "v{} entity count", section.save_version);
    if expect_short {
        let len = u16::from_be_bytes(bytes[4..6].try_into().unwrap()) as usize;
        let mistaken_i32 = i32::from_be_bytes(bytes[4..8].try_into().unwrap());
        let remaining = bytes.len() - 8;
        assert!(
            mistaken_i32 < 0 || (mistaken_i32 as usize) > remaining,
            "v{} fixture must fail the old always-i32 reader \
             (mistaken_len={mistaken_i32}, remaining={remaining})",
            section.save_version
        );
        assert_eq!(
            4 + 2 + len,
            bytes.len(),
            "v{} short chunk must consume the payload",
            section.save_version
        );
    } else {
        let len = i32::from_be_bytes(bytes[4..8].try_into().unwrap());
        assert!(len > 0, "v{} int chunk length", section.save_version);
        let mistaken_u16 = u16::from_be_bytes(bytes[4..6].try_into().unwrap()) as i32;
        assert_ne!(
            mistaken_u16, len,
            "v{} fixture must fail a u16-only reader (len={len}, as_u16={mistaken_u16})",
            section.save_version
        );
        assert_eq!(
            4 + 4 + len as usize,
            bytes.len(),
            "v{} int chunk must consume the payload",
            section.save_version
        );
    }
}

#[test]
fn tracked_v4_to_v11_fixtures_restore_one_unit() {
    for &(version, msav) in VERSIONED_UNIT_FIXTURES {
        assert_chunk_prefix_reproduces_old_bug(msav, version <= 9);
        let section = msav_world_entity_section(msav).unwrap();
        assert_eq!(section.save_version, version);

        let world = load_msav_world(msav, &format!("ci-frame-v{version}"))
            .unwrap_or_else(|err| panic!("v{version} load_msav_world: {err}"));
        assert_eq!(world.enemies().len(), 1, "v{version}");
        let unit = world.enemies().iter().next().unwrap();
        if version <= 5 {
            assert_eq!(unit.id, 3_000_000, "v{version} allocates via nextId");
        } else {
            assert_eq!(unit.id, 42, "v{version} keeps serialized id");
        }
        assert_eq!(unit.entity_class, 3, "v{version}");
        assert_eq!(unit.unit_type, 0, "v{version}");
    }
}

#[test]
fn tracked_v9_short_chunk_is_not_an_int_chunk() {
    assert_chunk_prefix_reproduces_old_bug(V9, true);
    let section = msav_world_entity_section(V9).unwrap();
    assert_eq!(section.save_version, 9);
}

#[test]
fn tracked_v11_int_chunk_is_not_a_short_chunk() {
    assert_chunk_prefix_reproduces_old_bug(V11, false);
    let section = msav_world_entity_section(V11).unwrap();
    assert_eq!(section.save_version, 11);
}

#[test]
fn tracked_archipelago_poly_short_chunk_hosts_without_core_tree() {
    // Same world-entity bytes as official archipelago.msav (Save5, class 18
    // UnitEntityLegacyPoly), wrapped in a 4×4 map so CI never needs ../core.
    assert_chunk_prefix_reproduces_old_bug(V5_ARCHIPELAGO_POLY, true);
    let section = msav_world_entity_section(V5_ARCHIPELAGO_POLY).unwrap();
    assert_eq!(section.save_version, 5);

    let template = replace_map_from_msav(embedded_template(), V5_ARCHIPELAGO_POLY)
        .expect("replace_map_from_msav");
    let world = fresh_world_from_template(
        &GameState::new(),
        template,
        "archipelago-poly".to_string(),
        std::env::temp_dir().join("ci-archipelago-poly.json"),
    )
    .expect("fresh world");
    apply_msav_entities(&world, V5_ARCHIPELAGO_POLY).expect("apply world entities");
    assert_eq!(world.enemies().len(), 1);
    let poly = world.enemies().iter().next().unwrap();
    assert_eq!(poly.entity_class, 18);
    assert_eq!(poly.unit_type, 37);
    assert_eq!(poly.team, 1);
    assert!((poly.health - 220.0).abs() < 0.1);
}
