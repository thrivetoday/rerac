//! RAC Maker maps in the engine: `rerac --map <file>` (crate::disc_source::map_file; the format is `rc_map`,
//! docs/plan/rac_maker.md).
//!
//! A map plays on its **host level**: the boot loads the host (crate::level_load) and, for that level only, puts the
//! map in place of the level's world before anything reads it:
//! * the gameplay file is the map's (`rc_map::patch`: Ratchet at the map's start, the map's objects, its rails; no
//!   level cameras, groups or loader ship), so the moby loader, the spawn pass, the trigger volumes and the hero's
//!   rails all read the map;
//! * the collision is the map's floors (`rc_map::floor`), and the host's terrain, ties and shrubs are not drawn
//!   (the map's floors and rails are drawn by [`MapRenderPlugin`]);
//! * **imports**, the objects of other levels, bring their moby class (model, animation, textures: [`import_classes`])
//!   and run the update their own level's class table names (crate::gameplay::level_ports, [`import_overlays`]).
//!
//! Map play never writes the player's memory card: without `RC_SAVE_DIR` the card is off ([`init`]).

use anyhow::{anyhow, bail, Context, Result};
use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use rc_formats::moby::LevelMobyClass;
use rc_formats::moby_anim::MobyAnimClass;
use rc_formats::texture::{LevelTexture, TextureTable};
use rc_map::patch::{GameplayPatch, TemplateRecord};
use rc_map::MapDef;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

/// The map this run plays: the file's, then the editor's edits (crate::editor) — a level reload loads the current one.
static ACTIVE: RwLock<Option<Arc<MapDef>>> = RwLock::new(None);
/// The map file (`--map` or `--edit`).
static PATH: OnceLock<PathBuf> = OnceLock::new();

/// Reads the `--map` / `--edit` file once, before the boot's level load: the boot level becomes the map's host, and
/// the memory card is off unless `RC_SAVE_DIR` names one. A map that does not read exits with one `error:` line;
/// `--edit` of a file that does not exist starts a new map (`MapDef::new_default`) saved there.
pub fn init() -> Option<Arc<MapDef>> {
    let path = crate::disc_source::map_file()?.to_path_buf();
    let def = if crate::disc_source::edit_mode() && !path.exists() {
        let name = path.file_stem().map_or("New map".to_string(), |s| s.to_string_lossy().into_owned());
        MapDef::new_default(&name, 1)
    } else {
        std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| MapDef::parse(&t).map_err(|e| e.to_string())).unwrap_or_else(|e| {
            eprintln!("error: map {}: {e}", path.display());
            std::process::exit(crate::disc_source::EXIT_USAGE)
        })
    };
    crate::level_load::set_level_index(def.host_level);
    if std::env::var_os("RC_SAVE_DIR").is_none() {
        // Before any thread starts (main's first lines). The map must not write the game's saves.
        std::env::set_var("RC_SAVE_DIR", "0");
    }
    println!(
        "map: {:?} ({}) on host level {:02}: {} floors, {} rails, {} objects, imports {:?}{}",
        def.name, path.display(), def.host_level, def.floors.len(), def.rails.len(), def.objects.len(), def.imports(),
        if crate::disc_source::edit_mode() { " (editing)" } else { "" }
    );
    let _ = PATH.set(path);
    let _ = ORIGINAL.set(def.clone());
    set(def);
    active()
}

/// The map as its file read at the start (the editor's working copy starts from it: the loaded copy has the edges
/// fitted and opened where islands meet, [`set`]).
static ORIGINAL: OnceLock<MapDef> = OnceLock::new();
pub fn original() -> Option<MapDef> { ORIGINAL.get().cloned() }

/// The map of this run (None: the game's own levels).
pub fn active() -> Option<Arc<MapDef>> { ACTIVE.read().unwrap_or_else(|e| e.into_inner()).clone() }

/// The editor's change: the next load of the host level loads this map.
pub fn set(mut def: MapDef) {
    fit_edges(&mut def);
    open_where_islands_meet(&mut def);
    *ACTIVE.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(def));
}

/// Whether `asset` is an enemy or creature by the catalog (`<data>/catalog/catalog.json`, read once): the enemies a
/// zone's Cleared rule waits for (crate::map_logic).
pub fn is_enemy(asset: rc_map::AssetId) -> bool {
    static KINDS: OnceLock<std::collections::HashMap<rc_map::AssetId, bool>> = OnceLock::new();
    KINDS
        .get_or_init(|| {
            let root = crate::level_load::extracted_root();
            let cat = std::fs::read_to_string(root.join("catalog/catalog.json")).ok().and_then(|t| rc_map::catalog::Catalog::parse(&t).ok());
            cat.map_or_else(Default::default, |c| c.entries.iter().flat_map(|e| e.ids().map(move |i| (*i, e.kind == rc_map::catalog::KIND_ENEMY))).collect())
        })
        .get(&asset)
        .copied()
        .unwrap_or(false)
}

/// The map file of this run.
pub fn path() -> Option<&'static Path> { PATH.get().map(PathBuf::as_path) }

/// The map, while level `index` (its host) is the one loaded or being loaded.
pub fn on(index: u32) -> Option<Arc<MapDef>> { active().filter(|d| d.host_level == index) }

/// The level whose data holds class `o_class` while level `index` is loaded: an import's own level, else `index`.
pub fn class_level(index: u32, o_class: i32) -> u32 {
    on(index).and_then(|d| all_imports(&d).into_iter().find(|&(_, c)| c == o_class)).map_or(index, |(l, _)| l)
}

/// One level's core: the parsed index and the decompressed data (through the Tier 1 cache).
pub fn level_core(root: &Path, index: u32) -> Result<(rc_formats::level::LevelCore, std::sync::Arc<[u8]>)> {
    let data = rc_data::level_core_data(root, index).with_context(|| format!("level {index:02}: decompressing core_data"))?;
    let core = rc_formats::level::parse_level_core(&crate::disc_source::level_file(root, index, "core_index.bin")?, data.len())
        .with_context(|| format!("level {index:02}: parsing the core index"))?;
    Ok((core, data))
}

/// The host's gameplay file with the map in it (`rc_map::patch`).
pub fn patch_gameplay(root: &Path, def: &MapDef, host: &[u8]) -> Result<Vec<u8>> {
    let mut sources: std::collections::HashMap<u32, Vec<u8>> = std::collections::HashMap::new();
    let ratchet = TemplateRecord::first_of_class(host, 0).map_err(|e| anyhow!("host level: Ratchet's record: {e}"))?;
    let (start, start_yaw) = ship_start(def, host).unwrap_or((def.start.position, def.start.yaw));
    let mut records = vec![ratchet.placed(start, start_yaw)];
    // The objects' own (home level) splines and cuboids, appended after the host's ([`away`]).
    let first_spline = rc_formats::gameplay::parse_splines(host).map_or(0, |s| s.len());
    let first_cuboid = rc_formats::volumes::parse_volumes(host).map_or(0, |v| v.cuboids.len());
    let (mut splines, mut cuboids) = (Vec::new(), Vec::new());
    let mut homes: std::collections::HashMap<u32, (Vec<Vec<[f32; 4]>>, Vec<rc_formats::volumes::Shape>)> = std::collections::HashMap::new();
    for (i, o) in def.objects.iter().enumerate() {
        let level = o.asset.level;
        if level != def.host_level && !sources.contains_key(&level) {
            sources.insert(level, rc_data::level_gameplay(root, level).with_context(|| format!("level {level:02}: decompressing gameplay_ntsc"))?.to_vec());
        }
        let source = if level == def.host_level { host } else { &sources[&level] };
        let t = match o.template.or_else(|| default_template(o.asset.number)) {
            Some(k) => TemplateRecord::from_level(source, k),
            None => TemplateRecord::first_of_class(source, o.asset.number),
        }
        .map_err(|e| anyhow!("objects[{i}] {}: {e}", o.asset))?;
        if t.instance.o_class != o.asset.number {
            bail!("objects[{i}] {}: template instance {:?} is class {}", o.asset, o.template, t.instance.o_class);
        }
        if !t.cut.is_empty() { println!("map: objects[{i}] {}: the template's links / shared words at {:?} cut", o.asset, t.cut); }
        let (from, from_yaw) = (t.instance.position, t.instance.rotation[2]);
        let mut t = t.placed(o.position, o.yaw);
        // A tint: the record's own light colour (its ambient, 128 = 1.0) scaled by it, brightened so it shows.
        if let Some(c) = o.tint {
            for k in 0..3 { t.instance.color[k] = ((t.instance.color[k].max(40) as f32) * c[k] as f32 / 255.0 * 1.6).round().clamp(0.0, 255.0) as i32; }
        }
        if AWAY.iter().any(|a| a.0 == o.asset.number) {
            let (hs, hc) = homes.entry(level).or_insert_with(|| {
                (rc_formats::gameplay::parse_splines(source).unwrap_or_default(), rc_formats::volumes::parse_volumes(source).map(|v| v.cuboids).unwrap_or_default())
            });
            away(&mut t, (from, from_yaw), (o.position, o.yaw), hs, hc, &mut splines, first_spline, &mut cuboids, first_cuboid);
        }
        records.push(t);
    }
    // The objects' map paths, named in the movers' pvars ([`path_pvars`]).
    for (i, o) in def.objects.iter().enumerate() {
        // A robot trooper without a path guards where it stands (its template's patrol is a host path elsewhere).
        let guard = (o.asset.number == 459 && o.path.len() < 2).then(|| vec![o.position, [o.position[0] + o.yaw.cos(), o.position[1] + o.yaw.sin(), o.position[2]]]);
        let Some(path) = guard.or_else(|| (o.path.len() >= 2).then(|| o.path.clone())) else { continue };
        let index = (first_spline + splines.len()) as i32;
        splines.push(path.iter().map(|q| [q[0], q[1], q[2], 0.0]).collect());
        path_pvars(&mut records[i + 1], index, o.time);
    }
    // The objects' groups: object i is record i + 1 (Ratchet is record 0).
    let mut groups: Vec<Vec<u16>> = Vec::new();
    for (i, o) in def.objects.iter().enumerate() {
        let Some(g) = o.group.map(usize::from) else { continue };
        if groups.len() <= g { groups.resize(g + 1, Vec::new()); }
        groups[g].push(i as u16 + 1);
    }
    // The world: another planet's background colour and fog (its level settings words 0x00..0x24), the map's fog.
    let sky_settings = def.world.sky.filter(|&l| l != def.host_level).and_then(|l| {
        let g = rc_data::level_gameplay(root, l).ok()?;
        let base = u32::from_le_bytes(g.get(0..4)?.try_into().ok()?) as usize;
        let mut w = [0u32; 10];
        for (k, x) in w.iter_mut().enumerate() { *x = u32::from_le_bytes(g.get(base + 4 * k..base + 4 * k + 4)?.try_into().ok()?); }
        Some(w)
    });
    let settings = def.settings_words(sky_settings);
    let ship = def.ship.as_ref().map(|s| (s.position, s.yaw));
    // Two-way rails: each also reversed (the game rides a rail from its first point to its last).
    let rails = def.rails.iter().flat_map(|r| { let l = r.line(); let back = (!r.one_way).then(|| l.iter().rev().copied().collect::<Vec<_>>()); std::iter::once(l).chain(back) }).collect();
    let patch = GameplayPatch { records, rails, groups, splines, cuboids, death_z: def.death_height, settings, ship };
    Ok(rc_map::patch::patch_gameplay(host, &patch)?)
}

/// With a ship, where Ratchet starts: by his ship's hatch as on the host (the host's start turned and moved from the
/// host's ship to the map's), facing as there. The landing scene's end walks him back from there.
pub fn ship_start(def: &MapDef, host: &[u8]) -> Option<([f32; 3], f32)> {
    let ship = def.ship.as_ref()?;
    let (hp, hyaw) = rc_formats::gameplay::ship_placement(host).ok()??;
    let r = TemplateRecord::first_of_class(host, 0).ok()?;
    let (sp, syaw) = (r.instance.position, r.instance.rotation[2]);
    let d = ship.yaw - hyaw;
    let (s, c) = d.sin_cos();
    let (dx, dy) = (sp[0] - hp[0], sp[1] - hp[1]);
    Some(([ship.position[0] + dx * c - dy * s, ship.position[1] + dx * s + dy * c, ship.position[2] + sp[2] - hp[2]], syaw + d))
}

