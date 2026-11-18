//! A contact sheet: small pictures of a film's frames tiled in one PNG, so an agent can see a
//! film in a single image.

use std::io::Cursor;

/// An 8-bit RGBA picture.
pub(crate) struct Image {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

fn base64_value(c: u8) -> Option<u32> {
    match c {
        b'A'..=b'Z' => Some(u32::from(c - b'A')),
        b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
        b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Standard base64 (RFC 4648), padding optional.
pub(crate) fn unbase64(text: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text
        .bytes()
        .filter(|c| *c != b'=' && !c.is_ascii_whitespace())
    {
        acc = (acc << 6) | base64_value(c).ok_or("bad base64")?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

/// A PNG's pixels (8-bit grey, RGB or RGBA).
pub(crate) fn decode(bytes: &[u8]) -> Result<Image, String> {
    let mut reader = png::Decoder::new(Cursor::new(bytes))
        .read_info()
        .map_err(|e| format!("cannot read the PNG: {e}"))?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("cannot read the PNG: {e}"))?;
    if info.bit_depth != png::BitDepth::Eight {
        return Err("the PNG is not 8 bits per channel".into());
    }
    let (w, h) = (info.width, info.height);
    let n = (w * h) as usize;
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf[..n * 4].to_vec(),
        png::ColorType::Rgb => buf[..n * 3]
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::Grayscale => buf[..n].iter().flat_map(|g| [*g, *g, *g, 255]).collect(),
        other => return Err(format!("the PNG's colour type {other:?} is not supported")),
    };
    Ok(Image { w, h, rgba })
}

pub(crate) fn encode(img: &Image) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, img.w, img.h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut w| w.write_image_data(&img.rgba))
        .map_err(|e| format!("cannot encode the PNG: {e}"))?;
    Ok(out)
}

/// `img` scaled to `w` wide by averaging the pixels each new one covers.
fn scaled(img: &Image, w: u32) -> Image {
    if img.w == w {
        return Image {
            w: img.w,
            h: img.h,
            rgba: img.rgba.clone(),
        };
    }
    let h = ((u64::from(img.h) * u64::from(w)) / u64::from(img.w)).max(1) as u32;
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        let (y0, y1) = (y * img.h / h, ((y + 1) * img.h / h).max(y * img.h / h + 1));
        for x in 0..w {
            let (x0, x1) = (x * img.w / w, ((x + 1) * img.w / w).max(x * img.w / w + 1));
            let mut sum = [0u32; 4];
            for yy in y0..y1.min(img.h) {
                for xx in x0..x1.min(img.w) {
                    let i = ((yy * img.w + xx) * 4) as usize;
                    for k in 0..4 {
                        sum[k] += u32::from(img.rgba[i + k]);
                    }
                }
            }
            let n = (y1.min(img.h) - y0) * (x1.min(img.w) - x0);
            let o = ((y * w + x) * 4) as usize;
            for k in 0..4 {
                rgba[o + k] = (sum[k] / n.max(1)) as u8;
            }
        }
    }
    Image { w, h, rgba }
}

/// The pictures in rows of `columns`, each `cell_w` wide, with a dark gap between them. The
/// time of a picture is in the command's answer, in the same order.
pub(crate) fn contact_sheet(images: &[Image], columns: usize, cell_w: u32, gap: u32) -> Image {
    let cells: Vec<Image> = images.iter().map(|i| scaled(i, cell_w)).collect();
    let cell_h = cells.iter().map(|c| c.h).max().unwrap_or(1);
    let columns = columns.clamp(1, cells.len().max(1)) as u32;
    let rows = (cells.len() as u32).div_ceil(columns).max(1);
    let (w, h) = (
        columns * cell_w + (columns + 1) * gap,
        rows * cell_h + (rows + 1) * gap,
    );
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for px in rgba.as_chunks_mut::<4>().0 {
        *px = [24, 24, 28, 255];
    }
    for (n, c) in cells.iter().enumerate() {
        let (col, row) = (n as u32 % columns, n as u32 / columns);
        let (ox, oy) = (gap + col * (cell_w + gap), gap + row * (cell_h + gap));
        for y in 0..c.h {
            let src = (y * c.w * 4) as usize;
            let dst = (((oy + y) * w + ox) * 4) as usize;
            rgba[dst..dst + (c.w * 4) as usize]
                .copy_from_slice(&c.rgba[src..src + (c.w * 4) as usize]);
        }
    }
    Image { w, h, rgba }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: u32, h: u32, v: [u8; 4]) -> Image {
        Image {
            w,
            h,
            rgba: v
                .iter()
                .copied()
                .cycle()
                .take((w * h * 4) as usize)
                .collect(),
        }
    }

    #[test]
    fn a_sheet_tiles_scaled_pictures_in_rows_and_round_trips_as_png() {
        let imgs = [
            flat(64, 36, [200, 0, 0, 255]),
            flat(64, 36, [0, 200, 0, 255]),
            flat(64, 36, [0, 0, 200, 255]),
        ];
        let sheet = contact_sheet(&imgs, 2, 32, 2);
        assert_eq!((sheet.w, sheet.h), (2 * 32 + 3 * 2, 2 * 18 + 3 * 2));
        let at = |x: u32, y: u32| {
            let i = ((y * sheet.w + x) * 4) as usize;
            [sheet.rgba[i], sheet.rgba[i + 1], sheet.rgba[i + 2]]
        };
        assert_eq!(at(10, 10), [200, 0, 0]);
        assert_eq!(at(2 + 32 + 2 + 5, 10), [0, 200, 0]);
        assert_eq!(at(10, 2 + 18 + 2 + 5), [0, 0, 200]);
        let again = decode(&encode(&sheet).unwrap()).unwrap();
        assert_eq!(
            (again.w, again.h, again.rgba),
            (sheet.w, sheet.h, sheet.rgba)
        );
        assert_eq!(
            unbase64(&crate::base64(b"hello, sheet")).unwrap(),
            b"hello, sheet"
        );
    }
}
