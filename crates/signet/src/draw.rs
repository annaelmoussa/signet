//! Drawing primitives on top of `Canvas`: lines, buttons, images.

use signet_fb::{Canvas, Rect};

pub fn hline(c: &mut Canvas, x: i32, y: i32, w: i32, gray: u8) {
    if w > 0 && y >= 0 {
        c.fill_rect(Rect::new(x.max(0) as u32, y as u32, w as u32, 1), gray);
    }
}

pub fn rect(c: &mut Canvas, x: i32, y: i32, w: i32, h: i32, gray: u8) {
    if w > 0 && h > 0 {
        let x0 = x.max(0);
        let y0 = y.max(0);
        c.fill_rect(
            Rect::new(
                x0 as u32,
                y0 as u32,
                (w - (x0 - x)) as u32,
                (h - (y0 - y)) as u32,
            ),
            gray,
        );
    }
}

pub fn frame(c: &mut Canvas, x: i32, y: i32, w: i32, h: i32, t: i32, gray: u8) {
    rect(c, x, y, w, t, gray);
    rect(c, x, y + h - t, w, t, gray);
    rect(c, x, y, t, h, gray);
    rect(c, x + w - t, y, t, h, gray);
}

/// Filled rounded-corner rectangle (radius r).
pub fn rounded(c: &mut Canvas, x: i32, y: i32, w: i32, h: i32, r: i32, gray: u8) {
    let r = r.min(w / 2).min(h / 2).max(0);
    rect(c, x + r, y, w - 2 * r, h, gray);
    rect(c, x, y + r, r, h - 2 * r, gray);
    rect(c, x + w - r, y + r, r, h - 2 * r, gray);
    // quarter discs
    for dy in 0..r {
        let yy = (r - dy) as f32 - 0.5;
        let dx = ((r as f32).powi(2) - yy * yy).max(0.0).sqrt().round() as i32;
        rect(c, x + r - dx, y + dy, dx, 1, gray);
        rect(c, x + w - r, y + dy, dx, 1, gray);
        rect(c, x + r - dx, y + h - 1 - dy, dx, 1, gray);
        rect(c, x + w - r, y + h - 1 - dy, dx, 1, gray);
    }
}

/// Darkens an area (veil for overlays).
/// Geometry of a box: outer shape, inner shape (without the border), corners.
struct FrameGeom {
    w: f32,
    h: f32,
    r: f32,
    ri: f32,
    bt: f32,
    br: f32,
    bb: f32,
    bl: f32,
    round_top: bool,
    round_bottom: bool,
}

impl FrameGeom {
    /// Signed distance to the edge of a rounded rectangle (negative inside).
    #[allow(clippy::too_many_arguments)]
    fn sd(&self, px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32, rad: f32) -> f32 {
        let top = self.round_top && py < y0 + rad;
        let bottom = self.round_bottom && py > y1 - rad;
        let left = px < x0 + rad;
        let right = px > x1 - rad;
        if (top || bottom) && (left || right) && rad > 0.0 {
            let cx = if left { x0 + rad } else { x1 - rad };
            let cy = if top { y0 + rad } else { y1 - rad };
            ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - rad
        } else {
            (x0 - px)
                .max(px - x1)
                .max(if self.round_top { y0 - py } else { -1e9 })
                .max(if self.round_bottom { py - y1 } else { -1e9 })
        }
    }

    /// Coverage (0..1) of the outer shape and of the interior at the center of a pixel.
    fn cover(&self, px: f32, py: f32) -> (f32, f32) {
        let outer = (0.5 - self.sd(px, py, 0.0, 0.0, self.w, self.h, self.r)).clamp(0.0, 1.0);
        let inner = (0.5
            - self.sd(
                px,
                py,
                self.bl,
                self.bt,
                self.w - self.br,
                self.h - self.bb,
                self.ri,
            ))
        .clamp(0.0, 1.0);
        (outer, inner)
    }

