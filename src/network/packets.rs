#![allow(dead_code)]

use super::codec::{Reads, Writes};
use byteorder::{ReadBytesExt, WriteBytesExt};
use std::io::{Error, ErrorKind, Read, Write};

#[derive(Debug, Clone)]
pub enum KickReason {
    Kick,
    ClientOutdated,
    ServerOutdated,
    Banned,
    Gameover,
    RecentKick,
    NameInUse,
    IdInUse,
    NameEmpty,
    CustomClient,
    ServerClose,
    Vote,
    TypeMismatch,
    Whitelist,
    PlayerLimit,
    ServerRestarting,
    Custom(String),
}

impl KickReason {
    pub fn read<R: Read>(mut r: R) -> std::io::Result<Self> {
        let id = r.read_u8()?;
        match id {
            0 => Ok(Self::Kick),
            1 => Ok(Self::ClientOutdated),
            2 => Ok(Self::ServerOutdated),
            3 => Ok(Self::Banned),
            4 => Ok(Self::Gameover),
            5 => Ok(Self::RecentKick),
            6 => Ok(Self::NameInUse),
            7 => Ok(Self::IdInUse),
            8 => Ok(Self::NameEmpty),
            9 => Ok(Self::CustomClient),
            10 => Ok(Self::ServerClose),
            11 => Ok(Self::Vote),
            12 => Ok(Self::TypeMismatch),
            13 => Ok(Self::Whitelist),
            14 => Ok(Self::PlayerLimit),
            15 => Ok(Self::ServerRestarting),
            16 => Ok(Self::Custom(r.read_utf()?)),
            _ => Err(Error::new(ErrorKind::InvalidData, "Unknown kick reason")),
        }
    }

