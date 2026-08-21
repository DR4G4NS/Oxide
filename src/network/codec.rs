#![allow(dead_code)]

use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use std::io::{Error, ErrorKind, Read, Write};
use tracing::{debug, error, trace};

pub trait Reads: Read {
    fn read_b(&mut self) -> std::io::Result<u8> {
        self.read_u8()
    }

    fn read_bool(&mut self) -> std::io::Result<bool> {
        Ok(self.read_u8()? != 0)
    }

    fn read_s(&mut self) -> std::io::Result<i16> {
        self.read_i16::<BigEndian>()
    }

    fn read_us(&mut self) -> std::io::Result<u16> {
        self.read_u16::<BigEndian>()
    }

    fn read_i(&mut self) -> std::io::Result<i32> {
        self.read_i32::<BigEndian>()
    }

    fn read_f(&mut self) -> std::io::Result<f32> {
        self.read_f32::<BigEndian>()
    }

    fn read_d(&mut self) -> std::io::Result<f64> {
        self.read_f64::<BigEndian>()
    }

    fn read_l(&mut self) -> std::io::Result<i64> {
        self.read_i64::<BigEndian>()
    }

    fn read_utf(&mut self) -> std::io::Result<String> {
        read_modified_utf8_public(self)
    }

    fn read_typeio_string(&mut self) -> std::io::Result<Option<String>> {
        let tag = self.read_u8()?;
        if tag == 0 {
            return Ok(None);
        }
        read_modified_utf8_public(self).map(Some)
    }
}

/// Java `DataInput.readUTF` (modified UTF-8): u16 byte length followed by the
/// MUTF-8 payload. Unlike plain UTF-8, code points above U+FFFF are encoded as
/// a 6-byte surrogate pair and NUL as 0xC0 0x80, so byte counts differ from
/// `String::as_bytes` for emoji etc. The official client uses
/// `DataOutput.writeUTF`/`DataInput.readUTF` (arc `Writes.str`/`Reads.str`)
/// for every protocol string (names, chat, messages, TypeIO strings), so a
/// plain-UTF-8 codec desyncs the stream whenever a multibyte character is
/// present.
pub fn read_modified_utf8_public<R: Read + ?Sized>(r: &mut R) -> std::io::Result<String> {
    let len = r.read_u16::<BigEndian>()?;
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    let mut out = String::with_capacity(len as usize);
    let mut i = 0usize;
    while i < buf.len() {
        let b = buf[i];
        if b & 0x80 == 0 {
            out.push(b as char);
            i += 1;
        } else if b & 0xE0 == 0xC0 {
            if i + 1 >= buf.len() {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "truncated MUTF-8 2-byte",
                ));
            }
            let c = (((b & 0x1F) as u32) << 6) | ((buf[i + 1] & 0x3F) as u32);
            out.push(
                char::from_u32(c).ok_or_else(|| {
                    Error::new(ErrorKind::InvalidData, "invalid MUTF-8 code point")
                })?,
            );
            i += 2;
        } else if b & 0xF0 == 0xE0 {
            if i + 2 >= buf.len() {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "truncated MUTF-8 3-byte",
                ));
            }
            let c = (((b & 0x0F) as u32) << 12)
                | (((buf[i + 1] & 0x3F) as u32) << 6)
                | ((buf[i + 2] & 0x3F) as u32);
            // Surrogate halves are combined back into the supplementary code
            // point exactly like DataInput.readUTF.
            if (0xD800..0xDC00).contains(&c) {
                if i + 5 >= buf.len() {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        "truncated surrogate pair",
                    ));
                }
                let b2 = buf[i + 3];
                let c2 = (((b2 & 0x0F) as u32) << 12)
                    | (((buf[i + 4] & 0x3F) as u32) << 6)
                    | ((buf[i + 5] & 0x3F) as u32);
                if !(0xDC00..0xE000).contains(&c2) {
                    return Err(Error::new(ErrorKind::InvalidData, "invalid low surrogate"));
                }
                let combined = 0x10000 + ((c - 0xD800) << 10) + (c2 - 0xDC00);
                out.push(char::from_u32(combined).ok_or_else(|| {
                    Error::new(ErrorKind::InvalidData, "invalid supplementary code point")
                })?);
                i += 6;
            } else {
                out.push(char::from_u32(c).ok_or_else(|| {
                    Error::new(ErrorKind::InvalidData, "invalid MUTF-8 code point")
                })?);
                i += 3;
            }
        } else {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "invalid MUTF-8 lead byte",
            ));
        }
    }
    Ok(out)
}

