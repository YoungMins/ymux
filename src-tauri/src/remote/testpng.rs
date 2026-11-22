//! A tiny PNG writer for tests and for the end-to-end fake desktop: 8-bit
//! truecolor, no filtering, and zlib "stored" (uncompressed) deflate blocks, so
//! it needs no compression library and the file still decodes in a browser.
//! Deterministic: the same arguments give the same bytes.

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for b in data {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

pub fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for x in data {
        a = (a + u32::from(*x)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, ty: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut body = ty.to_vec();
    body.extend_from_slice(data);
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc32(&body).to_be_bytes());
}

/// A `w` x `h` PNG whose pixel `(x, y)` is `pixel(x, y)` (RGB).
pub fn png_rgb(w: u32, h: u32, mut pixel: impl FnMut(u32, u32) -> [u8; 3]) -> Vec<u8> {
    let mut raw = Vec::with_capacity((h * (w * 3 + 1)) as usize);
    for y in 0..h {
        raw.push(0); // filter: none
        for x in 0..w {
            raw.extend_from_slice(&pixel(x, y));
        }
    }
    // zlib: header, stored blocks of at most 65535 bytes, Adler-32.
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, b) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        z.extend_from_slice(&(b.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(b.len() as u16)).to_le_bytes());
        z.extend_from_slice(b);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// Two colours in `cell`-pixel squares.
pub fn checker(w: u32, h: u32, cell: u32, a: [u8; 3], b: [u8; 3]) -> Vec<u8> {
    png_rgb(
        w,
        h,
        |x, y| if (x / cell + y / cell) % 2 == 0 { a } else { b },
    )
}

/// Pseudo-random colour noise (a fixed generator seeded by `seed`): does not
/// compress, which is the point.
pub fn noisy(w: u32, h: u32, seed: u32) -> Vec<u8> {
    let mut x = seed | 1;
    png_rgb(w, h, move |_, _| {
        let mut px = [0u8; 3];
        for c in &mut px {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *c = (x >> 24) as u8;
        }
        px
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(bytes: &[u8]) -> (png::OutputInfo, Vec<u8>) {
        let mut r = png::Decoder::new(std::io::Cursor::new(bytes))
            .read_info()
            .unwrap();
        let mut buf = vec![0; r.output_buffer_size()];
        let info = r.next_frame(&mut buf).unwrap();
        (info, buf)
    }

    #[test]
    fn the_checksums_match_their_published_check_values() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(crc32(b""), 0);
        assert_eq!(adler32(b""), 1);
    }

    #[test]
    fn a_checker_decodes_to_the_pixels_it_was_given() {
        let a = [0x2d, 0x6c, 0xdf];
        let b = [0xf2, 0xc9, 0x4c];
        let bytes = checker(64, 64, 8, a, b);
        let (info, px) = decode(&bytes);
        assert_eq!((info.width, info.height), (64, 64));
        assert_eq!(&px[0..3], &a);
        assert_eq!(&px[8 * 3..8 * 3 + 3], &b);
        assert_eq!(&px[(8 * 64) * 3..(8 * 64) * 3 + 3], &b, "next row of cells");
        assert_eq!(super::super::chat::pics::sniff(&bytes), Some("image/png"));
    }

    #[test]
    fn noise_is_big_deterministic_and_decodes() {
        let bytes = noisy(400, 300, 7);
        assert_eq!(bytes, noisy(400, 300, 7));
        assert_ne!(bytes, noisy(400, 300, 8));
        assert!(bytes.len() > 4 * 96 * 1024 - 40 * 1024, "{}", bytes.len());
        let (info, px) = decode(&bytes);
        assert_eq!(
            (info.width, info.height, px.len()),
            (400, 300, 400 * 300 * 3)
        );
        // Several stored blocks (more than 65535 raw bytes).
        assert!(bytes.len() > 3 * 65_535);
    }
}
