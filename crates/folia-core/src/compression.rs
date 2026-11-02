use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use std::io::{self, Read, Write};

/// Maximum decompressed size accepted from a network peer.
pub const DEFAULT_MAX_DECOMPRESSED: usize = 8 * 1024 * 1024;

pub fn compress(data: &[u8]) -> io::Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data)?;
    encoder.finish()
}

pub fn decompress(data: &[u8], maximum_size: usize) -> io::Result<Vec<u8>> {
    let mut decoder = ZlibDecoder::new(data);
    let mut output = Vec::new();
    let mut buffer = [0u8; 8192];

    loop {
        let read = decoder.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if output.len() + read > maximum_size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "decompressed payload exceeds the configured limit",
            ));
        }
        output.extend_from_slice(&buffer[..read]);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_payloads() {
        for size in [0, 1, 15, 1024, 65_536] {
            let data: Vec<u8> = (0..size).map(|index| (index % 251) as u8).collect();
            let compressed = compress(&data).unwrap();
            let decompressed = decompress(&compressed, DEFAULT_MAX_DECOMPRESSED).unwrap();
            assert_eq!(decompressed, data);
        }
    }

    #[test]
    fn rejects_decompressed_payload_over_limit() {
        let data = vec![0u8; 4096];
        let compressed = compress(&data).unwrap();
        assert!(decompress(&compressed, 1024).is_err());
    }

    #[test]
    fn rejects_invalid_zlib_data() {
        assert!(decompress(&[0x00, 0x01, 0x02], DEFAULT_MAX_DECOMPRESSED).is_err());
    }
}
