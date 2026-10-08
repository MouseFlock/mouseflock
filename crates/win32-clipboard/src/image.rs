//! Images in and out of the clipboard's formats, as pure functions with no
//! clipboard calls (add-clipboard-images D5, D6).
//!
//! - [`dib_to_png`]: a copied `CF_DIB` or `CF_DIBV5` (24 or 32 bpp) to PNG.
//! - [`png_to_dibv5`]: a received PNG to a 32 bpp `CF_DIBV5` with alpha.
//! - [`check_png`]: the signature, dimensions and length of a PNG, read from
//!   its header without decoding it.
//!
//! A received PNG came from another machine, so it is untrusted: its
//! dimensions are checked against [`MAX_PIXELS`] before anything the size of
//! the image is allocated, and every failure is an error, never a panic.

use std::fmt;

/// Most bytes of PNG an image may have (32 MiB).
pub const MAX_PNG_LEN: usize = 33_554_432;

/// Most pixels, width times height, an image may have (64 megapixels).
pub const MAX_PIXELS: u64 = 67_108_864;

/// The first eight bytes of every PNG file.
pub const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// Size of a `BITMAPV5HEADER`.
const V5_HEADER_LEN: usize = 124;
/// Size of a `BITMAPINFOHEADER`.
const INFO_HEADER_LEN: usize = 40;
const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;
/// `LCS_sRGB`, the colour space of the bitmaps this module writes.
const LCS_SRGB: u32 = 0x7352_4742;
/// `LCS_GM_IMAGES`.
const LCS_GM_IMAGES: u32 = 4;

/// Why an image could not be converted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageError {
    /// Over [`MAX_PIXELS`], or a PNG over [`MAX_PNG_LEN`] bytes.
    TooLarge,
    /// A bitmap this module does not convert: a depth other than 24 or 32
    /// bits, compression, or unusual colour masks.
    Unsupported,
    /// Truncated or malformed data.
    Corrupt,
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TooLarge => "image over the size limit",
            Self::Unsupported => "unsupported bitmap format",
            Self::Corrupt => "malformed image data",
        })
    }
}

impl std::error::Error for ImageError {}

/// A little-endian reader over a byte slice that fails instead of panicking.
struct Le<'a>(&'a [u8]);

impl Le<'_> {
    fn u16(&self, at: usize) -> Result<u16, ImageError> {
        let b = self
            .0
            .get(at..at.wrapping_add(2))
            .ok_or(ImageError::Corrupt)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&self, at: usize) -> Result<u32, ImageError> {
        let b = self
            .0
            .get(at..at.wrapping_add(4))
            .ok_or(ImageError::Corrupt)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn i32(&self, at: usize) -> Result<i32, ImageError> {
        self.u32(at).map(|v| v as i32)
    }
}

/// Width and height, checked: both at least 1, and at most [`MAX_PIXELS`]
/// pixels in all.
fn dimensions(width: u64, height: u64) -> Result<(usize, usize), ImageError> {
    if width == 0 || height == 0 {
        return Err(ImageError::Corrupt);
    }
    if width.checked_mul(height).is_none_or(|p| p > MAX_PIXELS) {
        return Err(ImageError::TooLarge);
    }
    let w = usize::try_from(width).map_err(|_| ImageError::TooLarge)?;
    let h = usize::try_from(height).map_err(|_| ImageError::TooLarge)?;
    Ok((w, h))
}

