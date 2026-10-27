//! A map's gameplay file: the host level's decompressed `gameplay_ntsc` with the map's content in place of the
//! level's (docs/formats/wad_layouts_rac1.md §3.5). New sections are **appended** and their header pointers moved to
//! them, so every section the map does not touch (level settings, lights, sounds, volumes, fog zones…) stays the
//! host's byte for byte, and every reader of the file sees one consistent level.
//!
//! What changes:
//! * moby instances (0x44): Ratchet's record (the host's, at the map's start), then the map's objects
//!   ([`TemplateRecord`]: a record and pvar block copied from an instance of the object's source level);
//! * pvar table / data / pointer fixups (0x54 / 0x58 / 0x5c): the placed records' blocks, renumbered;
//! * moby groups (0x48): the map's ([`GameplayPatch::groups`]; each member's record +0x48 is its group): crates of a
//!   group stack on each other and fall when one under them breaks (rc_game `crate_`); none: absent;
//! * moby-link fixups, shared data, cameras, sound instances, occlusion mappings (0x50 / 0x4c / 0x08 / 0x0c / 0x8c):
//!   absent (pointer 0, which every reader takes as empty). Maps have no links, level cameras or ambient sounds yet;
//!   the host's cameras and sound instances index the host's pvar table and its occlusion mappings name the host's
//!   instances;
//! * spawn ids: an object whose template carries death bits (spawn id ≥ 0: its bolts and "broken" memory,
//!   rc_game `set_death_bits`) gets the next free id and spawn flag 8 (created on every load, not again in the
//!   same visit once dead), so templates from different levels never share bits;
//! * grind paths (0x74): the map's rails;
//! * level settings (0x00, in place): the death height (+0x28) and no loader ship (+0x2c x = 0).

use crate::{err, Result};
use rc_formats::gameplay::{self, MobyInstance, MOBY_INSTANCES_POINTER, MOBY_INSTANCE_SIZE};

