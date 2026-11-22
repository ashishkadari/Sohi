//! A minimal PNG writer for snapshots: 8-bit RGBA, no filtering, and
//! "stored" (uncompressed) deflate blocks. Files are larger than a real
//! encoder's, but it needs no dependencies and is a few dozen lines.

/// Encodes straight-alpha RGBA pixels as a PNG file.
pub fn encode(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let stride = width as usize * 4;
    // Each scanline is prefixed with filter type 0 (none).
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for row in rgba.chunks_exact(stride) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit, RGBA, deflate, no filter, no interlace
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, *b"IHDR", &ihdr);
    chunk(&mut out, *b"IDAT", &zlib_stored(&raw));
    chunk(&mut out, *b"IEND", &[]);
    out
}

fn chunk(out: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
    let len = u32::try_from(data.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_be_bytes());
    let start = out.len();
    out.extend_from_slice(&kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// A zlib stream of stored deflate blocks (at most 65 535 bytes each).
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut blocks = data.chunks(0xFFFF).peekable();
    if blocks.peek().is_none() {
        out.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    while let Some(block) = blocks.next() {
        out.push(u8::from(blocks.peek().is_none())); // BFINAL on the last
        let len = u16::try_from(block.len()).unwrap_or(u16::MAX);
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(block);
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_match_known_values() {
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn png_has_signature_header_and_end() {
        let png = encode(2, 1, &[255, 0, 0, 255, 0, 0, 255, 128]);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
    }

    #[test]
    fn large_images_split_into_stored_blocks() {
        let data = vec![7u8; 70_000];
        let z = zlib_stored(&data);
        // Header, two blocks with 5-byte headers, and the checksum.
        assert_eq!(z.len(), 2 + 5 + 65_535 + 5 + (70_000 - 65_535) + 4);
        assert_eq!(z[2], 0, "first block is not final");
        assert_eq!(z[2 + 5 + 65_535], 1, "second block is final");
    }
}
