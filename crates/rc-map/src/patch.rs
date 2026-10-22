//! A map's gameplay file: the host level's decompressed `gameplay_ntsc` with the map's content in place of the
//! level's (docs/formats/wad_layouts_rac1.md §3.5). New sections are **appended** and their header pointers moved to
//! them, so every section the map does not touch (level settings, lights, sounds, volumes, fog zones…) stays the
//! host's byte for byte, and every reader of the file sees one consistent level.
//!
//! What changes:
//! * moby instances (0x44): Ratchet's record (the host's, at the map's start), then the map's objects
//!   ([`TemplateRecord`]: a record and pvar block copied from an instance of the object's source level);
//! * pvar table / data / pointer fixups (0x54 / 0x58 / 0x5c): the placed records' blocks, renumbered;
//! * moby groups, moby-link fixups, shared data, cameras, occlusion mappings (0x48 / 0x50 / 0x4c / 0x08 / 0x8c):
//!   absent (pointer 0, which every reader takes as empty). Phase 0 maps have no groups, links or level cameras;
//!   the host's occlusion mappings name the host's instances;
//! * grind paths (0x74): the map's rails;
//! * level settings (0x00, in place): the death height (+0x28) and no loader ship (+0x2c x = 0).

use crate::{err, Result};
use rc_formats::gameplay::{self, MobyInstance, MOBY_INSTANCES_POINTER, MOBY_INSTANCE_SIZE};

pub const CAMERAS_POINTER: usize = 0x08;
pub const MOBY_GROUPS_POINTER: usize = 0x48;
pub const GRIND_PATHS_POINTER: usize = 0x74;
pub const OCCLUSION_MAPPINGS_POINTER: usize = 0x8c;
/// Level settings: the death height and the loader ship's x (rc_formats::gameplay::ship_placement).
const SETTINGS_DEATH_Z: usize = 0x28;
const SETTINGS_SHIP_X: usize = 0x2c;
/// Grind path record size (`{f32 bsphere[4], s32 flag, pad[3]}`, rc_formats::volumes::parse_grind_paths).
const GRIND_PATH_SIZE: usize = 0x20;
/// Extra radius around a rail's points in its bounding sphere (the rail search tests the sphere first).
const RAIL_SPHERE_MARGIN: f32 = 4.0;

fn rd(g: &[u8], o: usize) -> Result<i32> {
    match g.get(o..o + 4) {
        Some(b) => Ok(i32::from_le_bytes(b.try_into().unwrap())),
        None => err(format!("gameplay file: read at {o:#x} past its end ({:#x})", g.len())),
    }
}

fn wr(g: &mut [u8], o: usize, v: i32) { g[o..o + 4].copy_from_slice(&v.to_le_bytes()); }

/// Appends `bytes` 16-byte aligned; returns where they start.
fn append(out: &mut Vec<u8>, bytes: &[u8]) -> usize {
    out.resize(out.len().next_multiple_of(16), 0);
    let at = out.len();
    out.extend_from_slice(bytes);
    at
}

fn words(v: &[i32]) -> Vec<u8> { v.iter().flat_map(|w| w.to_le_bytes()).collect() }

/// An instance record and its pvar block as they leave the source level, ready to be placed.
#[derive(Clone, Debug, PartialEq)]
pub struct TemplateRecord {
    pub instance: MobyInstance,
    /// The raw pvar block (the file's bytes: no fixup applied), None for pvar index −1.
    pub pvar: Option<Vec<u8>>,
    /// Byte offsets in `pvar` that the loader turns into block addresses (pointer fixups, 0x5c): kept.
    pub pointer_fixups: Vec<i32>,
    /// Byte offsets of moby links (0x50) and shared-data words (0x4c) the copy cut (set to −1 / 0): links to the
    /// source level's instances and its shared blob mean nothing on the map.
    pub cut: Vec<i32>,
}

