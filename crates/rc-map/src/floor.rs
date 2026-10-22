//! Map floors as level collision: the parsed [`Collision`] the hero's and the mobys' queries read
//! (rc_game::collision_query), built cell by cell the way the disc's mesh is stored (docs/formats/collision_rac1.md):
//! every 4×4 cell under a floor gets the floor's four corners and one quad. Like `rc_game::hero::testkit::floor`.

use crate::Floor;
use rc_formats::collision::{Collision, CollisionCell, CollisionFace, CollisionLeafHeader, PackedCollisionVertex, CELL_SIZE};
use std::collections::BTreeMap;

/// The face type byte of plain ground: surface id none (0x1f, the most common byte on the disc: Novalis's walkable
/// ground), footstep sound class 0. (Surface id 1, `testkit`'s 0x21, kills Gaspar's pack biters that stand on it.)
pub const GROUND: u8 = 0x1f;

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

/// The cell layers a flat face at height `z` overlaps (the disc stores a face in every cell it overlaps): the one
/// whose box holds `z`, and the one below too when `z` lies on their shared boundary. The quad's packed z offset from
/// either centre is then in `[-2, 2]`.
fn cell_zs(z: f32) -> Vec<i16> {
    let c = (z / CELL_SIZE).floor() as i16;
    if (z / CELL_SIZE).fract() == 0.0 { vec![c - 1, c] } else { vec![c] }
}

/// The collision of `floors` (each [`check`]ed), with `surface` on every face.
pub fn collision(floors: &[Floor], surface: u8) -> Collision {
    let mut cells: BTreeMap<(i16, i16, i16), CollisionCell> = BTreeMap::new();
    for f in floors {
        let (x0, y0) = ((f.min[0] / CELL_SIZE) as i16, (f.min[1] / CELL_SIZE) as i16);
        let (x1, y1) = ((f.max[0] / CELL_SIZE) as i16, (f.max[1] / CELL_SIZE) as i16);
        let columns = (x0..x1).flat_map(|cx| (y0..y1).map(move |cy| (cx, cy)));
        for (cz, (cx, cy)) in cell_zs(f.height).into_iter().flat_map(|cz| columns.clone().map(move |c| (cz, c))) {
            let cell = cells.entry((cz, cy, cx)).or_insert_with(|| CollisionCell { x: cx, y: cy, z: cz, ..Default::default() });
            if cell.packed.len() + 4 > 255 { continue; }
            let (x, y) = (cx as f32 * CELL_SIZE, cy as f32 * CELL_SIZE);
            let centre = cell.centre();
            let base = cell.packed.len() as u8;
            for v in [[x, y, f.height], [x, y + CELL_SIZE, f.height], [x + CELL_SIZE, y + CELL_SIZE, f.height], [x + CELL_SIZE, y, f.height]] {
                let o = [(v[0] - centre[0]) * 16.0, (v[1] - centre[1]) * 16.0, (v[2] - centre[2]) * 64.0];
                cell.packed.push(PackedCollisionVertex::pack(o[0] as i32, o[1] as i32, o[2] as i32));
                cell.vertices.push(v);
            }
            // Quads first: this cell holds quads only.
            cell.faces.push(CollisionFace { v: [base, base + 1, base + 2], surface });
            cell.quad_v3.push(base + 3);
            cell.header = CollisionLeafHeader { face_count: cell.faces.len() as u16, vertex_count: cell.packed.len() as u8, quad_count: cell.faces.len() as u8 };
        }
    }
    // `Collision::cell_at` binary-searches the cells in (z, y, x) order.
    Collision { cells: cells.into_values().collect(), ..Default::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floor(min: [f32; 2], max: [f32; 2], height: f32) -> Floor { Floor { min, max, height, texture: None, tile: 8.0 } }

    #[test]
    fn cells_cover_the_floor() {
        let c = collision(&[floor([128.0, 112.0], [136.0, 120.0], 60.5)], GROUND);
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
        let c = collision(&[floor([0.0, 0.0], [4.0, 4.0], 60.0)], GROUND);
        assert_eq!(c.cells.iter().map(|c| c.z).collect::<Vec<_>>(), vec![14, 15]);
        for cell in &c.cells { for (p, w) in cell.packed.iter().zip(&cell.vertices) { assert_eq!(p.world(cell.centre()), *w); } }
    }

    #[test]
    fn floors_in_one_cell_share_it() {
        let c = collision(&[floor([0.0, 0.0], [4.0, 4.0], 1.0), floor([0.0, 0.0], [4.0, 4.0], 2.5)], GROUND);
        assert_eq!(c.cells.len(), 1);
        assert_eq!((c.cells[0].header.vertex_count, c.cells[0].header.quad_count), (8, 2));
    }

    #[test]
    fn checks_floors() {
        assert!(check(&floor([1.0, 0.0], [8.0, 8.0], 0.0)).is_err());
        assert!(check(&floor([8.0, 0.0], [8.0, 8.0], 0.0)).is_err());
        assert!(check(&floor([0.0, 0.0], [8.0, 8.0], 0.01)).is_err());
        assert!(check(&floor([0.0, 0.0], [8.0, 8.0], 0.25)).is_ok());
    }
}