    /// Coverage far from the left and right edges and from the corners: depends only on the row.
    fn cover_mid(&self, py: f32) -> (f32, f32) {
        let ot = if self.round_top { -py } else { -1e9 };
        let ob = if self.round_bottom { py - self.h } else { -1e9 };
        let it = if self.round_top { self.bt - py } else { -1e9 };
        let ib = if self.round_bottom {
            py - (self.h - self.bb)
        } else {
            -1e9
        };
        (
            (0.5 - ot.max(ob)).clamp(0.0, 1.0),
            (0.5 - it.max(ib)).clamp(0.0, 1.0),
        )
    }
}

/// Blend of a box pixel: background (or whatever is underneath) inside,
/// border in the ring, faded with what is underneath at the outer edge.
#[inline]
fn frame_pixel(under: u8, outer: f32, inner: f32, border_gray: u8, bg: Option<u8>) -> u8 {
    let under = under as f32;
    let fill = bg.map(|g| g as f32).unwrap_or(under);
    let ring = (outer - inner).max(0.0);
    // + 0.5 then truncation: rounding without a libm call (slow on the e-reader).
    (fill * inner + border_gray as f32 * ring + under * (1.0 - outer) + 0.5).clamp(0.0, 255.0) as u8
}

/// Integer weights (out of 256) of the background, the border and what is underneath for a coverage.
#[inline]
fn frame_weights(outer: f32, inner: f32) -> (u32, u32, u32) {
    let wf = (inner * 256.0 + 0.5) as u32;
    let wo = (outer * 256.0 + 0.5) as u32;
    (wf, wo.saturating_sub(wf), 256 - wo)
}

/// Box: background (optional) and border of `border` px on the sides `sides`
/// (1 top, 2 right, 4 bottom, 8 left), rounded corners of radius `r` where the frame
/// is closed (`round_top`, `round_bottom`). Rounded edges are smoothed (anti-aliasing).
/// Only the pixels near the left and right edges are computed one by one; the
/// middle of each row has the same coverage everywhere and is filled in one block.
#[allow(clippy::too_many_arguments)]
pub fn frame_box(
    c: &mut Canvas,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    border: f32,
    border_gray: u8,
    sides: u8,
    bg: Option<u8>,
    r: f32,
    round_top: bool,
    round_bottom: bool,
) {
    if w <= 0 || h <= 0 {
        return;
    }
    let (wf, hf) = (w as f32, h as f32);
    let r = r.min(wf / 2.0).min(hf / 2.0).max(0.0);
    let b = border.max(0.0);
    let g = FrameGeom {
        w: wf,
        h: hf,
        r,
        ri: (r - b).max(0.0),
        bt: if sides & 1 != 0 && round_top { b } else { 0.0 },
        br: if sides & 2 != 0 { b } else { 0.0 },
        bb: if sides & 4 != 0 && round_bottom {
            b
        } else {
            0.0
        },
        bl: if sides & 8 != 0 { b } else { 0.0 },
        round_top,
        round_bottom,
    };
    // Columns computed one by one: [0, edge_l) and [edge_r, w).
    let edge_l = ((r.max(g.bl) + 1.0).ceil() as i32).min(w);
    let edge_r = ((wf - r.max(g.br) - 1.0).floor() as i32).max(edge_l);
    let (cw, ch) = (c.width as i32, c.height as i32);
    // Straight rows (outside the corners and the top and bottom edges): the coverage of the
    // edge columns is the same on every row. Computed once, in integer
    // weights, at the first straight row.
    let straight = |py: f32| {
        let (ri, bt, bb) = (g.ri, g.bt, g.bb);
        !(round_top && (py < r || py < bt + ri))
            && !(round_bottom && (py > hf - r || py > hf - bb - ri))
            && g.cover_mid(py) == (1.0, 1.0)
    };
    let mut template: Option<Vec<(i32, u32, u32, u32)>> = None;
    for yy in 0..h {
        let cy = y + yy;
        if cy < 0 || cy >= ch {
            continue;
        }
        let py = yy as f32 + 0.5;
        let row = (cy * cw) as usize;
        if straight(py) {
            let tpl = template.get_or_insert_with(|| {
                (0..edge_l)
                    .chain(edge_r..w)
                    .map(|xx| {
                        let (outer, inner) = g.cover(xx as f32 + 0.5, py);
                        let (wf, wb, wu) = frame_weights(outer, inner);
                        (xx, wf, wb, wu)
                    })
                    .filter(|&(_, _, _, wu)| wu < 256)
                    .collect()
            });
            for &(xx, wf, wb, wu) in tpl.iter() {
                let cx = x + xx;
                if cx < 0 || cx >= cw {
                    continue;
                }
                let idx = row + cx as usize;
                let under = c.data[idx] as u32;
                let fill = bg.map(|b| b as u32).unwrap_or(under);
                c.data[idx] =
                    ((fill * wf + border_gray as u32 * wb + under * wu + 128) >> 8).min(255) as u8;
            }
        } else {
            for xx in (0..edge_l).chain(edge_r..w) {
                let cx = x + xx;
                if cx < 0 || cx >= cw {
                    continue;
                }
                let (outer, inner) = g.cover(xx as f32 + 0.5, py);
                if outer <= 0.0 {
                    continue;
                }
                let idx = row + cx as usize;
                c.data[idx] = frame_pixel(c.data[idx], outer, inner, border_gray, bg);
            }
        }
        let (from, to) = ((x + edge_l).max(0), (x + edge_r).min(cw));
        if from >= to {
            continue;
        }
        let (outer, inner) = g.cover_mid(py);
        let span = &mut c.data[row + from as usize..row + to as usize];
        if outer <= 0.0 || (outer >= 1.0 && inner >= 1.0 && bg.is_none()) {
            continue;
        } else if outer >= 1.0 && inner >= 1.0 {
            span.fill(bg.unwrap_or(255));
        } else if outer >= 1.0 && inner <= 0.0 {
            span.fill(border_gray);
        } else {
            for p in span.iter_mut() {
                *p = frame_pixel(*p, outer, inner, border_gray, bg);
            }
        }
    }
}

