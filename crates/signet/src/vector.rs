//! Smoothed vector shapes, for the icons Signet draws itself
//! (battery, margins, line spacing, cursors): paths of segments and cubic
//! Bézier curves, filled by the rasterizer that draws the letters. A contour
//! traversed in the opposite direction cuts out the shape (ring, frame).

use ab_glyph_rasterizer::{point, Rasterizer};
use signet_fb::Canvas;

type P = (f32, f32);

#[derive(Clone, Copy)]
enum Seg {
    Line(P, P),
    Cubic(P, P, P, P),
}

/// Quarter circle as a cubic Bézier: distance of the control points.
const K: f32 = 0.552_284_8;

#[derive(Clone, Default)]
pub struct Path {
    segs: Vec<Seg>,
}

impl Path {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a closed contour of points joined by segments.
    pub fn polygon(mut self, pts: &[P], reverse: bool) -> Self {
        let mut v: Vec<P> = pts.to_vec();
        if reverse {
            v.reverse();
        }
        for i in 0..v.len() {
            self.segs.push(Seg::Line(v[i], v[(i + 1) % v.len()]));
        }
        self
    }

    /// Quarter circle centered on `c`, from radius vector `u` to radius vector `v`.
    fn quarter(&mut self, c: P, u: P, v: P) {
        let p0 = (c.0 + u.0, c.1 + u.1);
        let p3 = (c.0 + v.0, c.1 + v.1);
        let p1 = (p0.0 + K * v.0, p0.1 + K * v.1);
        let p2 = (p3.0 + K * u.0, p3.1 + K * u.1);
        self.segs.push(Seg::Cubic(p0, p1, p2, p3));
    }

    /// Circle (`reverse`: opposite direction, for a hole).
    pub fn circle(mut self, cx: f32, cy: f32, r: f32, reverse: bool) -> Self {
        let dirs = [(r, 0.0), (0.0, r), (-r, 0.0), (0.0, -r)];
        for i in 0..4 {
            let (u, v) = if reverse {
                (dirs[(4 - i) % 4], dirs[(3 - i) % 4])
            } else {
                (dirs[i], dirs[(i + 1) % 4])
            };
            self.quarter((cx, cy), u, v);
        }
        self
    }

    /// Rectangle with rounded corners of radius `r`.
    pub fn rounded_rect(mut self, x: f32, y: f32, w: f32, h: f32, r: f32, reverse: bool) -> Self {
        let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
        // Clockwise on screen (y down): top, right, bottom, left.
        let corners = [
            ((x + w - r, y + r), (0.0, -r), (r, 0.0)),
            ((x + w - r, y + h - r), (r, 0.0), (0.0, r)),
            ((x + r, y + h - r), (0.0, r), (-r, 0.0)),
            ((x + r, y + r), (-r, 0.0), (0.0, -r)),
        ];
        let mut segs = Vec::new();
        let mut tmp = Path::new();
        for (c, u, v) in corners {
            tmp.quarter(c, u, v);
        }
        // Corners joined by the straight sides.
        for i in 0..4 {
            let Seg::Cubic(a, b, cc, d) = tmp.segs[i] else {
                unreachable!()
            };
            segs.push(Seg::Cubic(a, b, cc, d));
            let Seg::Cubic(n, ..) = tmp.segs[(i + 1) % 4] else {
                unreachable!()
            };
            if (d.0 - n.0).abs() > 1e-3 || (d.1 - n.1).abs() > 1e-3 {
                segs.push(Seg::Line(d, n));
            }
        }
        if reverse {
            segs.reverse();
            for s in &mut segs {
                *s = match *s {
                    Seg::Line(a, b) => Seg::Line(b, a),
                    Seg::Cubic(a, b, c, d) => Seg::Cubic(d, c, b, a),
                };
            }
        }
        self.segs.extend(segs);
        self
    }

    /// Rounded frame of thickness `t` (the edge is inside the rectangle).
    pub fn rounded_frame(self, x: f32, y: f32, w: f32, h: f32, r: f32, t: f32) -> Self {
        self.rounded_rect(x, y, w, h, r, false).rounded_rect(
            x + t,
            y + t,
            w - 2.0 * t,
            h - 2.0 * t,
            (r - t).max(0.0),
            true,
        )
    }

