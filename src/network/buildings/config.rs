//! Strict decoders for the subset of `TypeIO.writeObject` used by buildings.
//!
//! `DynamicTile::config` stores the complete TypeIO object (including its
//! tag).  Domain code must decode through this module instead of indexing the
//! byte vector directly.  A small legacy escape hatch is retained for old
//! persisted logic tiles that stored only the compressed byte-array payload.

use std::io::Read;

pub const TYPEIO_NULL: u8 = 0;
pub const TYPEIO_CONTENT: u8 = 5;
pub const TYPEIO_BYTE_ARRAY: u8 = 14;

pub const CONTENT_ITEM: u8 = 0;
pub const CONTENT_BLOCK: u8 = 1;
pub const CONTENT_LIQUID: u8 = 4;
pub const CONTENT_UNIT: u8 = 6;

// Official LogicBlock.java limits live in the logic domain
// (src/logic/container.rs); re-exported here for the TypeIO boundary.
pub use crate::logic::{LOGIC_MAX_COMPRESSED, LOGIC_MAX_LINKS, LOGIC_MAX_UNCOMPRESSED};

/// Decodes a nullable Content object and checks its content type and ID.
///
/// The outer `Option` reports whether the object is structurally valid; the
/// inner one is Java's nullable content value.
pub fn content_selection(
    object: &[u8],
    content_type: u8,
    maximum_exclusive: i16,
) -> Option<Option<i16>> {
    match object {
        [TYPEIO_NULL] => Some(None),
        [TYPEIO_CONTENT, kind, high, low] if *kind == content_type => {
            let id = i16::from_be_bytes([*high, *low]);
            (0..maximum_exclusive).contains(&id).then_some(Some(id))
        }
        _ => None,
    }
}

pub fn selected_item(object: &[u8]) -> Option<Option<i16>> {
    content_selection(object, CONTENT_ITEM, 22)
}

pub fn selected_liquid(object: &[u8]) -> Option<Option<i16>> {
    // Desktop 158.1 currently registers eleven liquids (IDs 0..10).
    content_selection(object, CONTENT_LIQUID, 11)
}

/// Returns the payload of a complete TypeIO byte[] object.
pub fn byte_array_payload(object: &[u8]) -> Option<&[u8]> {
    let [TYPEIO_BYTE_ARRAY, a, b, c, d, rest @ ..] = object else {
        return None;
    };
    let length = i32::from_be_bytes([*a, *b, *c, *d]);
    let length = usize::try_from(length).ok()?;
    (length == rest.len()).then_some(rest)
}

/// Validates the raw zlib payload produced by `LogicBlock.compress`.
///
/// This function deliberately accepts a raw payload (not the outer TypeIO
/// byte[] object) because it is also used by snapshot/save codecs.  Decoding
/// is bounded to prevent a small malicious config from inflating without
/// limit.
pub fn valid_logic_payload(payload: &[u8]) -> bool {
    // P1: single bounded grammar — the logic domain parser owns the
    // container layout and its limits (LogicBlock.java maxByteLen/
    // maxCompressedLen/maxLinks/maxNameLength).
    crate::logic::parse_logic_container(payload).is_some()
}

/// Extracts a validated logic payload from the canonical TypeIO byte[]
/// object. Raw payloads are accepted only for old persisted worlds.
pub fn logic_payload(object_or_legacy_payload: &[u8]) -> Option<&[u8]> {
    if let Some(payload) = byte_array_payload(object_or_legacy_payload) {
        return valid_logic_payload(payload).then_some(payload);
    }
    valid_logic_payload(object_or_legacy_payload).then_some(object_or_legacy_payload)
}

pub fn valid_logic_object(object: &[u8]) -> bool {
    object == [TYPEIO_NULL] || byte_array_payload(object).is_some_and(valid_logic_payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use std::io::Write;

    fn empty_logic_payload() -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&[1, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn content_selection_is_typed_and_nullable() {
        assert_eq!(selected_item(&[0]), Some(None));
        assert_eq!(selected_item(&[5, 0, 0, 7]), Some(Some(7)));
        assert_eq!(selected_item(&[5, 4, 0, 7]), None);
        assert_eq!(selected_liquid(&[5, 4, 0, 10]), Some(Some(10)));
        assert_eq!(selected_liquid(&[5, 4, 0, 11]), None);
    }

    #[test]
    fn logic_payload_requires_the_byte_array_envelope_at_the_wire_boundary() {
        let payload = empty_logic_payload();
        let mut object = vec![TYPEIO_BYTE_ARRAY];
        object.extend_from_slice(&(payload.len() as i32).to_be_bytes());
        object.extend_from_slice(&payload);
        assert!(valid_logic_object(&object));
        assert_eq!(logic_payload(&object), Some(payload.as_slice()));
        assert!(
            logic_payload(&payload).is_some(),
            "legacy saves stay readable"
        );

        object[4] ^= 1;
        assert!(!valid_logic_object(&object));
    }
}