/// The world triangles of the map's solid objects (`Object::solid`): each one's model (its low LOD when it has one:
/// fewer faces; else its high LOD) at its placement (the record's scale, its facing, its position). Each triangle is
/// wound as the collision faces are (the cross product of its edges against the face's outward normal, judged from the
/// model's middle); a face whose side cannot be told (a flat, wall-like model) goes in both ways.
pub fn solid_triangles(def: &MapDef, classes: &[LevelMobyClass], gameplay: &[u8]) -> Vec<[[f32; 3]; 3]> {
    let instances = rc_formats::gameplay::parse_moby_instances(gameplay).unwrap_or_default();
    let mut out = Vec::new();
    for (i, o) in def.objects.iter().enumerate() {
        if o.solid != Some(true) { continue; }
        let Some(class) = classes.iter().find(|c| c.o_class == o.asset.number) else { continue };
        let scale = instances.get(i + 1).map_or(1.0, |m| m.scale);
        let c = &class.class;
        let lists: Vec<&rc_formats::moby::MobySubmesh> = if c.low_lod.iter().any(|sm| !sm.triangles.is_empty()) { c.low_lod.iter().collect() } else { c.high_lod.iter().collect() };
        let (s, co) = o.yaw.sin_cos();
        let place = |p: [f32; 3]| {
            let (x, y, z) = (p[0] * scale, p[1] * scale, p[2] * scale);
            [o.position[0] + x * co - y * s, o.position[1] + x * s + y * co, o.position[2] + z]
        };
        // Simplified for the collision cells (at most 255 corners each): the corners snapped to a quarter unit, the
        // triangles that collapse dropped.
        let snap = |p: [f32; 3]| p.map(|v| (v * 4.0).round() / 4.0);
        let tris: Vec<[[f32; 3]; 3]> = lists
            .iter()
            .flat_map(|sm| sm.triangles.iter().map(move |t| [t.a, t.b, t.c].map(|k| snap(place(c.position(&sm.vertices[k as usize]))))))
            .filter(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2])
            .collect();
        if tris.is_empty() { continue; }
        let n = (tris.len() * 3) as f32;
        let mid: [f32; 3] = std::array::from_fn(|k| tris.iter().flat_map(|t| t.iter()).map(|p| p[k]).sum::<f32>() / n);
        for t in tris {
            let e1 = [t[1][0] - t[0][0], t[1][1] - t[0][1], t[1][2] - t[0][2]];
            let e2 = [t[2][0] - t[0][0], t[2][1] - t[0][1], t[2][2] - t[0][2]];
            let cr = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
            let len = (cr[0] * cr[0] + cr[1] * cr[1] + cr[2] * cr[2]).sqrt();
            if len < 1e-6 { continue; }
            let c3: [f32; 3] = std::array::from_fn(|k| (t[0][k] + t[1][k] + t[2][k]) / 3.0 - mid[k]);
            let cl = (c3[0] * c3[0] + c3[1] * c3[1] + c3[2] * c3[2]).sqrt().max(1e-6);
            let side = (cr[0] * c3[0] + cr[1] * c3[1] + cr[2] * c3[2]) / (len * cl);
            // The collision faces' cross product points against their outward side.
            let flipped = [t[0], t[2], t[1]];
            if side > 0.2 { out.push(flipped); } else if side < -0.2 { out.push(t); } else { out.extend([t, flipped]); }
        }
    }
    out
}

/// The classes that move along a spline the map can give them (`Object::path`; the editor offers a path for these).
pub const PATH_CLASSES: [i32; 3] = [726, 459, 1454];

/// Points an object's pvars at map spline `index` (`time`: a mover's travel time): the path platform 726 (rc_game
/// `path_platform`: spline +0xb4, starts at the path's first point +0xa0 = 1, travel time +0xb0) and the robot
/// trooper 459 (`path_enemy`: its init reads the patrol spline +0x164 and the arrival spline +0x134 (-1: none) and
/// writes their "+ 1" pointers +0x160 / +0x130 itself; +0x1b4 set: it stands on its path instead of waiting 20 units up
/// to jet down when Ratchet comes near) and the Veldin tank 1454
/// (`veldin_tank`: drives to a random next point of its path, +0x100).
fn path_pvars(t: &mut TemplateRecord, index: i32, time: Option<f32>) {
    let set: Vec<(usize, [u8; 4])> = match t.instance.o_class {
        726 => {
            let mut v = vec![(0xb4, index.to_le_bytes()), (0xa0, [1, 0, 0, 0])];
            if let Some(s) = time { v.push((0xb0, s.to_le_bytes())); }
            v
        }
        459 => vec![(0x164, index.to_le_bytes()), (0x134, (-1i32).to_le_bytes()), (0x160, (index + 1).to_le_bytes()), (0x130, 0i32.to_le_bytes()), (0x1b4, 1i32.to_le_bytes())],
        1454 => vec![(0x100, index.to_le_bytes())],
        _ => return,
    };
    let Some(p) = t.pvar.as_mut() else { return };
    for (o, b) in set {
        if o == 0xa0 {
            if let Some(x) = p.get_mut(o) { *x = b[0]; }
        } else if let Some(w) = p.get_mut(o..o + 4) {
            w.copy_from_slice(&b);
        }
    }
}

/// A template pvar word that names (or holds) something of the template's home level, and what a map does with it
/// ([`AWAY`]).
#[derive(Clone, Copy, Debug)]
enum LevelRef {
    /// A spline index (a path, an area, a waypoint graph): the home spline (its w words kept: area walls, graph
    /// masks, node marks) copied, moved and turned with the object, the word pointed at the copy.
    Spline(usize),
    /// A cuboid index (a trigger or wake box, a post): the home cuboid copied, moved and turned with the object.
    Cuboid(usize),
    /// A word forced (−1: none; dormant flags; start awake).
    Word(usize, i32),
    /// An absolute yaw (f32, radians) set to the object's facing.
    SetYaw(usize),
    /// A yaw in world space turned by as much as the object was (the Veldin tank's turret).
    Yaw(usize),
    /// The spline whose first (or, `true`, last) point the class's code puts it on: all its copies are shifted so that
    /// point is where the object was put (else the game moved it from there to the route's start, its made pieces
    /// staying behind). Listed before the splines it shifts.
    Anchor(usize, bool),
}

/// Per enemy class away from home: its level words ([`LevelRef`]) and the classes its code makes (guns, shots, treads,
/// riders first, then the death pieces) that are imported with it. From a read of every ported class
/// (docs/plan/rac_maker.md, "Enemies away from home"; 2026-10-25).
const AWAY: &[(i32, &[LevelRef], &[i32])] = {
    use LevelRef::{Anchor as A, Cuboid as C, SetYaw as SY, Spline as S, Word as W, Yaw as Y};
    &[
        (459, &[W(0x1e4, -1)], &[]),                                             // robot trooper: no drop box
        (749, &[S(0x1e0), S(0x1e4), S(0x1e8)], &[]),                              // horny toad: areas
        (572, &[S(0x240), S(0x244), S(0x24c)], &[]),                              // amoeboid
        (865, &[S(0x240), S(0x244), S(0x24c), W(0x230, 1)], &[]),                 // amoeboid (split): up and ready
        (866, &[S(0x240), S(0x244), S(0x24c), W(0x230, 1)], &[]),
        (688, &[S(0x74), W(0x188, -1), W(0x18c, -1), W(0x190, -1), W(0x1a0, -1), W(0x1b0, -1), W(0x1c0, -1)], &[]), // gunship
        (666, &[S(0x74)], &[]),                                                   // dropship: its route
        (580, &[S(0x290)], &[]),                                                  // sandshark
        (668, &[], &[1764, 1765]),                                                // sandshark nest
        (612, &[C(0x188)], &[1751, 1752, 1753, 1769, 1922]),                      // aridia flamer: its post
        (733, &[], &[1208]),                                                      // aridia cannon: shell
        (573, &[W(0x270, -1), W(0x27c, -1)], &[1742, 1743, 1917]),                // kerwan hound
        (574, &[A(0x134, false), S(0x134), S(0x290), W(0x29c, -1)], &[833, 1744, 1745, 1746]),     // kerwan trooper: rocket
        (217, &[W(0x430, -1)], &[570, 1944, 1945, 1946]),                         // eudora logger: prop
        (427, &[S(0xe0), S(0xe4)], &[499, 184]),                                  // eudora gunner: gun, shot
        (563, &[S(0x3dc)], &[1516, 1739, 1740, 1741]),                            // eudora walker
        (623, &[S(0x2c0), S(0x2c4), S(0x2cc)], &[1754, 1755, 1756]),              // rilgar biter
        (625, &[A(0xd4, false), S(0xd4), S(0x190)], &[872, 873, 1757, 1759, 1760, 1790, 1791]),   // rilgar flame tank: treads
        (827, &[A(0x1f8, true), S(0x1ec), S(0x1f4), S(0x1f8)], &[]),                              // blarg crawler
        (1048, &[S(0x1b0)], &[1695, 1696]),                                       // blarg trooper: its arena
        (1068, &[S(0x180), S(0x1a4), W(0x1a8, -1), W(0x1ac, -1)], &[1702, 1703, 1704]), // blarg wave bot
        (1041, &[], &[882, 1569, 1570]),                                          // umbris lobber: shot
        (1126, &[SY(0x80)], &[880]),                                              // umbris turret: shot
        (252, &[S(0x21c)], &[]),                                                  // hover zapper
        (253, &[A(0x150, false), S(0x150), S(0x154)], &[331, 425, 1728, 1729, 1730, 1731, 1732, 1790, 1791]), // batalia tank: treads, shell
        (333, &[], &[248]),                                                       // batalia grenadier: grenade
        (452, &[A(0xe4, false), S(0xe4), S(0x220)], &[]),                                         // batalia runner
        (1023, &[A(0x1d0, false), S(0x1d0), S(0x1d4), S(0x1fc)], &[1025, 1292, 1692, 1693, 1694]), // hop gunner: gun, shot
        (1201, &[], &[324, 1258]),                                                // gaspar cannon: base, shell
        (1885, &[W(0x84, -1), W(0xa0, -1), W(0xa4, -1), W(0xa8, -1), W(0xac, -1)], &[1886, 1733, 1734, 1735]), // pod launcher
        (1202, &[], &[1909, 1914, 1915, 1916, 1571, 1572]),                       // orxon brawler
        (1229, &[A(0x140, false), S(0x140), S(0x144), S(0x190), C(0x148), W(0x1b0, -1)], &[819, 1533, 1534]), // orxon drone: wake box
        (1231, &[S(0x13c)], &[1297]),                                             // pokitaru thrower: ball
        (1246, &[S(0x230), S(0x234)], &[]),                                       // pokitaru biter
        (238, &[S(0x2d0)], &[]),                                                  // hoven burrower: area
        (294, &[W(0x268, 0), W(0x274, 0), W(0x290, -1), S(0x26c), S(0x254)], &[499, 184]), // hoven gunner: gun, shot
        (326, &[S(0xa0), S(0xa4), C(0x80)], &[1371, 409]),                        // hoven drone: rider, shot
        (29, &[W(0x50, -1)], &[36, 1238]),                                        // gemlik turret: rider, shot
        (101, &[S(0xd0)], &[1233]),                                               // gemlik launcher: missiles
        (1261, &[], &[1634]),                                                     // explosive tank
        (1262, &[S(0x180), S(0x198), W(0x1a4, -1)], &[]),                         // gemlik robot
        (8, &[S(0x74), S(0x78), S(0x204)], &[]),                                  // oltanis sentry
        (28, &[S(0x240), S(0x244), W(0x248, 0)], &[325, 403]),                    // oltanis zapper
        (30, &[C(0x80), C(0x90), W(0x88, 1)], &[681]),                            // popup turret: wake and aim boxes
        (31, &[S(0xd0), S(0xd4), S(0xd8), S(0xdc), S(0x150)], &[1193, 81]),       // oltanis drone: grenade
        (44, &[S(0x120), S(0x124), W(0x2d0, -1), W(0x2d4, -1), W(0x2d8, -1), W(0x2dc, -1)], &[934]), // quartu guard
        (221, &[], &[1867, 1868, 1869]),                                          // quartu critter
        (541, &[S(0xc4), S(0xd8), C(0xd0), C(0xd4)], &[876, 863, 884, 281, 1621, 1622, 1623]), // kalebo trooper
        (1445, &[S(0x1f0)], &[]),                                                 // area stalker
        (1382, &[S(0x1d0), W(0x2b0, -1), W(0x2b4, -1), W(0x2b8, -1), W(0x2bc, -1)], &[1742, 1743, 1917]), // fleet crew: cull boxes
        (1454, &[S(0x100), Y(0x110)], &[331, 41, 1790, 1791, 1792, 1793, 1794, 1796]), // veldin tank: treads, shells
        // Not enemies: the gold bolt's two pickup-camera cuboids (the camera flew into the sky): the orbit about Ratchet.
        (1134, &[W(0x04, -1), W(0x08, -1)], &[]),
    ]
};