pub const CAMERAS_POINTER: usize = 0x08;
pub const SOUND_INSTANCES_POINTER: usize = 0x0c;
pub const MOBY_GROUPS_POINTER: usize = 0x48;
pub const GRIND_PATHS_POINTER: usize = 0x74;
pub const OCCLUSION_MAPPINGS_POINTER: usize = 0x8c;
/// Level settings: the death height and the loader ship's x (rc_formats::gameplay::ship_placement).
const SETTINGS_DEATH_Z: usize = 0x28;
const SETTINGS_SHIP_X: usize = 0x2c;
/// Grind path record size (`{f32 bsphere[4], s32 flag, pad[3]}`, rc_formats::volumes::parse_grind_paths).
const GRIND_PATH_SIZE: usize = 0x20;
/// Spawn flag 8 (rc_formats::moby_spawn::spawn_test): created unless this visit's death bit is set; unlike flags 0 it
/// keeps the spawner byte 0xff, so `set_death_bits` runs (the bolt drop).
const SPAWN_ONCE_PER_VISIT: i32 = 8;
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

    /// Placed at `position` facing `yaw`: always spawned (no save-bit test; [`patch_gameplay`] gives objects with
    /// death bits their own), alone (no group), not snapped, in no mission of the source level.
    pub fn placed(mut self, position: [f32; 3], yaw: f32) -> TemplateRecord {
        let m = &mut self.instance;
        m.position = position;
        m.rotation = [0.0, 0.0, yaw];
        m.spawn_flags = 0;
        // +0x04: the mission byte (moby+0xb0): the source level's missions mean nothing here (0xff = none).
        m.unknown_4 = -1;
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
    /// Moby groups: per group, its members' record indices (each record's +0x48 is set to its group).
    pub groups: Vec<Vec<u16>>,
    /// The map's paths (each a spline's points; w = 0), appended after the host's splines: path `k` is spline
    /// `host count + k` ([`patch_gameplay`] returns the first index), named by the records' pvars.
    pub splines: Vec<Vec<[f32; 4]>>,
    /// Cuboids appended after the host's (the objects' own trigger and post boxes from their home level, moved with
    /// them): cuboid `k` is `host count + k`.
    pub cuboids: Vec<rc_formats::volumes::Shape>,
    pub death_z: Option<f32>,
    /// Level settings words written over the host's (the sky level's background colour and fog, the map's fog).
    pub settings: Vec<(usize, u32)>,
    /// Where the loader makes Ratchet's ship (position, yaw); None: no ship.
    pub ship: Option<([f32; 3], f32)>,
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

/// The paths section (0x70, `rc_formats::gameplay::parse_splines`): `{count, data offset, data size, pad}`, the
/// splines' offsets in the data, then each spline as `s32 count, pad[3]` and its points.
fn paths_section(splines: &[Vec<[f32; 4]>]) -> Vec<u8> {
    let mut data = Vec::new();
    let mut offsets = Vec::new();
    for s in splines {
        offsets.push(data.len() as i32);
        data.extend(words(&[s.len() as i32, 0, 0, 0]));
        for p in s { for v in p { data.extend(v.to_le_bytes()); } }
    }
    let data_ofs = (0x10 + 4 * splines.len()).next_multiple_of(16);
    let mut out = words(&[splines.len() as i32, data_ofs as i32, data.len() as i32, 0]);
    out.extend(words(&offsets));
    out.resize(data_ofs, 0);
    out.extend(data);
    out
}

/// The moby groups section (`InitLevelRenderGlobals` 0x255958 reads it): `s32 count, s32 data size, pad[2],
/// s32 offset[count]` (−1: an empty group), then each group's u16 instance indices, the last with bit 15.
fn groups_section(groups: &[Vec<u16>]) -> Vec<u8> {
    let (mut offsets, mut data) = (Vec::new(), Vec::new());
    for g in groups {
        if g.is_empty() {
            offsets.push(-1);
            continue;
        }
        offsets.push(data.len() as i32);
        for (k, &i) in g.iter().enumerate() {
            let e = if k + 1 == g.len() { i | 0x8000 } else { i & 0x7fff };
            data.extend(e.to_le_bytes());
        }
    }
    data.resize(data.len().next_multiple_of(4), 0);
    let mut s = words(&[groups.len() as i32, data.len() as i32, 0, 0]);
    s.extend(words(&offsets));
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
    let mut next_id = 0;
    for (k, r) in patch.records.iter().enumerate() {
        let mut m = r.instance;
        if let Some(g) = patch.groups.iter().position(|g| g.contains(&(k as u16))) { m.group = g as i32; }
        m.size = MOBY_INSTANCE_SIZE as i32;
        m.pvar_index = -1;
        if m.o_class != 0 && m.spawn_id >= 0 {
            (m.spawn_id, m.spawn_flags) = (next_id, SPAWN_ONCE_PER_VISIT);
            next_id += 1;
        }
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

    for ptr in [gameplay::PVAR_MOBY_LINK_FIXUPS_POINTER, gameplay::PVAR_SHARED_DATA_POINTER, CAMERAS_POINTER, SOUND_INSTANCES_POINTER, OCCLUSION_MAPPINGS_POINTER] {
        wr(&mut out, ptr, 0);
    }
    let g = if patch.groups.iter().all(|g| g.is_empty()) { 0 } else { append(&mut out, &groups_section(&patch.groups)) };
    wr(&mut out, MOBY_GROUPS_POINTER, g as i32);
    let g = if patch.rails.is_empty() { 0 } else { append(&mut out, &grind_section(&patch.rails)) };
    wr(&mut out, GRIND_PATHS_POINTER, g as i32);
    if !patch.splines.is_empty() {
        let mut all = gameplay::parse_splines(host).map_err(|e| crate::MapError(format!("host paths: {e}")))?;
        all.extend(patch.splines.iter().cloned());
        let at = append(&mut out, &paths_section(&all));
        wr(&mut out, gameplay::PATHS_POINTER, at as i32);
    }
    if !patch.cuboids.is_empty() {
        // The host's cuboids then the map's, as a new section (host indices keep their meaning).
        let mut all = rc_formats::volumes::parse_volumes(host).map_err(|e| crate::MapError(format!("host volumes: {e}")))?.cuboids;
        all.extend(patch.cuboids.iter().copied());
        let mut s = words(&[all.len() as i32, 0, 0, 0]);
        for c in &all { s.extend_from_slice(bytemuck::bytes_of(c)); }
        let at = append(&mut out, &s);
        wr(&mut out, rc_formats::volumes::CUBOIDS_POINTER, at as i32);
    }

    let settings = rd(host, gameplay::LEVEL_SETTINGS_POINTER)? as usize;
    if settings == 0 { return err("the host level has no level settings"); }
    rd(host, settings + SETTINGS_SHIP_X)?;
    if let Some(z) = patch.death_z { wr(&mut out, settings + SETTINGS_DEATH_Z, z.to_bits() as i32); }
    for &(o, v) in &patch.settings { if o < SETTINGS_DEATH_Z { wr(&mut out, settings + o, v as i32); } }
    // The host's fog zones sit where its own level is: none on a map.
    wr(&mut out, gameplay::FOG_ZONES_POINTER, 0);
    match patch.ship {
        Some((p, yaw)) => {
            for (k, v) in [p[0], p[1], p[2], yaw].into_iter().enumerate() { wr(&mut out, settings + SETTINGS_SHIP_X + 4 * k, v.to_bits() as i32); }
        }
        None => wr(&mut out, settings + SETTINGS_SHIP_X, 0),
    }
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
        let p = GameplayPatch { records: vec![ratchet, a, b], rails: vec![rail.clone()], groups: vec![vec![], vec![1, 2]], splines: vec![vec![[1.0, 2.0, 3.0, 0.0], [4.0, 5.0, 6.0, -1.0]]], cuboids: Vec::new(), settings: Vec::new(), ship: None, death_z: Some(30.0) };
        let g = patch_gameplay(&h, &p).unwrap();
        // The host's bytes up to its end are kept apart from the moved pointers and the level settings.
        assert_eq!(&g[0x2c0..0x400], &h[0x2c0..0x400]);
        let inst = gameplay::parse_moby_instances(&g).unwrap();
        assert_eq!(inst.iter().map(|m| (m.o_class, m.pvar_index, m.position[0])).collect::<Vec<_>>(), vec![(0, -1, 20.0), (500, 0, 21.0), (500, 1, 22.0)]);
        // Ratchet keeps his record's spawn fields; the two crates get their own death bits.
        assert_eq!(inst.iter().map(|m| (m.spawn_id, m.spawn_flags)).collect::<Vec<_>>(), vec![(0, 0), (0, 8), (1, 8)]);
        assert_eq!(rd(&g, SOUND_INSTANCES_POINTER).unwrap(), 0);
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
        // The map's path after the host's (the test host has none).
        assert_eq!(gameplay::parse_splines(&g).unwrap(), vec![vec![[1.0, 2.0, 3.0, 0.0], [4.0, 5.0, 6.0, -1.0]]]);
        // The crates are group 1 (group 0 is empty): their records say so and the section lists them.
        assert_eq!(inst.iter().map(|m| m.group).collect::<Vec<_>>(), vec![-1, 1, 1]);
        let s = rd(&g, MOBY_GROUPS_POINTER).unwrap() as usize;
        assert_eq!((rd(&g, s).unwrap(), rd(&g, s + 0x10).unwrap(), rd(&g, s + 0x14).unwrap()), (2, -1, 0));
        let d = s + 0x18;
        assert_eq!((u16::from_le_bytes([g[d], g[d + 1]]), u16::from_le_bytes([g[d + 2], g[d + 3]])), (1, 0x8002));
    }

    fn rc_formats_dist(s: [f32; 4], p: [f32; 4]) -> f32 { ((s[0] - p[0]).powi(2) + (s[1] - p[1]).powi(2) + (s[2] - p[2]).powi(2)).sqrt() }
}
