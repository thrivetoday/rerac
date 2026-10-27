//! Sculpted terrain (docs/plan/rac_maker.md, Phase 3): a [`Patch`] is a grid of height points over a rectangle, the
//! editor's brushes raise, lower, smooth and flatten them, and its edges can drop as cliff walls down to a depth (a
//! raised patch with walls is a floating island, a mesa, a ledge). The same triangles are drawn (crate `rc-engine`'s
//! map renderer) and become level collision ([`crate::floor::collision`]).
//!
//! **Shapes.** A patch's `mask` gives each point a land value (game units, land where it is above 0; empty: all land).
//! Between points the outline runs where the value crosses 0 (marching squares: each cell keeps the part on the land
//! side, its corners and the crossings on its edges), so islands are round, blobby or holed rather than rectangles, and
//! the cliff walls and the edge (a game piece repeated along it, a railing or a wall, or an invisible barrier) follow
//! that outline.
//!
//! **Reshaping.** [`Patch::warped`] pulls the outline about a point (the editor's coastline anchors), [`Patch::offset`]
//! grows or shrinks all of it, [`Patch::grow_to`] makes the grid bigger for land that reaches past it; each resamples
//! the land, heights and paint onto the new grid.

use crate::AssetId;

/// Heights and positions are kept at the packed collision vertex precision on z (1/64).
fn q64(v: f32) -> f32 { (v * 64.0).round() / 64.0 }

/// The cell sizes a patch may use (game units between height points).
pub const SPACINGS: [f32; 3] = [1.0, 2.0, 4.0];
/// The most cells along one side.
pub const MAX_CELLS: u16 = 128;

/// A sculptable height grid.
#[derive(Clone, Debug, PartialEq)]
pub struct Patch {
    /// Its corner with the smallest x and y, and its base height.
    pub origin: [f32; 3],
    /// Game units between neighbouring height points ([`SPACINGS`]).
    pub spacing: f32,
    /// Cells along x and y (1..=[`MAX_CELLS`]); the points are one more each way.
    pub size: [u16; 2],
    /// Per point, row by row (y), each row along x: the height above `origin[2]` (1/64 precision).
    pub heights: Vec<f32>,
    /// The texture drawn on its top (None: a plain colour).
    pub texture: Option<AssetId>,
    /// World units per texture repeat.
    pub tile: f32,
    /// Cliff walls from every edge point down to `origin[2] − depth`, closed underneath (0: none: an open sheet).
    pub depth: f32,
    /// Textures painted over the top, in order (each drawn over the ones before it).
    pub layers: Vec<PaintLayer>,
    /// Per point: its land value (game units; land where above 0, the outline where it crosses 0). Empty: all land
    /// (the rectangle). The border points stay below 0 in a shaped patch, so its outline closes inside it.
    pub mask: Vec<f32>,
    /// What runs along its outline so Ratchet does not walk off.
    pub edge: Option<Edge>,
    /// A flat textured plane over its land (a ceiling, a canopy; walkable on top).
    pub roof: Option<Roof>,
    /// Its top's collision surface byte ([`crate::floor::SURFACES`]; crate::floor::GROUND: plain ground).
    pub surface: u8,
    /// Per point: a colour its top is shaded with (0xRRGGBB, 0xffffff: none). Empty: no shading.
    pub shade: Vec<u32>,
    /// Plants and details strewn over its land (drawn only, not solid).
    pub scatter: Vec<Scatter>,
}

/// One scattered piece: a shrub, tie or moby model at `at` (x, y; it stands on the land there, wherever the land is
/// sculpted to), turned by `yaw`, scaled by `scale`.
#[derive(Clone, Debug, PartialEq)]
pub struct Scatter {
    pub asset: AssetId,
    pub at: [f32; 2],
    pub yaw: f32,
    pub scale: f32,
}

/// A roof over a patch: its land's shape, `height` above the patch's base height.
#[derive(Clone, Debug, PartialEq)]
pub struct Roof {
    pub height: f32,
    pub texture: Option<AssetId>,
    /// World units per texture repeat.
    pub tile: f32,
}

/// A barrier along a patch's outline: a game piece (a railing or wall tie or moby) repeated along it, a textured wall,
/// or nothing drawn; solid to Ratchet up to `height` whatever it looks like, except in its openings.
#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub kind: EdgeKind,
    /// How high above the ground it blocks (and a wall stands).
    pub height: f32,
    /// Openings `[x, y, radius]`: the stretches whose middle lies within one are left out, drawn and solid alike
    /// (they stay put when the coast moves).
    pub gaps: Vec<[f32; 3]>,
    /// The length of one stretch (a piece's length; 0: not known yet, 3 units are taken). The pieces and the collision
    /// both go in stretches of it ([`crate::coast::Coast::runs`]).
    pub step: f32,
    /// How far into the land the barrier stands from the outline (a piece's depth: Ratchet stops at its land side).
    pub inset: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EdgeKind {
    /// Nothing drawn.
    Invisible,
    /// A game piece repeated along the outline.
    Piece(AssetId),
    /// A plain wall `height` high, textured (a tfrag texture, `tile` units per repeat).
    Wall { texture: Option<AssetId>, tile: f32 },
}