impl<R: Read + ?Sized> Reads for R {}

pub trait Writes: Write {
    fn write_b(&mut self, val: u8) -> std::io::Result<()> {
        self.write_u8(val)
    }

    fn write_bool(&mut self, val: bool) -> std::io::Result<()> {
        self.write_u8(if val { 1 } else { 0 })
    }

    fn write_s(&mut self, val: i16) -> std::io::Result<()> {
        self.write_i16::<BigEndian>(val)
    }

    fn write_us(&mut self, val: u16) -> std::io::Result<()> {
        self.write_u16::<BigEndian>(val)
    }

    fn write_d(&mut self, val: f64) -> std::io::Result<()> {
        self.write_f64::<BigEndian>(val)
    }

    fn write_i(&mut self, val: i32) -> std::io::Result<()> {
        self.write_i32::<BigEndian>(val)
    }

    fn write_f(&mut self, val: f32) -> std::io::Result<()> {
        self.write_f32::<BigEndian>(val)
    }

    fn write_l(&mut self, val: i64) -> std::io::Result<()> {
        self.write_i64::<BigEndian>(val)
    }

    fn write_utf(&mut self, val: &str) -> std::io::Result<()> {
        let bytes = encode_modified_utf8(val);
        let len = u16::try_from(bytes.len()).map_err(|_| {
            Error::new(ErrorKind::InvalidInput, "string exceeds 65535 MUTF-8 bytes")
        })?;
        self.write_u16::<BigEndian>(len)?;
        self.write_all(&bytes)
    }

    fn write_typeio_string(&mut self, val: Option<&str>) -> std::io::Result<()> {
        match val {
            Some(v) => {
                self.write_u8(1)?;
                self.write_utf(v)
            }
            None => self.write_u8(0),
        }
    }
}

/// Java `DataOutput.writeUTF` (modified UTF-8). ASCII is 1 byte; U+0000 is
/// 0xC0 0x80; U+0080..U+07FF is 2 bytes; U+0800..U+FFFF is 3 bytes; code
/// points above U+FFFF are encoded as a 6-byte surrogate pair (3 bytes per
/// surrogate half). Byte-for-byte identical to what the official client
/// produces with `Writes.str`, which keeps every protocol string in sync.
pub fn encode_modified_utf8(val: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(val.len() + val.len() / 2);
    for ch in val.chars() {
        let c = ch as u32;
        if c == 0 {
            out.extend_from_slice(&[0xC0, 0x80]);
        } else if c <= 0x7F {
            out.push(c as u8);
        } else if c <= 0x7FF {
            out.push(0xC0 | ((c >> 6) as u8));
            out.push(0x80 | ((c & 0x3F) as u8));
        } else if c <= 0xFFFF {
            out.push(0xE0 | ((c >> 12) as u8));
            out.push(0x80 | (((c >> 6) & 0x3F) as u8));
            out.push(0x80 | ((c & 0x3F) as u8));
        } else {
            // supplementary plane: surrogate pair, 3 bytes each
            let v = c - 0x10000;
            let hi = 0xD800 + (v >> 10);
            let lo = 0xDC00 + (v & 0x3FF);
            for s in [hi, lo] {
                out.push(0xE0 | ((s >> 12) as u8));
                out.push(0x80 | (((s >> 6) & 0x3F) as u8));
                out.push(0x80 | ((s & 0x3F) as u8));
            }
        }
    }
    out
}

impl<W: Write + ?Sized> Writes for W {}

pub fn compress_lz4(data: &[u8]) -> Vec<u8> {
    lz4_flex::block::compress(data)
}

pub fn decompress_lz4(
    data: &[u8],
    uncompressed_size: usize,
) -> Result<Vec<u8>, lz4_flex::block::DecompressError> {
    lz4_flex::block::decompress(data, uncompressed_size)
}