/// Converts a `CF_DIB` or `CF_DIBV5` to a PNG: 24 bpp, or 32 bpp with the
/// usual BGRA layout. A 32 bpp bitmap keeps its alpha when its header has an
/// alpha mask, or, with no mask, when any pixel's fourth byte is not zero;
/// otherwise the PNG is opaque RGB. Bottom-up and top-down bitmaps both come
/// out the right way up.
pub fn dib_to_png(dib: &[u8]) -> Result<Vec<u8>, ImageError> {
    let r = Le(dib);
    let header = r.u32(0)? as usize;
    if header < INFO_HEADER_LEN || header > dib.len() {
        return Err(ImageError::Corrupt);
    }
    let width = r.i32(4)?;
    let height = r.i32(8)?;
    let bpp = r.u16(14)?;
    let compression = r.u32(16)?;
    let colours = r.u32(32)? as usize;
    if width <= 0 || height == 0 {
        return Err(ImageError::Corrupt);
    }
    let top_down = height < 0;
    let (w, h) = dimensions(
        u64::from(width.unsigned_abs()),
        u64::from(height.unsigned_abs()),
    )?;

    // Where the colour masks are, if any, and where the pixels start.
    let (masks, after_masks) = match compression {
        BI_RGB => (None, header),
        BI_BITFIELDS if header >= 56 => {
            // V4 and V5 headers hold red, green, blue and alpha masks.
            (
                Some([r.u32(40)?, r.u32(44)?, r.u32(48)?, r.u32(52)?]),
                header,
            )
        }
        BI_BITFIELDS => {
            // A BITMAPINFOHEADER is followed by three masks; no alpha.
            let m = [r.u32(header)?, r.u32(header + 4)?, r.u32(header + 8)?, 0];
            (Some(m), header + 12)
        }
        _ => return Err(ImageError::Unsupported),
    };
    let bytes_per_pixel = match (bpp, masks) {
        (24, None) => 3,
        (32, None) => 4,
        (32, Some([0x00ff_0000, 0x0000_ff00, 0x0000_00ff, 0 | 0xff00_0000])) => 4,
        _ => return Err(ImageError::Unsupported),
    };
    // A colour table may follow, even for 24 and 32 bpp; skip it.
    let start = colours
        .checked_mul(4)
        .and_then(|t| t.checked_add(after_masks))
        .ok_or(ImageError::Corrupt)?;
    let stride = (w * usize::from(bpp)).div_ceil(32) * 4;
    let end = stride
        .checked_mul(h)
        .and_then(|n| n.checked_add(start))
        .ok_or(ImageError::Corrupt)?;
    let pixels = dib.get(start..end).ok_or(ImageError::Corrupt)?;

    let alpha = bytes_per_pixel == 4
        && match masks {
            Some([.., a]) => a != 0,
            None => pixels
                .chunks_exact(stride)
                .any(|row| row.as_chunks::<4>().0.iter().take(w).any(|px| px[3] != 0)),
        };
    let channels = if alpha { 4 } else { 3 };
    let mut rgb = Vec::with_capacity(w * h * channels);
    for y in 0..h {
        let src_row = if top_down { y } else { h - 1 - y };
        let row = &pixels[src_row * stride..src_row * stride + w * bytes_per_pixel];
        for px in row.chunks_exact(bytes_per_pixel) {
            rgb.extend_from_slice(&[px[2], px[1], px[0]]);
            if alpha {
                rgb.push(px[3]);
            }
        }
    }
    encode_png(&rgb, w, h, alpha)
}

fn encode_png(pixels: &[u8], w: usize, h: usize, alpha: bool) -> Result<Vec<u8>, ImageError> {
    let mut out = Vec::new();
    {
        let width = u32::try_from(w).map_err(|_| ImageError::TooLarge)?;
        let height = u32::try_from(h).map_err(|_| ImageError::TooLarge)?;
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(if alpha {
            png::ColorType::Rgba
        } else {
            png::ColorType::Rgb
        });
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().map_err(|_| ImageError::Corrupt)?;
        writer
            .write_image_data(pixels)
            .map_err(|_| ImageError::Corrupt)?;
    }
    if out.len() > MAX_PNG_LEN {
        return Err(ImageError::TooLarge);
    }
    Ok(out)
}

