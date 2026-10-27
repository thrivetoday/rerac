//! Map floors as level collision: the parsed [`Collision`] the hero's and the mobys' queries read
//! (rc_game::collision_query), built cell by cell the way the disc's mesh is stored (docs/formats/collision_rac1.md):
//! every 4×4 cell under a floor gets the floor's four corners and one quad (like `rc_game::hero::testkit::floor`); terrain
//! patches' triangles and cliff walls, and rail strips, go into every cell they overlap.

use crate::Floor;
use rc_formats::collision::{Collision, CollisionCell, CollisionFace, CollisionLeafHeader, PackedCollisionVertex, CELL_SIZE};
use std::collections::{BTreeMap, HashMap, HashSet};

/// The face type byte of plain ground: surface id none (0x1f, the most common byte on the disc: Novalis's walkable
/// ground), footstep sound class 0. (Surface id 1, `testkit`'s 0x21, kills Gaspar's pack biters that stand on it.)
pub const GROUND: u8 = 0x1f;

/// The surfaces terrain can have (custom maps play by the full set of the game's surface rules,
/// rc_game::hero::surface): (byte, name, what it does).
pub const SURFACES: [(u8, &str, &str); 5] = [
    (GROUND, "Ground", "Plain ground"),
    (7, "Slippery", "Ice: Ratchet slides about and cannot stop at once"),
    (1, "Hot", "Burning: Ratchet bounces off it, hurt"),
    (8, "Deadly", "A fatal fall: touching it costs a life"),
    (2, "Magnetic", "With the Magneboots Ratchet walks on it, up walls and across ceilings"),
];

/// A surface byte's name.
pub fn surface_name(s: u8) -> &'static str { SURFACES.iter().find(|x| x.0 == s).map_or("Other", |x| x.1) }

/// Why a floor cannot be built ([`crate::MapDef::parse`] reports it).
pub fn check(f: &Floor) -> Result<(), String> {
    let cell = |v: f32| (v / CELL_SIZE).fract() == 0.0;
    if !(f.min.iter().chain(&f.max).all(|&v| cell(v))) { return Err(format!("min / max must be multiples of 4, got {:?} / {:?}", f.min, f.max)); }
    if !(f.min[0] < f.max[0] && f.min[1] < f.max[1]) { return Err("min must be below max on x and y".into()); }
    if !crate::in_world(&[f.min[0], f.min[1], f.max[0] - 0.001, f.max[1] - 0.001, f.height]) { return Err("outside the world (0..1024)".into()); }
    if (f.height * 64.0).fract() != 0.0 { return Err(format!("height {} is not a multiple of 1/64", f.height)); }
    if !(f.tile > 0.0) { return Err("tile must be above 0".into()); }
    Ok(())
}

/// Half the width of a rail's collision strip.
pub const RAIL_HALF_WIDTH: f32 = 0.15;
/// The longest piece a rail strip is cut into (each piece is stored in every cell its box touches).
const RAIL_PIECE: f32 = 2.0;
/// The tallest piece a cliff wall is cut into: every corner of a face must lie within ±32 units of the centre of each
/// cell it is stored in (the packed vertex range).
const WALL_PIECE: f32 = 8.0;

/// What one cell will hold: quads (stored first), then triangles, each with its surface byte.
#[derive(Default)]
struct Parts {
    quads: Vec<([[f32; 3]; 4], u8)>,
    tris: Vec<([[f32; 3]; 3], u8)>,
}

type Cells = BTreeMap<(i16, i16, i16), Parts>;

/// `p` at the packed vertex precision (1/16 on x and y, 1/64 on z: exact against every cell centre, `4c + 2`).
fn snap(p: [f32; 3]) -> [f32; 3] { [(p[0] * 16.0).round() / 16.0, (p[1] * 16.0).round() / 16.0, (p[2] * 64.0).round() / 64.0] }