/// The songs a map can play, per level: (level, start track, name). Each song is a start track and the loop after it
/// (`levels/NN/music/`); only Novalis's tracks carry names, the others are numbered.
pub fn songs(root: &Path) -> &'static [(u32, u32, String)] {
    static SONGS: OnceLock<Vec<(u32, u32, String)>> = OnceLock::new();
    SONGS.get_or_init(|| {
        let mut out = Vec::new();
        for l in 0..19u32 {
            let size = |k: u32| std::fs::metadata(root.join(format!("levels/{l:02}/music/{k:03}.bin"))).map(|m| m.len()).ok();
            let mut n = 0;
            let mut k = 0;
            while size(k).is_some() {
                // A song: a start, then a long loop (the short ones after them are the transitions).
                if size(k + 1).is_some_and(|s| s > 1_000_000) {
                    n += 1;
                    let name = match (l, k) {
                        (1, 0) => "Novalis: enemy theme".to_string(),
                        (1, 2) => "Novalis: cave".to_string(),
                        (1, 4) => "Novalis: waterworks".to_string(),
                        _ => format!("{}: theme {n}", rc_map::LEVEL_NAMES[l as usize]),
                    };
                    out.push((l, k, name));
                    k += 2;
                } else {
                    k += 1;
                }
            }
        }
        out
    })
}

/// The classes a class's code makes that a map imports with it ([`AWAY`]).
pub fn children(o_class: i32) -> &'static [i32] { AWAY.iter().find(|a| a.0 == o_class).map_or(&[], |a| a.2) }

/// The home-level instance a class is copied from when the map does not say (`Object::template`): its first one
/// starts in a state a map cannot wake (the Hoven burrower #140 is buried until Ratchet is within 2 units).
fn default_template(o_class: i32) -> Option<usize> {
    match o_class {
        238 => Some(164),
        _ => None,
    }
}

/// The classes a map cannot place yet: they belong to their level's set pieces (the Oltanis rail bot needs a grind
/// rail, marks and a cuboid; the racers, the boss, the dropships and gunships fly their level's routes).
pub const NOT_PLACEABLE: &[i32] = &[211, 717, 1422, 638, 1356, 444, 462, 463, 440, 111, 491, 556, 568, 1906, 1267, 114, 1106, 1319, 1843, 35, 857, 347, 331];

/// The enemies seen to vanish, hide or wander off on a map that are not fixed yet (the asset browser warns): their
/// templates need more of their home level than [`AWAY`] gives them (companions, level-wide routes, a level's script).
pub const MAY_VANISH: &[i32] = &[28, 666, 688, 1051, 294, 541, 438, 336, 8, 1246, 77, 333, 1885];

/// The template's level words ([`AWAY`]) for an object placed at `to` facing `yaw`, copied from the home instance at
/// `from` facing `from_yaw`: the words forced, the yaws turned, and the home splines and cuboids it names (from
/// `home_splines` / `home_cuboids`) moved and turned with it into `splines` / `cuboids` (their indices counted from
/// `first_spline` / `first_cuboid`), the words pointed at the copies. A spline or cuboid named twice is copied once.
#[allow(clippy::too_many_arguments)]
fn away(
    t: &mut TemplateRecord, (from, from_yaw): ([f32; 3], f32), (to, yaw): ([f32; 3], f32),
    home_splines: &[Vec<[f32; 4]>], home_cuboids: &[rc_formats::volumes::Shape],
    splines: &mut Vec<Vec<[f32; 4]>>, first_spline: usize, cuboids: &mut Vec<rc_formats::volumes::Shape>, first_cuboid: usize,
) {
    let class = t.instance.o_class;
    let Some(&(_, refs, _)) = AWAY.iter().find(|a| a.0 == class) else { return };
    let Some(p) = t.pvar.as_mut() else { return };
    let rd = |p: &[u8], o: usize| p.get(o..o + 4).map(|b| i32::from_le_bytes(b.try_into().unwrap()));
    let d = yaw - from_yaw;
    let (s, c) = d.sin_cos();
    let turn = |v: [f32; 4]| [v[0] * c - v[1] * s, v[0] * s + v[1] * c, v[2], v[3]];
    let turned = |q: [f32; 4]| { let (dx, dy) = (q[0] - from[0], q[1] - from[1]); [to[0] + dx * c - dy * s, to[1] + dx * s + dy * c, to[2] + q[2] - from[2], q[3]] };
    // An anchor: the shift that puts its point where the object stands.
    let shift = refs.iter().find_map(|r| match *r {
        LevelRef::Anchor(o, last) => {
            let k = usize::try_from(rd(p, o)?).ok()?;
            let sp = home_splines.get(k)?;
            let q = turned(*if last { sp.last()? } else { sp.first()? });
            Some([to[0] - q[0], to[1] - q[1], to[2] - q[2]])
        }
        _ => None,
    }).unwrap_or([0.0; 3]);
    let place = |q: [f32; 4]| { let t = turned(q); [t[0] + shift[0], t[1] + shift[1], t[2] + shift[2], t[3]] };
    let (mut copied_s, mut copied_c): (Vec<(i32, i32)>, Vec<(i32, i32)>) = (Vec::new(), Vec::new());
    for r in refs {
        match *r {
            LevelRef::Anchor(..) => {}
            LevelRef::Word(o, v) => { if let Some(w) = p.get_mut(o..o + 4) { w.copy_from_slice(&v.to_le_bytes()); } }
            LevelRef::SetYaw(o) => { if let Some(w) = p.get_mut(o..o + 4) { w.copy_from_slice(&yaw.to_le_bytes()); } }
            LevelRef::Yaw(o) => {
                if let Some(v) = rd(p, o).map(|b| f32::from_bits(b as u32)).filter(|v| v.is_finite()) { p[o..o + 4].copy_from_slice(&(v + d).to_le_bytes()); }
            }
            LevelRef::Spline(o) => {
                let Some(k) = rd(p, o) else { continue };
                let new = match copied_s.iter().find(|x| x.0 == k) {
                    Some(&(_, n)) => n,
                    None => {
                        let Some(sp) = usize::try_from(k).ok().and_then(|k| home_splines.get(k)) else { continue };
                        let n = (first_spline + splines.len()) as i32;
                        splines.push(sp.iter().map(|&q| place(q)).collect());
                        copied_s.push((k, n));
                        n
                    }
                };
                p[o..o + 4].copy_from_slice(&new.to_le_bytes());
            }
            LevelRef::Cuboid(o) => {
                let Some(k) = rd(p, o) else { continue };
                let new = match copied_c.iter().find(|x| x.0 == k) {
                    Some(&(_, n)) => n,
                    None => {
                        let Some(&home) = usize::try_from(k).ok().and_then(|k| home_cuboids.get(k)) else { continue };
                        let mut b = home;
                        for r in 0..3 { b.matrix[r] = turn(home.matrix[r]); }
                        let cen = place(home.matrix[3]);
                        b.matrix[3] = [cen[0], cen[1], cen[2], home.matrix[3][3]];
                        // world → local: the rows applied to the turned offset; inverse' = inverse · Rz(−d) on the left.
                        for r in 0..3 {
                            let (x, y) = (home.inverse[0][r], home.inverse[1][r]);
                            b.inverse[0][r] = x * c - y * s;
                            b.inverse[1][r] = x * s + y * c;
                        }
                        b.euler[2] += d;
                        let n = (first_cuboid + cuboids.len()) as i32;
                        cuboids.push(b);
                        copied_c.push((k, n));
                        n
                    }
                };
                p[o..o + 4].copy_from_slice(&new.to_le_bytes());
            }
        }
    }
}

/// The classes a level's core lists (each level's read once).
pub fn level_classes(root: &Path, level: u32) -> &'static [i32] {
    static CLASSES: crate::level_load::PerLevel<Vec<i32>> = [const { std::sync::OnceLock::new() }; 20];
    CLASSES[(level as usize).min(19)].get_or_init(|| level_core(root, level).map(|(c, _)| c.moby_classes.iter().filter(|e| e.offset_in_asset_wad != 0).map(|e| e.o_class).collect()).unwrap_or_default())
}

/// Every class a map brings from other levels: its objects' classes ([`MapDef::imports`]) and the classes their code
/// makes ([`children`]) that the host lacks and the object's level has.
pub fn all_imports(def: &MapDef) -> Vec<(u32, i32)> {
    let root = crate::level_load::extracted_root();
    let mut out = def.imports();
    let placed = out.clone();
    let host = level_classes(&root, def.host_level);
    for o in &def.objects {
        for &c in children(o.asset.number) {
            let l = o.asset.level;
            if host.contains(&c) || out.iter().any(|x| x.1 == c) || !level_classes(&root, l).contains(&c) { continue; }
            out.push((l, c));
        }
    }
    // The objects' classes first (they must fit), then the children.
    out.sort_by_key(|x| (!placed.contains(x), x.0, x.1));
    out
}

/// The map's imported moby classes for its host's moby loader: each class's model and animation from its own level,
/// its textures appended to the host's moby texture table (`textures`) under new indices and its texture slots moved
/// to them (a texture two classes of one level share is added once). A class number the host already uses is refused
/// for an object (the class tables are keyed by number); the classes the objects' code makes ([`all_imports`]) are
/// skipped when they cannot come (no model on their level, the texture table full): their mobys are then not made.
pub fn import_classes(root: &Path, def: &MapDef, host_classes: &[i32], textures: &mut Vec<LevelTexture>) -> Result<Vec<(LevelMobyClass, MobyAnimClass)>> {
    let mut out = Vec::new();
    let mut next = textures.iter().filter(|t| t.table == TextureTable::Moby).map(|t| t.index + 1).max().unwrap_or(0);
    let placed = def.imports();
    // Per source level: its textures moved so far (old index → new index).
    let mut moved_by_level: std::collections::HashMap<u32, Vec<(u8, u8)>> = std::collections::HashMap::new();
    for (level, o_class) in all_imports(def) {
        let object = placed.contains(&(level, o_class));
        if host_classes.contains(&o_class) {
            if object { bail!("import {o_class} of level {level:02}: the host level {:02} has its own class {o_class}", def.host_level); }
            continue;
        }
        let (core, data) = level_core(root, level)?;
        let gs_ram = crate::disc_source::level_file(root, level, "gs_ram.bin")?;
        let classes = rc_formats::moby::parse_level_mobys(&core, &data).with_context(|| format!("level {level:02}: parsing moby classes"))?;
        let Some(mut class) = classes.into_iter().find(|c| c.o_class == o_class) else {
            if object { bail!("level {level:02} has no moby class {o_class} with a model"); }
            println!("map: class {o_class} (made by an object's code) has no model on level {level:02}: not imported");
            continue;
        };
        let Some(anim) = crate::moby_anim::load_anim_classes(&core, &data, std::slice::from_ref(&class)).pop() else {
            if object { bail!("level {level:02} class {o_class}: no animation class"); }
            continue;
        };
        let source = rc_formats::texture::parse_textures(&core, &data, &gs_ram).with_context(|| format!("level {level:02}: decoding textures"))?;
        let moved = moved_by_level.entry(level).or_default();
        let need: Vec<u8> = class.entry.textures.iter().copied().filter(|&s| s != 0xff && !moved.iter().any(|m| m.0 == s)).collect::<std::collections::BTreeSet<u8>>().into_iter().collect();
        if next + need.len() > 0xff {
            if object { bail!("the host's moby texture table is full: class {o_class} of level {level:02} needs {} more", need.len()); }
            println!("map: class {o_class} (made by an object's code) left out: the host's moby texture table is full");
            continue;
        }
        let mut added = Vec::new();
        for slot in class.entry.textures.iter_mut().filter(|s| **s != 0xff) {
            let old = *slot;
            if let Some(&(_, new)) = moved.iter().find(|m| m.0 == old) { *slot = new; continue; }
            let t = source.iter().find(|t| t.table == TextureTable::Moby && t.index == old as usize).ok_or_else(|| anyhow!("level {level:02}: moby texture {old} missing"))?;
            let new = next as u8;
            textures.push(LevelTexture { index: new as usize, ..t.clone() });
            moved.push((old, new));
            added.push(new);
            *slot = new;
            next += 1;
        }
        println!("map: imported class {o_class} from level {level:02}{}: {} new textures {added:?}", if object { "" } else { " (made by an object)" }, added.len());
        out.push((class, anim));
    }
    Ok(out)
}