pub fn dim(c: &mut Canvas, x: i32, y: i32, w: i32, h: i32, amount: u8) {
    let r = Rect::new(
        x.max(0) as u32,
        y.max(0) as u32,
        w.max(0) as u32,
        h.max(0) as u32,
    )
    .clip(c.width, c.height);
    for yy in r.y..r.y + r.h {
        for xx in r.x..r.x + r.w {
            let v = c.get(xx, yy) as u32;
            c.put(xx, yy, (v * (255 - amount as u32) / 255) as u8);
        }
    }
}

/// 8-bit gray image (coverage, illustration).
#[derive(Clone, Debug)]
pub struct Gray {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    /// Coverage of a transparent image (PNG with alpha), `data` then being
    /// the image already laid over white; `None` for an opaque image. Copying
    /// onto the page composites the image with what is underneath (the gray background of a
    /// box, the border that an ornament overlaps).
    pub alpha: Option<Vec<u8>>,
}

impl Gray {
    /// Copies the image onto the canvas, dithering it to the 16 grays of the e-ink
    /// (Floyd-Steinberg error diffusion): the EPDC keeps only the
    /// 4 high bits, without dithering gradients show banding.
    pub fn blit(&self, c: &mut Canvas, x: i32, y: i32) {
        let w = self.width as usize;
        // Errors to carry over: current and next row, with a margin on each side.
        let mut cur = vec![0i32; w + 2];
        let mut next = vec![0i32; w + 2];
        for yy in 0..self.height {
            let cy = y + yy as i32;
            let row = (yy * self.width) as usize;
            next.iter_mut().for_each(|e| *e = 0);
            for xx in 0..w {
                let cx = x + xx as i32;
                let inside = cy >= 0 && cy < c.height as i32 && cx >= 0 && cx < c.width as i32;
                let a = self
                    .alpha
                    .as_ref()
                    .map(|al| al[row + xx] as i32)
                    .unwrap_or(255);
                if a == 0 {
                    // Transparent: the page stays as is, without dithering.
                    continue;
                }
                let mut src = self.data[row + xx] as i32;
                if a < 255 && inside {
                    // `data` = image × a + white × (1 − a); we replace the white with
                    // what is underneath.
                    let under = c.data[(cy as u32 * c.width + cx as u32) as usize] as i32;
                    src -= (255 - under) * (255 - a) / 255;
                }
                let v = (src + cur[xx + 1]).clamp(0, 255);
                let q = (v + 8) / 17; // 0..15
                let out = (q * 17) as u8;
                let err = v - out as i32;
                // Distribution 7/16, 3/16, 5/16 and the remainder: the whole error is
                // kept, otherwise small differences (|err| < 3) vanish in
                // integer division and flat areas are no longer dithered.
                let e7 = err * 7 / 16;
                let e3 = err * 3 / 16;
                let e5 = err * 5 / 16;
                cur[xx + 2] += e7;
                next[xx] += e3;
                next[xx + 1] += e5;
                next[xx + 2] += err - e7 - e3 - e5;
                if inside {
                    c.put(cx as u32, cy as u32, out);
                }
            }
            std::mem::swap(&mut cur, &mut next);
        }
    }