/// The cells a face with corners `v` is stored in: every cell its box overlaps on x and y (a corner on a cell's far
/// edge is not in it), and on z also the cell below when it starts on a cell boundary (a flat face at a boundary height
/// is in both layers, as the disc stores them).
fn cells_of(v: &[[f32; 3]]) -> impl Iterator<Item = (i16, i16, i16)> {
    let lo = |k: usize| v.iter().map(|p| p[k]).fold(f32::MAX, f32::min);
    let hi = |k: usize| v.iter().map(|p| p[k]).fold(f32::MIN, f32::max);
    let span = |k: usize| {
        let (a, b) = (lo(k), hi(k));
        let first = (a / CELL_SIZE).floor() as i16;
        let last = if b > a { ((b - 1e-4) / CELL_SIZE).floor() as i16 } else { first };
        (first, last.max(first))
    };
    let (x, y) = (span(0), span(1));
    let z = (((lo(2) - 1e-4) / CELL_SIZE).floor() as i16, (hi(2) / CELL_SIZE).floor() as i16);
    (z.0..=z.1).flat_map(move |cz| (y.0..=y.1).flat_map(move |cy| (x.0..=x.1).map(move |cx| (cz, cy, cx))))
}

fn add_quad(cells: &mut Cells, v: [[f32; 3]; 4], surface: u8) {
    let v = v.map(snap);
    for c in cells_of(&v) { cells.entry(c).or_default().quads.push((v, surface)); }
}

fn add_tri(cells: &mut Cells, v: [[f32; 3]; 3], surface: u8) {
    let v = v.map(snap);
    for c in cells_of(&v) { cells.entry(c).or_default().tris.push((v, surface)); }
}

/// A rail's collision: a strip `2 ×` [`RAIL_HALF_WIDTH`] wide along its points. On the disc the rails are tie models
/// whose collision is in the level mesh, and the grind code needs that ground under Ratchet: more than 0.87 above
/// the ground for 30 ticks drops him off the rail (rc_game::hero::boots, the `high` test).
fn add_rail(cells: &mut Cells, points: &[[f32; 3]], surface: u8) {
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let flat = (d[0] * d[0] + d[1] * d[1]).sqrt();
        if flat < 1e-4 { continue; }
        // The left normal on the ground plane (the floor's winding: left edge first).
        let l = [-d[1] / flat * RAIL_HALF_WIDTH, d[0] / flat * RAIL_HALF_WIDTH, 0.0];
        let n = ((flat.hypot(d[2])) / RAIL_PIECE).ceil().max(1.0) as usize;
        let at = |t: f32| [a[0] + d[0] * t, a[1] + d[1] * t, a[2] + d[2] * t];
        for k in 0..n {
            let (p, q) = (at(k as f32 / n as f32), at((k + 1) as f32 / n as f32));
            add_quad(cells, [[p[0] - l[0], p[1] - l[1], p[2]], [p[0] + l[0], p[1] + l[1], p[2]], [q[0] + l[0], q[1] + l[1], q[2]], [q[0] - l[0], q[1] - l[1], q[2]]], surface);
        }
    }
}

/// One cell's record from its parts: shared vertices packed against the cell centre, quads first. A face that would
/// overflow the cell's 255 vertices, or has a corner out of the packing range, is left out (returned: the count).
fn build_cell((cz, cy, cx): (i16, i16, i16), parts: Parts) -> (CollisionCell, usize) {
    let mut cell = CollisionCell { x: cx, y: cy, z: cz, ..Default::default() };
    let centre = cell.centre();
    let mut index: HashMap<[i32; 3], u8> = HashMap::new();
    let mut dropped = 0;
    let mut face = |cell: &mut CollisionCell, corners: &[[f32; 3]]| -> Option<Vec<u8>> {
        let fields: Vec<[i32; 3]> = corners
            .iter()
            .map(|p| [((p[0] - centre[0]) * 16.0).round() as i32, ((p[1] - centre[1]) * 16.0).round() as i32, ((p[2] - centre[2]) * 64.0).round() as i32])
            .collect();
        if !fields.iter().all(|f| (-512..=511).contains(&f[0]) && (-512..=511).contains(&f[1]) && (-2048..=2047).contains(&f[2])) { return None; }
        let new: HashSet<&[i32; 3]> = fields.iter().filter(|f| !index.contains_key(*f)).collect();
        if cell.packed.len() + new.len() > 255 { return None; }
        Some(
            fields
                .iter()
                .zip(corners)
                .map(|(f, p)| {
                    *index.entry(*f).or_insert_with(|| {
                        cell.packed.push(PackedCollisionVertex::pack(f[0], f[1], f[2]));
                        cell.vertices.push(*p);
                        (cell.packed.len() - 1) as u8
                    })
                })
                .collect(),
        )
    };
    for (q, surface) in parts.quads {
        match face(&mut cell, &q) {
            Some(i) => {
                cell.faces.push(CollisionFace { v: [i[0], i[1], i[2]], surface });
                cell.quad_v3.push(i[3]);
            }
            None => dropped += 1,
        }
    }
    let quads = cell.faces.len();
    for (t, surface) in parts.tris {
        match face(&mut cell, &t) {
            Some(i) => cell.faces.push(CollisionFace { v: [i[0], i[1], i[2]], surface }),
            None => dropped += 1,
        }
    }
    cell.header = CollisionLeafHeader { face_count: cell.faces.len() as u16, vertex_count: cell.packed.len() as u8, quad_count: quads as u8 };
    (cell, dropped)
}