/// The host's Ratchet animations completed from the other levels. A level carries only the sequences of the moves it
/// needs (Novalis has no rails, so none of the grind sequences 47..55: Ratchet grinds in the bind pose), and a map
/// may use every move. The skeleton is the same on every level, so another level's sequence plays as it is. Veldin
/// (00, the most sequences) first, then the others; each donor's core is dropped from the cache after use.
pub fn complete_ratchet_sequences(root: &Path, host: u32, mobys: &mut crate::moby_render::LevelMobys) -> Result<()> {
    let Some(ci) = mobys.classes.iter().position(|c| c.o_class == 0) else { return Ok(()) };
    let joints = mobys.classes[ci].class.header.joint_count as usize;
    let keep: Vec<u32> = on(host).map(|d| all_imports(&d).into_iter().map(|(l, _)| l).collect()).unwrap_or_default();
    let seqs = &mut mobys.anim[ci].sequences;
    let before = seqs.iter().filter(|s| s.is_some()).count();
    // Once per host and session: the editor reloads the map after every change.
    type Seqs = Vec<Option<rc_formats::moby_anim::MobySequence>>;
    static DONE: std::sync::Mutex<Option<std::collections::HashMap<u32, Seqs>>> = std::sync::Mutex::new(None);
    if let Some(done) = DONE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).get(&host) {
        if seqs.len() < done.len() { seqs.resize(done.len(), None); }
        for (s, d) in seqs.iter_mut().zip(done) { if s.is_none() { s.clone_from(d); } }
        return Ok(());
    }
    for level in (0..19u32).filter(|&l| l != host) {
        let (core, data) = level_core(root, level)?;
        let block = |name: &str| core.blocks.iter().find(|b| b.name == name).and_then(|b| data.get(b.offset..b.offset + b.size));
        let donor = crate::moby_anim::ratchet_sequences(&core, &block, joints);
        if seqs.len() < donor.len() { seqs.resize(donor.len(), None); }
        for (s, d) in seqs.iter_mut().zip(donor) { if s.is_none() { *s = d; } }
        if !keep.contains(&level) { rc_data::global().forget_level(root, level); }
    }
    println!("map: Ratchet's animations: {before} from the host, {} with the other levels'", seqs.iter().filter(|s| s.is_some()).count());
    DONE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(host, seqs.clone());
    Ok(())
}

/// The overlays the imports' updates come from (crate::gameplay::level_ports): (source level, its classes).
pub fn import_overlays(index: u32) -> Vec<(u32, Vec<i16>)> {
    let Some(def) = on(index) else { return Vec::new() };
    let mut out: Vec<(u32, Vec<i16>)> = Vec::new();
    for (l, c) in all_imports(&def) {
        match out.iter_mut().find(|e| e.0 == l) {
            Some(e) => e.1.push(c as i16),
            None => out.push((l, vec![c as i16])),
        }
    }
    out
}

// ------------------------------------------------------------------------------------------------
// Drawing the map's own geometry: the floors (a textured quad grid) and the rails (a beam along each spline).

/// Draws the map's floors and rails (only when a map plays).
pub struct MapRenderPlugin;

impl Plugin for MapRenderPlugin {
    fn build(&self, app: &mut App) {
        if active().is_none() { return; }
        app.init_resource::<Drawn>()
            .add_systems(crate::level_switch::LevelStartup, spawn_geometry)
            .add_systems(Update, (redraw_changed.run_if(resource_exists::<crate::Level>), map_fog));
    }
}

/// The game's fog (the level settings the map wrote: the sky planet's, or its own) over the map's own geometry: the
/// game's renderers fog the objects and the sky with it; the map's terrain, paths, pieces and rails are drawn by Bevy's
/// standard material, which takes it as a distance fog on the camera (`RC_FOG=0`: none, as the game's renderers).
fn map_fog(mut commands: Commands, fog: Option<Res<crate::game_camera::GameFog>>, cams: Query<(Entity, Option<&DistanceFog>), With<crate::fly_cam::FlyCam>>) {
    let own = active().and_then(|d| d.world.fog);
    let want = fog.filter(|_| !std::env::var("RC_FOG").is_ok_and(|v| v.trim() == "0")).map(|f| {
        let mut f = f.fog;
        // The map's own fog as the editor has it now (live; the game's renderers take it at the next load).
        if let Some(o) = own {
            (f.color, f.near_dist, f.far_dist) = (o.color, o.near * 1024.0, o.far.max(o.near + 1.0) * 1024.0);
        }
        let amount = |i: f32| (1.0 - i / 255.0).clamp(0.0, 1.0);
        let (near, far) = (f.near_dist / 1024.0, (f.far_dist / 1024.0).max(f.near_dist / 1024.0 + 1.0));
        DistanceFog {
            color: Color::srgba_u8(f.color[0], f.color[1], f.color[2], (amount(f.far_intensity).max(amount(f.near_intensity)) * 255.0) as u8),
            falloff: FogFalloff::Linear { start: near, end: far },
            ..default()
        }
    });
    for (e, now) in &cams {
        match (&want, now) {
            (Some(w), Some(n)) if n.color == w.color && matches!((&n.falloff, &w.falloff), (FogFalloff::Linear { start: a, end: b }, FogFalloff::Linear { start: c, end: d }) if a == c && b == d) => {}
            (Some(w), _) => { commands.entity(e).insert(w.clone()); }
            (None, Some(_)) => { commands.entity(e).remove::<DistanceFog>(); }
            (None, None) => {}
        }
    }
}

/// What the map's geometry was drawn from, and the textures decoded for it (kept across redraws and level loads).
#[derive(Resource, Default)]
struct Drawn {
    def: Option<Arc<MapDef>>,
    textures: std::collections::HashMap<rc_map::AssetId, Option<Handle<Image>>>,
    /// Per edge piece: its parts' materials.
    pieces: std::collections::HashMap<rc_map::AssetId, Vec<Handle<StandardMaterial>>>,
}

/// The geometry a map draws: what a redraw compares (its light and palette too: they are baked into it).
#[allow(clippy::type_complexity)]
fn geometry_of(d: &MapDef) -> (&[rc_map::Floor], &[rc_map::terrain::Patch], &[rc_map::walkway::Walkway], &[rc_map::Rail], Option<rc_map::Light>, Option<rc_map::Palette>, &[rc_map::PointLight], &[(rc_map::AssetId, rc_map::Palette)]) {
    (&d.floors, &d.patches, &d.walkways, &d.rails, d.world.light, d.world.palette, &d.world.lights, &d.world.recolour)
}

/// The palette the map's terrain and pieces are drawn with now (set by [`draw`]).
static PALETTE: RwLock<Option<rc_map::Palette>> = RwLock::new(None);
/// The terrain textures recoloured on their own (material groups, `World::recolour`; set by [`draw`]).
static RECOLOUR: RwLock<Vec<(rc_map::AssetId, rc_map::Palette)>> = RwLock::new(Vec::new());

/// The floors' and rails' entities (despawned with the level).
#[derive(Component)]
struct MapGeometry;

/// Game units → Bevy (crate::tfrag_render::game_to_bevy) for one point.
fn to_bevy(p: [f32; 3]) -> Vec3 { crate::tfrag_render::game_to_bevy(p) }

fn texture_image(level: &crate::level_load::LoadedLevel, id: rc_map::AssetId) -> Option<Image> {
    // The host's own tfrag textures are loaded; another level's are decoded once and kept (a recolour redraws often:
    // the colour sliders live).
    static DECODED: OnceLock<std::sync::Mutex<std::collections::HashMap<rc_map::AssetId, Option<Arc<rc_formats::texture::Texture>>>>> = OnceLock::new();
    let from_host = id.level == level.particles.level;
    let t: Option<Arc<rc_formats::texture::Texture>> = if from_host {
        level.textures.iter().find(|t| t.table == TextureTable::Tfrag && t.index == id.number as usize).map(|t| Arc::new(t.texture.clone()))
    } else {
        let cache = DECODED.get_or_init(Default::default);
        let known = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&id).cloned();
        match known {
            Some(t) => t,
            None => {
                let root = crate::level_load::extracted_root();
                let all = level_core(&root, id.level).ok().and_then(|(core, data)| {
                    let gs = crate::disc_source::level_file(&root, id.level, "gs_ram.bin").ok()?;
                    rc_formats::texture::parse_textures(&core, &data, &gs).ok()
                }).unwrap_or_default();
                let mut c = cache.lock().unwrap_or_else(|e| e.into_inner());
                // Every terrain texture of that level at once (the next one picked is there too).
                for t in all.iter().filter(|t| t.table == TextureTable::Tfrag) {
                    let k = rc_map::AssetId { number: t.index as i32, ..id };
                    c.entry(k).or_insert_with(|| Some(Arc::new(t.texture.clone())));
                }
                c.entry(id).or_insert(None).clone()
            }
        }
    };
    let Some(t) = t.as_deref() else {
        eprintln!("map: terrain texture {id} not found: drawn plain");
        return None;
    };
    // PS2 alpha is 0..0x80: the floor is opaque. The map's palette over it.
    let pal = *PALETTE.read().unwrap_or_else(|e| e.into_inner());
    let own = RECOLOUR.read().unwrap_or_else(|e| e.into_inner()).iter().find(|r| r.0 == id).map(|r| r.1);
    let rgba: Vec<u8> = t.rgba.chunks(4).flat_map(|p| {
        let c = pal.map_or([p[0], p[1], p[2]], |q| q.apply([p[0], p[1], p[2]]));
        let c = own.map_or(c, |q| q.apply(c));
        [c[0], c[1], c[2], 255]
    }).collect();
    let mut image = Image::new(
        Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        ..ImageSamplerDescriptor::linear()
    });
    Some(image)
}

/// A floor as two triangles, its UVs repeating every `tile` units.
fn floor_mesh(f: &rc_map::Floor) -> Mesh {
    let [x0, y0] = f.min;
    let [x1, y1] = f.max;
    let z = f.height;
    let corners = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
    let positions: Vec<[f32; 3]> = corners.iter().map(|c| to_bevy([c[0], c[1], z]).to_array()).collect();
    let uvs: Vec<[f32; 2]> = corners.iter().map(|c| [(c[0] - x0) / f.tile, (c[1] - y0) / f.tile]).collect();
    let up = to_bevy([0.0, 0.0, 1.0]).to_array();
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![up; 4])
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        // Both windings: the floor shows from above and below.
        .with_inserted_indices(Indices::U32(vec![0, 1, 2, 0, 2, 3, 0, 2, 1, 0, 3, 2]))
}