impl Edge {
    pub fn new(kind: EdgeKind, height: f32) -> Edge { Edge { kind, height, gaps: Vec::new(), step: 0.0, inset: 0.0 } }
    /// The stretch length used ([`Edge::step`], 3 units while unknown).
    pub fn stretch(&self) -> f32 { if self.step > 0.0 { self.step } else { 3.0 } }
    /// Its spans along a coast, in order: the openings (where the coast runs inside a gap, to an eighth of a unit) and
    /// between them the standing stretches, each stretch of coast between two openings cut into a whole number of
    /// pieces of about [`Edge::stretch`] (squeezed or stretched a little), so the pieces end where an opening begins.
    /// (start, end, open). Without openings: the coast in whole stretches.
    pub fn spans(&self, c: &crate::coast::Coast) -> Vec<(f32, f32, bool)> {
        let step = self.stretch();
        let whole = || c.runs(step).into_iter().map(|(a, b)| (a, b, false)).collect::<Vec<_>>();
        if self.gaps.is_empty() || c.len < 0.5 { return whole(); }
        let n = (c.len / 0.125).ceil().max(1.0) as usize;
        let ds = c.len / n as f32;
        let open: Vec<bool> = (0..n).map(|i| { let p = c.point((i as f32 + 0.5) * ds); self.open_at(p[0], p[1]) }).collect();
        if !open.iter().any(|&o| o) { return whole(); }
        if open.iter().all(|&o| o) { return vec![(0.0, c.len, true)]; }
        // On a loop, start where the state changes (so no span runs over the end).
        let start = if c.is_closed() { (0..n).find(|&i| open[i] != open[(i + n - 1) % n]).unwrap_or(0) } else { 0 };
        let at = |i: usize| open[(start + i) % n];
        let mut out: Vec<(f32, f32, bool)> = Vec::new();
        let mut i = 0;
        while i < n {
            let mut j = i;
            while j < n && at(j) == at(i) { j += 1; }
            let (a, b) = ((start + i) as f32 * ds, (start + j) as f32 * ds);
            if at(i) || b - a < 0.3 {
                // An opening (a sliver of a stretch left between two openings goes with them).
                match out.last_mut() {
                    Some((_, e, true)) if (*e - a).abs() < 1e-3 => *e = b,
                    _ => out.push((a, b, true)),
                }
            } else {
                let k = ((b - a) / step).round().max(1.0) as usize;
                out.extend((0..k).map(|m| (a + (b - a) * m as f32 / k as f32, a + (b - a) * (m + 1) as f32 / k as f32, false)));
            }
            i = j;
        }
        out
    }

    /// The barrier along a coast (left: the land): per standing stretch ([`Edge::spans`]), quads every unit or so from
    /// the ground up `height`, `inset` in from the coast, cut into pieces at most `piece` tall, both ways (a face stops
    /// Ratchet only coming at its front).
    pub fn fence_along(&self, c: &crate::coast::Coast, piece: f32, out: &mut Vec<Quad>) {
        let n = (self.height / piece).ceil().max(1.0) as usize;
        for (s0, s1, open) in self.spans(c) {
            if open { continue; }
            let parts = (s1 - s0).ceil().max(1.0) as usize;
            for i in 0..parts {
                let (a, b) = (c.inset(s0 + (s1 - s0) * i as f32 / parts as f32, self.inset), c.inset(s0 + (s1 - s0) * (i + 1) as f32 / parts as f32, self.inset));
                for k in 0..n {
                    let (z0, z1) = (self.height * k as f32 / n as f32, self.height * (k + 1) as f32 / n as f32);
                    let q = [[a[0], a[1], q64(a[2] + z0)], [a[0], a[1], q64(a[2] + z1)], [b[0], b[1], q64(b[2] + z1)], [b[0], b[1], q64(b[2] + z0)]];
                    out.push(q);
                    out.push([q[3], q[2], q[1], q[0]]);
                }
            }
        }
    }
    /// The game piece it is made of.
    pub fn piece(&self) -> Option<AssetId> { if let EdgeKind::Piece(id) = self.kind { Some(id) } else { None } }
    /// `(x, y)` lies in an opening.
    pub fn open_at(&self, x: f32, y: f32) -> bool { self.gaps.iter().any(|g| (x - g[0]).powi(2) + (y - g[1]).powi(2) <= g[2] * g[2]) }
}

/// The land value a shaped patch keeps on its border points (water: the outline closes inside the patch).
pub const BORDER: f32 = -1.0;
/// The largest land value kept (deep inside the land).
pub const LAND_MAX: f32 = 8.0;

/// A texture painted over a patch: how strongly it shows at each point (0..255, the points' order).
#[derive(Clone, Debug, PartialEq)]
pub struct PaintLayer {
    pub texture: AssetId,
    /// World units per texture repeat.
    pub tile: f32,
    pub weights: Vec<u8>,
}

/// A triangle (three corners in world units).
pub type Tri = [[f32; 3]; 3];
/// A quad (four corners, a planar wall piece).
pub type Quad = [[f32; 3]; 4];

/// A counter-clockwise polygon (seen from above) as triangles wound clockwise (the collision faces' up side): a fan
/// from its first point; a square (four points) split along its first and third points.
fn fan_cw(poly: &[[f32; 3]]) -> Vec<Tri> {
    // (A triangle with no area, where the outline runs through a point, is left out.)
    (1..poly.len().saturating_sub(1))
        .map(|k| [poly[0], poly[k + 1], poly[k]])
        .filter(|t| ((t[1][0] - t[0][0]) * (t[2][1] - t[0][1]) - (t[2][0] - t[0][0]) * (t[1][1] - t[0][1])).abs() > 1e-6)
        .collect()
}

impl Patch {
    /// A flat patch of `size` cells at `origin`.
    pub fn flat(origin: [f32; 3], spacing: f32, size: [u16; 2]) -> Patch {
        let n = (size[0] as usize + 1) * (size[1] as usize + 1);
        Patch { origin, spacing, size, heights: vec![0.0; n], texture: None, tile: 8.0, depth: 0.0, layers: Vec::new(), mask: Vec::new(), edge: None, roof: None, surface: crate::floor::GROUND, shade: Vec::new(), scatter: Vec::new() }
    }

