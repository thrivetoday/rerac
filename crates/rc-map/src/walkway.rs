//! Walkways and bridges (docs/plan/rac_maker.md, Phase 3 "path extrusions"): a [`Walkway`] is a strip of ground
//! `width` wide laid along a smooth line through its points, `depth` thick (sides down from its top, closed
//! underneath), with an optional edge (railings or walls, as an island's) along both sides. Its top is walkable
//! collision of its own surface; it is drawn by the map renderer (crate `rc-engine`).

use crate::terrain::{Edge, Quad, Tri};
use crate::AssetId;

/// A smooth curve (Catmull-Rom) through points, about a unit between samples, the points themselves among them.
pub fn curve(p: &[[f32; 3]]) -> Vec<[f32; 3]> {
    let n = p.len();
    if n < 2 { return p.to_vec(); }
    let at = |i: isize| p[i.clamp(0, n as isize - 1) as usize];
    let mut out = vec![p[0]];
    for i in 0..n - 1 {
        let (p0, p1, p2, p3) = (at(i as isize - 1), at(i as isize), at(i as isize + 1), at(i as isize + 2));
        let len = ((p2[0] - p1[0]).powi(2) + (p2[1] - p1[1]).powi(2) + (p2[2] - p1[2]).powi(2)).sqrt();
        let steps = len.ceil().max(1.0) as usize;
        for s in 1..=steps {
            let t = s as f32 / steps as f32;
            let (t2, t3) = (t * t, t * t * t);
            out.push(std::array::from_fn(|k| {
                0.5 * (2.0 * p1[k] + (p2[k] - p0[k]) * t + (2.0 * p0[k] - 5.0 * p1[k] + 4.0 * p2[k] - p3[k]) * t2 + (3.0 * p1[k] - p0[k] - 3.0 * p2[k] + p3[k]) * t3)
            }));
        }
    }
    out
}

/// Heights and positions at the packed collision vertex precision on z (1/64).
fn q64(v: f32) -> f32 { (v * 64.0).round() / 64.0 }

#[derive(Clone, Debug, PartialEq)]
pub struct Walkway {
    /// The points its centre line runs through (at least two), at the top's height.
    pub points: Vec<[f32; 3]>,
    pub width: f32,
    /// How far its sides go down from the top (0: a sheet).
    pub depth: f32,
    pub texture: Option<AssetId>,
    /// World units per texture repeat.
    pub tile: f32,
    /// Its top's collision surface byte (crate::floor::GROUND: plain ground).
    pub surface: u8,
    /// Along both sides.
    pub edge: Option<Edge>,
    /// Built only where it is off the land: where the islands' ground comes up to its top it stops (a bridge from
    /// coast to coast). See [`Walkway::pieces`].
    pub cut: bool,
}

impl Walkway {
    pub fn new(points: Vec<[f32; 3]>, width: f32) -> Walkway {
        Walkway { points, width, depth: 1.0, texture: None, tile: 4.0, surface: crate::floor::GROUND, edge: None, cut: false }
    }