/// A rail as a square beam (0.12 units across) along its points.
fn rail_mesh(points: &[[f32; 3]]) -> Mesh {
    const R: f32 = 0.06;
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    for w in points.windows(2) {
        let (a, b) = (to_bevy(w[0]), to_bevy(w[1]));
        let d = (b - a).normalize_or_zero();
        let side = d.cross(Vec3::Y).normalize_or(Vec3::X) * R;
        let up = side.cross(d).normalize_or(Vec3::Y) * R;
        let base = positions.len() as u32;
        for p in [a, b] {
            for o in [side + up, -side + up, -side - up, side - up] { positions.push((p + o).to_array()); }
        }
        // Four sides, both windings.
        for k in 0..4u32 {
            let (i0, i1) = (base + k, base + (k + 1) % 4);
            let (j0, j1) = (i0 + 4, i1 + 4);
            indices.extend([i0, j0, j1, i0, j1, i1, i0, j1, j0, i0, i1, j1]);
        }
    }
    let n = positions.len();
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; n])
        .with_inserted_indices(Indices::U32(indices))
}

/// The light the map's terrain is baked with now (`World::light`, set by [`draw`] before it builds the meshes): the
/// sun's direction (game axes, towards the sun), its colour and the ambient's (1 = full), the hollows' shading.
#[derive(Clone, Copy)]
struct Rig {
    sun: [f32; 3],
    sun_colour: [f32; 3],
    ambient: [f32; 3],
    shade: f32,
}

static RIG: RwLock<Rig> = RwLock::new(Rig { sun: [-0.42, -0.55, 0.72], sun_colour: [0.55; 3], ambient: [0.5; 3], shade: 0.0 });

/// The map's point lights the meshes are baked with now (set by [`draw`]).
static POINTS: RwLock<Vec<rc_map::PointLight>> = RwLock::new(Vec::new());

/// [`lit`] at the point `p`: plus the map's point lights reaching it (falling off evenly to nothing at their radius,
/// stronger on the side of the face that looks at them).
fn lit_at(n: [f32; 3], p: [f32; 3]) -> [f32; 4] {
    let mut c = lit(n);
    for l in POINTS.read().unwrap_or_else(|e| e.into_inner()).iter() {
        let d = [l.position[0] - p[0], l.position[1] - p[1], l.position[2] - p[2]];
        let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if dist >= l.radius { continue; }
        let facing = if dist < 1e-3 { 1.0 } else { ((n[0] * d[0] + n[1] * d[1] + n[2] * d[2]) / dist).max(0.0) * 0.5 + 0.5 };
        let k = (1.0 - dist / l.radius) * l.intensity * facing;
        for i in 0..3 { c[i] += l.colour[i] as f32 / 255.0 * k; }
    }
    c
}

/// The rig of a map's light (the default: the light the terrain always had).
fn rig_of(l: Option<&rc_map::Light>) -> Rig {
    let Some(l) = l else { return Rig { sun: [-0.42, -0.55, 0.72], sun_colour: [0.55; 3], ambient: [0.5; 3], shade: 0.0 } };
    let (az, el) = (l.sun_azimuth.to_radians(), l.sun_elevation.to_radians());
    // The colours as the default's scale: 140 → 0.55 of the sun, 128 → 0.5 of the ambient.
    Rig {
        sun: [az.cos() * el.cos(), az.sin() * el.cos(), el.sin()],
        sun_colour: l.sun.map(|c| c as f32 / 255.0),
        ambient: l.ambient.map(|c| c as f32 / 255.0),
        shade: l.shade,
    }
}

/// The height grid's surface at `(x, y)` (bilinear, the land mask aside) and its normal there: the light and the
/// normals of a patch's top.
fn surface(p: &rc_map::terrain::Patch, x: f32, y: f32) -> (f32, [f32; 3]) {
    let (nx, ny) = p.points();
    let h = |x: f32, y: f32| {
        let fx = ((x - p.origin[0]) / p.spacing).clamp(0.0, (nx - 1) as f32);
        let fy = ((y - p.origin[1]) / p.spacing).clamp(0.0, (ny - 1) as f32);
        let (i, j) = ((fx.floor() as usize).min(nx - 2), (fy.floor() as usize).min(ny - 2));
        let (u, v) = (fx - i as f32, fy - j as f32);
        let g = |a: usize, b: usize| p.heights[p.at(a, b)];
        g(i, j) * (1.0 - u) * (1.0 - v) + g(i + 1, j) * u * (1.0 - v) + g(i, j + 1) * (1.0 - u) * v + g(i + 1, j + 1) * u * v
    };
    let e = p.spacing;
    let (dx, dy) = ((h(x + e, y) - h(x - e, y)) / (2.0 * e), (h(x, y + e) - h(x, y - e)) / (2.0 * e));
    let l = (dx * dx + dy * dy + 1.0).sqrt();
    (h(x, y), [-dx / l, -dy / l, 1.0 / l])
}

/// The baked light of a face with normal `n` (game axes): the rig's ambient plus its sun on it, as a colour (alpha 1).
fn lit(n: [f32; 3]) -> [f32; 4] {
    let r = *RIG.read().unwrap_or_else(|e| e.into_inner());
    let len = (r.sun[0] * r.sun[0] + r.sun[1] * r.sun[1] + r.sun[2] * r.sun[2]).sqrt().max(1e-6);
    let d = ((n[0] * r.sun[0] + n[1] * r.sun[1] + n[2] * r.sun[2]) / len).max(0.0);
    [r.ambient[0] + r.sun_colour[0] * d, r.ambient[1] + r.sun_colour[1] * d, r.ambient[2] + r.sun_colour[2] * d, 1.0]
}

/// A light colour darkened by `k` (the cliffs, the sides).
fn dim(c: [f32; 4], k: f32) -> [f32; 4] { [c[0] * k, c[1] * k, c[2] * k, c[3]] }

/// How much light reaches the land at `(x, y)` (1 = all): hollows and the feet of slopes, lower than the land around
/// them, darken by the rig's shade (ambient occlusion from the heights around, 1.5 and 4 units away).
fn open_sky(p: &rc_map::terrain::Patch, x: f32, y: f32) -> f32 {
    let shade = RIG.read().unwrap_or_else(|e| e.into_inner()).shade;
    if shade <= 0.0 { return 1.0; }
    let h = surface(p, x, y).0;
    let mut above = 0.0;
    for (r, w) in [(1.5f32, 0.6f32), (4.0, 0.4)] {
        for k in 0..8 {
            let a = k as f32 * std::f32::consts::FRAC_PI_4;
            let q = surface(p, x + a.cos() * r, y + a.sin() * r).0;
            above += ((q - h) / r).clamp(0.0, 1.0) * w / 8.0;
        }
    }
    1.0 - shade * 0.7 * above.clamp(0.0, 1.0)
}

/// Triangles' colours multiplied by a tint (an island's or a path's own colour).
#[allow(clippy::type_complexity)]
fn tinted(mut tris: Vec<([[f32; 3]; 3], [[f32; 2]; 3], [[f32; 3]; 3], [[f32; 4]; 3])>, tint: Option<[u8; 3]>) -> Vec<([[f32; 3]; 3], [[f32; 2]; 3], [[f32; 3]; 3], [[f32; 4]; 3])> {
    let Some(t) = tint else { return tris };
    let k = t.map(|c| c as f32 / 255.0);
    for tri in &mut tris { for c in &mut tri.3 { for i in 0..3 { c[i] *= k[i]; } } }
    tris
}

/// A mesh from triangles given with their game-space corners (wound clockwise seen from their front, as the collision
/// faces), UVs, normals and colours per corner: wound for Bevy (counter-clockwise front).
fn soup_mesh(tris: &[([[f32; 3]; 3], [[f32; 2]; 3], [[f32; 3]; 3], [[f32; 4]; 3])]) -> Mesh {
    let mut positions = Vec::with_capacity(tris.len() * 3);
    let mut uvs = Vec::with_capacity(tris.len() * 3);
    let mut normals = Vec::with_capacity(tris.len() * 3);
    let mut colors = Vec::with_capacity(tris.len() * 3);
    for (p, uv, n, c) in tris {
        for k in [0usize, 2, 1] {
            positions.push(to_bevy(p[k]).to_array());
            uvs.push(uv[k]);
            normals.push(to_bevy(n[k]).to_array());
            colors.push(c[k]);
        }
    }
    let count = positions.len() as u32;
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
        .with_inserted_indices(Indices::U32((0..count).collect()))
}

/// A patch's mesh: its top (the land's triangles: smooth normals, UVs repeating every `tile` units), the cliff walls
/// along its outline and the cap under them, with the PS2 look's light baked into the vertex colours (ambient plus
/// the sun on the normal).
fn patch_mesh(p: &rc_map::terrain::Patch) -> Mesh {
    let uv = |q: [f32; 3]| [(q[0] - p.origin[0]) / p.tile, (q[1] - p.origin[1]) / p.tile];
    let mut tris = Vec::new();
    for t in p.top() {
        let n = t.map(|q| surface(p, q[0], q[1]).1);
        let c = [0, 1, 2].map(|i| { let l = dim(lit_at(n[i], t[i]), open_sky(p, t[i][0], t[i][1])); let s = p.shade_xy(t[i][0], t[i][1]); [l[0] * s[0], l[1] * s[1], l[2] * s[2], 1.0] });
        tris.push((t, t.map(uv), n, c));
    }
    if p.depth > 0.0 {
        let bottom = p.bottom();
        let mut along = 0.0;
        for (a, b) in p.outline() {
            let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
            if len < 1e-5 { continue; }
            // Out of the land: the land is on the segment's left.
            let out = [(b[1] - a[1]) / len, -(b[0] - a[0]) / len, 0.0];
            let (a0, b0, a1, b1) = (a, b, [a[0], a[1], bottom], [b[0], b[1], bottom]);
            let uv = |u: f32, top: f32, z: f32| [u / p.tile, (top - z) / p.tile];
            let c = dim(lit(out), 0.85);
            // Seen from outside: a lower, a upper, b upper (clockwise), and a lower, b upper, b lower.
            tris.push(([a1, a0, b0], [uv(along, a[2], bottom), uv(along, a[2], a[2]), uv(along + len, b[2], b[2])], [out; 3], [c; 3]));
            tris.push(([a1, b0, b1], [uv(along, a[2], bottom), uv(along + len, b[2], b[2]), uv(along + len, b[2], bottom)], [out; 3], [c; 3]));
            along += len;
        }
        // The cap: the land's shape at the bottom, facing down.
        for poly in p.land_polygons() {
            let flat: Vec<[f32; 3]> = poly.iter().map(|q| [q[0], q[1], bottom]).collect();
            for k in 1..flat.len().saturating_sub(1) {
                // Counter-clockwise from above is clockwise from below: the down side's front.
                let t = [flat[0], flat[k], flat[k + 1]];
                tris.push((t, t.map(uv), [[0.0, 0.0, -1.0]; 3], [[0.35, 0.35, 0.35, 1.0]; 3]));
            }
        }
    }
    let tris = tinted(tris, p.tint);
    soup_mesh(&tris)
}

/// A paint layer of a patch (`k`): the top's triangles under any of its weight, lifted a hair above the top (and
/// above the layers before it), its texture blended in by the weights (vertex alpha) over the baked light.
fn layer_mesh(p: &rc_map::terrain::Patch, k: usize) -> Option<Mesh> {
    let layer = p.layers.get(k)?;
    let (nx, ny) = p.points();
    let lift = 0.012 * (k + 1) as f32;
    // The weight between points (bilinear).
    let weight = |x: f32, y: f32| {
        let fx = ((x - p.origin[0]) / p.spacing).clamp(0.0, (nx - 1) as f32);
        let fy = ((y - p.origin[1]) / p.spacing).clamp(0.0, (ny - 1) as f32);
        let (i, j) = ((fx.floor() as usize).min(nx - 2), (fy.floor() as usize).min(ny - 2));
        let (u, v) = (fx - i as f32, fy - j as f32);
        let w = |a: usize, b: usize| layer.weights[p.at(a, b)] as f32 / 255.0;
        w(i, j) * (1.0 - u) * (1.0 - v) + w(i + 1, j) * u * (1.0 - v) + w(i, j + 1) * (1.0 - u) * v + w(i + 1, j + 1) * u * v
    };
    let tris: Vec<_> = p
        .top()
        .into_iter()
        .filter_map(|t| {
            let w = t.map(|q| weight(q[0], q[1]));
            if w.iter().all(|&w| w <= 0.0) { return None; }
            let n = t.map(|q| surface(p, q[0], q[1]).1);
            let c = [0, 1, 2].map(|i| { let l = dim(lit_at(n[i], t[i]), open_sky(p, t[i][0], t[i][1])); let s = p.shade_xy(t[i][0], t[i][1]); [l[0] * s[0], l[1] * s[1], l[2] * s[2], w[i]] });
            let lifted = t.map(|q| [q[0], q[1], q[2] + lift]);
            Some((lifted, t.map(|q| [(q[0] - p.origin[0]) / layer.tile, (q[1] - p.origin[1]) / layer.tile]), n, c))
        })
        .collect();
    let tris = tinted(tris, p.tint);
    (!tris.is_empty()).then(|| soup_mesh(&tris))
}

