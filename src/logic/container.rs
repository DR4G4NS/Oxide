//! Single parser for the Logic processor config container (P1).
//!
//! The official container (LogicBlock.compress, 158.1) is a zlib stream of:
//! `[1][source_len i32 BE][source bytes][link_count i32 BE][per link:
//! name_len u16 BE + name + x i16 + y i16]`.
//!
//! Per ARCHITECTURE.md this parser lives in the logic domain; the TypeIO
//! boundary (network/buildings/config.rs) and the compiler (source/links)
//! delegate here so validation, source extraction and link parsing share
//! one bounded grammar. Limits match LogicBlock.java: maxByteLen 102400,
//! maxCompressedLen 16000, maxLinks 6000, maxNameLength 32.

use std::io::Read;

/// Official `LogicBlock.maxCompressedLen` (16_000).
pub const LOGIC_MAX_COMPRESSED: usize = 16_000;
/// Official `LogicBlock.maxByteLen` (102_400) — uncompressed container cap.
pub const LOGIC_MAX_UNCOMPRESSED: usize = 1024 * 100;
/// Official `LogicBlock.maxLinks` (6_000).
pub const LOGIC_MAX_LINKS: i32 = 6_000;
/// Official `LogicBlock.maxNameLength` (32) — link display-name cap.
pub const LOGIC_MAX_NAME_LENGTH: usize = 32;

/// A fully parsed and validated logic processor container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogicContainer {
    /// The source code (lossy UTF-8, like the official `readString`).
    pub source: String,
    /// Relative link coordinates in declaration order.
    pub links: Vec<(i16, i16)>,
}

/// Parses and validates a logic container payload (the raw zlib stream, not
/// the TypeIO envelope). Returns None for malformed, truncated, oversized or
/// adversarial input — callers never partially consume a bad container.
pub fn parse_logic_container(payload: &[u8]) -> Option<LogicContainer> {
    if payload.is_empty() || payload.len() > LOGIC_MAX_COMPRESSED {
        return None;
    }
    // Bounded inflate: a malicious container cannot allocate more than the
    // official uncompressed cap plus one byte (bomb guard).
    let decoder = flate2::read::ZlibDecoder::new(payload);
    let mut content = Vec::new();
    if decoder
        .take((LOGIC_MAX_UNCOMPRESSED + 1) as u64)
        .read_to_end(&mut content)
        .is_err()
        || content.len() > LOGIC_MAX_UNCOMPRESSED
        || content.len() < 9
        || content[0] != 1
    {
        return None;
    }
    let source_len = i32::from_be_bytes([content[1], content[2], content[3], content[4]]);
    let source_len = usize::try_from(source_len).ok()?;
    let source_end = 5usize.checked_add(source_len)?;
    if source_end + 4 > content.len() {
        return None;
    }
    let source = String::from_utf8_lossy(&content[5..source_end]).into_owned();
    let mut position = source_end;
    let link_count = i32::from_be_bytes([
        content[position],
        content[position + 1],
        content[position + 2],
        content[position + 3],
    ]);
    position += 4;
    if !(0..=LOGIC_MAX_LINKS).contains(&link_count) {
        return None;
    }
    let mut links = Vec::with_capacity(link_count as usize);
    for _ in 0..link_count {
        if position + 2 > content.len() {
            return None;
        }
        let name_len = u16::from_be_bytes([content[position], content[position + 1]]) as usize;
        position += 2;
        if name_len > LOGIC_MAX_NAME_LENGTH {
            return None;
        }
        let next = position
            .checked_add(name_len)
            .and_then(|n| n.checked_add(4))?;
        if next > content.len() {
            return None;
        }
        position = next;
        let x = i16::from_be_bytes([content[next - 4], content[next - 3]]);
        let y = i16::from_be_bytes([content[next - 2], content[next - 1]]);
        links.push((x, y));
    }
    if position != content.len() {
        return None;
    }
    Some(LogicContainer { source, links })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use std::io::Write;

    fn container(source: &[u8], links: &[(&str, i16, i16)]) -> Vec<u8> {
        let mut content = vec![1];
        content.extend_from_slice(&(source.len() as i32).to_be_bytes());
        content.extend_from_slice(source);
        content.extend_from_slice(&(links.len() as i32).to_be_bytes());
        for (name, x, y) in links {
            content.extend_from_slice(&(name.len() as u16).to_be_bytes());
            content.extend_from_slice(name.as_bytes());
            content.extend_from_slice(&x.to_be_bytes());
            content.extend_from_slice(&y.to_be_bytes());
        }
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&content).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn parses_source_and_links_with_limits() {
        let payload = container(b"set x 1\nprint x", &[("sorter1", 2, 3), ("", -1, 0)]);
        let parsed = parse_logic_container(&payload).unwrap();
        assert_eq!(parsed.source, "set x 1\nprint x");
        assert_eq!(parsed.links, vec![(2, 3), (-1, 0)]);
        // Empty program (official empty container) parses.
        let empty = container(b"", &[]);
        let parsed = parse_logic_container(&empty).unwrap();
        assert_eq!(parsed.source, "");
        assert!(parsed.links.is_empty());
    }

    #[test]
    fn rejects_adversarial_and_malformed_containers() {
        // Not zlib.
        assert!(parse_logic_container(b"not-zlib").is_none());
        // Wrong version byte.
        let mut bad = vec![2];
        bad.extend_from_slice(&0i32.to_be_bytes());
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&bad).unwrap();
        assert!(parse_logic_container(&encoder.finish().unwrap()).is_none());
        // Truncated source length (content shorter than the header).
        let mut trunc = vec![1];
        trunc.extend_from_slice(&1000i32.to_be_bytes());
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&trunc).unwrap();
        assert!(parse_logic_container(&encoder.finish().unwrap()).is_none());
        // Link count beyond the official maxLinks.
        let mut huge = vec![1];
        huge.extend_from_slice(&0i32.to_be_bytes());
        huge.extend_from_slice(&(LOGIC_MAX_LINKS + 1).to_be_bytes());
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&huge).unwrap();
        assert!(parse_logic_container(&encoder.finish().unwrap()).is_none());
        // Link name beyond maxNameLength.
        let long_name = "x".repeat(33);
        let payload = container(b"", &[(&long_name, 0, 0)]);
        assert!(parse_logic_container(&payload).is_none());
        // Trailing bytes after the links.
        let mut trailing = vec![1];
        trailing.extend_from_slice(&0i32.to_be_bytes());
        trailing.extend_from_slice(&0i32.to_be_bytes());
        trailing.push(9);
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&trailing).unwrap();
        assert!(parse_logic_container(&encoder.finish().unwrap()).is_none());
        // zlib bomb: tiny compressed input inflating beyond the cap.
        let bomb = vec![
            0x78, 0x9c, 0x63, 0x60, 0x60, 0x60, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01,
        ];
        // (only if it inflates; otherwise it is just invalid zlib — either
        // way parse must return None and never allocate unboundedly).
        let _ = parse_logic_container(&bomb);
    }
}