    /// An island: a patch of `spacing` around `centre` (its top at `centre`'s height) shaped by `land(dx, dy)` (the land
    /// value at an offset from the centre: above 0 inside), `radius` the shape's reach; a border of water around it.
    pub fn island(centre: [f32; 3], spacing: f32, radius: f32, land: impl Fn(f32, f32) -> f32) -> Patch {
        let cells = (((radius * 2.0) / spacing).ceil() as u16 + 4).min(MAX_CELLS);
        let half = cells as f32 * spacing / 2.0;
        let snap = |v: f32| (v / spacing).round() * spacing;
        let origin = [snap(centre[0] - half).max(0.0), snap(centre[1] - half).max(0.0), q64(centre[2])];
        let mut p = Patch::flat(origin, spacing, [cells, cells]);
        let (nx, ny) = p.points();
        p.mask = (0..nx * ny)
            .map(|k| {
                let (i, j) = (k % nx, k / nx);
                let (x, y) = (origin[0] + i as f32 * spacing - centre[0], origin[1] + j as f32 * spacing - centre[1]);
                if i == 0 || j == 0 || i == nx - 1 || j == ny - 1 { BORDER } else { land(x, y).clamp(-LAND_MAX, LAND_MAX) }
            })
            .collect();
        p.quantize_mask();
        p
    }

    /// A point's land value (all land without a mask).
    pub fn land(&self, i: usize, j: usize) -> f32 { if self.mask.is_empty() { LAND_MAX } else { self.mask[self.at(i, j)] } }

    /// The land values kept at 1/8 (the map file's precision) and the border points as water.
    pub fn quantize_mask(&mut self) {
        if self.mask.is_empty() { return; }
        let (nx, ny) = self.points();
        for j in 0..ny {
            for i in 0..nx {
                let k = self.at(i, j);
                // Never exactly 0: the outline would run through the point itself and leave crumbs of zero length.
                let v = match (self.mask[k] * 8.0).round() / 8.0 { 0.0 => -0.125, v => v };
                self.mask[k] = if i == 0 || j == 0 || i == nx - 1 || j == ny - 1 { v.min(BORDER) } else { v.clamp(-LAND_MAX, LAND_MAX) };
            }
        }
    }

    /// Points along x and y.
    pub fn points(&self) -> (usize, usize) { (self.size[0] as usize + 1, self.size[1] as usize + 1) }

    /// The index of point `(i, j)` (`i` along x, `j` along y).
    pub fn at(&self, i: usize, j: usize) -> usize { j * self.points().0 + i }

    /// Point `(i, j)` in world units.
    pub fn point(&self, i: usize, j: usize) -> [f32; 3] {
        [self.origin[0] + i as f32 * self.spacing, self.origin[1] + j as f32 * self.spacing, self.origin[2] + self.heights[self.at(i, j)]]
    }

    /// The far corner (largest x and y).
    pub fn max(&self) -> [f32; 2] { [self.origin[0] + self.size[0] as f32 * self.spacing, self.origin[1] + self.size[1] as f32 * self.spacing] }

    /// The bottom of its cliff walls.
    pub fn bottom(&self) -> f32 { q64(self.origin[2] - self.depth) }

    /// Why it cannot be built (the map file reports it).
    pub fn check(&self) -> Result<(), String> {
        if !SPACINGS.contains(&self.spacing) { return Err(format!("spacing {} must be one of {SPACINGS:?}", self.spacing)); }
        if !(1..=MAX_CELLS).contains(&self.size[0]) || !(1..=MAX_CELLS).contains(&self.size[1]) {
            return Err(format!("size {:?} must be 1..={MAX_CELLS} cells each way", self.size));
        }
        let (nx, ny) = self.points();
        if self.heights.len() != nx * ny { return Err(format!("{} heights for {nx} × {ny} points", self.heights.len())); }
        let max = self.max();
        let tops = self.heights.iter().map(|h| self.origin[2] + h);
        let all = [self.origin[0], self.origin[1], max[0] - 0.001, max[1] - 0.001, self.bottom()].into_iter().chain(tops);
        if !all.into_iter().all(|v| v.is_finite() && (0.0..crate::WORLD_SIZE).contains(&v)) { return Err("outside the world (0..1024)".into()); }
        if !(self.tile > 0.0) { return Err("tile must be above 0".into()); }
        if !(self.depth >= 0.0) { return Err("depth must be 0 or more".into()); }
        for (k, l) in self.layers.iter().enumerate() {
            if l.weights.len() != nx * ny { return Err(format!("layer {k}: {} weights for {nx} × {ny} points", l.weights.len())); }
            if !(l.tile > 0.0) { return Err(format!("layer {k}: tile must be above 0")); }
        }
        if !self.mask.is_empty() && self.mask.len() != nx * ny { return Err(format!("{} mask values for {nx} × {ny} points", self.mask.len())); }
        if !self.shade.is_empty() && self.shade.len() != nx * ny { return Err(format!("{} shade values for {nx} × {ny} points", self.shade.len())); }
        if !self.scatter.iter().all(|s| s.at.iter().all(|v| v.is_finite()) && s.scale > 0.0 && s.scale <= 10.0) { return Err("scatter: positions and scales 0..10".into()); }
        if let Some(e) = &self.edge {
            if !(0.1..=40.0).contains(&e.height) { return Err(format!("edge height {} must be 0.1..40", e.height)); }
            if let EdgeKind::Wall { tile, .. } = e.kind {
                if !(tile > 0.0) { return Err("edge wall tile must be above 0".into()); }
            }
            if !e.gaps.iter().all(|g| g.iter().all(|v| v.is_finite()) && g[2] > 0.0) { return Err("edge gaps: [x, y, radius above 0]".into()); }
        }
        if let Some(r) = &self.roof {
            if !(0.25..=200.0).contains(&r.height) { return Err(format!("roof height {} must be 0.25..200", r.height)); }
            if !(r.tile > 0.0) { return Err("roof tile must be above 0".into()); }
            if !(self.origin[2] + r.height < crate::WORLD_SIZE) { return Err("roof outside the world".into()); }
        }
        Ok(())
    }