    pub fn write<W: Write>(&self, mut w: W) -> std::io::Result<()> {
        match self {
            Self::Kick => w.write_u8(0),
            Self::ClientOutdated => w.write_u8(1),
            Self::ServerOutdated => w.write_u8(2),
            Self::Banned => w.write_u8(3),
            Self::Gameover => w.write_u8(4),
            Self::RecentKick => w.write_u8(5),
            Self::NameInUse => w.write_u8(6),
            Self::IdInUse => w.write_u8(7),
            Self::NameEmpty => w.write_u8(8),
            Self::CustomClient => w.write_u8(9),
            Self::ServerClose => w.write_u8(10),
            Self::Vote => w.write_u8(11),
            Self::TypeMismatch => w.write_u8(12),
            Self::Whitelist => w.write_u8(13),
            Self::PlayerLimit => w.write_u8(14),
            Self::ServerRestarting => w.write_u8(15),
            Self::Custom(s) => {
                w.write_u8(16)?;
                w.write_utf(s)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum AdminAction {
    Ban,
    Kick,
    Trace,
}

#[derive(Debug, Clone)]
pub struct ConnectPacket {
    pub version: i32,
    pub version_type: String,
    pub name: String,
    pub locale: String,
    pub usid: String,
    pub uuid: String,
    pub mobile: bool,
    pub color: i32,
    pub mods: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Connect {
    pub uuid: String,
    pub usid: String,
    pub address: String,
}

#[derive(Debug, Clone)]
pub struct Disconnect {
    pub reason: KickReason,
}

#[derive(Debug, Clone)]
pub struct StreamBegin {}

#[derive(Debug, Clone)]
pub struct StreamChunk {
    pub id: i32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct WorldStream {}

#[derive(Debug, Clone)]
pub struct AssetRequirementStream {}

#[derive(Debug, Clone)]
pub struct AssetStream {}

#[derive(Debug, Clone)]
pub struct KeepAlive {}

#[derive(Debug, Clone)]
pub struct Ping {}

#[derive(Debug, Clone)]
pub enum Packet {
    ConnectPacket(ConnectPacket),
    Connect(Connect),
    Disconnect(Disconnect),
    StreamBegin(StreamBegin),
    StreamChunk(StreamChunk),
    WorldStream(WorldStream),
    AssetRequirementStream(AssetRequirementStream),
    AssetStream(AssetStream),
    KeepAlive(KeepAlive),
    Ping(Ping),
    /// Generated Call packets are build-dependent. Keeping their raw payload
    /// prevents a newer client packet from killing the transport connection.
    Unknown {
        id: u8,
        payload: Vec<u8>,
    },
}

impl Packet {
    pub fn write<W: Write>(&self, mut w: W) -> std::io::Result<()> {
        match self {
            Packet::ConnectPacket(p) => {
                use base64::{engine::general_purpose, Engine as _};
                w.write_i(p.version)?;
                w.write_typeio_string(Some(&p.version_type))?;
                w.write_typeio_string(Some(&p.name))?;
                w.write_typeio_string(Some(&p.locale))?;
                w.write_typeio_string(Some(&p.usid))?;
                let uuid = general_purpose::STANDARD.decode(&p.uuid).map_err(|err| {
                    Error::new(ErrorKind::InvalidInput, format!("invalid UUID: {err}"))
                })?;
                // Official desktop `Platform.getUUID` is 8 decoded bytes.
                // Oxide tests and load harnesses also use a 16-byte identity.
                if uuid.len() != 8 && uuid.len() != 16 {
                    return Err(Error::new(
                        ErrorKind::InvalidInput,
                        "Mindustry UUID must decode to 8 or 16 bytes",
                    ));
                }
                w.write_all(&uuid)?;
                let mut crc = crc32fast::Hasher::new();
                crc.update(&uuid);
                w.write_l(crc.finalize() as i64)?;
                w.write_b(u8::from(p.mobile))?;
                w.write_i(p.color)?;
                let mod_count = u8::try_from(p.mods.len())
                    .map_err(|_| Error::new(ErrorKind::InvalidInput, "too many mods"))?;
                w.write_b(mod_count)?;
                for module in &p.mods {
                    w.write_typeio_string(Some(module))?;
                }
            }
            Packet::Connect(p) => {
                w.write_typeio_string(Some(&p.uuid))?;
                w.write_typeio_string(Some(&p.usid))?;
                w.write_typeio_string(Some(&p.address))?;
            }
            Packet::Disconnect(p) => {
                p.reason.write(&mut w)?;
            }
            Packet::StreamBegin(_) => {}
            Packet::StreamChunk(p) => {
                w.write_i(p.id)?;
                w.write_us(p.data.len() as u16)?;
                w.write_all(&p.data)?;
            }
            Packet::WorldStream(_) => {}
            Packet::AssetRequirementStream(_) => {}
            Packet::AssetStream(_) => {}
            Packet::KeepAlive(_) => {}
            Packet::Ping(_) => {}
            Packet::Unknown { payload, .. } => w.write_all(payload)?,
        }
        Ok(())
    }
}

fn uuid_crc_matches(uuid: &[u8], crc_be: [u8; 8]) -> bool {
    let sent = i64::from_be_bytes(crc_be) as u64;
    let mut crc = crc32fast::Hasher::new();
    crc.update(uuid);
    sent == u64::from(crc.finalize())
}

/// Split the ConnectPacket tail after `usid` into base64 uuid + remaining body.
fn split_connect_uuid(identity: &[u8]) -> std::io::Result<(String, &[u8])> {
    use base64::{engine::general_purpose, Engine as _};
    if identity.len() >= 16 {
        let mut crc = [0u8; 8];
        crc.copy_from_slice(&identity[8..16]);
        if uuid_crc_matches(&identity[..8], crc) {
            return Ok((
                general_purpose::STANDARD.encode(&identity[..8]),
                &identity[16..],
            ));
        }
    }
    if identity.len() >= 24 {
        let mut crc = [0u8; 8];
        crc.copy_from_slice(&identity[16..24]);
        if uuid_crc_matches(&identity[..16], crc) {
            return Ok((
                general_purpose::STANDARD.encode(&identity[..16]),
                &identity[24..],
            ));
        }
    }
    Err(Error::new(
        ErrorKind::InvalidData,
        "invalid ConnectPacket UUID CRC",
    ))
}

impl Packet {
    pub fn read<R: Read>(mut r: R, id: u8) -> std::io::Result<Self> {
        match id {
            3 => {
                let version = r.read_i()?;
                let version_type = r.read_typeio_string()?.unwrap_or_default();
                let name = r.read_typeio_string()?.unwrap_or_default();
                let locale = r.read_typeio_string()?.unwrap_or_default();
                let usid = r.read_typeio_string()?.unwrap_or_default();
                // Official ConnectPacket.write emits `uuid_bytes + CRC32 long`.
                // Desktop/Android IDs are 8 bytes (`Platform.getUUID`). Java's
                // ConnectPacket.read then blindly takes 16 bytes, so 8+CRC
                // accidentally lines up with mobile/color/mods. Reading a
                // fixed 16-byte UUID plus another CRC over-reads the official
                // client and dies with UnexpectedEof before join.
                let mut identity = Vec::new();
                r.read_to_end(&mut identity)?;
                let (uuid, after_crc) = split_connect_uuid(&identity)?;
                let mut r = std::io::Cursor::new(after_crc);
                let mobile = r.read_b()? == 1;
                let color = r.read_i()?;
                let total_mods = r.read_b()?;
                let mut mods = Vec::with_capacity(total_mods as usize);
                for _ in 0..total_mods {
                    mods.push(r.read_typeio_string()?.ok_or_else(|| {
                        Error::new(ErrorKind::InvalidData, "null mod name in ConnectPacket")
                    })?);
                }

                let p = ConnectPacket {
                    version,
                    version_type,
                    name,
                    locale,
                    usid,
                    uuid,
                    mobile,
                    color,
                    mods,
                };
                tracing::trace!("Read ConnectPacket: {:?}", p);
                Ok(Packet::ConnectPacket(p))
            }
            0 => {
                tracing::trace!("Read StreamBegin");
                Ok(Packet::StreamBegin(StreamBegin {}))
            }
            1 => {
                let p = StreamChunk {
                    id: r.read_i()?,
                    data: {
                        let len = r.read_us()?;
                        let mut buf = vec![0; len as usize];
                        r.read_exact(&mut buf)?;
                        buf
                    },
                };
                tracing::trace!("Read StreamChunk: id={}, data_len={}", p.id, p.data.len());
                Ok(Packet::StreamChunk(p))
            }
            2 => {
                tracing::trace!("Read WorldStream");
                Ok(Packet::WorldStream(WorldStream {}))
            }
            4 => {
                tracing::trace!("Read AssetRequirementStream");
                Ok(Packet::AssetRequirementStream(AssetRequirementStream {}))
            }
            5 => {
                tracing::trace!("Read AssetStream");
                Ok(Packet::AssetStream(AssetStream {}))
            }
            _ => {
                let mut payload = Vec::new();
                r.read_to_end(&mut payload)?;
                Ok(Packet::Unknown { id, payload })
            }
        }
    }
}