    /// Stroke of thickness `width` with round caps, from (x0, y0) to (x1, y1).
    pub fn stroke(mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32) -> Self {
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len = (dx * dx + dy * dy).sqrt().max(1e-3);
        let r = width / 2.0;
        let d = (dx / len * r, dy / len * r);
        let n = (-d.1, d.0);
        let neg = |p: P| (-p.0, -p.1);
        self.segs
            .push(Seg::Line((x0 + n.0, y0 + n.1), (x1 + n.0, y1 + n.1)));
        self.quarter((x1, y1), n, d);
        self.quarter((x1, y1), d, neg(n));
        self.segs
            .push(Seg::Line((x1 - n.0, y1 - n.1), (x0 - n.0, y0 - n.1)));
        self.quarter((x0, y0), neg(n), neg(d));
        self.quarter((x0, y0), neg(d), n);
        self
    }

    fn bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let mut it = self.segs.iter().flat_map(|s| match *s {
            Seg::Line(a, b) => vec![a, b],
            Seg::Cubic(a, b, c, d) => vec![a, b, c, d],
        });
        let first = it.next()?;
        Some(it.fold(
            (first.0, first.1, first.0, first.1),
            |(x0, y0, x1, y1), p| (x0.min(p.0), y0.min(p.1), x1.max(p.0), y1.max(p.1)),
        ))
    }
}

/// Coverage of the path (0..1) pixel by pixel, passed to `paint(under, coverage)`.
fn raster(c: &mut Canvas, path: &Path, paint: impl Fn(u8, f32) -> u8) {
    let Some((x0, y0, x1, y1)) = path.bounds() else {
        return;
    };
    let (ox, oy) = (x0.floor(), y0.floor());
    let (w, h) = ((x1 - ox).ceil() as usize + 1, (y1 - oy).ceil() as usize + 1);
    if w == 0 || h == 0 || w > 4096 || h > 4096 {
        return;
    }
    let mut r = Rasterizer::new(w, h);
    let p = |q: P| point(q.0 - ox, q.1 - oy);
    for s in &path.segs {
        match *s {
            Seg::Line(a, b) => r.draw_line(p(a), p(b)),
            Seg::Cubic(a, b, cc, d) => r.draw_cubic(p(a), p(b), p(cc), p(d)),
        }
    }
    let (cw, ch) = (c.width as i32, c.height as i32);
    r.for_each_pixel_2d(|px, py, a| {
        if a <= 0.0 {
            return;
        }
        let (x, y) = (ox as i32 + px as i32, oy as i32 + py as i32);
        if x < 0 || y < 0 || x >= cw || y >= ch {
            return;
        }
        let idx = (y * cw + x) as usize;
        c.data[idx] = paint(c.data[idx], a.min(1.0));
    });
}

/// Fills the path with `gray`, edges smoothed.
pub fn fill(c: &mut Canvas, path: &Path, gray: u8) {
    raster(c, path, |under, a| {
        (under as f32 + (gray as f32 - under as f32) * a + 0.5) as u8
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ink(c: &Canvas) -> f32 {
        c.data.iter().map(|&v| (255 - v) as f32 / 255.0).sum()
    }

    /// Areas: a disc, a rounded rectangle, a frame (hollow ring), a stroke.
    #[test]
    fn areas_match_geometry() {
        let mut c = Canvas::new(120, 120);
        c.clear(255);
        fill(&mut c, &Path::new().circle(60.0, 60.0, 30.0, false), 0);
        let disk = std::f32::consts::PI * 900.0;
        assert!(
            (ink(&c) - disk).abs() / disk < 0.01,
            "disque {} / {disk}",
            ink(&c)
        );
        let mut c = Canvas::new(120, 120);
        c.clear(255);
        fill(
            &mut c,
            &Path::new().rounded_frame(10.0, 10.0, 100.0, 60.0, 8.0, 2.0),
            0,
        );
        let outer = 100.0 * 60.0 - (4.0 - std::f32::consts::PI) * 64.0;
        let inner = 96.0 * 56.0 - (4.0 - std::f32::consts::PI) * 36.0;
        assert!(
            (ink(&c) - (outer - inner)).abs() < 6.0,
            "cadre {} / {}",
            ink(&c),
            outer - inner
        );
        let mut c = Canvas::new(120, 120);
        c.clear(255);
        fill(&mut c, &Path::new().stroke(20.0, 20.0, 100.0, 20.0, 4.0), 0);
        let capsule = 80.0 * 4.0 + std::f32::consts::PI * 4.0;
        assert!(
            (ink(&c) - capsule).abs() < 3.0,
            "trait {} / {capsule}",
            ink(&c)
        );
    }
}