impl TemplateRecord {
    /// Instance `index` of `source` (a decompressed gameplay file), with its pvar block.
    pub fn from_level(source: &[u8], index: usize) -> Result<TemplateRecord> {
        let instances = gameplay::parse_moby_instances(source).map_err(|e| crate::MapError(format!("source instances: {e}")))?;
        let Some(&instance) = instances.get(index) else { return err(format!("source level has no instance {index} ({} instances)", instances.len())) };
        let pi = instance.pvar_index;
        let (table, data) = (rd(source, gameplay::PVAR_TABLE_POINTER)? as usize, rd(source, gameplay::PVAR_DATA_POINTER)? as usize);
        let mut out = TemplateRecord { instance, pvar: None, pointer_fixups: Vec::new(), cut: Vec::new() };
        if pi < 0 || table == 0 || data == 0 { return Ok(out); }
        let (ofs, size) = (rd(source, table + 8 * pi as usize)?, rd(source, table + 8 * pi as usize + 4)?);
        let (Ok(ofs), Ok(size)) = (usize::try_from(ofs), usize::try_from(size)) else { return err(format!("instance {index}: bad pvar entry {ofs}, {size}")) };
        let Some(block) = source.get(data + ofs..data + ofs + size) else { return err(format!("instance {index}: pvar block past the end")) };
        let mut block = block.to_vec();
        let fixups = gameplay::parse_pvar_fixups(source).map_err(|e| crate::MapError(format!("source pvar fixups: {e}")))?;
        let shared = gameplay::parse_pvar_shared_records(source).map_err(|e| crate::MapError(format!("source shared data: {e}")))?;
        let mut cut = |o: i32, v: i32, cuts: &mut Vec<i32>| {
            if let Some(f) = usize::try_from(o).ok().filter(|&o| o + 4 <= block.len()) { wr(&mut block, f, v); cuts.push(o); }
        };
        for &(i, o) in &fixups.moby_links { if i == pi { cut(o, -1, &mut out.cut); } }
        for &(i, o, _) in &shared { if i as i32 == pi { cut(o as i32, 0, &mut out.cut); } }
        out.pointer_fixups = fixups.pointers.iter().filter(|f| f.0 == pi).map(|f| f.1).collect();
        out.pvar = Some(block);
        Ok(out)
    }

    /// The first instance of class `o_class` in `source`.
    pub fn first_of_class(source: &[u8], o_class: i32) -> Result<TemplateRecord> {
        let instances = gameplay::parse_moby_instances(source).map_err(|e| crate::MapError(format!("source instances: {e}")))?;
        match instances.iter().position(|m| m.o_class == o_class) {
            Some(i) => TemplateRecord::from_level(source, i),
            None => err(format!("the source level has no instance of class {o_class} to copy")),
        }
    }

    /// Placed at `position` facing `yaw`: always spawned (no save-bit test), alone (no group), not snapped.
    pub fn placed(mut self, position: [f32; 3], yaw: f32) -> TemplateRecord {
        let m = &mut self.instance;
        m.position = position;
        m.rotation = [0.0, 0.0, yaw];
        m.spawn_flags = 0;
        m.group = -1;
        m.is_rooted = 0;
        // Never hidden by the occlusion grid (the host's grid does not know the map).
        m.occlusion = 0;
        // No talk slot hook (`MobyUnknown74Hook`): the host's talk tables are not the map's.
        m.unknown_74 = -1;
        self
    }
}

/// The map content that goes into the host's gameplay file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameplayPatch {
    /// Ratchet's record first, then the objects, in instance order.
    pub records: Vec<TemplateRecord>,
    /// Grind rails, each an open spline.
    pub rails: Vec<Vec<[f32; 3]>>,
    pub death_z: Option<f32>,
}

/// A rail as the file's spline: `(x, y, z, w)` with w the chord to the next point (the last: back to the first),
/// as on the disc (rc_game::spline module doc), and its bounding sphere.
pub fn rail_spline(points: &[[f32; 3]]) -> (Vec<[f32; 4]>, [f32; 4]) {
    let d = |a: [f32; 3], b: [f32; 3]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
    let n = points.len();
    let spline = (0..n).map(|i| { let p = points[i]; [p[0], p[1], p[2], d(p, points[(i + 1) % n])] }).collect();
    let (lo, hi) = points.iter().fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), p| (std::array::from_fn(|k| lo[k].min(p[k])), std::array::from_fn(|k| hi[k].max(p[k]))));
    let c: [f32; 3] = std::array::from_fn(|k| (lo[k] + hi[k]) / 2.0);
    let r = points.iter().map(|&p| d(p, c)).fold(0.0, f32::max) + RAIL_SPHERE_MARGIN;
    (spline, [c[0], c[1], c[2], r])
}

/// The grind path section of `rails` (rc_formats::volumes::parse_grind_paths reads it back).
fn grind_section(rails: &[Vec<[f32; 3]>]) -> Vec<u8> {
    let n = rails.len();
    let head = (0x10 + n * GRIND_PATH_SIZE + 4 * n).next_multiple_of(16);
    let mut records = Vec::new();
    let mut offsets = Vec::new();
    let mut data = Vec::new();
    for r in rails {
        let (spline, sphere) = rail_spline(r);
        records.extend(sphere.iter().flat_map(|v| v.to_le_bytes()));
        records.extend(words(&[0, 0, 0, 0])); // flag 0: open
        offsets.push(data.len() as i32);
        data.extend(words(&[spline.len() as i32, 0, 0, 0]));
        data.extend(spline.iter().flatten().flat_map(|v| v.to_le_bytes()));
    }
    let mut s = words(&[n as i32, head as i32, data.len() as i32, 0]);
    s.extend(records);
    s.extend(words(&offsets));
    s.resize(head, 0);
    s.extend(data);
    s
}