/// An edge piece's model (`terrain::Edge::piece`: a railing or wall tie or moby of any level): its triangles by
/// texture (class space, game axes) and its box.
pub struct PieceModel {
    pub parts: Vec<(Option<rc_formats::texture::Texture>, Vec<rc_extract::thumbs::Tri>)>,
    pub lo: [f32; 3],
    pub hi: [f32; 3],
}

impl PieceModel {
    /// The horizontal axis it runs along (0: x, 1: y): its longer side.
    pub fn long_axis(&self) -> usize { if self.hi[0] - self.lo[0] >= self.hi[1] - self.lo[1] { 0 } else { 1 } }
    /// Its length along that axis.
    pub fn length(&self) -> f32 { let k = self.long_axis(); (self.hi[k] - self.lo[k]).max(0.25) }
    /// Its height.
    pub fn height(&self) -> f32 { self.hi[2] - self.lo[2] }
}

static PIECES: OnceLock<std::sync::Mutex<std::collections::HashMap<rc_map::AssetId, Option<Arc<PieceModel>>>>> = OnceLock::new();

/// The grind rail looks (`Rail::look`): (key, name, level, grind path, piece length). On the disc the rails are the
/// level's own geometry (tfrags), not repeated models: a piece is cut from the straightest stretch of that grind
/// path ([`rail_look`]) and repeated along a map's rail.
///
/// Each: (key, name, level, grind path, its point to cut at (None: the straightest stretch), piece length, how far
/// across the line its geometry reaches, how far below and above it).
pub type RailLook = (&'static str, &'static str, u32, usize, Option<usize>, f32, f32, f32, f32);
pub const RAIL_LOOKS: [RailLook; 5] = [
    ("kerwan", "Kerwan cable", 3, 1, Some(0), 3.3, 0.3, -0.35, 0.3),
    ("batalia", "Batalia rail", 8, 2, Some(466), 4.0, 0.45, -1.25, 0.05),
    ("oltanis", "Oltanis rail", 14, 2, Some(91), 2.2, 0.6, -1.25, 0.05),
    ("kalebo", "Kalebo rail", 16, 3, Some(207), 3.7, 0.45, -1.4, 0.05),
    ("veldin", "Veldin rail", 18, 0, None, 2.0, 0.4, -0.8, 0.05),
];

/// A rail look's piece (read once; None when its level, path or geometry is not there): the level's tfrag
/// triangles (LOD 0) along the straightest `length` of the grind path, in the rail's frame (x along it, y across,
/// z up from the grind line).
pub fn rail_look(key: &str) -> Option<Arc<PieceModel>> {
    static LOOKS: OnceLock<std::sync::Mutex<std::collections::HashMap<String, Option<Arc<PieceModel>>>>> = OnceLock::new();
    let cache = LOOKS.get_or_init(Default::default);
    if let Some(m) = cache.lock().unwrap().get(key) { return m.clone(); }
    let m = RAIL_LOOKS.iter().find(|l| l.0 == key).and_then(|l| {
        let m = load_rail_cut(l).map(Arc::new);
        let name = l.1;
        if m.is_none() { eprintln!("map: rail look {name}: no geometry found (a plain rail)"); }
        m
    });
    cache.lock().unwrap().insert(key.to_string(), m.clone());
    m
}

fn load_rail_cut(&(_, _, level, path, start, length, across, below, above): &RailLook) -> Option<PieceModel> {
    let root = crate::level_load::extracted_root();
    let gameplay = rc_data::level_gameplay(&root, level).ok()?;
    let pts: Vec<[f32; 3]> = rc_formats::volumes::parse_volumes(&gameplay).ok()?.grind_paths.get(path)?.points.iter().map(|p| [p[0], p[1], p[2]]).collect();
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let len = |a: [f32; 3]| dot(a, a).sqrt();
    // The straightest stretch `length` long: from each point, where the line is `length` on, and how far the points
    // between stray from the chord.
    let mut best: Option<(f32, [f32; 3], [f32; 3])> = None;
    for i in start.map_or(0..pts.len(), |k| k..(k + 1).min(pts.len())) {
        let mut walked = 0.0;
        let mut j = i;
        while j + 1 < pts.len() && walked < length { walked += len(sub(pts[j + 1], pts[j])); j += 1; }
        if walked < length { break; }
        let d = sub(pts[j], pts[i]);
        let l = len(d).max(1e-6);
        let u = d.map(|v| v / l);
        let stray = (i..=j).map(|k| { let w = sub(pts[k], pts[i]); let t = dot(w, u); len(sub(w, u.map(|v| v * t))) }).fold(0.0, f32::max);
        if best.is_none_or(|b| stray < b.0) { best = Some((stray, pts[i], u)); }
    }
    let (_, a, u) = best?;
    // The rail's frame: x along, y across (level), z up from the grind line.
    let flat = (u[0] * u[0] + u[1] * u[1]).sqrt().max(1e-6);
    let y = [-u[1] / flat, u[0] / flat, 0.0];
    let z = [u[1] * y[2] - u[2] * y[1], u[2] * y[0] - u[0] * y[2], u[0] * y[1] - u[1] * y[0]];
    let (core, data) = level_core(&root, level).ok()?;
    let gs = crate::disc_source::level_file(&root, level, "gs_ram.bin").ok()?;
    let textures = rc_formats::texture::parse_textures(&core, &data, &gs).ok()?;
    let tfrags = rc_formats::tfrag::parse_level_tfrags(&core, &data).ok()?;
    let mut groups: Vec<(i32, Vec<rc_extract::thumbs::Tri>)> = Vec::new();
    for t in &tfrags {
        let Ok(tris) = rc_formats::tfrag::tfrag_triangles(t, 0) else { continue };
        for tri in tris {
            let idx = [tri.a, tri.b, tri.c];
            let local = idx.map(|vi| { let w = sub(t.world_position(vi as usize), a); [dot(w, u), dot(w, y), dot(w, z)] });
            let mid = [0, 1, 2].map(|k| (local[0][k] + local[1][k] + local[2][k]) / 3.0);
            if !(0.0..length).contains(&mid[0]) || local.iter().any(|q| q[1].abs() > across || !(below..above).contains(&q[2]) || q[0] < -0.6 || q[0] > length + 0.6) { continue; }
            let uv = idx.map(|vi| { let e = t.vertex_info[vi as usize]; [e.s as f32 / 4096.0, e.t as f32 / 4096.0] });
            let tex = t.texture_index(tri.ad_gif as usize);
            let tri = rc_extract::thumbs::Tri { p: local, uv, tex: usize::try_from(tex).ok().map(|i| (TextureTable::Tfrag, i)) };
            match groups.iter_mut().find(|g| g.0 == tex) {
                Some(g) => g.1.push(tri),
                None => groups.push((tex, vec![tri])),
            }
        }
    }
    if groups.is_empty() { return None; }
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for q in groups.iter().flat_map(|g| g.1.iter().flat_map(|t| t.p)) {
        for k in 0..3 { lo[k] = lo[k].min(q[k]); hi[k] = hi[k].max(q[k]); }
    }
    // The piece's length is the cut's (the triangles reaching past it overlap the next piece a little).
    lo[0] = 0.0;
    hi[0] = length;
    let parts = groups.into_iter().map(|(tex, tris)| (usize::try_from(tex).ok().and_then(|i| textures.iter().find(|t| t.table == TextureTable::Tfrag && t.index == i)).map(|t| t.texture.clone()), tris)).collect();
    Some(PieceModel { parts, lo, hi })
}

/// A rail look's pieces laid along a rail: each piece's x along its stretch, its y across the rail (centred), its z
/// over the line there (game axes).
fn rail_piece_tris(m: &PieceModel, runs: &[(Coast, Vec<(f32, f32)>)]) -> Vec<Vec<([[f32; 3]; 3], [[f32; 2]; 3])>> {
    let length = m.length();
    let mut out = vec![Vec::new(); m.parts.len()];
    for (coast, stretches) in runs {
        for &(s0, s1) in stretches {
            let place = |q: [f32; 3]| {
                let s = (s0 + q[0] / length * (s1 - s0)).clamp(0.0, coast.len);
                let (c, d) = (coast.point(s), coast.direction(s));
                [c[0] + d[0] * 0.0 - d[1] * q[1], c[1] + d[1] * 0.0 + d[0] * q[1], c[2] + q[2]]
            };
            for (part, (_, tris)) in out.iter_mut().zip(&m.parts) {
                for t in tris { part.push((t.p.map(place), t.uv)); }
            }
        }
    }
    out
}

/// The model of edge piece `id` (read from its level once; None when it has none).
pub fn piece_model(id: rc_map::AssetId) -> Option<Arc<PieceModel>> {
    let cache = PIECES.get_or_init(Default::default);
    if let Some(m) = cache.lock().unwrap().get(&id) { return m.clone(); }
    let m = load_piece(id).map(Arc::new);
    if m.is_none() { eprintln!("map: edge piece {id} not found: an invisible edge"); }
    cache.lock().unwrap().insert(id, m.clone());
    m
}

fn load_piece(id: rc_map::AssetId) -> Option<PieceModel> {
    use rc_extract::thumbs as th;
    let root = crate::level_load::extracted_root();
    let (core, data) = level_core(&root, id.level).ok()?;
    let gs = crate::disc_source::level_file(&root, id.level, "gs_ram.bin").ok()?;
    let textures = rc_formats::texture::parse_textures(&core, &data, &gs).ok()?;
    let tris = match id.kind {
        rc_map::AssetKind::Tie => rc_formats::tie::parse_level_ties(&core, &data).ok()?.iter().find(|c| c.o_class == id.number).map(th::tie_tris)?,
        rc_map::AssetKind::Moby => rc_formats::moby::parse_level_mobys(&core, &data).ok()?.iter().find(|c| c.o_class == id.number).map(th::moby_tris)?,
        rc_map::AssetKind::Shrub => rc_formats::shrub::parse_level_shrubs(&core, &data).ok()?.iter().find(|c| c.o_class == id.number).map(th::shrub_tris)?,
        _ => return None,
    };
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for q in tris.iter().flat_map(|t| t.p) {
        for k in 0..3 { lo[k] = lo[k].min(q[k]); hi[k] = hi[k].max(q[k]); }
    }
    if tris.is_empty() { return None; }
    let mut groups: Vec<(Option<(TextureTable, usize)>, Vec<rc_extract::thumbs::Tri>)> = Vec::new();
    for t in tris {
        match groups.iter_mut().find(|g| g.0 == t.tex) {
            Some(g) => g.1.push(t),
            None => groups.push((t.tex, vec![t])),
        }
    }
    let parts = groups
        .into_iter()
        .map(|(tex, tris)| (tex.and_then(|(table, index)| textures.iter().find(|t| t.table == table && t.index == index)).map(|t| t.texture.clone()), tris))
        .collect();
    Some(PieceModel { parts, lo, hi })
}

pub use rc_map::coast::Coast;

/// A patch's outline as coasts, each cut into a whole number of stretches of about `length` (stretched a little to
/// close it): per coast its stretches' start and end distances.
pub fn coast_runs(p: &rc_map::terrain::Patch, length: f32) -> Vec<(Coast, Vec<(f32, f32)>)> {
    p.loops()
        .into_iter()
        .map(Coast::new)
        .filter(|c| c.len >= 0.5)
        .map(|c| {
            let runs = c.runs(length);
            (c, runs)
        })
        .collect()
}