    /// Resizes by box averaging (good quality when shrinking).
    pub fn resize(&self, nw: u32, nh: u32) -> Gray {
        let plane = |p: &[u8]| box_plane(p, self.width, self.height, nw, nh);
        Gray {
            width: nw,
            height: nh,
            data: plane(&self.data),
            alpha: self.alpha.as_deref().map(plane),
        }
    }

    /// Exactly `nw × nh`: bilinear enlargement, box-average reduction.
    pub fn resize_exact(&self, nw: u32, nh: u32) -> Gray {
        if nw == self.width && nh == self.height {
            self.clone()
        } else if nw > self.width || nh > self.height {
            self.resize_bilinear(nw, nh)
        } else {
            self.resize(nw, nh)
        }
    }

    /// Enlargement by bilinear interpolation (box averaging, made
    /// for shrinking, would give blocks when enlarging).
    pub fn resize_bilinear(&self, nw: u32, nh: u32) -> Gray {
        let plane = |p: &[u8]| bilinear_plane(p, self.width, self.height, nw, nh);
        Gray {
            width: nw,
            height: nh,
            data: plane(&self.data),
            alpha: self.alpha.as_deref().map(plane),
        }
    }

    /// Resizes to fit within `max_w × max_h` while keeping proportions.
    pub fn fit(&self, max_w: u32, max_h: u32) -> Gray {
        let s = (max_w as f32 / self.width as f32)
            .min(max_h as f32 / self.height as f32)
            .min(1.0);
        let nw = ((self.width as f32 * s).round() as u32).max(1);
        let nh = ((self.height as f32 * s).round() as u32).max(1);
        if nw == self.width && nh == self.height {
            self.clone()
        } else {
            self.resize(nw, nh)
        }
    }

    pub fn from_pgm(bytes: &[u8]) -> Option<Gray> {
        let mut fields = Vec::new();
        let mut pos = 0;
        while fields.len() < 4 && pos < bytes.len() {
            while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
                pos += 1;
            }
            let start = pos;
            while pos < bytes.len() && !bytes[pos].is_ascii_whitespace() {
                pos += 1;
            }
            fields.push(std::str::from_utf8(&bytes[start..pos]).ok()?.to_string());
        }
        pos += 1;
        if fields.first().map(String::as_str) != Some("P5") {
            return None;
        }
        let (w, h): (u32, u32) = (fields[1].parse().ok()?, fields[2].parse().ok()?);
        let data = bytes.get(pos..pos + (w * h) as usize)?.to_vec();
        Some(Gray {
            width: w,
            height: h,
            data,
            alpha: None,
        })
    }

    pub fn to_pgm(&self) -> Vec<u8> {
        let mut v = format!("P5\n{} {}\n255\n", self.width, self.height).into_bytes();
        v.extend_from_slice(&self.data);
        v
    }
}

