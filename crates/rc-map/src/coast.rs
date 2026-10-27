//! A coast: a line of points along an island's outline (a loop) or a walkway's side (open), measured along its
//! length. An edge is laid along it in stretches of its step (a piece's length): the drawn pieces (crate `rc-engine`)
//! and the collision ([`crate::terrain::Patch::fence`]) take the same stretches, so an opening takes out the same
//! stretch of both.

#[derive(Clone, Debug)]
pub struct Coast {
    pts: Vec<[f32; 3]>,
    at: Vec<f32>,
    pub len: f32,
    /// A loop (else an open line).
    closed: bool,
}

impl Coast {
    /// A loop of points.
    pub fn new(pts: Vec<[f32; 3]>) -> Coast { Coast::line(pts, true) }

    /// A line of points (`closed`: back to the first).
    pub fn line(pts: Vec<[f32; 3]>, closed: bool) -> Coast {
        let mut at = Vec::with_capacity(pts.len() + 1);
        let mut d = 0.0;
        let segs = if closed { pts.len() } else { pts.len().saturating_sub(1) };
        for k in 0..pts.len() {
            at.push(d);
            if k < segs {
                let (a, b) = (pts[k], pts[(k + 1) % pts.len()]);
                d += ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
            }
        }
        at.push(d);
        Coast { pts, at, len: d, closed }
    }

    /// A loop (else an open line).
    pub fn is_closed(&self) -> bool { self.closed }

    /// The point `s` along it (wrapping round a loop, held at the ends of a line).
    pub fn point(&self, s: f32) -> [f32; 3] {
        let n = self.pts.len();
        if n == 1 { return self.pts[0]; }
        let (k, t) = if self.closed {
            let s = s.rem_euclid(self.len.max(1e-6));
            let k = self.at.partition_point(|&d| d <= s).saturating_sub(1).min(n - 1);
            (k, s - self.at[k])
        } else {
            let s = s.clamp(0.0, self.len);
            let k = self.at[..n].partition_point(|&d| d <= s).saturating_sub(1).min(n - 2);
            (k, s - self.at[k])
        };
        let (a, b) = (self.pts[k], self.pts[(k + 1) % n]);
        let l = self.at[k + 1] - self.at[k];
        let t = if l > 1e-6 { (t / l).clamp(0.0, 1.0) } else { 0.0 };
        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
    }

    /// The way it runs at `s` (horizontal, unit; smoothed over a unit either side: the outline's steps are small).
    pub fn direction(&self, s: f32) -> [f32; 2] {
        let (s0, s1) = if self.closed { (s - 0.5, s + 0.5) } else { ((s - 0.5).max(0.0), (s + 0.5).min(self.len)) };
        let (a, b) = (self.point(s0), self.point(s1));
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = (dx * dx + dy * dy).sqrt().max(1e-6);
        [dx / l, dy / l]
    }

    /// The point `s` along it moved `inset` to its left (into the land, the walkway).
    pub fn inset(&self, s: f32, inset: f32) -> [f32; 3] {
        let (p, d) = (self.point(s), self.direction(s));
        [p[0] - d[1] * inset, p[1] + d[0] * inset, p[2]]
    }

    /// It cut into a whole number of stretches of about `step` (stretched a little to fit): their start and end.
    pub fn runs(&self, step: f32) -> Vec<(f32, f32)> {
        if self.len < 0.5 { return Vec::new(); }
        let count = (self.len / step.max(0.25)).round().max(1.0) as usize;
        let s = self.len / count as f32;
        (0..count).map(|i| (i as f32 * s, (i + 1) as f32 * s)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loops_wrap_and_lines_stop() {
        let sq = vec![[0.0, 0.0, 1.0], [4.0, 0.0, 1.0], [4.0, 4.0, 1.0], [0.0, 4.0, 1.0]];
        let l = Coast::new(sq.clone());
        assert_eq!(l.len, 16.0);
        assert_eq!(l.point(17.0), [1.0, 0.0, 1.0]);
        assert_eq!(l.runs(4.2).len(), 4);
        // Counter-clockwise: the left is inside.
        let i = l.inset(2.0, 0.5);
        assert!((i[0] - 2.0).abs() < 1e-4 && (i[1] - 0.5).abs() < 1e-4);
        let o = Coast::line(sq, false);
        assert_eq!(o.len, 12.0);
        assert_eq!(o.point(20.0), [0.0, 4.0, 1.0]);
    }
}