/// Where a patch's edge stretches stand: per stretch the two points (on the outline, at the ground) its ends go to.
pub fn piece_spans(p: &rc_map::terrain::Patch, length: f32) -> Vec<([f32; 3], [f32; 3])> {
    coast_runs(p, length).iter().flat_map(|(c, runs)| runs.iter().map(move |&(s0, s1)| (c.point(s0), c.point(s1)))).collect()
}

/// Lines cut into a whole number of stretches of about `length` each, the ones whose middle is in an opening of `e`
/// left out.
fn line_runs(lines: Vec<(Vec<[f32; 3]>, bool)>, length: f32, e: &rc_map::terrain::Edge) -> Vec<(Coast, Vec<(f32, f32)>)> {
    lines
        .into_iter()
        .map(|(pts, closed)| Coast::line(pts, closed))
        .filter(|c| c.len >= 0.5)
        .map(|c| {
            let _ = length;
            let keep = e.spans(&c).into_iter().filter(|x| !x.2).map(|x| (x.0, x.1)).collect();
            (c, keep)
        })
        .collect()
}

/// A walkway's edge stretches that stand, along both its sides.
pub fn walkway_runs(w: &rc_map::walkway::Walkway) -> Vec<(Coast, Vec<(f32, f32)>)> {
    let Some(e) = &w.edge else { return Vec::new() };
    line_runs(w.edge_lines().into_iter().map(|l| (l, false)).collect(), edge_step(e), e)
}

/// The edge's stretches that stand (outside its openings, judged at each stretch's middle).
pub fn edge_runs(p: &rc_map::terrain::Patch) -> Vec<(Coast, Vec<(f32, f32)>)> {
    let Some(e) = &p.edge else { return Vec::new() };
    line_runs(p.loops().into_iter().map(|l| (l, true)).collect(), edge_step(e), e)
}

/// How long one stretch of an edge is: its own step, else its piece's length, else 3 units (what an opening takes out
/// at a click).
pub fn edge_step(e: &rc_map::terrain::Edge) -> f32 {
    if e.step > 0.0 { return e.step; }
    e.piece().and_then(piece_model).map_or(3.0, |m| m.length())
}

/// An edge's stretch length and inset taken from its piece's model (its length; its depth, so Ratchet stops on the
/// land side of it), where the map does not give them yet.
pub fn fit_edge(e: &mut rc_map::terrain::Edge) {
    if e.step > 0.0 { return; }
    let Some(m) = e.piece().and_then(piece_model) else { return };
    let side = 1 - m.long_axis();
    let q = |v: f32| (v * 64.0).round() / 64.0;
    e.step = q(m.length());
    e.inset = q((m.hi[side] - m.lo[side] + 0.05).min(4.0));
}

/// Every edge of a map fitted ([`fit_edge`]).
pub fn fit_edges(def: &mut MapDef) {
    for e in def.patches.iter_mut().filter_map(|p| p.edge.as_mut()).chain(def.walkways.iter_mut().filter_map(|w| w.edge.as_mut())) { fit_edge(e); }
}

/// An island's edge opened where another island's land is (two islands pushed into each other become one: no railing
/// across the land between them). Only land about as high as this island's own there counts (within 2 units): an
/// island below or above keeps the railing. Built in the loaded copy of the map: the file keeps the openings the map maker
/// made.
pub fn open_where_islands_meet(def: &mut MapDef) {
    let shapes = def.patches.clone();
    for (i, p) in def.patches.iter_mut().enumerate() {
        if p.edge.is_none() { continue; }
        let me = shapes[i].clone();
        p.open_where(|x, y| {
            let Some(z) = me.height_at(x, y) else { return false };
            shapes.iter().enumerate().any(|(j, o)| j != i && o.height_at(x, y).is_some_and(|h| (h - z).abs() <= 2.0))
        });
    }
}

/// The stretches of a patch's edge that stand (outside its openings), each from one point on the outline to the next.
#[cfg_attr(not(test), allow(dead_code))]
pub fn edge_spans(p: &rc_map::terrain::Patch) -> Vec<([f32; 3], [f32; 3])> {
    edge_runs(p).iter().flat_map(|(c, runs)| runs.iter().map(move |&(s0, s1)| (c.point(s0), c.point(s1)))).collect()
}

/// Both sides of a triangle (thin walls and roofs are seen from both).
fn both_sides(t: ([[f32; 3]; 3], [[f32; 2]; 3], [[f32; 3]; 3], [[f32; 4]; 3]), back: f32) -> [([[f32; 3]; 3], [[f32; 2]; 3], [[f32; 3]; 3], [[f32; 4]; 3]); 2] {
    let (p, uv, n, c) = t;
    let rev = ([p[0], p[2], p[1]], [uv[0], uv[2], uv[1]], [n[0], n[2], n[1]].map(|v| [-v[0], -v[1], -v[2]]), [c[0], c[2], c[1]].map(|c| [c[0] * back, c[1] * back, c[2] * back, c[3]]));
    [t, rev]
}

/// A textured wall `height` high along a patch's outline (outside the openings), both sides drawn, its texture `tile`
/// units per repeat along it and up.
fn wall_mesh(p: &rc_map::terrain::Patch, height: f32, tile: f32) -> Mesh { wall_along(p.outline(), p.edge.as_ref(), height, tile) }

/// A textured wall `height` high along segments (each with what it bounds on its left), outside `e`'s openings.
fn wall_along(segs: Vec<([f32; 3], [f32; 3])>, e: Option<&rc_map::terrain::Edge>, height: f32, tile: f32) -> Mesh {
    let mut tris = Vec::new();
    let mut along = 0.0;
    for (a, b) in segs {
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        if len < 1e-5 { continue; }
        let (u0, u1) = (along / tile, (along + len) / tile);
        along += len;
        if e.is_some_and(|e| e.open_at((a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0)) { continue; }
        let out = [(b[1] - a[1]) / len, -(b[0] - a[0]) / len, 0.0];
        let c = [lit(out); 3];
        let (a1, b1) = ([a[0], a[1], a[2] + height], [b[0], b[1], b[2] + height]);
        let v = height / tile;
        // Seen from outside: a bottom, a top, b top; a bottom, b top, b bottom (clockwise).
        for t in [([a, a1, b1], [[u0, v], [u0, 0.0], [u1, 0.0]], [out; 3], c), ([a, b1, b], [[u0, v], [u1, 0.0], [u1, v]], [out; 3], c)] {
            tris.extend(both_sides(t, 0.8));
        }
    }
    soup_mesh(&tris)
}

/// A walkway: its top (textured, lit by its slope), its sides and ends down its depth, its underside.
fn walkway_mesh(w: &rc_map::walkway::Walkway) -> Mesh {
    let mut tris = Vec::new();
    let uv = |q: [f32; 3]| [q[0] / w.tile, q[1] / w.tile];
    for t in w.top_tris() {
        let (e1, e2) = ([t[1][0] - t[0][0], t[1][1] - t[0][1], t[1][2] - t[0][2]], [t[2][0] - t[0][0], t[2][1] - t[0][1], t[2][2] - t[0][2]]);
        // Clockwise from above: the up normal is e2 × e1.
        let n = [e2[1] * e1[2] - e2[2] * e1[1], e2[2] * e1[0] - e2[0] * e1[2], e2[0] * e1[1] - e2[1] * e1[0]];
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
        let n = n.map(|v| v / l);
        tris.push((t, t.map(uv), [n; 3], t.map(|q| lit_at(n, q))));
        if w.depth > 0.0 {
            let b = t.map(|q| [q[0], q[1], q[2] - w.depth]);
            tris.push(([b[0], b[2], b[1]], [b[0], b[2], b[1]].map(uv), [[0.0, 0.0, -1.0]; 3], [[0.35, 0.35, 0.35, 1.0]; 3]));
        }
    }
    if w.depth > 0.0 {
        let o = w.outline();
        let mut along = 0.0;
        for k in 0..o.len() {
            let (a, b) = (o[k], o[(k + 1) % o.len()]);
            let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
            if len < 1e-5 { continue; }
            let out = [(b[1] - a[1]) / len, -(b[0] - a[0]) / len, 0.0];
            let c = [dim(lit(out), 0.85); 3];
            let (a1, b1) = ([a[0], a[1], a[2] - w.depth], [b[0], b[1], b[2] - w.depth]);
            let (u0, u1, v) = (along / w.tile, (along + len) / w.tile, w.depth / w.tile);
            along += len;
            tris.push(([a1, a, b], [[u0, v], [u0, 0.0], [u1, 0.0]], [out; 3], c));
            tris.push(([a1, b, b1], [[u0, v], [u1, 0.0], [u1, v]], [out; 3], c));
        }
    }
    let tris = tinted(tris, w.tint);
    soup_mesh(&tris)
}

/// Scattered pieces' triangles per model part: each copy turned, scaled and stood on the land under it (copies off
/// the land are left out).
fn scatter_tris(m: &PieceModel, p: &rc_map::terrain::Patch, id: rc_map::AssetId) -> Vec<Vec<([[f32; 3]; 3], [[f32; 2]; 3])>> {
    let mut out = vec![Vec::new(); m.parts.len()];
    for s in p.scatter.iter().filter(|s| s.asset == id) {
        let Some(z) = p.height_at(s.at[0], s.at[1]) else { continue };
        let (sin, cos) = s.yaw.sin_cos();
        let place = |q: [f32; 3]| [s.at[0] + (q[0] * cos - q[1] * sin) * s.scale, s.at[1] + (q[0] * sin + q[1] * cos) * s.scale, z + q[2] * s.scale];
        for (part, (_, tris)) in out.iter_mut().zip(&m.parts) {
            for t in tris { part.push((t.p.map(place), t.uv)); }
        }
    }
    out
}

/// A roof: the land's shape at its height, lit from above, its underside darker.
fn roof_mesh(p: &rc_map::terrain::Patch, r: &rc_map::terrain::Roof) -> Mesh {
    let up = [0.0, 0.0, 1.0];
    let l = lit(up);
    let tris: Vec<_> = p
        .roof_tris()
        .into_iter()
        .flat_map(|t| both_sides((t, t.map(|q| [(q[0] - p.origin[0]) / r.tile, (q[1] - p.origin[1]) / r.tile]), [up; 3], [l; 3]), 0.55))
        .collect();
    let tris = tinted(tris, p.tint);
    soup_mesh(&tris)
}

/// The edge pieces' triangles per model part: each piece laid along its stretch of the coast, bent to follow it (every
/// vertex at its own distance along the coast, its back to the outline: it stands on the land), on the ground there.
/// (Game axes, wound as the model.)
fn piece_tris(m: &PieceModel, runs: &[(Coast, Vec<(f32, f32)>)]) -> Vec<Vec<([[f32; 3]; 3], [[f32; 2]; 3])>> {
    let k = m.long_axis();
    let side = 1 - k;
    let length = m.length();
    let mut out = vec![Vec::new(); m.parts.len()];
    for (coast, stretches) in runs {
        for &(s0, s1) in stretches {
            let place = |q: [f32; 3]| {
                let s = s0 + (q[k] - m.lo[k]) / length * (s1 - s0);
                let (c, d) = (coast.point(s), coast.direction(s));
                // The land is on the left of the outline.
                let across = q[side] - m.lo[side] + 0.02;
                [c[0] - d[1] * across, c[1] + d[0] * across, c[2] + q[2] - m.lo[2]]
            };
            for (part, (_, tris)) in out.iter_mut().zip(&m.parts) {
                for t in tris { part.push((t.p.map(place), t.uv)); }
            }
        }
    }
    out
}

/// A mesh of edge-piece triangles (both sides drawn by its material), lit flat like the PS2 look's scenery.
fn piece_mesh(tris: &[([[f32; 3]; 3], [[f32; 2]; 3])]) -> Mesh {
    let mut positions = Vec::with_capacity(tris.len() * 3);
    let mut uvs = Vec::with_capacity(tris.len() * 3);
    let mut normals = Vec::with_capacity(tris.len() * 3);
    let mut colors = Vec::with_capacity(tris.len() * 3);
    for (p, uv) in tris {
        let b = p.map(to_bevy);
        let n = (b[1] - b[0]).cross(b[2] - b[0]).normalize_or(Vec3::Y);
        // The rig's light on the face (either side: the pieces are drawn both ways), never darker than its ambient.
        let g = [n.x, -n.z, n.y];
        for k in 0..3 {
            let l = lit_at(g, p[k]);
            let l2 = lit_at(g.map(|v| -v), p[k]);
            positions.push(b[k].to_array());
            uvs.push(uv[k]);
            normals.push(n.to_array());
            colors.push([l[0].max(l2[0]), l[1].max(l2[1]), l[2].max(l2[2]), 1.0]);
        }
    }
    let count = positions.len() as u32;
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
        .with_inserted_indices(Indices::U32((0..count).collect()))
}

/// The materials of an edge piece's parts (its textures, PS2 alpha cut out: fences and grates see through).
fn piece_materials(m: &PieceModel, images: &mut Assets<Image>, materials: &mut Assets<StandardMaterial>) -> Vec<Handle<StandardMaterial>> {
    m.parts
        .iter()
        .map(|(tex, _)| {
            let image = tex.as_ref().map(|t| {
                let pal = *PALETTE.read().unwrap_or_else(|e| e.into_inner());
                let rgba: Vec<u8> = t.rgba.chunks(4).flat_map(|p| { let c = pal.map_or([p[0], p[1], p[2]], |q| q.apply([p[0], p[1], p[2]])); [c[0], c[1], c[2], (p[3] as u16 * 2).min(255) as u8] }).collect();
                let mut img = Image::new(Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 }, TextureDimension::D2, rgba, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::RENDER_WORLD);
                img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor { address_mode_u: ImageAddressMode::Repeat, address_mode_v: ImageAddressMode::Repeat, ..ImageSamplerDescriptor::linear() });
                images.add(img)
            });
            materials.add(StandardMaterial {
                base_color: if image.is_some() { Color::WHITE } else { Color::srgb(0.6, 0.6, 0.62) },
                base_color_texture: image,
                unlit: true,
                alpha_mode: AlphaMode::Mask(0.4),
                cull_mode: None,
                double_sided: true,
                ..default()
            })
        })
        .collect()
}