/// The host's gameplay file `host` with `patch` in it (module doc).
pub fn patch_gameplay(host: &[u8], patch: &GameplayPatch) -> Result<Vec<u8>> {
    let host_instances = rd(host, MOBY_INSTANCES_POINTER)? as usize;
    if host_instances == 0 { return err("the host level has no moby instance section"); }
    let spawnable = rd(host, host_instances + 4)?;
    let mut out = host.to_vec();

    // Pvars: one block per record that has one, renumbered in record order.
    let (mut table, mut data, mut pointers) = (Vec::new(), Vec::new(), Vec::new());
    let mut records = Vec::with_capacity(patch.records.len() * MOBY_INSTANCE_SIZE);
    for r in &patch.records {
        let mut m = r.instance;
        m.size = MOBY_INSTANCE_SIZE as i32;
        m.pvar_index = -1;
        if let Some(b) = &r.pvar {
            let index = (table.len() / 2) as i32;
            data.resize(data.len().next_multiple_of(16), 0);
            table.extend([data.len() as i32, b.len() as i32]);
            data.extend_from_slice(b);
            pointers.extend(r.pointer_fixups.iter().flat_map(|&o| [index, o]));
            m.pvar_index = index;
        }
        records.extend_from_slice(bytemuck::bytes_of(&m));
    }
    let mut section = words(&[patch.records.len() as i32, spawnable, 0, 0]);
    section.extend(records);
    let at = append(&mut out, &section);
    wr(&mut out, MOBY_INSTANCES_POINTER, at as i32);
    let (t, d) = if table.is_empty() { (0, 0) } else { (append(&mut out, &words(&table)), append(&mut out, &data)) };
    wr(&mut out, gameplay::PVAR_TABLE_POINTER, t as i32);
    wr(&mut out, gameplay::PVAR_DATA_POINTER, d as i32);
    let p = if pointers.is_empty() { 0 } else { pointers.extend([-1, 0]); append(&mut out, &words(&pointers)) };
    wr(&mut out, gameplay::PVAR_POINTER_FIXUPS_POINTER, p as i32);

    for ptr in [gameplay::PVAR_MOBY_LINK_FIXUPS_POINTER, gameplay::PVAR_SHARED_DATA_POINTER, MOBY_GROUPS_POINTER, CAMERAS_POINTER, OCCLUSION_MAPPINGS_POINTER] {
        wr(&mut out, ptr, 0);
    }
    let g = if patch.rails.is_empty() { 0 } else { append(&mut out, &grind_section(&patch.rails)) };
    wr(&mut out, GRIND_PATHS_POINTER, g as i32);

    let settings = rd(host, gameplay::LEVEL_SETTINGS_POINTER)? as usize;
    if settings == 0 { return err("the host level has no level settings"); }
    rd(host, settings + SETTINGS_SHIP_X)?;
    if let Some(z) = patch.death_z { wr(&mut out, settings + SETTINGS_DEATH_Z, z.to_bits() as i32); }
    wr(&mut out, settings + SETTINGS_SHIP_X, 0);
    out.resize(out.len().next_multiple_of(16), 0);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal host: the 37-pointer header (0x100 bytes), level settings, two instances (Ratchet; a class-500
    /// crate with a pvar block holding a moby link at +4 and a pointer at +8), a pvar table and the two fixup lists.
    fn host() -> Vec<u8> {
        let mut g = vec![0u8; 0x400];
        wr(&mut g, 0x00, 0x100); // level settings
        wr(&mut g, 0x100 + 0x28, 10.0f32.to_bits() as i32);
        wr(&mut g, 0x100 + 0x2c, 50.0f32.to_bits() as i32);
        wr(&mut g, MOBY_INSTANCES_POINTER, 0x140);
        wr(&mut g, 0x140, 2);
        wr(&mut g, 0x144, 7); // spawnable count
        let mut ratchet = MobyInstance { size: 0x78, pvar_index: -1, ..Default::default() };
        ratchet.position = [1.0, 2.0, 3.0];
        let crate_ = MobyInstance { size: 0x78, o_class: 500, pvar_index: 0, group: 3, spawn_flags: 12, ..Default::default() };
        g[0x150..0x150 + 0x78].copy_from_slice(bytemuck::bytes_of(&ratchet));
        g[0x1c8..0x1c8 + 0x78].copy_from_slice(bytemuck::bytes_of(&crate_));
        wr(&mut g, gameplay::PVAR_TABLE_POINTER, 0x260);
        wr(&mut g, 0x260, 0);
        wr(&mut g, 0x264, 0x10);
        wr(&mut g, gameplay::PVAR_DATA_POINTER, 0x280);
        for k in 0..4 { wr(&mut g, 0x280 + 4 * k, 0x11 * (k as i32 + 1)); }
        wr(&mut g, gameplay::PVAR_MOBY_LINK_FIXUPS_POINTER, 0x2a0);
        g[0x2a0..0x2b0].copy_from_slice(&words(&[0, 4, -1, 0]));
        wr(&mut g, gameplay::PVAR_POINTER_FIXUPS_POINTER, 0x2b0);
        g[0x2b0..0x2c0].copy_from_slice(&words(&[0, 8, -1, 0]));
        wr(&mut g, MOBY_GROUPS_POINTER, 0x300);
        wr(&mut g, CAMERAS_POINTER, 0x310);
        g
    }

    #[test]
    fn template_cuts_links_and_keeps_pointers() {
        let t = TemplateRecord::first_of_class(&host(), 500).unwrap();
        assert_eq!(t.pvar.as_deref().unwrap(), &words(&[0x11, -1, 0x33, 0x44])[..]);
        assert_eq!((t.pointer_fixups.as_slice(), t.cut.as_slice()), (&[8][..], &[4][..]));
        let t = t.placed([5.0, 6.0, 7.0], 1.0);
        assert_eq!((t.instance.group, t.instance.spawn_flags, t.instance.rotation[2]), (-1, 0, 1.0));
        assert!(TemplateRecord::first_of_class(&host(), 193).is_err());
    }

    #[test]
    fn patched_file_reads_back() {
        let h = host();
        let ratchet = TemplateRecord::from_level(&h, 0).unwrap().placed([20.0, 30.0, 40.0], 0.5);
        let a = TemplateRecord::first_of_class(&h, 500).unwrap().placed([21.0, 30.0, 40.0], 0.0);
        let b = TemplateRecord::first_of_class(&h, 500).unwrap().placed([22.0, 30.0, 40.0], 0.0);
        let rail = vec![[10.0, 10.0, 41.0], [13.0, 14.0, 41.0], [13.0, 20.0, 45.0]];
        let p = GameplayPatch { records: vec![ratchet, a, b], rails: vec![rail.clone()], death_z: Some(30.0) };
        let g = patch_gameplay(&h, &p).unwrap();
        // The host's bytes up to its end are kept apart from the moved pointers and the level settings.
        assert_eq!(&g[0x2c0..0x400], &h[0x2c0..0x400]);
        let inst = gameplay::parse_moby_instances(&g).unwrap();
        assert_eq!(inst.iter().map(|m| (m.o_class, m.pvar_index, m.position[0])).collect::<Vec<_>>(), vec![(0, -1, 20.0), (500, 0, 21.0), (500, 1, 22.0)]);
        assert_eq!(rd(&g, rd(&g, MOBY_INSTANCES_POINTER).unwrap() as usize + 4).unwrap(), 7);
        let pv = gameplay::parse_pvars(&g).unwrap();
        assert_eq!(pv.len(), 2);
        assert_eq!(pv[1].as_deref().unwrap(), &words(&[0x11, -1, 0x33, 0x44])[..]);
        assert_eq!(gameplay::parse_pvar_fixups(&g).unwrap().pointers, vec![(0, 8), (1, 8)]);
        assert!(gameplay::parse_pvar_fixups(&g).unwrap().moby_links.is_empty());
        let gp = rc_formats::volumes::parse_grind_paths(&g).unwrap();
        assert_eq!(gp.len(), 1);
        assert_eq!(gp[0].points.iter().map(|p| [p[0], p[1], p[2]]).collect::<Vec<_>>(), rail);
        assert_eq!(gp[0].points[0][3], 5.0);
        assert!(gp[0].points.iter().all(|p| rc_formats_dist(gp[0].bsphere, *p) <= gp[0].bsphere[3]));
        assert!(rc_formats::cameras::parse_cameras(&g).unwrap().is_empty());
        assert_eq!(gameplay::ship_placement(&g).unwrap(), None);
        assert_eq!(f32::from_bits(rd(&g, 0x128).unwrap() as u32), 30.0);
        assert!(gameplay::sections(&g).is_ok());
    }

    fn rc_formats_dist(s: [f32; 4], p: [f32; 4]) -> f32 { ((s[0] - p[0]).powi(2) + (s[1] - p[1]).powi(2) + (s[2] - p[2]).powi(2)).sqrt() }
}