pub const FRAMEWORK_MESSAGE_ID: i8 = -2;
/// Mindustry creates its ArcNet server with a 32 KiB TCP read buffer. Although
/// the wire prefix is a u16, larger objects are rejected by the official peer.
pub const MAX_PACKET_SIZE: usize = 32 * 1024;
pub const PACKET_HEADER_SIZE: usize = 4;
pub const MAX_PAYLOAD_SIZE: usize = MAX_PACKET_SIZE - PACKET_HEADER_SIZE;
/// `ArcNetProvider.PacketSerializer`: payloads shorter than this skip LZ4
/// (`length < 36 || pack instanceof StreamChunk`).
pub const PACKET_COMPRESS_MIN_BYTES: usize = 36;
/// Framework `StreamChunk` packet id (Net.packetId 1). Never LZ4-compressed.
pub const STREAM_CHUNK_WIRE_ID: u8 = 1;

/// Java `PacketSerializer.write` compression predicate (desktop 158.1).
pub fn should_lz4_compress(packet_id: u8, uncompressed_len: usize) -> bool {
    uncompressed_len >= PACKET_COMPRESS_MIN_BYTES && packet_id != STREAM_CHUNK_WIRE_ID
}

pub fn read_packet<R: Read>(mut r: R) -> std::io::Result<Vec<u8>> {
    let id = r.read_b()?;
    if id as i8 == FRAMEWORK_MESSAGE_ID {
        let mut buf = vec![id];
        r.read_to_end(&mut buf)?;
        trace!("Read framework message: {} bytes", buf.len());
        return Ok(buf);
    }

    let packet_id = id;
    let uncompressed_len = r.read_us()? as usize;
    if uncompressed_len > MAX_PACKET_SIZE {
        error!(
            "Packet payload size {} exceeds maximum limit of {}",
            uncompressed_len, MAX_PACKET_SIZE
        );
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "Packet payload size {} exceeds maximum limit of {}",
                uncompressed_len, MAX_PACKET_SIZE
            ),
        ));
    }

    let compressed = r.read_b()? != 0;

    // Read all remaining bytes in the packet frame
    let mut payload = Vec::new();
    r.read_to_end(&mut payload)?;
    trace!(
        "Read packet payload: id={}, uncompressed_len={}, compressed={}, payload_len={}",
        packet_id,
        uncompressed_len,
        compressed,
        payload.len()
    );

    let decompressed = if compressed {
        decompress_lz4(&payload, uncompressed_len)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?
    } else {
        if payload.len() != uncompressed_len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "Uncompressed packet payload size {} does not match expected length {}",
                    payload.len(),
                    uncompressed_len
                ),
            ));
        }
        payload
    };

    let mut final_buf = vec![packet_id];
    final_buf.extend(decompressed);
    Ok(final_buf)
}

pub fn write_packet<W: Write>(
    mut w: W,
    packet_id: u8,
    payload: &[u8],
    compress: bool,
) -> std::io::Result<()> {
    if payload.len() > MAX_PAYLOAD_SIZE || payload.len() > u16::MAX as usize {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("packet payload is too large: {} bytes", payload.len()),
        ));
    }
    w.write_b(packet_id)?;
    if compress {
        let compressed = compress_lz4(payload);
        w.write_us(payload.len() as u16)?;
        w.write_b(1)?;
        w.write_all(&compressed)?;
        trace!(
            "Wrote compressed packet: id={}, orig_len={}, comp_len={}",
            packet_id,
            payload.len(),
            compressed.len()
        );
    } else {
        w.write_us(payload.len() as u16)?;
        w.write_b(0)?;
        w.write_all(payload)?;
        trace!(
            "Wrote uncompressed packet: id={}, len={}",
            packet_id,
            payload.len()
        );
    }
    Ok(())
}

/// Reads a TCP packet prefixed with Kryonet/ArcNet 2-byte length header
pub fn read_tcp_packet<R: Read>(mut r: R) -> std::io::Result<Vec<u8>> {
    let tcp_len = r.read_u16::<BigEndian>()? as usize;
    if tcp_len > MAX_PACKET_SIZE {
        error!(
            "TCP Packet size {} exceeds maximum limit of {}",
            tcp_len, MAX_PACKET_SIZE
        );
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "TCP Packet size {} exceeds maximum limit of {}",
                tcp_len, MAX_PACKET_SIZE
            ),
        ));
    }
    let mut tcp_buf = vec![0u8; tcp_len];
    r.read_exact(&mut tcp_buf)?;
    debug!("Read TCP packet frame of length {}", tcp_len);
    read_packet(std::io::Cursor::new(tcp_buf))
}

