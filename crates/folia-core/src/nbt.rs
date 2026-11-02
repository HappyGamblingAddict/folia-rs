use flate2::Compression;
use flate2::read::{GzDecoder, ZlibDecoder};
use flate2::write::{GzEncoder, ZlibEncoder};
use std::io::{self, Read, Write};

const MAX_COLLECTION_LEN: usize = 16 * 1024 * 1024;

/// Minecraft NBT tag types (big-endian, Java DataOutput format).
#[derive(Debug, Clone, PartialEq)]
pub enum Tag {
    End,
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<u8>),
    String(String),
    List(Vec<Tag>),
    Compound(Vec<(String, Tag)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Tag {
    pub fn id(&self) -> u8 {
        match self {
            Tag::End => 0,
            Tag::Byte(_) => 1,
            Tag::Short(_) => 2,
            Tag::Int(_) => 3,
            Tag::Long(_) => 4,
            Tag::Float(_) => 5,
            Tag::Double(_) => 6,
            Tag::ByteArray(_) => 7,
            Tag::String(_) => 8,
            Tag::List(_) => 9,
            Tag::Compound(_) => 10,
            Tag::IntArray(_) => 11,
            Tag::LongArray(_) => 12,
        }
    }

    pub fn as_byte(&self) -> Option<i8> {
        match self {
            Tag::Byte(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i32> {
        match self {
            Tag::Int(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_long(&self) -> Option<i64> {
        match self {
            Tag::Long(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_float(&self) -> Option<f32> {
        match self {
            Tag::Float(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_double(&self) -> Option<f64> {
        match self {
            Tag::Double(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Tag::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Tag]> {
        match self {
            Tag::List(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_compound(&self) -> Option<&[(String, Tag)]> {
        match self {
            Tag::Compound(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_byte_array(&self) -> Option<&[u8]> {
        match self {
            Tag::ByteArray(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_int_array(&self) -> Option<&[i32]> {
        match self {
            Tag::IntArray(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_long_array(&self) -> Option<&[i64]> {
        match self {
            Tag::LongArray(value) => Some(value),
            _ => None,
        }
    }

    pub fn get(&self, name: &str) -> Option<&Tag> {
        self.as_compound()?
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    }
}

pub fn read_root<R: Read>(reader: &mut R) -> io::Result<Tag> {
    let (_, tag) = read_named_root(reader)?;
    Ok(tag)
}

/// Network NBT: type byte + payload, no name. `FriendlyByteBuf.readNbt` uses
/// `NbtIo.readAnyTag`; output is an unnamed root tag.
pub fn read_any_tag<R: Read>(reader: &mut R) -> io::Result<Tag> {
    let id = read_u8(reader)?;
    if id == 0 {
        return Ok(Tag::End);
    }
    read_payload(reader, id)
}

pub fn write_any_tag<W: Write>(writer: &mut W, tag: &Tag) -> io::Result<()> {
    writer.write_all(&[tag.id()])?;
    if matches!(tag, Tag::End) {
        return Ok(());
    }
    write_payload(writer, tag)
}

pub fn read_named_root<R: Read>(reader: &mut R) -> io::Result<(String, Tag)> {
    let id = read_u8(reader)?;
    if id == 0 {
        return Ok((String::new(), Tag::End));
    }
    let name = read_string(reader)?;
    let tag = read_payload(reader, id)?;
    Ok((name, tag))
}

fn read_payload<R: Read>(reader: &mut R, id: u8) -> io::Result<Tag> {
    match id {
        1 => Ok(Tag::Byte(read_u8(reader)? as i8)),
        2 => Ok(Tag::Short(read_i16(reader)?)),
        3 => Ok(Tag::Int(read_i32(reader)?)),
        4 => Ok(Tag::Long(read_i64(reader)?)),
        5 => Ok(Tag::Float(read_f32(reader)?)),
        6 => Ok(Tag::Double(read_f64(reader)?)),
        7 => {
            let length = checked_len(reader)?;
            let mut bytes = vec![0u8; length];
            reader.read_exact(&mut bytes)?;
            Ok(Tag::ByteArray(bytes))
        }
        8 => Ok(Tag::String(read_string(reader)?)),
        9 => {
            let element_id = read_u8(reader)?;
            let length = checked_len(reader)?;
            if length == 0 {
                return Ok(Tag::List(Vec::new()));
            }
            if element_id == 0 {
                return Err(invalid("non-empty NBT list with End element type"));
            }
            let mut items = Vec::with_capacity(length.min(1024 * 1024));
            for _ in 0..length {
                items.push(read_payload(reader, element_id)?);
            }
            Ok(Tag::List(items))
        }
        10 => {
            let mut entries = Vec::new();
            loop {
                let child_id = read_u8(reader)?;
                if child_id == 0 {
                    return Ok(Tag::Compound(entries));
                }
                let name = read_string(reader)?;
                let value = read_payload(reader, child_id)?;
                entries.push((name, value));
            }
        }
        11 => {
            let length = checked_len(reader)?;
            let mut values = Vec::with_capacity(length.min(1024 * 1024));
            for _ in 0..length {
                values.push(read_i32(reader)?);
            }
            Ok(Tag::IntArray(values))
        }
        12 => {
            let length = checked_len(reader)?;
            let mut values = Vec::with_capacity(length.min(1024 * 1024));
            for _ in 0..length {
                values.push(read_i64(reader)?);
            }
            Ok(Tag::LongArray(values))
        }
        other => Err(invalid(format!("unknown NBT tag id {other}"))),
    }
}

pub fn write_root<W: Write>(writer: &mut W, tag: &Tag) -> io::Result<()> {
    write_named(writer, "", tag)
}

pub fn write_named<W: Write>(writer: &mut W, name: &str, tag: &Tag) -> io::Result<()> {
    writer.write_all(&[tag.id()])?;
    if matches!(tag, Tag::End) {
        return Ok(());
    }
    write_string(writer, name)?;
    write_payload(writer, tag)
}

fn write_payload<W: Write>(writer: &mut W, tag: &Tag) -> io::Result<()> {
    match tag {
        Tag::End => Ok(()),
        Tag::Byte(value) => writer.write_all(&[*value as u8]),
        Tag::Short(value) => writer.write_all(&value.to_be_bytes()),
        Tag::Int(value) => writer.write_all(&value.to_be_bytes()),
        Tag::Long(value) => writer.write_all(&value.to_be_bytes()),
        Tag::Float(value) => writer.write_all(&value.to_be_bytes()),
        Tag::Double(value) => writer.write_all(&value.to_be_bytes()),
        Tag::ByteArray(values) => {
            write_len(writer, values.len())?;
            writer.write_all(values)
        }
        Tag::String(value) => write_string(writer, value),
        Tag::List(items) => {
            let element_id = items.first().map(Tag::id).unwrap_or(0);
            if items.iter().any(|item| item.id() != element_id) {
                return Err(invalid("NBT list contains mixed element types"));
            }
            writer.write_all(&[element_id])?;
            write_len(writer, items.len())?;
            for item in items {
                write_payload(writer, item)?;
            }
            Ok(())
        }
        Tag::Compound(entries) => {
            for (name, value) in entries {
                write_named(writer, name, value)?;
            }
            writer.write_all(&[0])
        }
        Tag::IntArray(values) => {
            write_len(writer, values.len())?;
            for value in values {
                writer.write_all(&value.to_be_bytes())?;
            }
            Ok(())
        }
        Tag::LongArray(values) => {
            write_len(writer, values.len())?;
            for value in values {
                writer.write_all(&value.to_be_bytes())?;
            }
            Ok(())
        }
    }
}

pub fn write_root_to_vec(tag: &Tag) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    write_root(&mut bytes, tag)?;
    Ok(bytes)
}

pub fn read_gzip_root(bytes: &[u8]) -> io::Result<Tag> {
    let mut decoder = GzDecoder::new(bytes);
    read_root(&mut decoder)
}

pub fn write_gzip_root(tag: &Tag) -> io::Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    write_root(&mut encoder, tag)?;
    encoder.finish()
}

pub fn read_zlib_root(bytes: &[u8]) -> io::Result<Tag> {
    let mut decoder = ZlibDecoder::new(bytes);
    read_root(&mut decoder)
}

pub fn write_zlib_root(tag: &Tag) -> io::Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    write_root(&mut encoder, tag)?;
    encoder.finish()
}

fn read_u8<R: Read>(reader: &mut R) -> io::Result<u8> {
    let mut buffer = [0u8; 1];
    reader.read_exact(&mut buffer)?;
    Ok(buffer[0])
}

fn read_i16<R: Read>(reader: &mut R) -> io::Result<i16> {
    let mut buffer = [0u8; 2];
    reader.read_exact(&mut buffer)?;
    Ok(i16::from_be_bytes(buffer))
}

fn read_i32<R: Read>(reader: &mut R) -> io::Result<i32> {
    let mut buffer = [0u8; 4];
    reader.read_exact(&mut buffer)?;
    Ok(i32::from_be_bytes(buffer))
}

fn read_i64<R: Read>(reader: &mut R) -> io::Result<i64> {
    let mut buffer = [0u8; 8];
    reader.read_exact(&mut buffer)?;
    Ok(i64::from_be_bytes(buffer))
}

fn read_f32<R: Read>(reader: &mut R) -> io::Result<f32> {
    let mut buffer = [0u8; 4];
    reader.read_exact(&mut buffer)?;
    Ok(f32::from_be_bytes(buffer))
}

fn read_f64<R: Read>(reader: &mut R) -> io::Result<f64> {
    let mut buffer = [0u8; 8];
    reader.read_exact(&mut buffer)?;
    Ok(f64::from_be_bytes(buffer))
}

fn read_string<R: Read>(reader: &mut R) -> io::Result<String> {
    let length = read_i16(reader)?;
    if length < 0 {
        return Err(invalid("negative NBT string length"));
    }
    let mut bytes = vec![0u8; length as usize];
    reader.read_exact(&mut bytes)?;
    String::from_utf8(bytes)
        .map_err(|error| invalid(format!("invalid UTF-8 in NBT string: {error}")))
}

fn write_string<W: Write>(writer: &mut W, value: &str) -> io::Result<()> {
    let bytes = value.as_bytes();
    if bytes.len() > u16::MAX as usize {
        return Err(invalid("NBT string is longer than 65535 bytes"));
    }
    writer.write_all(&(bytes.len() as u16).to_be_bytes())?;
    writer.write_all(bytes)
}

fn checked_len<R: Read>(reader: &mut R) -> io::Result<usize> {
    let length = read_i32(reader)?;
    if length < 0 || length as usize > MAX_COLLECTION_LEN {
        return Err(invalid(format!("invalid NBT collection length {length}")));
    }
    Ok(length as usize)
}

fn write_len<W: Write>(writer: &mut W, length: usize) -> io::Result<()> {
    if length > i32::MAX as usize {
        return Err(invalid("NBT collection is too large"));
    }
    writer.write_all(&(length as i32).to_be_bytes())
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Tag {
        Tag::Compound(vec![
            ("byte".into(), Tag::Byte(-7)),
            ("short".into(), Tag::Short(1234)),
            ("int".into(), Tag::Int(-123456)),
            ("long".into(), Tag::Long(9_000_000_000)),
            ("float".into(), Tag::Float(1.25)),
            ("double".into(), Tag::Double(-3.5)),
            ("byte_array".into(), Tag::ByteArray(vec![1, 2, 3, 255])),
            ("string".into(), Tag::String("hello NBT".into())),
            (
                "list".into(),
                Tag::List(vec![Tag::Int(1), Tag::Int(2), Tag::Int(3)]),
            ),
            (
                "nested".into(),
                Tag::Compound(vec![("name".into(), Tag::String("Folia".into()))]),
            ),
            ("int_array".into(), Tag::IntArray(vec![1, -2, 3])),
            ("long_array".into(), Tag::LongArray(vec![1, -2, i64::MAX])),
        ])
    }

    #[test]
    fn round_trips_all_tag_types() {
        let tag = sample();
        let bytes = write_root_to_vec(&tag).unwrap();
        let decoded = read_root(&mut bytes.as_slice()).unwrap();
        assert_eq!(tag, decoded);
    }

    #[test]
    fn matches_known_java_bytes() {
        let tag = Tag::Compound(vec![("name".into(), Tag::String("Hello".into()))]);
        let expected = [
            0x0A, 0x00, 0x00, // compound, empty root name
            0x08, 0x00, 0x04, b'n', b'a', b'm', b'e', // string tag "name"
            0x00, 0x05, b'H', b'e', b'l', b'l', b'o', // "Hello"
            0x00, // end compound
        ];
        assert_eq!(write_root_to_vec(&tag).unwrap(), expected);
        assert_eq!(read_root(&mut expected.as_slice()).unwrap(), tag);
    }

    #[test]
    fn gzip_round_trip() {
        let tag = sample();
        let compressed = write_gzip_root(&tag).unwrap();
        assert_eq!(read_gzip_root(&compressed).unwrap(), tag);
    }

    #[test]
    fn zlib_round_trip() {
        let tag = sample();
        let compressed = write_zlib_root(&tag).unwrap();
        assert_eq!(read_zlib_root(&compressed).unwrap(), tag);
    }

    #[test]
    fn empty_list_and_compound() {
        let tag = Tag::Compound(vec![
            ("empty_list".into(), Tag::List(Vec::new())),
            ("empty_compound".into(), Tag::Compound(Vec::new())),
        ]);
        let bytes = write_root_to_vec(&tag).unwrap();
        assert_eq!(read_root(&mut bytes.as_slice()).unwrap(), tag);
    }

    #[test]
    fn rejects_unknown_tag_id() {
        let bytes = [0x7F, 0x00, 0x00];
        assert!(read_root(&mut bytes.as_slice()).is_err());
    }

    #[test]
    fn nested_list_of_compounds() {
        let tag = Tag::Compound(vec![(
            "entries".into(),
            Tag::List(vec![
                Tag::Compound(vec![("id".into(), Tag::Int(1))]),
                Tag::Compound(vec![("id".into(), Tag::Int(2))]),
            ]),
        )]);
        let bytes = write_root_to_vec(&tag).unwrap();
        assert_eq!(read_root(&mut bytes.as_slice()).unwrap(), tag);
    }
}
