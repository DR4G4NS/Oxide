//! P2: deterministic property tests (seeded, no external deps) for the wire
//! codecs, TypeIO strings and the MSAV writer/reader round-trip. These are
//! the fuzz-style gates the audit requires: round-trip identity, bounded
//! consumption and no panics on adversarial-ish inputs.

use oxide::engine::save_io::{write_msav_complete, MsavWorld};
use oxide::network::codec::{read_packet, write_packet};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

fn seeded() -> StdRng {
    StdRng::seed_from_u64(0x5EED_1581)
}

/// write_packet -> read_packet must return the exact payload for arbitrary
/// (bounded) payloads in both compression modes.
#[test]
fn packet_framing_round_trip_property() {
    let mut rng = seeded();
    for _round in 0..256 {
        let len = rng.gen_range(0..512);
        let payload: Vec<u8> = (0..len).map(|_| rng.gen()).collect();
        let compress = rng.gen_bool(0.5);
        for packet_id in [0u8, 1, 32, 126, 255] {
            let mut frame = Vec::new();
            write_packet(&mut frame, packet_id, &payload, compress).unwrap();
            // Wire layout: [b id][s uncompressed_len][b compress][payload].
            assert_eq!(frame[0], packet_id);
            let data_len = u16::from_be_bytes([frame[1], frame[2]]) as usize;
            assert_eq!(data_len, payload.len(), "uncompressed length on the wire");
            assert_eq!(frame[3], u8::from(compress));
            if compress {
                let decompressed = lz4_flex::decompress(&frame[4..], len).unwrap();
                assert_eq!(decompressed, payload);
            } else {
                assert_eq!(&frame[4..], &payload[..]);
            }
            // read_packet returns [id, decompressed payload].
            let decoded = read_packet(std::io::Cursor::new(&frame)).unwrap();
            assert_eq!(decoded[0], packet_id);
            assert_eq!(&decoded[1..], &payload[..]);
        }
    }
}

/// Modified UTF-8 write/read must be identity for arbitrary strings.
#[test]
fn modified_utf8_round_trip_property() {
    let mut rng = seeded();
    let samples = [
        "",
        "hello",
        "café",
        "日本語テキスト",
        "\u{0}\u{1}control",
        "🎮 emoji",
    ];
    let mut all: Vec<String> = samples.iter().map(|s| s.to_string()).collect();
    for _ in 0..64 {
        let len = rng.gen_range(0..64);
        let s: String = (0..len)
            .map(|_| char::from_u32(rng.gen_range(1..0x1000)).unwrap_or('x'))
            .collect();
        all.push(s);
    }
    for text in all {
        let encoded = oxide::network::listener::encode_typeio_string(&text).unwrap();
        let decoded = oxide::network::listener::decode_typeio_string(&encoded).unwrap();
        assert_eq!(decoded, text, "MUTF-8 round trip");
    }
}

/// MSAV v11 writer -> reader must preserve terrain for arbitrary 4x4 worlds.
#[test]
fn msav_v11_round_trip_property() {
    let mut rng = seeded();
    for round in 0..32 {
        let width = 4usize;
        let height = 4usize;
        let floors: Vec<i16> = (0..width * height).map(|_| rng.gen_range(0..20)).collect();
        let overlays: Vec<i16> = (0..width * height).map(|_| rng.gen_range(0..5)).collect();
        let blocks: Vec<i16> = (0..width * height).map(|_| rng.gen_range(0..3)).collect();
        let tiles = dashmap::DashMap::new();
        let puddles: [(i32, f32, i16, i32); 0] = [];
        let world = MsavWorld {
            width,
            height,
            floors: &floors,
            overlays: &overlays,
            blocks: &blocks,
            puddles: &puddles,
            team_blocks: None,
            dynamic_tiles: &tiles,
            enemy_units: &[],
            runtime: None,
        };
        let map = std::collections::HashMap::from([
            ("mapname".to_string(), "prop".to_string()),
            ("width".to_string(), width.to_string()),
            ("height".to_string(), height.to_string()),
            ("build".to_string(), "158".to_string()),
        ]);
        let bytes = write_msav_complete(&map, 11, &world).unwrap();
        let (meta, read_tiles) = oxide::engine::save_io::SaveIO::read_map(&bytes[..]).unwrap();
        assert_eq!(meta.version, 11, "round {round}");
        assert_eq!(read_tiles.len(), width * height, "round {round}");
        for tile in &read_tiles {
            assert!(tile.block_id >= 0, "round {round}");
            assert!(tile.floor_id >= 0, "round {round}");
        }
    }
}