/// PacketSerializer body using the official 158.1 LZ4 threshold.
pub fn write_serialized_packet<W: Write>(
    w: W,
    packet_id: u8,
    payload: &[u8],
) -> std::io::Result<()> {
    write_packet(
        w,
        packet_id,
        payload,
        should_lz4_compress(packet_id, payload.len()),
    )
}

/// Writes a TCP packet prefixed with Kryonet/ArcNet 2-byte length header
pub fn write_tcp_packet<W: Write>(
    mut w: W,
    packet_id: u8,
    payload: &[u8],
    compress: bool,
) -> std::io::Result<()> {
    let mut frame = Vec::new();
    write_packet(&mut frame, packet_id, payload, compress)?;
    let tcp_len = u16::try_from(frame.len()).map_err(|_| {
        Error::new(
            ErrorKind::InvalidInput,
            "serialized ArcNet frame exceeds 65535 bytes",
        )
    })?;
    w.write_u16::<BigEndian>(tcp_len)?;
    w.write_all(&frame)?;
    debug!("Wrote TCP packet frame of length {}", tcp_len);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modified_utf8_matches_java_writeutf_semantics() {
        // Emoji (U+1F600) is 4 bytes in UTF-8 but 6 bytes (surrogate pair)
        // in Java modified UTF-8; NUL is 2 bytes; ASCII is unchanged.
        let emoji = "a😀b";
        let bytes = encode_modified_utf8(emoji);
        assert_eq!(bytes.len(), 1 + 6 + 1);
        // high surrogate 0xD83D -> ED A0 BD, low surrogate 0xDE00 -> ED B8 80
        assert_eq!(&bytes[1..4], &[0xED, 0xA0, 0xBD]);
        assert_eq!(&bytes[4..7], &[0xED, 0xB8, 0x80]);

        let nul = encode_modified_utf8("\u{0}");
        assert_eq!(nul, vec![0xC0, 0x80]);

        // round-trip through the Read trait
        let mut data = Vec::new();
        data.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
        data.extend_from_slice(&bytes);
        let decoded = read_modified_utf8_public(&mut std::io::Cursor::new(&data)).unwrap();
        assert_eq!(decoded, emoji);

        // accented latin-1 (2 bytes in both encodings)
        let acc = encode_modified_utf8("éñ");
        assert_eq!(acc, "éñ".as_bytes());
        let mut data = Vec::new();
        data.extend_from_slice(&(acc.len() as u16).to_be_bytes());
        data.extend_from_slice(&acc);
        assert_eq!(
            read_modified_utf8_public(&mut std::io::Cursor::new(&data)).unwrap(),
            "éñ"
        );

        // CJK (3 bytes in both)
        let cjk = encode_modified_utf8("中文");
        assert_eq!(cjk, "中文".as_bytes());
    }

    #[test]
    fn typeio_string_uses_modified_utf8() {
        use std::io::Write as _;
        let mut out = Vec::new();
        out.write_typeio_string(Some("test 😀")).unwrap();
        let mut cur = std::io::Cursor::new(&out[..]);
        assert_eq!(
            cur.read_typeio_string().unwrap().as_deref(),
            Some("test 😀")
        );
    }

    #[test]
    fn packet_serializer_lz4_threshold_matches_arcnet_provider() {
        let small = vec![7u8; PACKET_COMPRESS_MIN_BYTES - 1];
        assert!(!should_lz4_compress(46, small.len()));
        let mut body = Vec::new();
        write_serialized_packet(&mut body, 46, &small).unwrap();
        assert_eq!(body[3], 0);

        let large = vec![7u8; PACKET_COMPRESS_MIN_BYTES];
        assert!(should_lz4_compress(46, large.len()));
        let mut body = Vec::new();
        write_serialized_packet(&mut body, 46, &large).unwrap();
        assert_eq!(body[3], 1);
        assert!(should_lz4_compress(0, large.len()));
        assert!(!should_lz4_compress(STREAM_CHUNK_WIRE_ID, large.len()));
    }

    #[test]
    fn truncated_serializer_body_is_rejected() {
        assert!(read_packet(std::io::Cursor::new(&[] as &[u8])).is_err());
        assert!(read_packet(std::io::Cursor::new(&[46u8])).is_err());
        assert!(read_packet(std::io::Cursor::new(&[46u8, 0, 4])).is_err());
        assert!(read_packet(std::io::Cursor::new(&[46u8, 0, 4, 0, 1])).is_err());
    }
}