/// Checks a PNG's header without decoding it: the signature, an `IHDR` chunk
/// first, its dimensions within [`MAX_PIXELS`], and the whole within
/// [`MAX_PNG_LEN`] bytes. Returns the width and height.
pub fn check_png(png: &[u8]) -> Result<(u32, u32), ImageError> {
    if png.len() > MAX_PNG_LEN {
        return Err(ImageError::TooLarge);
    }
    if png.get(..8) != Some(&PNG_SIGNATURE[..]) || png.get(12..16) != Some(&b"IHDR"[..]) {
        return Err(ImageError::Corrupt);
    }
    let be = |at: usize| -> Result<u32, ImageError> {
        let b = png.get(at..at + 4).ok_or(ImageError::Corrupt)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    let (width, height) = (be(16)?, be(20)?);
    dimensions(u64::from(width), u64::from(height))?;
    Ok((width, height))
}

/// Where a PNG ends: just after its `IEND` chunk, found by walking the chunk
/// lengths from the signature. Clipboard memory can be larger than the data
/// put in it, so a PNG read from the clipboard is cut here. `None` if the
/// chunks run past the end before `IEND`.
pub fn png_end(bytes: &[u8]) -> Option<usize> {
    if bytes.get(..8) != Some(&PNG_SIGNATURE[..]) {
        return None;
    }
    let mut at = 8usize;
    loop {
        let len = bytes.get(at..at.checked_add(4)?)?;
        let len = usize::try_from(u32::from_be_bytes([len[0], len[1], len[2], len[3]])).ok()?;
        let kind = bytes.get(at + 4..at + 8)?;
        // Length, type, data, CRC.
        let next = at.checked_add(12)?.checked_add(len)?;
        if next > bytes.len() {
            return None;
        }
        if kind == b"IEND" {
            return Some(next);
        }
        at = next;
    }
}

/// Decodes a PNG to a bottom-up 32 bpp `CF_DIBV5` with an alpha mask, in the
/// sRGB colour space. Every PNG colour type and depth is accepted. The
/// dimensions are checked before the pixels are allocated.
///
/// Bottom-up because some apps mishandle a negative-height bitmap on the
/// clipboard; Windows synthesizes `CF_DIB` and `CF_BITMAP` from this.
pub fn png_to_dibv5(png: &[u8]) -> Result<Vec<u8>, ImageError> {
    let (width, height) = check_png(png)?;
    let (w, h) = dimensions(u64::from(width), u64::from(height))?;
    let mut decoder = png::Decoder::new_with_limits(
        std::io::Cursor::new(png),
        png::Limits {
            // The 8-bit RGBA image, plus the decoder's own working rows.
            bytes: w * h * 4 + 4 * 1024 * 1024,
        },
    );
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|_| ImageError::Corrupt)?;
    let mut buf = vec![0u8; reader.output_buffer_size().ok_or(ImageError::TooLarge)?];
    let frame = reader
        .next_frame(&mut buf)
        .map_err(|_| ImageError::Corrupt)?;
    if (frame.width, frame.height) != (width, height) || frame.bit_depth != png::BitDepth::Eight {
        return Err(ImageError::Corrupt);
    }
    let channels = match frame.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err(ImageError::Corrupt),
    };
    let line = frame.line_size;
    let image = buf.get(..line * h).ok_or(ImageError::Corrupt)?;

    let mut dib = Vec::with_capacity(V5_HEADER_LEN + w * h * 4);
    dib.extend_from_slice(&v5_header(width, height)?);
    // Rows bottom-up, pixels as BGRA.
    for y in (0..h).rev() {
        let row = image
            .get(y * line..y * line + w * channels)
            .ok_or(ImageError::Corrupt)?;
        for px in row.chunks_exact(channels) {
            let (r, g, b, a) = match *px {
                [l] => (l, l, l, 255),
                [l, a] => (l, l, l, a),
                [r, g, b] => (r, g, b, 255),
                [r, g, b, a] => (r, g, b, a),
                _ => return Err(ImageError::Corrupt),
            };
            dib.extend_from_slice(&[b, g, r, a]);
        }
    }
    Ok(dib)
}