fn spawn_geometry(
    mut commands: Commands,
    level: Res<crate::Level>,
    old: Query<Entity, With<MapGeometry>>,
    mut drawn: ResMut<Drawn>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for e in &old { commands.entity(e).despawn(); }
    let lv = &level.0;
    drawn.def = on(lv.particles.level);
    let Some(def) = drawn.def.clone() else { return };
    draw(&mut commands, lv, &def, &mut drawn, &mut meshes, &mut images, &mut materials);
    println!("map: drawing {} floors, {} terrain patches and {} rails", def.floors.len(), def.patches.len(), def.rails.len());
}

/// The editor changed the map's terrain or rails (crate::editor: a brush stroke, a new patch): drawn again at once,
/// before the reload that brings the collision.
#[allow(clippy::too_many_arguments)]
fn redraw_changed(
    mut commands: Commands,
    level: Res<crate::Level>,
    old: Query<Entity, With<MapGeometry>>,
    mut drawn: ResMut<Drawn>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let lv = &level.0;
    let Some(now) = on(lv.particles.level) else { return };
    if drawn.def.as_ref().is_some_and(|d| Arc::ptr_eq(d, &now)) { return; }
    let same = drawn.def.as_deref().is_some_and(|d| geometry_of(d) == geometry_of(&now));
    drawn.def = Some(now.clone());
    if same { return; }
    for e in &old { commands.entity(e).despawn(); }
    draw(&mut commands, lv, &now, &mut drawn, &mut meshes, &mut images, &mut materials);
}

/// Spawns the floors, patches and rails of `def`.
fn draw(
    commands: &mut Commands,
    lv: &crate::level_load::LoadedLevel,
    def: &MapDef,
    drawn: &mut Drawn,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) {
    // The map's light and palette for the meshes and textures built below; a palette change makes the textures again.
    *RIG.write().unwrap_or_else(|e| e.into_inner()) = rig_of(def.world.light.as_ref());
    *POINTS.write().unwrap_or_else(|e| e.into_inner()) = def.world.lights.clone();
    let pal = def.world.palette.filter(|p| *p != rc_map::Palette::default());
    if *PALETTE.read().unwrap_or_else(|e| e.into_inner()) != pal || *RECOLOUR.read().unwrap_or_else(|e| e.into_inner()) != def.world.recolour {
        *PALETTE.write().unwrap_or_else(|e| e.into_inner()) = pal;
        *RECOLOUR.write().unwrap_or_else(|e| e.into_inner()) = def.world.recolour.clone();
        drawn.textures.clear();
        drawn.pieces.clear();
    }
    // A texture's material (plain colour without one); `blend`: drawn over what is under it by its vertex alpha.
    let mut material = |texture: Option<rc_map::AssetId>, plain: Color, blend: bool| {
        let image = texture.and_then(|id| drawn.textures.entry(id).or_insert_with(|| texture_image(lv, id).map(|i| images.add(i))).clone());
        let alpha_mode = if blend { AlphaMode::Blend } else { AlphaMode::Opaque };
        materials.add(StandardMaterial { base_color: if image.is_some() { Color::WHITE } else { plain }, base_color_texture: image, unlit: true, alpha_mode, ..default() })
    };
    for f in &def.floors {
        let m = material(f.texture, Color::srgb(0.45, 0.5, 0.4), false);
        commands.spawn((Mesh3d(meshes.add(floor_mesh(f))), MeshMaterial3d(m), Transform::IDENTITY, MapGeometry));
    }
    for p in &def.patches {
        let m = material(p.texture, Color::srgb(0.5, 0.56, 0.42), false);
        commands.spawn((Mesh3d(meshes.add(patch_mesh(p))), MeshMaterial3d(m), Transform::IDENTITY, MapGeometry));
        for k in 0..p.layers.len() {
            let Some(mesh) = layer_mesh(p, k) else { continue };
            let m = material(Some(p.layers[k].texture), Color::WHITE, true);
            commands.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(m), Transform::IDENTITY, MapGeometry));
        }
    }
    // The edges along the outlines (their openings left out), the roofs.
    for p in &def.patches {
        if let Some(r) = &p.roof {
            let m = material(r.texture, Color::srgb(0.55, 0.5, 0.45), false);
            commands.spawn((Mesh3d(meshes.add(roof_mesh(p, r))), MeshMaterial3d(m), Transform::IDENTITY, MapGeometry));
        }
        let Some(e) = &p.edge else { continue };
        match &e.kind {
            rc_map::terrain::EdgeKind::Invisible => {}
            rc_map::terrain::EdgeKind::Wall { texture, tile } => {
                let m = material(*texture, Color::srgb(0.55, 0.57, 0.62), false);
                commands.spawn((Mesh3d(meshes.add(wall_mesh(p, e.height, *tile))), MeshMaterial3d(m), Transform::IDENTITY, MapGeometry));
            }
            rc_map::terrain::EdgeKind::Piece(_) => {}
        }
    }
    // The walkways as built (a cut one only off the land).
    let walkways = def.walkway_pieces();
    for w in &walkways {
        let m = material(w.texture, Color::srgb(0.5, 0.5, 0.52), false);
        commands.spawn((Mesh3d(meshes.add(walkway_mesh(w))), MeshMaterial3d(m), Transform::IDENTITY, MapGeometry));
        if let Some(rc_map::terrain::Edge { kind: rc_map::terrain::EdgeKind::Wall { texture, tile }, height, .. }) = &w.edge {
            let segs = w.edge_lines().into_iter().flat_map(|l| l.windows(2).map(|s| (s[0], s[1])).collect::<Vec<_>>()).collect();
            let m = material(*texture, Color::srgb(0.55, 0.57, 0.62), false);
            commands.spawn((Mesh3d(meshes.add(wall_along(segs, w.edge.as_ref(), *height, *tile))), MeshMaterial3d(m), Transform::IDENTITY, MapGeometry));
        }
    }
    // The game pieces (their own materials): the edges' along the coasts and the walkways' sides, the scatter.
    let mut lay = |id: rc_map::AssetId, parts: &dyn Fn(&PieceModel) -> Vec<Vec<([[f32; 3]; 3], [[f32; 2]; 3])>>| {
        let Some(model) = piece_model(id) else { return };
        let mats = drawn.pieces.entry(id).or_insert_with(|| piece_materials(&model, images, materials)).clone();
        for (tris, m) in parts(&model).iter().zip(mats) {
            if tris.is_empty() { continue; }
            commands.spawn((Mesh3d(meshes.add(piece_mesh(tris))), MeshMaterial3d(m), Transform::IDENTITY, MapGeometry));
        }
    };
    for p in &def.patches {
        if let Some(id) = p.edge.as_ref().and_then(|e| e.piece()) { lay(id, &|m| piece_tris(m, &edge_runs(p))); }
        let mut kinds: Vec<rc_map::AssetId> = p.scatter.iter().map(|s| s.asset).collect();
        kinds.sort_by_key(|a| (a.level, a.kind as u8, a.number));
        kinds.dedup();
        for id in kinds { lay(id, &|m| scatter_tris(m, p, id)); }
    }
    for w in &walkways {
        if let Some(id) = w.edge.as_ref().and_then(|e| e.piece()) { lay(id, &|m| piece_tris(m, &walkway_runs(w))); }
    }
    let rail = materials.add(StandardMaterial { base_color: Color::srgb(1.0, 0.75, 0.2), unlit: true, ..default() });
    for r in &def.rails {
        // A planet's rail look: its piece repeated along the line ([`rail_look`]); else the plain beam.
        let look = r.look.as_deref().and_then(rail_look);
        let Some(model) = look else {
            commands.spawn((Mesh3d(meshes.add(rail_mesh(&r.line()))), MeshMaterial3d(rail.clone()), Transform::IDENTITY, MapGeometry));
            continue;
        };
        let coast = Coast::line(r.line(), false);
        let runs = vec![(coast.clone(), coast.runs(model.length()))];
        let key = rc_map::AssetId { game: rc_map::Game::Rac1, level: 99, kind: rc_map::AssetKind::Tie, number: RAIL_LOOKS.iter().position(|l| Some(l.0) == r.look.as_deref()).unwrap_or(0) as i32 };
        let mats = drawn.pieces.entry(key).or_insert_with(|| piece_materials(&model, images, materials)).clone();
        for (tris, m) in rail_piece_tris(&model, &runs).iter().zip(mats) {
            if tris.is_empty() { continue; }
            commands.spawn((Mesh3d(meshes.add(piece_mesh(tris))), MeshMaterial3d(m), Transform::IDENTITY, MapGeometry));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two islands pushed into each other: each one's railing opens over the other's land, nowhere else; an island
    /// far below keeps its railing.
    #[test]
    fn railings_open_where_islands_meet() {
        let island = |x: f32, z: f32| {
            let mut p = rc_map::terrain::Patch::island([x, 200.0, z], 1.0, 10.0, |a, b| 10.0 - (a * a + b * b).sqrt());
            p.edge = Some(rc_map::terrain::Edge::new(rc_map::terrain::EdgeKind::Invisible, 1.0));
            p
        };
        let mut def = MapDef::new_default("t", 1);
        def.patches = vec![island(200.0, 60.0), island(214.0, 60.0), island(260.0, 60.0), island(228.0, 40.0)];
        open_where_islands_meet(&mut def);
        let gaps = |k: usize| def.patches[k].edge.as_ref().unwrap().gaps.clone();
        assert!(!gaps(0).is_empty() && !gaps(1).is_empty(), "{:?} {:?}", gaps(0), gaps(1));
        // The openings are where the other island is (between x 204 and 210), not on the far sides.
        assert!(gaps(0).iter().all(|g| g[0] > 200.0) && gaps(1).iter().all(|g| g[0] < 214.0));
        assert!(gaps(2).is_empty() && gaps(3).is_empty());
    }
}