/// Display size of a book image of `w × h`: `scale` screen pixels per
/// image pixel (one CSS pixel is the text size divided by 16, as in
/// Apple Books where text is 16 CSS px), without exceeding `max_w × max_h`.
pub fn fit_size(w: u32, h: u32, scale: f32, max_w: u32, max_h: u32) -> (u32, u32) {
    let s = scale
        .min(max_w as f32 / w.max(1) as f32)
        .min(max_h as f32 / h.max(1) as f32);
    (
        ((w as f32 * s).round() as u32).max(1),
        ((h as f32 * s).round() as u32).max(1),
    )
}

/// Dimensions of a JPEG or PNG, read from the header without decoding anything:
/// layout only needs the space the image will occupy.
pub fn image_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        // Signature (8 bytes), length and type of the IHDR chunk (8), width, height.
        let be = |i: usize| {
            bytes
                .get(i..i + 4)
                .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        };
        return match (be(16), be(20)) {
            (Some(w), Some(h)) if w > 0 && h > 0 && bytes.get(12..16) == Some(b"IHDR") => {
                Some((w, h))
            }
            _ => None,
        };
    }
    if !bytes.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    // JPEG: FF xx segments + length, up to the start of frame (SOF0 to SOF15,
    // except DHT C4, JPG C8 and DAC CC) which carries height and width.
    let mut i = 2;
    while i + 4 <= bytes.len() {
        if bytes[i] != 0xFF {
            return None;
        }
        let marker = bytes[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        if marker == 0xD8 || (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            i += 2;
            continue;
        }
        let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let h = u16::from_be_bytes([*bytes.get(i + 5)?, *bytes.get(i + 6)?]) as u32;
            let w = u16::from_be_bytes([*bytes.get(i + 7)?, *bytes.get(i + 8)?]) as u32;
            return (w > 0 && h > 0).then_some((w, h));
        }
        if marker == 0xD9 || marker == 0xDA || len < 2 {
            return None;
        }
        i += 2 + len;
    }
    None
}

/// Decodes a JPEG or PNG to grayscale. For a JPEG meant to be displayed at most
/// at `max_w × max_h`, the decoder works directly at 1/2, 1/4 or 1/8
/// (scaling in the DCT): 4 to 60 times fewer pixels to compute.
/// Decisive on a Cortex-A7 for 1,500 × 2,000 covers.
pub fn decode_image_for(bytes: &[u8], max_w: u32, max_h: u32) -> Option<Gray> {
    if bytes.starts_with(&[0xFF, 0xD8]) {
        let mut d = jpeg_decoder::Decoder::new(bytes);
        if max_w > 0 && max_h > 0 {
            let _ = d.scale(max_w as u16, max_h as u16);
        }
        let px = d.decode().ok()?;
        let info = d.info()?;
        let (w, h) = (info.width as u32, info.height as u32);
        let data = match info.pixel_format {
            jpeg_decoder::PixelFormat::L8 => px,
            jpeg_decoder::PixelFormat::RGB24 => px.chunks_exact(3).map(luma).collect(),
            jpeg_decoder::PixelFormat::CMYK32 => px.chunks_exact(4).map(|c| 255 - c[3]).collect(),
            jpeg_decoder::PixelFormat::L16 => px.chunks_exact(2).map(|c| c[0]).collect(),
        };
        return Some(Gray {
            width: w,
            height: h,
            data,
            alpha: None,
        });
    }
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        let mut d = png::Decoder::new(std::io::Cursor::new(bytes));
        d.set_transformations(png::Transformations::normalize_to_color8());
        let mut r = d.read_info().ok()?;
        let mut buf = vec![0; r.output_buffer_size()?];
        let info = r.next_frame(&mut buf).ok()?;
        let px = &buf[..info.buffer_size()];
        let (data, alpha): (Vec<u8>, Option<Vec<u8>>) = match info.color_type {
            png::ColorType::Grayscale => (px.to_vec(), None),
            png::ColorType::GrayscaleAlpha => (
                px.chunks_exact(2)
                    .map(|c| blend_white(c[0], c[1]))
                    .collect(),
                Some(px.chunks_exact(2).map(|c| c[1]).collect()),
            ),
            png::ColorType::Rgb => (px.chunks_exact(3).map(luma).collect(), None),
            png::ColorType::Rgba => (
                px.chunks_exact(4)
                    .map(|c| blend_white(luma(&c[..3]), c[3]))
                    .collect(),
                Some(px.chunks_exact(4).map(|c| c[3]).collect()),
            ),
            png::ColorType::Indexed => return None,
        };
        // Coverage kept only if the image really has transparent parts.
        let alpha = alpha.filter(|a: &Vec<u8>| a.iter().any(|&v| v < 255));
        return Some(Gray {
            width: info.width,
            height: info.height,
            data,
            alpha,
        });
    }
    None
}