/// The collision of `floors` (each [`check`]ed), terrain `patches` (their tops and cliff walls), `rails` (each a
/// strip along its points) and `extra` triangles (solid objects' models), with `surface` on every face.
pub fn collision(floors: &[Floor], patches: &[crate::terrain::Patch], walkways: &[crate::walkway::Walkway], rails: &[Vec<[f32; 3]>], extra: &[[[f32; 3]; 3]], surface: u8) -> Collision {
    collision_counted(floors, patches, walkways, rails, extra, surface).0
}

/// [`collision`] and how many faces it had to leave out (full cells, out of the packing range: holes).
pub fn collision_counted(floors: &[Floor], patches: &[crate::terrain::Patch], walkways: &[crate::walkway::Walkway], rails: &[Vec<[f32; 3]>], extra: &[[[f32; 3]; 3]], surface: u8) -> (Collision, usize) {
    let mut cells = Cells::new();
    for f in floors {
        let (x0, y0) = ((f.min[0] / CELL_SIZE) as i16, (f.min[1] / CELL_SIZE) as i16);
        let (x1, y1) = ((f.max[0] / CELL_SIZE) as i16, (f.max[1] / CELL_SIZE) as i16);
        for cx in x0..x1 {
            for cy in y0..y1 {
                let (x, y, z) = (cx as f32 * CELL_SIZE, cy as f32 * CELL_SIZE, f.height);
                add_quad(&mut cells, [[x, y, z], [x, y + CELL_SIZE, z], [x + CELL_SIZE, y + CELL_SIZE, z], [x + CELL_SIZE, y, z]], surface);
            }
        }
    }
    for p in patches {
        // The top its own surface; the cliffs, the edge and the roof plain.
        let top = if p.surface == GROUND { surface } else { p.surface };
        for t in p.top() { add_tri(&mut cells, t, top); }
        for w in p.walls(WALL_PIECE) { add_quad(&mut cells, w, surface); }
        for q in p.fence(WALL_PIECE) { add_quad(&mut cells, q, surface); }
        for t in p.roof_tris() { add_tri(&mut cells, t, surface); }
    }
    for w in walkways {
        let top = if w.surface == GROUND { surface } else { w.surface };
        for q in w.top() { add_quad(&mut cells, q, top); }
        for q in w.walls(WALL_PIECE) { add_quad(&mut cells, q, surface); }
        for q in w.fence(WALL_PIECE) { add_quad(&mut cells, q, surface); }
    }
    for r in rails { add_rail(&mut cells, r, surface); }
    for &t in extra { add_tri(&mut cells, t, surface); }
    let mut dropped = 0;
    // `Collision::cell_at` binary-searches the cells in (z, y, x) order: the map's keys are in that order.
    let cells: Vec<CollisionCell> = cells
        .into_iter()
        .map(|(k, p)| {
            let (c, d) = build_cell(k, p);
            dropped += d;
            c
        })
        .collect();
    if dropped > 0 { eprintln!("map collision: {dropped} faces left out of full cells or out of the packing range"); }
    (Collision { cells, ..Default::default() }, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floor(min: [f32; 2], max: [f32; 2], height: f32) -> Floor { Floor { min, max, height, texture: None, tile: 8.0 } }

    #[test]
    fn cells_cover_the_floor() {
        let c = collision(&[floor([128.0, 112.0], [136.0, 120.0], 60.5)], &[], &[], &[], &[], GROUND);
        assert_eq!(c.cells.len(), 4);
        let i = c.cell_at(33, 29, 15).expect("the cell over (132..136, 116..120) at z 60..64");
        let cell = &c.cells[i];
        assert_eq!((cell.header.vertex_count, cell.header.face_count, cell.header.quad_count), (4, 1, 1));
        // The packed corners decode back to the floor's corners.
        for (p, w) in cell.packed.iter().zip(&cell.vertices) { assert_eq!(p.world(cell.centre()), *w); }
        assert_eq!(cell.vertices[2], [136.0, 120.0, 60.5]);
        assert!(c.cells.windows(2).all(|w| (w[0].z, w[0].y, w[0].x) < (w[1].z, w[1].y, w[1].x)));
    }

    #[test]
    fn a_floor_on_a_cell_boundary_is_in_both_layers() {
        let c = collision(&[floor([0.0, 0.0], [4.0, 4.0], 60.0)], &[], &[], &[], &[], GROUND);
        assert_eq!(c.cells.iter().map(|c| c.z).collect::<Vec<_>>(), vec![14, 15]);
        for cell in &c.cells { for (p, w) in cell.packed.iter().zip(&cell.vertices) { assert_eq!(p.world(cell.centre()), *w); } }
    }

    #[test]
    fn floors_in_one_cell_share_it() {
        let c = collision(&[floor([0.0, 0.0], [4.0, 4.0], 1.0), floor([0.0, 0.0], [4.0, 4.0], 2.5)], &[], &[], &[], &[], GROUND);
        assert_eq!(c.cells.len(), 1);
        assert_eq!((c.cells[0].header.vertex_count, c.cells[0].header.quad_count), (8, 2));
    }

    #[test]
    fn rails_get_a_strip_in_every_cell_they_cross() {
        let rail = vec![[130.0, 120.0, 61.4], [141.0, 126.0, 63.0]];
        let c = collision(&[], &[], &[], &[rail.clone()], &[], GROUND);
        // Every vertex packs exactly and lies within the packing range of its cell.
        for cell in &c.cells { for (p, w) in cell.packed.iter().zip(&cell.vertices) { assert_eq!(p.world(cell.centre()), *w); } }
        // The cells under points along the rail hold faces.
        for t in [0.0, 0.3, 0.5, 0.9] {
            let p: [f32; 3] = std::array::from_fn(|k| rail[0][k] + (rail[1][k] - rail[0][k]) * t);
            let at = |k: usize| (p[k] / CELL_SIZE).floor() as i16;
            let i = c.cell_at(at(0), at(1), at(2)).unwrap_or_else(|| panic!("no cell under {p:?}"));
            assert!(c.cells[i].header.quad_count > 0);
        }
    }

    #[test]
    fn checks_floors() {
        assert!(check(&floor([1.0, 0.0], [8.0, 8.0], 0.0)).is_err());
        assert!(check(&floor([8.0, 0.0], [8.0, 8.0], 0.0)).is_err());
        assert!(check(&floor([0.0, 0.0], [8.0, 8.0], 0.01)).is_err());
        assert!(check(&floor([0.0, 0.0], [8.0, 8.0], 0.25)).is_ok());
    }

    #[test]
    fn patch_tops_and_walls_are_solid_everywhere() {
        let mut p = crate::terrain::Patch::flat([100.0, 100.0, 60.0], 2.0, [8, 8]);
        for (k, h) in p.heights.iter_mut().enumerate() { *h = (k % 9) as f32 * 0.75; }
        p.depth = 30.0;
        let c = collision(&[], &[p.clone()], &[], &[], &[], GROUND);
        for cell in &c.cells {
            assert!(cell.header.quad_count as usize <= cell.faces.len());
            for (q, w) in cell.packed.iter().zip(&cell.vertices) { assert_eq!(q.world(cell.centre()), *w); }
        }
        // Under every top point on a grid, the cell holding it has faces.
        for i in 0..16 {
            for j in 0..16 {
                let (x, y) = (100.5 + i as f32, 100.5 + j as f32);
                let z = p.height_at(x, y).unwrap();
                let at = |v: f32| (v / CELL_SIZE).floor() as i16;
                let k = c.cell_at(at(x), at(y), at(z)).unwrap_or_else(|| panic!("no cell at {x} {y} {z}"));
                assert!(c.cells[k].header.face_count > 0);
            }
        }
        // The walls reach the bottom: a cell at the bottom of the x = 100 wall holds a quad.
        let k = c.cell_at(25, 26, ((60.0 - 30.0 + 1.0) / CELL_SIZE) as i16).expect("a wall cell near the bottom");
        assert!(c.cells[k].header.quad_count > 0);
    }
}