    /// The land of each cell: its polygons (counter-clockwise seen from above, at the top's heights), the whole square
    /// for a cell all on land. A cell the outline crosses keeps its land corners and the crossings on its edges; a cell
    /// with land on two opposite corners only keeps two pieces.
    pub fn land_polygons(&self) -> Vec<Vec<[f32; 3]>> {
        let (nx, ny) = self.points();
        let mut out = Vec::new();
        for j in 0..ny - 1 {
            for i in 0..nx - 1 {
                let corners = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)];
                let v = corners.map(|(a, b)| self.land(a, b));
                if v.iter().all(|&x| x > 0.0) {
                    out.push(corners.iter().map(|&(a, b)| self.point(a, b)).collect());
                    continue;
                }
                if v.iter().all(|&x| x <= 0.0) { continue; }
                // Walk the cell's border counter-clockwise: from each crossing into the land, its land corners up to the
                // crossing out of it; the outline closes the piece.
                let cross = |k: usize| {
                    let (a, b) = (k, (k + 1) % 4);
                    let t = v[a] / (v[a] - v[b]);
                    let (pa, pb) = (self.point(corners[a].0, corners[a].1), self.point(corners[b].0, corners[b].1));
                    [pa[0] + (pb[0] - pa[0]) * t, pa[1] + (pb[1] - pa[1]) * t, q64(pa[2] + (pb[2] - pa[2]) * t)]
                };
                for start in 0..4 {
                    // A crossing into the land on edge `start` (from corner start, water, to the next, land).
                    if !(v[start] <= 0.0 && v[(start + 1) % 4] > 0.0) { continue; }
                    let mut poly = vec![cross(start)];
                    let mut k = (start + 1) % 4;
                    while v[k] > 0.0 {
                        poly.push(self.point(corners[k].0, corners[k].1));
                        if v[(k + 1) % 4] <= 0.0 { poly.push(cross(k)); break; }
                        k = (k + 1) % 4;
                    }
                    out.push(poly);
                }
            }
        }
        out
    }

    /// The top's triangles, wound like the floors' quads (clockwise seen from above): two per land cell (split along
    /// the cell's (0, 0)–(1, 1) diagonal), fans of the pieces the outline cuts.
    pub fn top(&self) -> Vec<Tri> {
        self.land_polygons().into_iter().flat_map(|poly| fan_cw(&poly)).collect()
    }

    /// The outline: its segments at the top's heights, each with the land on its left (counter-clockwise around the
    /// land seen from above). Without a mask the rectangle's rim.
    pub fn outline(&self) -> Vec<([f32; 3], [f32; 3])> {
        if self.mask.is_empty() {
            let rim = self.rim();
            return (0..rim.len()).map(|k| (self.point(rim[k].0, rim[k].1), self.point(rim[(k + 1) % rim.len()].0, rim[(k + 1) % rim.len()].1))).collect();
        }
        let mut out = Vec::new();
        for poly in self.land_polygons() {
            // The polygon's sides that are not on a cell edge are the outline's: they join a crossing out of the land to
            // the next crossing into it (the piece's last point back to its first).
            let n = poly.len();
            for k in 0..n {
                let (a, b) = (poly[k], poly[(k + 1) % n]);
                if !self.on_cell_edge(a, b) { out.push((a, b)); }
            }
        }
        out
    }

    /// Both points lie on one grid line (a side of the cell, not the outline crossing it).
    fn on_cell_edge(&self, a: [f32; 3], b: [f32; 3]) -> bool {
        let g = |v: f32, o: f32| ((v - o) / self.spacing).round() * self.spacing + o;
        let on = |v: f32, o: f32| (v - g(v, o)).abs() < 1e-4;
        (on(a[0], self.origin[0]) && on(b[0], self.origin[0]) && (a[0] - b[0]).abs() < 1e-4)
            || (on(a[1], self.origin[1]) && on(b[1], self.origin[1]) && (a[1] - b[1]).abs() < 1e-4)
    }

    /// The edge points in order around the patch (counter-clockwise seen from above), the first not repeated.
    pub fn rim(&self) -> Vec<(usize, usize)> {
        let (nx, ny) = self.points();
        let mut r: Vec<(usize, usize)> = (0..nx - 1).map(|i| (i, 0)).collect();
        r.extend((0..ny - 1).map(|j| (nx - 1, j)));
        r.extend((1..nx).rev().map(|i| (i, ny - 1)));
        r.extend((1..ny).rev().map(|j| (0, j)));
        r
    }

    /// The cliff walls (none without a depth): one wall per outline segment from the top down to [`Patch::bottom`],
    /// cut into pieces at most `piece` tall, each facing out and wound like the floors' quads seen from outside.
    pub fn walls(&self, piece: f32) -> Vec<Quad> {
        if self.depth <= 0.0 { return Vec::new(); }
        let bottom = self.bottom();
        let mut out = Vec::new();
        for (pa, pb) in self.outline() {
            // From the top down in steps: at each level the wall's two top corners lower towards the bottom.
            let span = (pa[2] - bottom).max(pb[2] - bottom).max(0.0);
            let n = (span / piece).ceil().max(1.0) as usize;
            for s in 0..n {
                let t0 = s as f32 / n as f32;
                let t1 = (s + 1) as f32 / n as f32;
                let z = |p: [f32; 3], t: f32| q64(p[2] + (bottom - p[2]) * t);
                let (a0, b0, a1, b1) = ([pa[0], pa[1], z(pa, t0)], [pb[0], pb[1], z(pb, t0)], [pa[0], pa[1], z(pa, t1)], [pb[0], pb[1], z(pb, t1)]);
                // Seen from outside (the land on the segment's left): a lower, a upper, b upper, b lower is clockwise.
                out.push([a1, a0, b0, b1]);
            }
        }
        out
    }

    /// The edge's barrier (none without an edge) along every loop of the outline ([`Edge::fence_along`]).
    pub fn fence(&self, piece: f32) -> Vec<Quad> {
        let Some(e) = &self.edge else { return Vec::new() };
        let mut out = Vec::new();
        for lp in self.loops() { e.fence_along(&crate::coast::Coast::new(lp), piece, &mut out); }
        out
    }

    /// Opens the edge where `on(x, y)` holds for its barrier's line (a walkway running over it): every run of coast
    /// whose barrier point is there becomes an opening exactly that long (a gap round its middle). The number made.
    pub fn open_where(&mut self, on: impl Fn(f32, f32) -> bool) -> usize {
        let Some(e) = self.edge.clone() else { return 0 };
        let mut gaps = e.gaps.clone();
        let mut opened = 0;
        for lp in self.loops() {
            let c = crate::coast::Coast::new(lp);
            if c.len < 0.5 { continue; }
            let n = (c.len / 0.125).ceil() as usize;
            let ds = c.len / n as f32;
            let mark: Vec<bool> = (0..n)
                .map(|i| {
                    let s = (i as f32 + 0.5) * ds;
                    let (p, q) = (c.point(s), c.inset(s, e.inset));
                    !e.open_at(p[0], p[1]) && on(q[0], q[1])
                })
                .collect();
            if !mark.iter().any(|&m| m) { continue; }
            // Start on an unmarked point (no run over the loop's end).
            let start = (0..n).find(|&i| !mark[i]).unwrap_or(0);
            let mut i = 0;
            while i < n {
                if !mark[(start + i) % n] { i += 1; continue; }
                let mut j = i;
                while j < n && mark[(start + j) % n] { j += 1; }
                let (s0, s1) = ((start + i) as f32 * ds, (start + j) as f32 * ds);
                let m = c.point((s0 + s1) / 2.0);
                gaps.push([q64(m[0]), q64(m[1]), q64((s1 - s0) / 2.0 + ds / 2.0)]);
                opened += 1;
                i = j;
            }
        }
        if let Some(e) = self.edge.as_mut() { e.gaps = gaps; }
        opened
    }

    /// The edge's openings near `at` (within `r`): every stretch with a point of its barrier there opened, or (`close`)
    /// every opening there closed. Whether anything changed.
    pub fn open_near(&mut self, at: [f32; 2], r: f32, close: bool) -> bool {
        let near = |x: f32, y: f32| (x - at[0]).powi(2) + (y - at[1]).powi(2) < r * r;
        if close {
            let Some(e) = self.edge.as_mut() else { return false };
            let n = e.gaps.len();
            e.gaps.retain(|g| !near(g[0], g[1]));
            return e.gaps.len() != n;
        }
        self.open_where(near) > 0
    }

    /// The roof's triangles (none without one): the land's shape at the roof's height, wound as the top (walkable).
    pub fn roof_tris(&self) -> Vec<Tri> {
        let Some(r) = &self.roof else { return Vec::new() };
        let z = q64(self.origin[2] + r.height);
        self.land_polygons().into_iter().flat_map(|poly| fan_cw(&poly.iter().map(|q| [q[0], q[1], z]).collect::<Vec<_>>())).collect()
    }

    /// The top's height at `(x, y)` (None: outside the patch or off its land).
    pub fn height_at(&self, x: f32, y: f32) -> Option<f32> {
        let max = self.max();
        if !(x >= self.origin[0] && y >= self.origin[1] && x <= max[0] && y <= max[1]) { return None; }
        let (fx, fy) = ((x - self.origin[0]) / self.spacing, (y - self.origin[1]) / self.spacing);
        let (i, j) = ((fx.floor() as usize).min(self.size[0] as usize - 1), (fy.floor() as usize).min(self.size[1] as usize - 1));
        let (u, v) = (fx - i as f32, fy - j as f32);
        if !self.mask.is_empty() {
            let m = |a: usize, b: usize| self.mask[self.at(a, b)];
            let land = m(i, j) * (1.0 - u) * (1.0 - v) + m(i + 1, j) * u * (1.0 - v) + m(i, j + 1) * (1.0 - u) * v + m(i + 1, j + 1) * u * v;
            if land <= 0.0 { return None; }
        }
        let h = |a: usize, b: usize| self.heights[self.at(a, b)];
        // The cell's two triangles (split along u = v).
        let rel = if v >= u { h(i, j) + (h(i + 1, j + 1) - h(i, j + 1)) * u + (h(i, j + 1) - h(i, j)) * v } else { h(i, j) + (h(i + 1, j) - h(i, j)) * u + (h(i + 1, j + 1) - h(i + 1, j)) * v };
        Some(self.origin[2] + rel)
    }

    /// Every height rounded to the collision precision (after a brush).
    pub fn quantize(&mut self) { for h in &mut self.heights { *h = q64(*h); } }

    /// The outline as closed loops of points (counter-clockwise around the land seen from above, each point once).
    pub fn loops(&self) -> Vec<Vec<[f32; 3]>> {
        let segs = self.outline();
        // Neighbouring cells find the same crossing to a hair: matched at 1/256.
        let key = |p: [f32; 3]| ((p[0] * 256.0).round() as i64, (p[1] * 256.0).round() as i64);
        let mut from: std::collections::HashMap<(i64, i64), Vec<usize>> = std::collections::HashMap::new();
        for (k, s) in segs.iter().enumerate() { from.entry(key(s.0)).or_default().push(k); }
        let mut used = vec![false; segs.len()];
        let mut out = Vec::new();
        for start in 0..segs.len() {
            if used[start] { continue; }
            let (mut lp, mut k) = (Vec::new(), start);
            loop {
                used[k] = true;
                lp.push(segs[k].0);
                match from.get(&key(segs[k].1)).and_then(|v| v.iter().copied().find(|&n| !used[n])) {
                    Some(n) => k = n,
                    None => break,
                }
            }
            if lp.len() >= 3 { out.push(lp); }
        }
        out
    }

    /// Grid coordinates of `(x, y)` (fractional, unclamped).
    fn frac(&self, x: f32, y: f32) -> (f32, f32) { ((x - self.origin[0]) / self.spacing, (y - self.origin[1]) / self.spacing) }

    /// `f` at grid coordinates `(fx, fy)`, bilinear between the points (clamped to the grid).
    fn bilinear(&self, fx: f32, fy: f32, f: impl Fn(usize, usize) -> f32) -> f32 {
        let (nx, ny) = self.points();
        let (fx, fy) = (fx.clamp(0.0, (nx - 1) as f32), fy.clamp(0.0, (ny - 1) as f32));
        let (i, j) = ((fx.floor() as usize).min(nx - 2), (fy.floor() as usize).min(ny - 2));
        let (u, v) = (fx - i as f32, fy - j as f32);
        f(i, j) * (1.0 - u) * (1.0 - v) + f(i + 1, j) * u * (1.0 - v) + f(i, j + 1) * (1.0 - u) * v + f(i + 1, j + 1) * u * v
    }

    /// The land value at `(x, y)` (bilinear; water outside the grid).
    pub fn land_at(&self, x: f32, y: f32) -> f32 {
        let (nx, ny) = self.points();
        let (fx, fy) = self.frac(x, y);
        if !(fx >= -1e-4 && fy >= -1e-4 && fx <= (nx - 1) as f32 + 1e-4 && fy <= (ny - 1) as f32 + 1e-4) { return BORDER; }
        self.bilinear(fx, fy, |i, j| self.land(i, j))
    }

    /// The patch on a new grid (corner `origin`, `size` cells; the same spacing and base height), each point taking the
    /// land, height and paint at `source(x, y)` on this one (water outside it, the nearest height and paint).
    pub fn resample(&self, origin: [f32; 2], size: [u16; 2], source: impl Fn(f32, f32) -> (f32, f32)) -> Patch {
        let mut p = Patch { origin: [origin[0], origin[1], self.origin[2]], size, heights: Vec::new(), mask: Vec::new(), shade: Vec::new(), ..self.clone() };
        for l in &mut p.layers { l.weights.clear(); }
        let (nx, ny) = p.points();
        for j in 0..ny {
            for i in 0..nx {
                let (x, y) = source(origin[0] + i as f32 * self.spacing, origin[1] + j as f32 * self.spacing);
                let (fx, fy) = self.frac(x, y);
                p.heights.push(self.bilinear(fx, fy, |a, b| self.heights[self.at(a, b)]));
                p.mask.push(self.land_at(x, y));
                for (k, l) in self.layers.iter().enumerate() {
                    p.layers[k].weights.push(self.bilinear(fx, fy, |a, b| l.weights[self.at(a, b)] as f32).round() as u8);
                }
                if !self.shade.is_empty() {
                    let ch = |c: u32| self.bilinear(fx, fy, |a, b| ((self.shade[self.at(a, b)] >> c) & 0xff) as f32).round().clamp(0.0, 255.0) as u32;
                    p.shade.push(ch(16) << 16 | ch(8) << 8 | ch(0));
                }
            }
        }
        p.quantize_mask();
        p.quantize();
        p
    }

    /// The grid made big enough to hold `lo`..`hi` (x, y) with its points where they were (kept inside the world and
    /// [`MAX_CELLS`]: Err when it cannot grow that far).
    pub fn grow_to(&mut self, lo: [f32; 2], hi: [f32; 2]) -> Result<(), String> {
        let s = self.spacing;
        let max = self.max();
        let mut origin = [self.origin[0], self.origin[1]];
        let mut far = max;
        for k in 0..2 {
            let below = ((self.origin[k] - lo[k].max(0.0)) / s).ceil().max(0.0);
            origin[k] = self.origin[k] - below * s;
            let above = ((hi[k].min(crate::WORLD_SIZE - 0.01) - max[k]) / s).ceil().max(0.0);
            far[k] = max[k] + above * s;
        }
        let size = [((far[0] - origin[0]) / s).round() as u32, ((far[1] - origin[1]) / s).round() as u32];
        if size[0] > MAX_CELLS as u32 || size[1] > MAX_CELLS as u32 {
            return Err(format!("this island is at its largest ({MAX_CELLS} cells across at {s} units): build another one beside it"));
        }
        if origin == [self.origin[0], self.origin[1]] && size == [self.size[0] as u32, self.size[1] as u32] { return Ok(()); }
        *self = self.resample(origin, [size[0] as u16, size[1] as u16], |x, y| (x, y));
        Ok(())
    }

    /// A shape for a patch without one: all of it land, the grid one cell bigger all round for the water border (or the
    /// border points made water where it cannot grow).
    pub fn ensure_mask(&mut self) {
        if !self.mask.is_empty() { return; }
        let (s, max) = (self.spacing, self.max());
        if self.grow_to([self.origin[0] - s, self.origin[1] - s], [max[0] + s, max[1] + s]).is_err() || self.mask.is_empty() {
            self.mask = vec![LAND_MAX; self.heights.len()];
            self.quantize_mask();
        }
    }

    /// `self` with its outline pulled: the land around `at` moved by `delta`, the pull fading over `reach` (a smooth
    /// falloff), the grid grown to take it. (Each frame of a drag starts again from the patch before it.)
    pub fn warped(&self, at: [f32; 2], delta: [f32; 2], reach: f32) -> Result<Patch, String> {
        let mut p = self.clone();
        p.ensure_mask();
        let len = (delta[0] * delta[0] + delta[1] * delta[1]).sqrt();
        // Never pulled further than the falloff carries (the land would tear).
        let reach = reach.max(len * 1.25).max(p.spacing * 2.0);
        let target = [at[0] + delta[0], at[1] + delta[1]];
        let pad = reach + p.spacing * 2.0;
        p.grow_to([target[0] - pad, target[1] - pad], [target[0] + pad, target[1] + pad])?;
        let pull = |x: f32, y: f32| {
            let d = ((x - target[0]).powi(2) + (y - target[1]).powi(2)).sqrt() / reach;
            let w = if d < 1.0 { (1.0 - d * d).powi(2) } else { 0.0 };
            (x - delta[0] * w, y - delta[1] * w)
        };
        let (origin, size) = ([p.origin[0], p.origin[1]], p.size);
        Ok(p.resample(origin, size, pull))
    }

    /// The whole outline moved out by `amount` game units (in: below 0), the grid grown for it.
    pub fn offset(&mut self, amount: f32) -> Result<(), String> {
        self.ensure_mask();
        if amount > 0.0 {
            let (pad, max) = (amount + self.spacing * 2.0, self.max());
            self.grow_to([self.origin[0] - pad, self.origin[1] - pad], [max[0] + pad, max[1] + pad])?;
        }
        for v in &mut self.mask { *v += amount; }
        self.quantize_mask();
        Ok(())
    }

    /// The patch moved by `d` (its grid, its edge's openings, its scatter).
    pub fn shift(&mut self, d: [f32; 3]) {
        for k in 0..3 { self.origin[k] = q64(self.origin[k] + d[k]); }
        if let Some(e) = self.edge.as_mut() { for g in &mut e.gaps { g[0] += d[0]; g[1] += d[1]; } }
        for s in &mut self.scatter { s.at[0] += d[0]; s.at[1] += d[1]; }
    }

    /// The shade colour at point `k` (white without shading).
    pub fn shade_at(&self, k: usize) -> [f32; 3] {
        let c = self.shade.get(k).copied().unwrap_or(0xffffff);
        [((c >> 16) & 0xff) as f32 / 255.0, ((c >> 8) & 0xff) as f32 / 255.0, (c & 0xff) as f32 / 255.0]
    }

    /// The shade colour at `(x, y)` (bilinear between the points).
    pub fn shade_xy(&self, x: f32, y: f32) -> [f32; 3] {
        if self.shade.is_empty() { return [1.0; 3]; }
        let (fx, fy) = self.frac(x, y);
        std::array::from_fn(|c| self.bilinear(fx, fy, |a, b| self.shade_at(self.at(a, b))[c]))
    }

    /// The land's middle (the average of its outline points; the grid's middle without land).
    pub fn centre(&self) -> [f32; 3] {
        let pts: Vec<[f32; 3]> = self.loops().into_iter().flatten().collect();
        if pts.is_empty() {
            let max = self.max();
            return [(self.origin[0] + max[0]) / 2.0, (self.origin[1] + max[1]) / 2.0, self.origin[2]];
        }
        let n = pts.len() as f32;
        [pts.iter().map(|p| p[0]).sum::<f32>() / n, pts.iter().map(|p| p[1]).sum::<f32>() / n, pts.iter().map(|p| p[2]).fold(f32::MIN, f32::max)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bump() -> Patch {
        let mut p = Patch::flat([100.0, 200.0, 50.0], 2.0, [4, 3]);
        let k = p.at(2, 1);
        p.heights[k] = 3.0;
        p
    }

    #[test]
    fn heights_follow_the_triangles() {
        let p = bump();
        assert_eq!(p.height_at(104.0, 202.0), Some(53.0));
        assert_eq!(p.height_at(100.0, 200.0), Some(50.0));
        assert_eq!(p.height_at(103.0, 201.0), Some(51.5));
        assert_eq!(p.height_at(99.0, 201.0), None);
        assert_eq!(p.top().len(), 4 * 3 * 2);
        assert!(p.check().is_ok());
    }

    #[test]
    fn walls_go_round_and_down() {
        let mut p = bump();
        assert!(p.walls(8.0).is_empty());
        p.depth = 20.0;
        // 14 rim cells, each 20 units tall in 3 pieces of at most 8.
        let w = p.walls(8.0);
        assert_eq!(p.rim().len(), 14);
        assert_eq!(w.len(), 14 * 3);
        assert!(w.iter().flatten().all(|v| v[2] >= 30.0 && v[2] <= 50.0));
        // The first wall (along y = 200) faces −y: clockwise seen from there.
        let q = w[0];
        let n = cross(sub(q[1], q[0]), sub(q[2], q[0]));
        assert!(n[1] > 0.0, "{n:?}");
    }

    #[test]
    fn checks_patches() {
        let mut p = bump();
        p.spacing = 3.0;
        assert!(p.check().is_err());
        let mut p = bump();
        p.heights.pop();
        assert!(p.check().is_err());
    }

    #[test]
    fn islands_are_round_and_closed() {
        let p = Patch::island([200.0, 200.0, 60.0], 2.0, 10.0, |x, y| 10.0 - (x * x + y * y).sqrt());
        assert!(p.check().is_ok());
        // Land in the middle, water outside the circle and on the border.
        assert_eq!(p.height_at(200.0, 200.0), Some(60.0));
        assert_eq!(p.height_at(212.0, 200.0), None);
        assert!(p.height_at(209.0, 200.0).is_some());
        // The outline: every segment's ends near the circle, and it closes (each end is another segment's start).
        let o = p.outline();
        assert!(o.len() > 20);
        for (a, b) in &o {
            for q in [a, b] {
                let r = ((q[0] - 200.0).powi(2) + (q[1] - 200.0).powi(2)).sqrt();
                assert!((r - 10.0).abs() < 0.6, "{q:?} at {r}");
            }
            assert!(o.iter().any(|(c, _)| (c[0] - b[0]).abs() < 1e-3 && (c[1] - b[1]).abs() < 1e-3), "open at {b:?}");
        }
        // Counter-clockwise around the land: the centre is on each segment's left.
        for (a, b) in &o {
            let side = (b[0] - a[0]) * (200.0 - a[1]) - (b[1] - a[1]) * (200.0 - a[0]);
            assert!(side > 0.0);
        }
        // The top's area is about the circle's.
        let area: f32 = p.top().iter().map(|t| ((t[1][0] - t[0][0]) * (t[2][1] - t[0][1]) - (t[2][0] - t[0][0]) * (t[1][1] - t[0][1])).abs() / 2.0).sum();
        assert!((area - std::f32::consts::PI * 100.0).abs() < 8.0, "{area}");
        // Every top triangle is wound clockwise seen from above.
        assert!(p.top().iter().all(|t| (t[1][0] - t[0][0]) * (t[2][1] - t[0][1]) - (t[2][0] - t[0][0]) * (t[1][1] - t[0][1]) < 0.0));
    }

    #[test]
    fn edges_follow_the_outline() {
        let mut p = Patch::island([200.0, 200.0, 60.0], 2.0, 6.0, |x, y| 6.0 - x.abs().max(y.abs()));
        assert!(p.fence(8.0).is_empty());
        p.edge = Some(Edge::new(EdgeKind::Invisible, 1.5));
        let f = p.fence(8.0);
        assert!(f.len() >= 2 * p.outline().len() / 2 && f.len() % 2 == 0);
        assert!(f.iter().all(|q| (q[1][2] - q[0][2]).abs().max((q[2][2] - q[3][2]).abs()) > 1.49));
        // A tall wall comes in pieces; an opening leaves its stretch out.
        p.edge = Some(Edge::new(EdgeKind::Wall { texture: None, tile: 4.0 }, 20.0));
        let all = p.fence(8.0).len();
        assert_eq!(all % 6, 0);
        p.edge.as_mut().unwrap().gaps.push([206.0, 200.0, 2.5]);
        let open = p.fence(8.0);
        assert!(open.len() < all && !open.is_empty());
        // The stretch at the opening's middle is gone, whole: nothing stands right there.
        assert!(open.iter().all(|q| ((q[0][0] + q[3][0]) / 2.0 - 206.0).powi(2) + ((q[0][1] + q[3][1]) / 2.0 - 200.0).powi(2) > 0.75 * 0.75));
        // Set in from the coast by the inset.
        p.edge.as_mut().unwrap().inset = 1.0;
        let inside = p.fence(8.0).iter().filter(|q| p.land_at(q[0][0], q[0][1]) > 0.0).count();
        assert!(inside * 10 >= p.fence(8.0).len() * 9, "{inside} of {} on the land", p.fence(8.0).len());
        // A roof: the land's shape up high.
        p.roof = Some(Roof { height: 6.0, texture: None, tile: 8.0 });
        p.check().unwrap();
        assert_eq!(p.roof_tris().len(), p.top().len());
        assert!(p.roof_tris().iter().flatten().all(|v| (v[2] - 66.0).abs() < 1e-3));
    }

    #[test]
    fn outlines_chain_into_loops() {
        let p = Patch::island([200.0, 200.0, 60.0], 1.0, 8.0, |x, y| 8.0 - (x * x + y * y).sqrt());
        let loops = p.loops();
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].len(), p.outline().len());
        // A holed island: two loops.
        let ring = Patch::island([200.0, 200.0, 60.0], 1.0, 10.0, |x, y| { let d = (x * x + y * y).sqrt(); (10.0 - d).min(d - 4.0) });
        assert_eq!(ring.loops().len(), 2);
        // A plain patch: its rim.
        assert_eq!(Patch::flat([10.0, 10.0, 5.0], 2.0, [4, 3]).loops()[0].len(), 14);
    }

    #[test]
    fn pulling_the_coast_grows_and_shrinks_it() {
        let p = Patch::island([200.0, 200.0, 60.0], 1.0, 6.0, |x, y| 6.0 - (x * x + y * y).sqrt());
        let east = |q: &Patch| q.loops()[0].iter().map(|v| v[0]).fold(f32::MIN, f32::max);
        let before = east(&p);
        // The east coast pulled 10 out: the grid grows, the land reaches it, the west stays.
        let out = p.warped([206.0, 200.0], [10.0, 0.0], 6.0).unwrap();
        out.check().unwrap();
        assert!(east(&out) > before + 8.0, "{} → {}", before, east(&out));
        assert!(out.land_at(194.5, 200.0) > 0.0 && out.land_at(193.0, 200.0) <= 0.0);
        assert_eq!(out.loops().len(), 1);
        // Pushed 3 in: smaller.
        let inn = p.warped([206.0, 200.0], [-3.0, 0.0], 4.0).unwrap();
        assert!(inn.land_at(204.0, 200.0) <= 0.0 && inn.land_at(202.0, 200.0) > 0.0 && inn.land_at(200.0, 205.0) > 0.0);
        // Grown all round by 2.
        let mut big = p.clone();
        big.offset(2.0).unwrap();
        assert!(east(&big) > before + 1.5 && big.land_at(193.0, 200.0) > 0.0);
        // A plain patch takes a shape first, its land where it was.
        let flat = Patch::flat([100.0, 100.0, 5.0], 2.0, [8, 8]);
        let pulled = flat.warped([116.0, 108.0], [6.0, 0.0], 6.0).unwrap();
        assert!(pulled.land_at(121.0, 108.0) > 0.0 && pulled.land_at(101.0, 101.0) > 0.0);
        assert!(pulled.land_at(97.0, 108.0) <= 0.0);
    }

    fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] }
    fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]] }
}