#[inline]
fn luma(c: &[u8]) -> u8 {
    ((c[0] as u32 * 54 + c[1] as u32 * 183 + c[2] as u32 * 19) / 256) as u8
}

#[inline]
fn blend_white(v: u8, a: u8) -> u8 {
    ((v as u32 * a as u32 + 255 * (255 - a as u32)) / 255) as u8
}

/// Reduction of a `w × h` byte plane to `nw × nh` by box averaging.
fn box_plane(src: &[u8], w: u32, h: u32, nw: u32, nh: u32) -> Vec<u8> {
    let mut out = vec![0u8; (nw * nh) as usize];
    for oy in 0..nh {
        let y0 = (oy as u64 * h as u64 / nh as u64) as u32;
        let y1 = (((oy + 1) as u64 * h as u64 / nh as u64) as u32)
            .max(y0 + 1)
            .min(h);
        for ox in 0..nw {
            let x0 = (ox as u64 * w as u64 / nw as u64) as u32;
            let x1 = (((ox + 1) as u64 * w as u64 / nw as u64) as u32)
                .max(x0 + 1)
                .min(w);
            let mut sum = 0u32;
            let mut n = 0u32;
            for yy in y0..y1 {
                let row = (yy * w) as usize;
                for xx in x0..x1 {
                    sum += src[row + xx as usize] as u32;
                    n += 1;
                }
            }
            out[(oy * nw + ox) as usize] = (sum / n.max(1)) as u8;
        }
    }
    out
}