    /// What is built of it: itself, or with `cut` the stretches of its centre line where the land (`land(x, y)`: the
    /// ground's height there, None off the land) stays below its top, each as a walkway of its own. Each end sits
    /// where the line meets the land (found to 1/32 of a unit), a fifth of a unit into it so no seam shows.
    pub fn pieces(&self, land: impl Fn(f32, f32) -> Option<f32>) -> Vec<Walkway> {
        if !self.cut { return vec![self.clone()]; }
        let c = self.centre();
        let blocked = |p: [f32; 3]| land(p[0], p[1]).is_some_and(|h| h >= p[2] - 0.05);
        let lerp = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] { std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t) };
        // Where the line between a free sample and a blocked one meets the land, pushed `into` the land a little.
        let crossing = |free: [f32; 3], hit: [f32; 3]| {
            let (mut lo, mut hi) = (0.0f32, 1.0f32);
            for _ in 0..6 {
                let m = (lo + hi) / 2.0;
                if blocked(lerp(free, hit, m)) { hi = m; } else { lo = m; }
            }
            let len = ((hit[0] - free[0]).powi(2) + (hit[1] - free[1]).powi(2)).sqrt().max(1e-6);
            lerp(free, hit, (hi + 0.2 / len).min(1.0))
        };
        let mut out = Vec::new();
        let mut run: Vec<[f32; 3]> = Vec::new();
        let finish = |run: &mut Vec<[f32; 3]>, out: &mut Vec<Walkway>| {
            // Points closer than half a unit to the one before are dropped (a check needs them apart); the end is
            // kept, the point before it dropped instead.
            let mut pts: Vec<[f32; 3]> = Vec::new();
            let n = run.len();
            for (k, p) in run.drain(..).enumerate() {
                let near = pts.last().is_some_and(|q: &[f32; 3]| ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt() < 0.5);
                if !near { pts.push(p); } else if k + 1 == n && pts.len() >= 2 { *pts.last_mut().unwrap() = p; }
            }
            if pts.len() >= 2 {
                let mut w = self.clone();
                w.points = pts.into_iter().map(|p| p.map(q64)).collect();
                w.cut = false;
                if w.check().is_ok() { out.push(w); }
            }
        };
        for i in 0..c.len() {
            let here = !blocked(c[i]);
            let before = i.checked_sub(1).map(|j| !blocked(c[j]));
            match (before, here) {
                (Some(false), true) => run.push(crossing(c[i], c[i - 1])),
                (Some(true), false) => {
                    run.push(crossing(c[i - 1], c[i]));
                    finish(&mut run, &mut out);
                    continue;
                }
                _ => {}
            }
            if here { run.push(c[i]); }
        }
        finish(&mut run, &mut out);
        out
    }

    /// Why it cannot be built.
    pub fn check(&self) -> Result<(), String> {
        if self.points.len() < 2 { return Err("a walkway needs at least 2 points".into()); }
        if !self.points.iter().flatten().all(|v| v.is_finite() && (0.0..crate::WORLD_SIZE).contains(v)) { return Err("a point is outside the world (0..1024)".into()); }
        if !(0.5..=40.0).contains(&self.width) { return Err(format!("width {} must be 0.5..40", self.width)); }
        if !(0.0..=50.0).contains(&self.depth) { return Err(format!("depth {} must be 0..50", self.depth)); }
        if !(self.tile > 0.0) { return Err("tile must be above 0".into()); }
        if self.points.windows(2).any(|w| ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt() < 0.25) {
            return Err("two points are on top of each other".into());
        }
        if let Some(e) = &self.edge {
            if !(0.1..=40.0).contains(&e.height) { return Err(format!("edge height {} must be 0.1..40", e.height)); }
        }
        Ok(())
    }

    /// The centre line: a smooth curve (Catmull-Rom) through the points, about a unit between samples, the points
    /// themselves among them.
    pub fn centre(&self) -> Vec<[f32; 3]> { curve(&self.points) }

    /// Its two sides along the centre line, from the first point to the last: (left, right), at the top's height.
    pub fn sides(&self) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
        let c = self.centre();
        let n = c.len();
        let half = self.width / 2.0;
        let (mut left, mut right) = (Vec::with_capacity(n), Vec::with_capacity(n));
        for i in 0..n {
            let (a, b) = (c[i.saturating_sub(1)], c[(i + 1).min(n - 1)]);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let l = (dx * dx + dy * dy).sqrt().max(1e-6);
            let (nx, ny) = (-dy / l * half, dx / l * half);
            let z = q64(c[i][2]);
            left.push([c[i][0] + nx, c[i][1] + ny, z]);
            right.push([c[i][0] - nx, c[i][1] - ny, z]);
        }
        (left, right)
    }

    /// Its outline as a loop, counter-clockwise seen from above (the walkway on its left): along the right side,
    /// across the far end, back along the left side, across the near end.
    pub fn outline(&self) -> Vec<[f32; 3]> {
        let (left, right) = self.sides();
        right.into_iter().chain(left.into_iter().rev()).collect()
    }

    /// The top: one quad per step along it, wound as the floors' quads (clockwise seen from above).
    pub fn top(&self) -> Vec<Quad> {
        let (l, r) = self.sides();
        (0..l.len() - 1).map(|i| [l[i], l[i + 1], r[i + 1], r[i]]).collect()
    }

    /// The top as triangles (each quad split along its first and third corners).
    pub fn top_tris(&self) -> Vec<Tri> { self.top().into_iter().flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]]).collect() }

    /// The sides and ends from the top down its depth (none without one), cut into pieces at most `piece` tall, each
    /// facing out (wound as the floors' quads seen from outside).
    pub fn walls(&self, piece: f32) -> Vec<Quad> {
        if self.depth <= 0.0 { return Vec::new(); }
        let o = self.outline();
        let n = (self.depth / piece).ceil().max(1.0) as usize;
        let mut out = Vec::new();
        for k in 0..o.len() {
            let (a, b) = (o[k], o[(k + 1) % o.len()]);
            for s in 0..n {
                let (z0, z1) = (self.depth * s as f32 / n as f32, self.depth * (s + 1) as f32 / n as f32);
                out.push([[a[0], a[1], q64(a[2] - z1)], [a[0], a[1], q64(a[2] - z0)], [b[0], b[1], q64(b[2] - z0)], [b[0], b[1], q64(b[2] - z1)]]);
            }
        }
        out
    }

    /// The lines its edge stands along: the right side forwards and the left side backwards (the walkway on each
    /// one's left).
    pub fn edge_lines(&self) -> [Vec<[f32; 3]>; 2] {
        let (left, right) = self.sides();
        [right, left.into_iter().rev().collect()]
    }

    /// The edge's barrier (none without an edge) along both sides ([`Edge::fence_along`]).
    pub fn fence(&self, piece: f32) -> Vec<Quad> {
        let Some(e) = &self.edge else { return Vec::new() };
        let mut out = Vec::new();
        for line in self.edge_lines() { e.fence_along(&crate::coast::Coast::line(line, false), piece, &mut out); }
        out
    }

    /// The top's height at `(x, y)` (None: not on it).
    pub fn height_at(&self, x: f32, y: f32) -> Option<f32> {
        for t in self.top_tris() {
            let (a, b, c) = (t[0], t[1], t[2]);
            let d = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
            if d.abs() < 1e-9 { continue; }
            let u = ((b[1] - c[1]) * (x - c[0]) + (c[0] - b[0]) * (y - c[1])) / d;
            let v = ((c[1] - a[1]) * (x - c[0]) + (a[0] - c[0]) * (y - c[1])) / d;
            if u >= -1e-4 && v >= -1e-4 && u + v <= 1.0 + 1e-4 { return Some(a[2] * u + b[2] * v + c[2] * (1.0 - u - v)); }
        }
        None
    }

    /// `(x, y)` is on its top at least `margin` in from its two sides (inside its railings when `margin` is their
    /// inset; its ends stay open).
    pub fn inside(&self, x: f32, y: f32, margin: f32) -> bool {
        if self.height_at(x, y).is_none() { return false; }
        let c = self.centre();
        let off = c.windows(2).map(|s| {
            let (a, b) = (s[0], s[1]);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let t = (((x - a[0]) * dx + (y - a[1]) * dy) / (dx * dx + dy * dy).max(1e-9)).clamp(0.0, 1.0);
            ((x - a[0] - dx * t).powi(2) + (y - a[1] - dy * t).powi(2)).sqrt()
        }).fold(f32::MAX, f32::min);
        off <= self.width / 2.0 - margin
    }

    /// Every point moved by `d` (the edge's openings too).
    pub fn shift(&mut self, d: [f32; 3]) {
        for p in &mut self.points { for k in 0..3 { p[k] = q64(p[k] + d[k]); } }
        if let Some(e) = self.edge.as_mut() { for g in &mut e.gaps { g[0] += d[0]; g[1] += d[1]; } }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn straight() -> Walkway { Walkway::new(vec![[100.0, 100.0, 50.0], [110.0, 100.0, 50.0]], 2.0) }

    #[test]
    fn a_straight_walkway_is_a_strip() {
        let w = straight();
        w.check().unwrap();
        let (l, r) = w.sides();
        assert_eq!(l.len(), 11);
        assert!(l.iter().all(|p| (p[1] - 101.0).abs() < 1e-4) && r.iter().all(|p| (p[1] - 99.0).abs() < 1e-4));
        assert_eq!(w.height_at(105.0, 100.5), Some(50.0));
        assert_eq!(w.height_at(105.0, 102.0), None);
        // Clockwise from above: the floors' winding (the cross product of the first two edges points down).
        let q = w.top()[0];
        let (e1, e2) = ([q[1][0] - q[0][0], q[1][1] - q[0][1]], [q[2][0] - q[1][0], q[2][1] - q[1][1]]);
        assert!(e1[0] * e2[1] - e1[1] * e2[0] < 0.0);
    }

    #[test]
    fn a_cut_bridge_stops_at_the_coasts() {
        let mut w = Walkway::new(vec![[100.0, 100.0, 50.0], [120.0, 100.0, 50.0]], 2.0);
        // Islands under both ends (their ground at the deck's height), open air between x 104 and 116.
        let land = |x: f32, _y: f32| (!(104.0..=116.0).contains(&x)).then_some(50.0);
        assert_eq!(w.pieces(land).len(), 1);
        assert_eq!(w.pieces(land)[0], w);
        w.cut = true;
        let p = w.pieces(land);
        assert_eq!(p.len(), 1);
        let (a, b) = (p[0].points[0], *p[0].points.last().unwrap());
        assert!((a[0] - 103.8).abs() < 0.1 && (b[0] - 116.2).abs() < 0.1, "{a:?} {b:?}");
        // Land below the deck does not cut it; land through the middle splits it.
        assert_eq!(w.pieces(|_, _| Some(40.0))[0].points.len(), w.centre().len());
        assert_eq!(w.pieces(|x, _| (109.0..=111.0).contains(&x).then_some(55.0)).len(), 2);
    }

    #[test]
    fn a_curved_bridge_goes_through_its_points_and_slopes() {
        let w = Walkway::new(vec![[100.0, 100.0, 50.0], [110.0, 105.0, 54.0], [120.0, 100.0, 50.0]], 3.0);
        let c = w.centre();
        for p in &w.points { assert!(c.iter().any(|q| (q[0] - p[0]).abs() + (q[1] - p[1]).abs() + (q[2] - p[2]).abs() < 1e-3)); }
        assert!(w.height_at(110.0, 105.0).is_some_and(|h| (h - 54.0).abs() < 0.1));
        // Walls all round, an edge both sides with an opening.
        let mut w = w;
        w.depth = 2.0;
        assert_eq!(w.walls(8.0).len(), w.outline().len());
        w.edge = Some(Edge::new(crate::terrain::EdgeKind::Invisible, 1.0));
        let all = w.fence(8.0).len();
        assert!(all > 0 && all % 2 == 0);
        w.edge.as_mut().unwrap().gaps.push([110.0, 106.5, 2.0]);
        assert!(w.fence(8.0).len() < all);
        w.check().unwrap();
    }
}
