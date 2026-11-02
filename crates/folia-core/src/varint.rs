use std::io::{self, Read, Write};

pub const MAX_VARINT_BYTES: usize = 5;
pub const MAX_VARLONG_BYTES: usize = 10;

/// Number of bytes used by a VarInt encoding.
pub fn varint_len(value: i32) -> usize {
    let mut value = value as u32;
    let mut length = 1;
    while value >= 0x80 {
        value >>= 7;
        length += 1;
    }
    length
}

pub fn varlong_len(value: i64) -> usize {
    let mut value = value as u64;
    let mut length = 1;
    while value >= 0x80 {
        value >>= 7;
        length += 1;
    }
    length
}

pub fn write_varint<W: Write>(writer: &mut W, value: i32) -> io::Result<usize> {
    let mut value = value as u32;
    let mut written = 0;
    loop {
        if value & !0x7F == 0 {
            writer.write_all(&[value as u8])?;
            return Ok(written + 1);
        }
        writer.write_all(&[((value & 0x7F) as u8) | 0x80])?;
        value >>= 7;
        written += 1;
        if written >= MAX_VARINT_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "VarInt exceeds five bytes",
            ));
        }
    }
}

pub fn read_varint<R: Read>(reader: &mut R) -> io::Result<i32> {
    let mut result = 0u32;
    let mut byte = [0u8; 1];
    for index in 0..MAX_VARINT_BYTES {
        reader.read_exact(&mut byte)?;
        result |= u32::from(byte[0] & 0x7F) << (7 * index);
        if byte[0] & 0x80 == 0 {
            return Ok(result as i32);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "VarInt exceeds five bytes",
    ))
}

pub fn write_varlong<W: Write>(writer: &mut W, value: i64) -> io::Result<usize> {
    let mut value = value as u64;
    let mut written = 0;
    loop {
        if value & !0x7F == 0 {
            writer.write_all(&[value as u8])?;
            return Ok(written + 1);
        }
        writer.write_all(&[((value & 0x7F) as u8) | 0x80])?;
        value >>= 7;
        written += 1;
        if written >= MAX_VARLONG_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "VarLong exceeds ten bytes",
            ));
        }
    }
}

pub fn read_varlong<R: Read>(reader: &mut R) -> io::Result<i64> {
    let mut result = 0u64;
    let mut byte = [0u8; 1];
    for index in 0..MAX_VARLONG_BYTES {
        reader.read_exact(&mut byte)?;
        result |= u64::from(byte[0] & 0x7F) << (7 * index);
        if byte[0] & 0x80 == 0 {
            return Ok(result as i64);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "VarLong exceeds ten bytes",
    ))
}

pub fn read_varint_slice(data: &[u8]) -> io::Result<(i32, usize)> {
    let mut cursor = io::Cursor::new(data);
    let value = read_varint(&mut cursor)?;
    Ok((value, cursor.position() as usize))
}

pub fn read_varlong_slice(data: &[u8]) -> io::Result<(i64, usize)> {
    let mut cursor = io::Cursor::new(data);
    let value = read_varlong(&mut cursor)?;
    Ok((value, cursor.position() as usize))
}

pub fn write_varint_to_vec(value: i32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(varint_len(value));
    write_varint(&mut bytes, value).expect("writing to Vec cannot fail");
    bytes
}

pub fn write_varlong_to_vec(value: i64) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(varlong_len(value));
    write_varlong(&mut bytes, value).expect("writing to Vec cannot fail");
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_varint_encodings() {
        let cases: &[(i32, &[u8])] = &[
            (0, &[0x00]),
            (1, &[0x01]),
            (127, &[0x7F]),
            (128, &[0x80, 0x01]),
            (255, &[0xFF, 0x01]),
            (300, &[0xAC, 0x02]),
            (2_097_151, &[0xFF, 0xFF, 0x7F]),
            (i32::MAX, &[0xFF, 0xFF, 0xFF, 0xFF, 0x07]),
            (-1, &[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]),
            (i32::MIN, &[0x80, 0x80, 0x80, 0x80, 0x08]),
        ];

        for &(value, expected) in cases {
            assert_eq!(write_varint_to_vec(value), expected, "encode {value}");
            assert_eq!(
                read_varint_slice(expected).unwrap(),
                (value, expected.len())
            );
            assert_eq!(varint_len(value), expected.len());
        }
    }

    #[test]
    fn varint_round_trip_all_widths() {
        for value in [i32::MIN, -1_000_000, -1, 0, 1, 1_000_000, i32::MAX] {
            let bytes = write_varint_to_vec(value);
            assert_eq!(read_varint_slice(&bytes).unwrap().0, value);
        }
    }

    #[test]
    fn known_varlong_encodings() {
        let cases: &[(i64, &[u8])] = &[
            (0, &[0x00]),
            (1, &[0x01]),
            (127, &[0x7F]),
            (128, &[0x80, 0x01]),
            (
                i64::MAX,
                &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F],
            ),
            (
                -1,
                &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01],
            ),
            (
                i64::MIN,
                &[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01],
            ),
        ];

        for &(value, expected) in cases {
            assert_eq!(write_varlong_to_vec(value), expected, "encode {value}");
            assert_eq!(
                read_varlong_slice(expected).unwrap(),
                (value, expected.len())
            );
            assert_eq!(varlong_len(value), expected.len());
        }
    }

    #[test]
    fn rejects_overlong_varint() {
        let bytes = [0x80, 0x80, 0x80, 0x80, 0x80, 0x00];
        assert!(read_varint_slice(&bytes).is_err());
    }
}