/// Bilinear enlargement of a `w × h` byte plane to `nw × nh`.
fn bilinear_plane(src: &[u8], w: u32, h: u32, nw: u32, nh: u32) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let (nw_, nh_) = (nw as usize, nh as usize);
    // Indices and weights (out of 256) computed once per column and per row:
    // the inner loop only has integers left (Cortex-A7 without a fast FPU).
    let axis = |n: usize, src: usize| -> Vec<(usize, usize, u32)> {
        let scale = src as f32 / n as f32;
        (0..n)
            .map(|o| {
                let f = ((o as f32 + 0.5) * scale - 0.5).max(0.0);
                let i0 = (f as usize).min(src - 1);
                let i1 = (i0 + 1).min(src - 1);
                (i0, i1, ((f - i0 as f32) * 256.0).round().min(256.0) as u32)
            })
            .collect()
    };
    let xs = axis(nw_, w);
    let ys = axis(nh_, h);
    let d = src;
    let mut out = vec![0u8; nw_ * nh_];
    for (oy, &(y0, y1, ty)) in ys.iter().enumerate() {
        let (r0, r1) = (y0 * w, y1 * w);
        let row = &mut out[oy * nw_..(oy + 1) * nw_];
        for (o, &(x0, x1, tx)) in row.iter_mut().zip(xs.iter()) {
            let top = d[r0 + x0] as u32 * (256 - tx) + d[r0 + x1] as u32 * tx;
            let bot = d[r1 + x0] as u32 * (256 - tx) + d[r1 + x1] as u32 * tx;
            *o = ((top * (256 - ty) + bot * ty + 32_768) >> 16) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests_scale {
    use super::Gray;

    #[test]
    fn book_images_scale_with_text_and_fit_the_page() {
        let g = Gray {
            width: 137,
            height: 35,
            data: vec![128; 137 * 35],
            alpha: None,
        };
        // Text at 30 px: one CSS pixel = 1.875 screen pixels.
        let (w, h) = super::fit_size(g.width, g.height, 30.0 / 16.0, 630, 814);
        let s = g.resize_exact(w, h);
        assert_eq!((s.width, s.height), (257, 66));
        assert!(
            s.data.iter().all(|&v| v == 128),
            "l'interpolation d'un aplat change sa valeur"
        );
        // A large image is limited to the column.
        let big = Gray {
            width: 600,
            height: 300,
            data: vec![0; 600 * 300],
            alpha: None,
        };
        assert_eq!(
            super::fit_size(big.width, big.height, 30.0 / 16.0, 630, 814),
            (630, 315)
        );
        // An enlarged gradient stays monotonic (no blocks or overshoot).
        let ramp = Gray {
            width: 4,
            height: 1,
            data: vec![0, 85, 170, 255],
            alpha: None,
        };
        let r = ramp.resize_bilinear(16, 1);
        assert!(r.data.windows(2).all(|p| p[0] <= p[1]), "{:?}", r.data);
    }
}

#[cfg(test)]
mod tests_dither {
    use super::Gray;
    use signet_fb::Canvas;

    /// Transparent image on a gray background (an ornament on a box): the
    /// transparent parts leave the page intact, the half-covering
    /// parts blend with what is underneath, not with white.
    #[test]
    fn transparent_image_composes_with_the_page() {
        let mut c = Canvas::new(3, 1);
        c.data.fill(238);
        // Black at 0%, 50% and 100% coverage, already laid over white by the decoder.
        let g = Gray {
            width: 3,
            height: 1,
            data: vec![255, 127, 0],
            alpha: Some(vec![0, 128, 255]),
        };
        g.blit(&mut c, 0, 0);
        assert_eq!(c.data[0], 238);
        assert_eq!(c.data[1], 119);
        assert_eq!(c.data[2], 0);
        // Coverage follows the image when it changes size.
        let big = g.resize_bilinear(6, 2);
        assert_eq!(big.alpha.as_ref().map(|a| a.len()), Some(12));
        assert_eq!(g.resize(1, 1).alpha, Some(vec![127]));
    }

    #[test]
    fn blit_quantizes_to_16_levels_and_keeps_the_mean() {
        // Horizontal gradient 0..255 over 64 rows.
        let (w, h) = (256u32, 64u32);
        let data: Vec<u8> = (0..h).flat_map(|_| (0..w).map(|x| x as u8)).collect();
        let g = Gray {
            width: w,
            height: h,
            data: data.clone(),
            alpha: None,
        };
        let mut c = Canvas::new(w, h);
        c.clear(0xFF);
        g.blit(&mut c, 0, 0);
        assert!(
            c.data.iter().all(|v| v % 17 == 0),
            "valeurs hors des 16 gris"
        );
        let src: f64 = data.iter().map(|&v| v as f64).sum::<f64>() / data.len() as f64;
        let out: f64 = c.data.iter().map(|&v| v as f64).sum::<f64>() / c.data.len() as f64;
        assert!((src - out).abs() < 1.0, "moyenne {src:.1} → {out:.1}");
        // A flat gray 100 must be rendered by a mix of 85 and 102 (average ≈ 100), not a uniform band.
        let flat = Gray {
            width: 64,
            height: 64,
            data: vec![100; 64 * 64],
            alpha: None,
        };
        let mut c = Canvas::new(64, 64);
        flat.blit(&mut c, 0, 0);
        assert!(
            c.data.contains(&85) && c.data.contains(&102),
            "pas de tramage"
        );
        let out: f64 = c.data.iter().map(|&v| v as f64).sum::<f64>() / c.data.len() as f64;
        assert!((out - 100.0).abs() < 1.5, "moyenne de l'aplat : {out:.1}");
    }
}

#[cfg(test)]
mod tests_frame {
    use super::*;

    /// Reference version, pixel by pixel.
    #[allow(clippy::too_many_arguments)]
    fn reference(
        c: &mut Canvas,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        border: f32,
        border_gray: u8,
        sides: u8,
        bg: Option<u8>,
        r: f32,
        round_top: bool,
        round_bottom: bool,
    ) {
        let (wf, hf) = (w as f32, h as f32);
        let r = r.min(wf / 2.0).min(hf / 2.0).max(0.0);
        let b = border.max(0.0);
        let g = FrameGeom {
            w: wf,
            h: hf,
            r,
            ri: (r - b).max(0.0),
            bt: if sides & 1 != 0 && round_top { b } else { 0.0 },
            br: if sides & 2 != 0 { b } else { 0.0 },
            bb: if sides & 4 != 0 && round_bottom {
                b
            } else {
                0.0
            },
            bl: if sides & 8 != 0 { b } else { 0.0 },
            round_top,
            round_bottom,
        };
        for yy in 0..h {
            for xx in 0..w {
                let (cx, cy) = (x + xx, y + yy);
                if cx < 0 || cy < 0 || cx >= c.width as i32 || cy >= c.height as i32 {
                    continue;
                }
                let (outer, inner) = g.cover(xx as f32 + 0.5, yy as f32 + 0.5);
                if outer <= 0.0 {
                    continue;
                }
                let idx = (cy as u32 * c.width + cx as u32) as usize;
                c.data[idx] = frame_pixel(c.data[idx], outer, inner, border_gray, bg);
            }
        }
    }

    /// The fast path (middle of the rows of a block) gives exactly the pixels of
    /// the reference version: open or closed corners, with or without background, frame
    /// overflowing the canvas, one-pixel rule.
    #[test]
    fn fast_frame_matches_reference() {
        #[allow(clippy::type_complexity)]
        let cases: &[(i32, i32, i32, i32, f32, u8, u8, Option<u8>, f32, bool, bool)] = &[
            (10, 12, 200, 150, 3.75, 189, 15, Some(237), 22.5, true, true),
            (
                10,
                12,
                200,
                150,
                3.75,
                189,
                15,
                Some(237),
                22.5,
                false,
                true,
            ),
            (
                10,
                12,
                200,
                150,
                3.75,
                189,
                15,
                Some(237),
                22.5,
                true,
                false,
            ),
            (10, 12, 200, 150, 2.8, 0, 15, None, 0.0, true, true),
            (-20, -5, 300, 90, 2.0, 0, 15, None, 8.0, true, true),
            (5, 40, 105, 2, 1.875, 0, 1, None, 0.0, true, true),
            (3, 3, 30, 30, 4.0, 100, 15, Some(220), 40.0, true, true),
        ];
        for &(x, y, w, h, b, bg_border, sides, bg, r, rt, rb) in cases {
            let mut a = Canvas::new(240, 180);
            let mut z = Canvas::new(240, 180);
            for (i, p) in a.data.iter_mut().enumerate() {
                *p = 200 + (i % 50) as u8;
            }
            z.data.copy_from_slice(&a.data);
            frame_box(&mut a, x, y, w, h, b, bg_border, sides, bg, r, rt, rb);
            reference(&mut z, x, y, w, h, b, bg_border, sides, bg, r, rt, rb);
            // Integer weights on straight rows: at most one unit of difference.
            let worst = a
                .data
                .iter()
                .zip(&z.data)
                .map(|(p, q)| (*p as i32 - *q as i32).abs())
                .max()
                .unwrap();
            assert!(
                worst <= 1,
                "cas {:?} : écart {worst}",
                (x, y, w, h, b, sides, bg, r, rt, rb)
            );
        }
    }
}
