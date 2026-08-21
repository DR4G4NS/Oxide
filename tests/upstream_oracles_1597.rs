//! Independently authored mirrors of selected Mindustry v159.7 test
//! contracts.  The Java test source is an external oracle; this file only
//! exercises the Rust wire behavior exposed by the server crate.

use oxide::network::codec::{Reads, Writes};
use oxide::network::packets::{ConnectPacket, Packet};
use oxide::network::rpc::{RpcPacket, SEND_CHAT_MESSAGE_PACKET_ID};
use std::io::Cursor;

/// Independent mirror of v159.7 `ApplicationTests.writeStringTest` for the
/// TypeIO string grammar and the packet fields implemented by this server.
#[test]
fn upstream_application_write_string_1597_typeio_and_packets() {
    let long = "x".repeat(512);
    let values = [
        None,
        Some("a".to_string()),
        Some("asd asd asd asd asdagagasasjakbgeah;jwrej 23424234".to_string()),
        Some("这个服务器可以用自己的语言说话".to_string()),
        Some("🚢".to_string()),
        Some(long),
    ];

    for value in &values {
        let mut bytes = Vec::new();
        bytes.write_typeio_string(value.as_deref()).unwrap();
        let expected = match value.as_deref() {
            None => vec![0],
            Some("a") => vec![1, 0, 1, b'a'],
            Some("🚢") => vec![1, 0, 6, 0xed, 0xa0, 0xbd, 0xed, 0xba, 0xa2],
            Some("这个服务器可以用自己的语言说话") => {
                let raw = "这个服务器可以用自己的语言说话".as_bytes();
                let mut v = vec![1, 0, raw.len() as u8];
                v.extend_from_slice(raw);
                v
            }
            Some(s) if s.len() == 512 => {
                let mut v = vec![1, 2, 0];
                v.extend(std::iter::repeat_n(b'x', 512));
                v
            }
            Some(_) => continue,
        };
        assert_eq!(bytes, expected, "independent v159.7 TypeIO fixture");
        let decoded = Cursor::new(bytes).read_typeio_string().unwrap();
        assert_eq!(decoded.as_deref(), value.as_deref());

        let Some(name) = value else {
            continue;
        };

        let expected = ConnectPacket {
            version: 159,
            version_type: "official".into(),
            name: name.clone(),
            locale: "es_MX".into(),
            usid: "session".into(),
            uuid: "AQEBAQEBAQEBAQEBAQEBAQ==".into(), // 16-byte UUID, v159.7 layout
            mobile: false,
            color: 0x1122_3344,
            mods: vec!["example-mod".into()],
        };
        let mut packet_bytes = Vec::new();
        Packet::ConnectPacket(expected.clone())
            .write(&mut packet_bytes)
            .unwrap();
        let decoded = Packet::read(Cursor::new(packet_bytes), 3).unwrap();
        let Packet::ConnectPacket(actual) = decoded else {
            panic!("expected ConnectPacket");
        };
        assert_eq!(actual.name, expected.name);
        assert_eq!(actual.mods, expected.mods);
        assert_eq!(actual.uuid, expected.uuid);

        let chat = RpcPacket::SendChatMessage {
            player_id: 17,
            message: name.clone(),
        };
        let mut chat_bytes = Vec::new();
        chat.write(&mut chat_bytes).unwrap();
        let decoded =
            RpcPacket::read(Cursor::new(chat_bytes), SEND_CHAT_MESSAGE_PACKET_ID).unwrap();
        assert!(
            matches!(decoded, RpcPacket::SendChatMessage { message, .. } if message.as_str() == name.as_str())
        );
    }
}