/// A `BITMAPV5HEADER` for a bottom-up 32 bpp BGRA bitmap.
fn v5_header(width: u32, height: u32) -> Result<[u8; V5_HEADER_LEN], ImageError> {
    let w = i32::try_from(width).map_err(|_| ImageError::TooLarge)?;
    let h = i32::try_from(height).map_err(|_| ImageError::TooLarge)?;
    let size = width
        .checked_mul(height)
        .and_then(|p| p.checked_mul(4))
        .ok_or(ImageError::TooLarge)?;
    let mut hd = [0u8; V5_HEADER_LEN];
    let mut put = |at: usize, bytes: &[u8]| hd[at..at + bytes.len()].copy_from_slice(bytes);
    put(0, &(V5_HEADER_LEN as u32).to_le_bytes());
    put(4, &w.to_le_bytes());
    put(8, &h.to_le_bytes());
    put(12, &1u16.to_le_bytes()); // planes
    put(14, &32u16.to_le_bytes()); // bits per pixel
    put(16, &BI_BITFIELDS.to_le_bytes());
    put(20, &size.to_le_bytes());
    put(40, &0x00ff_0000u32.to_le_bytes()); // red mask
    put(44, &0x0000_ff00u32.to_le_bytes()); // green mask
    put(48, &0x0000_00ffu32.to_le_bytes()); // blue mask
    put(52, &0xff00_0000u32.to_le_bytes()); // alpha mask
    put(56, &LCS_SRGB.to_le_bytes());
    put(108, &LCS_GM_IMAGES.to_le_bytes());
    Ok(hd)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bitmap with a BITMAPINFOHEADER, `bpp` 24 or 32, BI_RGB, from rows of
    /// `[b, g, r, a]` pixels given top to bottom.
    fn dib(rows: &[&[[u8; 4]]], bpp: u16, top_down: bool) -> Vec<u8> {
        let (w, h) = (rows[0].len(), rows.len());
        let stride = (w * usize::from(bpp)).div_ceil(32) * 4;
        let mut out = vec![0u8; INFO_HEADER_LEN];
        out[0..4].copy_from_slice(&(INFO_HEADER_LEN as u32).to_le_bytes());
        out[4..8].copy_from_slice(&(w as i32).to_le_bytes());
        let height = if top_down { -(h as i32) } else { h as i32 };
        out[8..12].copy_from_slice(&height.to_le_bytes());
        out[12..14].copy_from_slice(&1u16.to_le_bytes());
        out[14..16].copy_from_slice(&bpp.to_le_bytes());
        let order: Vec<&&[[u8; 4]]> = if top_down {
            rows.iter().collect()
        } else {
            rows.iter().rev().collect()
        };
        for row in order {
            let mut line = Vec::new();
            for px in row.iter() {
                line.extend_from_slice(&px[..usize::from(bpp / 8)]);
            }
            line.resize(stride, 0);
            out.extend(line);
        }
        out
    }

    /// Decodes a PNG to (width, height, RGBA rows top to bottom).
    fn rgba(png: &[u8]) -> (u32, u32, Vec<u8>, png::ColorType) {
        let mut d = png::Decoder::new(std::io::Cursor::new(png));
        d.set_transformations(png::Transformations::normalize_to_color8());
        let mut r = d.read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        let f = r.next_frame(&mut buf).unwrap();
        buf.truncate(f.buffer_size());
        (f.width, f.height, buf, f.color_type)
    }

    const RED: [u8; 4] = [0, 0, 255, 0];
    const GREEN: [u8; 4] = [0, 255, 0, 0];
    const BLUE: [u8; 4] = [255, 0, 0, 0];
    const WHITE: [u8; 4] = [255, 255, 255, 0];

    #[test]
    fn bottom_up_24_bpp_comes_out_the_right_way_up() {
        // Three wide, so each row needs padding.
        let rows: [&[[u8; 4]]; 2] = [&[RED, GREEN, BLUE], &[WHITE, WHITE, RED]];
        let png = dib_to_png(&dib(&rows, 24, false)).unwrap();
        let (w, h, px, ty) = rgba(&png);
        assert_eq!((w, h, ty), (3, 2, png::ColorType::Rgb));
        assert_eq!(
            px,
            [
                255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0
            ]
        );
    }

    #[test]
    fn top_down_and_bottom_up_agree() {
        let rows: [&[[u8; 4]]; 2] = [&[RED, GREEN], &[BLUE, WHITE]];
        let a = rgba(&dib_to_png(&dib(&rows, 32, true)).unwrap()).2;
        let b = rgba(&dib_to_png(&dib(&rows, 32, false)).unwrap()).2;
        assert_eq!(a, b);
    }

    #[test]
    fn alpha_is_kept_when_present_and_dropped_when_all_zero() {
        // All alpha bytes 0: opaque RGB, as 32 bpp BI_RGB usually is.
        let opaque: [&[[u8; 4]]; 1] = [&[RED, GREEN]];
        let (.., ty) = rgba(&dib_to_png(&dib(&opaque, 32, false)).unwrap());
        assert_eq!(ty, png::ColorType::Rgb);
        // Real alpha: kept.
        let half = [0, 0, 255, 128];
        let clear = [0, 0, 0, 0];
        let translucent: [&[[u8; 4]]; 1] = [&[half, clear]];
        let (_, _, px, ty) = rgba(&dib_to_png(&dib(&translucent, 32, false)).unwrap());
        assert_eq!(ty, png::ColorType::Rgba);
        assert_eq!(px, [255, 0, 0, 128, 0, 0, 0, 0]);
    }

    #[test]
    fn round_trip_preserves_pixels_and_alpha() {
        let rows: [&[[u8; 4]]; 2] = [
            &[[1, 2, 3, 4], [5, 6, 7, 8], [9, 10, 11, 12]],
            &[[13, 14, 15, 16], [17, 18, 19, 255], [21, 22, 23, 0]],
        ];
        let png = dib_to_png(&dib(&rows, 32, true)).unwrap();
        let v5 = png_to_dibv5(&png).unwrap();
        assert_eq!(v5.len(), V5_HEADER_LEN + 3 * 2 * 4);
        // Bottom-up: the last source row comes first.
        assert_eq!(
            &v5[V5_HEADER_LEN..],
            [
                13, 14, 15, 16, 17, 18, 19, 255, 21, 22, 23, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11,
                12
            ]
        );
        // And back again, through the V5 header's alpha mask.
        let again = dib_to_png(&v5).unwrap();
        assert_eq!(rgba(&again).2, rgba(&png).2);
    }

    #[test]
    fn v5_header_is_what_windows_expects() {
        let png = dib_to_png(&dib(&[&[RED]], 24, false)).unwrap();
        let v5 = png_to_dibv5(&png).unwrap();
        let r = Le(&v5);
        assert_eq!(r.u32(0), Ok(124));
        assert_eq!((r.i32(4), r.i32(8)), (Ok(1), Ok(1)));
        assert_eq!((r.u16(12), r.u16(14)), (Ok(1), Ok(32)));
        assert_eq!(r.u32(16), Ok(BI_BITFIELDS));
        assert_eq!(r.u32(52), Ok(0xff00_0000));
        assert_eq!(r.u32(56), Ok(LCS_SRGB));
        assert_eq!(&v5[V5_HEADER_LEN..], [0, 0, 255, 255]);
    }

    #[test]
    fn grayscale_and_palette_pngs_become_bgra() {
        for color in [png::ColorType::Grayscale, png::ColorType::GrayscaleAlpha] {
            let mut out = Vec::new();
            {
                let mut e = png::Encoder::new(&mut out, 2, 1);
                e.set_color(color);
                e.set_depth(png::BitDepth::Eight);
                let data: &[u8] = if color == png::ColorType::Grayscale {
                    &[10, 200]
                } else {
                    &[10, 50, 200, 255]
                };
                e.write_header().unwrap().write_image_data(data).unwrap();
            }
            let v5 = png_to_dibv5(&out).unwrap();
            let px = &v5[V5_HEADER_LEN..];
            assert_eq!(&px[..3], [10, 10, 10]);
            assert_eq!(&px[4..7], [200, 200, 200]);
        }
    }

    #[test]
    fn unsupported_bitmaps_are_refused() {
        // 8 bpp with a palette.
        let mut eight = dib(&[&[RED]], 24, false);
        eight[14..16].copy_from_slice(&8u16.to_le_bytes());
        assert_eq!(dib_to_png(&eight), Err(ImageError::Unsupported));
        // RLE-compressed.
        let mut rle = dib(&[&[RED]], 24, false);
        rle[16..20].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(dib_to_png(&rle), Err(ImageError::Unsupported));
        // 32 bpp bitfields with unusual masks.
        let mut odd = dib(&[&[RED]], 32, false);
        odd[16..20].copy_from_slice(&BI_BITFIELDS.to_le_bytes());
        odd.splice(
            INFO_HEADER_LEN..INFO_HEADER_LEN,
            [0x0000_00ffu32, 0x0000_ff00, 0x00ff_0000]
                .iter()
                .flat_map(|m| m.to_le_bytes()),
        );
        assert_eq!(dib_to_png(&odd), Err(ImageError::Unsupported));
    }

    #[test]
    fn truncated_bitmaps_are_errors_not_panics() {
        let whole = dib(&[&[RED, GREEN], &[BLUE, WHITE]], 32, false);
        for len in 0..whole.len() {
            assert!(dib_to_png(&whole[..len]).is_err(), "length {len}");
        }
    }

    /// A PNG whose IHDR claims `w` × `h`, with no image data.
    fn claimed(w: u32, h: u32) -> Vec<u8> {
        let mut png = PNG_SIGNATURE.to_vec();
        png.extend(13u32.to_be_bytes());
        png.extend(b"IHDR");
        png.extend(w.to_be_bytes());
        png.extend(h.to_be_bytes());
        png.extend([8, 6, 0, 0, 0]);
        png.extend([0; 4]); // CRC, wrong
        png.resize(10_000, 0);
        png
    }

    #[test]
    fn huge_claimed_dimensions_are_refused_before_allocating() {
        let png = claimed(100_000, 100_000);
        assert_eq!(check_png(&png), Err(ImageError::TooLarge));
        assert_eq!(png_to_dibv5(&png), Err(ImageError::TooLarge));
        // Exactly the limit passes the header check.
        assert_eq!(check_png(&claimed(8192, 8192)), Ok((8192, 8192)));
        assert_eq!(check_png(&claimed(8192, 8193)), Err(ImageError::TooLarge));
        assert_eq!(check_png(&claimed(0, 5)), Err(ImageError::Corrupt));
        // Huge bitmaps too.
        let mut big = dib(&[&[RED]], 24, false);
        big[4..8].copy_from_slice(&100_000i32.to_le_bytes());
        big[8..12].copy_from_slice(&100_000i32.to_le_bytes());
        assert_eq!(dib_to_png(&big), Err(ImageError::TooLarge));
    }

    #[test]
    fn corrupt_and_truncated_pngs_are_errors_not_panics() {
        let good = dib_to_png(&dib(&[&[RED, GREEN], &[BLUE, WHITE]], 32, false)).unwrap();
        assert!(png_to_dibv5(&good).is_ok());
        // Cut anywhere before the image data ends: an error. A PNG missing
        // only its 12-byte IEND trailer still holds the whole image, and the
        // decoder may accept it; either way, no panic.
        let iend = good.len() - 12;
        assert_eq!(&good[iend + 4..iend + 8], b"IEND");
        for len in 0..iend {
            assert!(
                png_to_dibv5(&good[..len]).is_err(),
                "length {len} of {}",
                good.len()
            );
        }
        for len in iend..good.len() {
            let _ = png_to_dibv5(&good[..len]);
        }
        let mut flipped = good.clone();
        for i in 8..flipped.len() {
            flipped[i] ^= 0x55;
            let _ = png_to_dibv5(&flipped);
            flipped[i] ^= 0x55;
        }
        assert_eq!(png_to_dibv5(b"not a png"), Err(ImageError::Corrupt));
        assert_eq!(png_to_dibv5(&claimed(4, 4)), Err(ImageError::Corrupt));
    }

    #[test]
    fn png_end_cuts_clipboard_padding() {
        let png = dib_to_png(&dib(&[&[RED, GREEN]], 24, false)).unwrap();
        assert_eq!(png_end(&png), Some(png.len()));
        let mut padded = png.clone();
        padded.extend([0; 13]);
        assert_eq!(png_end(&padded), Some(png.len()));
        for len in 0..png.len() {
            assert_eq!(png_end(&png[..len]), None, "length {len}");
        }
        assert_eq!(png_end(b"GIF89a"), None);
    }

    #[test]
    fn an_oversized_png_is_refused_by_length() {
        let mut png = claimed(1, 1);
        png.resize(MAX_PNG_LEN + 1, 0);
        assert_eq!(check_png(&png), Err(ImageError::TooLarge));
    }
}
