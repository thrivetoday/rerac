//! The RAC Maker asset catalog builder: `rerac --data-dir <data> --build-catalog` indexes every moby, tie and shrub
//! class, sky and tfrag texture of the 19 levels into `<data>/catalog/catalog.json` (`rc_map::catalog`) and exits.
//! It lives in the engine, not the extractor, because a moby's behaviour status needs the class ports (rc_game) and
//! the catalog's thumbnails will need the renderer.
//!
//! Per level: the core's class lists (every class, with or without a model), the textures each class uses, the
//! gameplay file's placements, and the level's ported updates (`LevelPorts` of its own overlay, the same matching the
//! game load uses). Assets with the same content on several levels become one entry (`rc_map::catalog::dedupe`):
//! the content hash is the class blob's with the pixels of every texture it uses.

use anyhow::{Context, Result};
use rc_formats::level_overlay::LevelOverlay;
use rc_formats::texture::TextureTable;
use rc_game::moby_update::classes::{units, ClassUpdate, LevelPorts};
use rc_map::catalog::{self, Behaviour, Catalog, Entry};
use rc_map::{AssetId, AssetKind, Game};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// The unit ports' index (`rc_game`'s `units` module doc), compiled in so the names follow the ports.
const UNIT_INDEX: &str = include_str!("../../rc-game/src/moby_update/classes/units/mod.rs");

/// The unit ports' names: per `units::PORTS` row (in order), its module (from the row's `update: <module>::…`), and per
/// module the description of its index row (`//! | U301 | 193 pack biters (09, 15) | … | [`pack_biter`] |`). Census
/// ids repeat across runs (U301 is also Gaspar's meteor shower), so the module is the key.
struct UnitNames {
    modules: Vec<String>,
    about: HashMap<String, String>,
}

fn unit_names() -> UnitNames {
    let module_of = |s: &str| s.rsplit("::").next().unwrap_or(s).to_string();
    let modules: Vec<String> = UNIT_INDEX
        .lines()
        .filter(|l| l.trim_start().starts_with("UnitPort {"))
        .map(|l| l.split("update: ").nth(1).and_then(|u| u.split("::").next()).map(module_of).unwrap_or_default())
        .collect();
    let mut about = HashMap::new();
    for line in UNIT_INDEX.lines().filter(|l| l.starts_with("//! | U")) {
        let cells: Vec<&str> = line.trim_start_matches("//!").split('|').map(str::trim).collect();
        let Some(classes) = cells.get(2) else { continue };
        let Some(module) = cells.iter().rev().find_map(|c| c.split("[`").nth(1).and_then(|m| m.split("`]").next())) else { continue };
        // The class list (numbers, commas, ranges), then the description up to the level list "(09)".
        let text = classes.trim_start_matches(|c: char| c.is_ascii_digit() || matches!(c, ',' | ' ' | '–' | '-'));
        let d = text.split(" (").next().unwrap_or("").trim();
        if !d.is_empty() { about.entry(module_of(module)).or_insert_with(|| d.to_string()); }
    }
    if modules.len() != units::PORTS.len() {
        eprintln!("catalog: {} unit port rows read for {} ports: unit names left out", modules.len(), units::PORTS.len());
        return UnitNames { modules: Vec::new(), about };
    }
    UnitNames { modules, about }
}

/// The port's id, name and description: a unit port's census id, module and index description (`U301`, "pack biter",
/// "pack biters"), else the port's own name.
fn port_name(u: ClassUpdate, names: &UnitNames) -> (String, String, String) {
    match u {
        ClassUpdate::Unit(i) => {
            let uid = units::PORTS.get(i as usize).map_or("", |p| p.unit.split(' ').next().unwrap_or(""));
            let module = names.modules.get(i as usize).cloned().unwrap_or_default();
            let about = names.about.get(&module).cloned().unwrap_or_default();
            (uid.to_string(), module.trim_end_matches('_').replace('_', " "), about)
        }
        other => {
            let n = format!("{other:?}");
            (n.clone(), n.to_lowercase(), String::new())
        }
    }
}

/// What a moby is for the editor's type filter (`Entry::kind`): pickups by their group, creatures by their reaction
/// table (or an enemy's name), effects and machines by the words of their port, name and description, scenery for
/// the classes with no behaviour code.
fn kind_of(e: &Entry, creature: bool) -> &'static str {
    use rc_map::catalog::*;
    // The name and port only: a description lists the classes a port also runs (an enemy's mentions its shots).
    let text = format!("{} {}", e.port, e.name).to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| text.contains(w));
    if e.group == GROUP_PICKUPS || has(&["breakable"]) { return KIND_PICKUP; }
    if e.group == GROUP_GEAR { return ""; }
    if creature { return KIND_ENEMY; }
    // Walls (the shells' walls 709–711, the big wall 729) are scenery, whatever else their name says.
    if has(&["wall"]) && !has(&["laser"]) { return KIND_SCENERY; }
    // Hazards that stay put (laser fences and gates) are machines, not effects.
    if has(&["laser gate", "laser fence", "swing laser", "water laser", "flame jet", "arc", "spout"]) { return KIND_PLATFORM; }
    if has(&["fxpiece", "debris", "emitter", "explosion", "particle", "shot", "missile", "glob", "spark", "smoke", "splash", "piece", "mine",
        "flash", "bomb", "fireball", "glow", "ember", "shell", "canister", "decoy", "doombot", "laser", " fx"]) {
        return KIND_EFFECT;
    }
    if has(&["trooper", "brawler", "biter", "sandshark", "flamer", "toad", "critter", "amoeboid", "robot", "drone", "turret", "sentry", "enemy",
        "guard", "snapper", "monster", "creature", "soldier", "gunship", "dropship", "tank", "grenadier", "runner", "fighter", "bomber", "lobber",
        "stalker", "boss", "gadgetbot", "wave bot", "beast", "hound", "crawler", "zapper", "cannon", "launcher", "gunner", "commando", "thrower",
        "walker", "logger", "jet bot", "rail bot", "copter", "racer"]) {
        return KIND_ENEMY;
    }
    if has(&["platform", "lift", "elevator", "door", "gate", "bridge", "switch", "button", "pad", "belt", "conveyor", "crank", "fan", "piston",
        "swing", "ramp", "teleport", "wheel", "crusher", "spinner", "mover", "hatch", "valve", "barrier", "fence", "lock", "vendor", "ship",
        "hoverboard", "target", "pulley", "magne", "tube", "ferry", "chain", "turner", "turnover", "extending", "collapse", "slider", "rotator", "cog",
        "car", "boat", "raft", "float", "riser", "turntable", "train", "transport", "carrier", "flap", "tilt", "glider", "pushable", "sinker"]) {
        return KIND_PLATFORM;
    }
    if e.behaviour == Some(Behaviour::Static) || has(&["grass", "lamp", "feather", "plant", "tree", "rock", "boulder", "sign", "flag", "light",
        "bush", "flower", "barricade", "asteroid", "drip"]) {
        return KIND_SCENERY;
    }
    KIND_OTHER
}

/// The box around triangles, scaled about the origin (`Entry::bounds`); None without triangles.
fn bounds(tris: &[rc_extract::thumbs::Tri], scale: f32) -> Option<[f32; 6]> {
    let mut b = [f32::MAX, f32::MAX, f32::MAX, f32::MIN, f32::MIN, f32::MIN];
    for p in tris.iter().flat_map(|t| t.p) {
        for k in 0..3 {
            b[k] = b[k].min(p[k] * scale);
            b[k + 3] = b[k + 3].max(p[k] * scale);
        }
    }
    // Rounded to 1/64 (the file stays short; far below what placement needs).
    (!tris.is_empty()).then(|| b.map(|v| (v * 64.0).round() / 64.0))
}

/// The models of Ratchet's items by class (the item definitions' `+0` classes, docs/plan/gadgets.md; the same on every
/// level): held, worn or carried by the hero code, never placed.
const GEAR: [(i16, &str); 34] = [
    (71, "OmniWrench"), (849, "Suck Cannon"), (192, "Bomb Glove"), (157, "Devastator"), (208, "Swingshot"),
    (163, "Visibomb Gun"), (175, "Taunter"), (168, "Blaster"), (176, "Pyrocitor"), (190, "Mine Glove"), (180, "Walloper"),
    (177, "Tesla Claw"), (229, "Glove of Doom"), (185, "Morph-o-Ray"), (1251, "Hydrodisplacer"), (454, "R.Y.N.O."),
    (483, "Drone Device / Hologuise"), (562, "Decoy Glove"), (188, "Trespasser"), (585, "Metal Detector"),
    (619, "Gadgetron PDA"), (173, "Magneboots"), (195, "Grindboots"), (439, "Hoverboard"), (433, "Sonic Summoner"),
    (1289, "O2 Mask"), (1290, "Pilot's Helmet"), (607, "Heli-Pack"), (608, "Thruster-Pack"), (609, "Hydro-Pack"),
    (601, "Clank (backpack)"), (614, "Map-o-matic"), (618, "Bolt Grabber"), (407, "Persuader"),
];

/// Classes the engine's own code names, with their group: Ratchet, his bodies, actors, ship and items
/// (`rc_map::catalog::GROUP_GEAR`), and the crates (`crate_`'s module doc: 500 bolts, 501 nanotech, 502 reinforced,
/// 505 TNT, 511 ammo). Not ports, or one port for several, so the port names do not tell them apart.
fn known_name(o: i32) -> Option<(&'static str, &'static str)> {
    use rc_game::hero::bodies::{CLANK_CLASS, DISGUISE_CLASS, GIANT_CLASS};
    use rc_map::catalog::{GROUP_GEAR, GROUP_PICKUPS};
    let gear = |n| Some((n, GROUP_GEAR));
    match o as i16 {
        0 => gear("Ratchet"),
        // `rc_game::scene_actors`: Clank as a cutscene actor.
        10 => gear("Clank (scene actor)"),
        c if c == CLANK_CLASS => gear("Clank"),
        c if c == GIANT_CLASS => gear("Giant Clank"),
        c if c == DISGUISE_CLASS => gear("Hologuise Ratchet"),
        c if rc_game::travel::ship::CLASSES.contains(&c) => gear("Ratchet's ship"),
        500 => Some(("Bolt crate", GROUP_PICKUPS)),
        501 => Some(("Nanotech crate", GROUP_PICKUPS)),
        502 => Some(("Reinforced crate", GROUP_PICKUPS)),
        505 => Some(("TNT crate", GROUP_PICKUPS)),
        511 => Some(("Ammo crate", GROUP_PICKUPS)),
        c => GEAR.iter().find(|g| g.0 == c).and_then(|g| gear(g.1)),
    }
}

/// What a model looks like, in words: (size, shape, colour), from its class-space triangles and the mean colour of
/// the textures they use. For the names of unnamed classes and the catalog's tags.
fn describe(tris: &[rc_extract::thumbs::Tri], mean: &dyn Fn(TextureTable, usize) -> Option<[f32; 3]>) -> Option<(&'static str, &'static str, &'static str)> {
    let pts: Vec<[f32; 3]> = tris.iter().flat_map(|t| t.p).filter(|p| p.iter().all(|c| c.is_finite())).collect();
    if pts.is_empty() { return None; }
    let ext: [f32; 3] = std::array::from_fn(|k| {
        let (lo, hi) = pts.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p[k]), hi.max(p[k])));
        hi - lo
    });
    let (w, d, h) = (ext[0].max(ext[1]), ext[0].min(ext[1]), ext[2]);
    let size = match w.max(h) {
        m if m < 1.0 => "tiny",
        m if m < 3.0 => "small",
        m if m < 8.0 => "medium",
        m if m < 20.0 => "large",
        _ => "huge",
    };
    let shape = if h > 1.8 * w { "column" } else if h < 0.25 * w { "platform" } else if w > 3.0 * d.max(1e-3) { "beam" } else { "block" };
    let cols: Vec<[f32; 3]> = tris.iter().filter_map(|t| t.tex.and_then(|(tb, i)| mean(tb, i))).collect();
    let colour = if cols.is_empty() { "grey" } else {
        let n = cols.len() as f32;
        let m: [f32; 3] = std::array::from_fn(|k| cols.iter().map(|c| c[k]).sum::<f32>() / n);
        colour_name(m)
    };
    Some((size, shape, colour))
}

/// The name of a colour (0..1 RGB), coarse: what a browser filter needs.
fn colour_name([r, g, b]: [f32; 3]) -> &'static str {
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let (v, s) = (max, if max > 0.0 { (max - min) / max } else { 0.0 });
    if s < 0.18 { return if v > 0.75 { "white" } else if v < 0.2 { "black" } else { "grey" }; }
    let h = if max == r { 60.0 * ((g - b) / (max - min)).rem_euclid(6.0) } else if max == g { 60.0 * ((b - r) / (max - min) + 2.0) } else { 60.0 * ((r - g) / (max - min) + 4.0) };
    match h {
        h if !(15.0..345.0).contains(&h) => if v < 0.55 { "brown" } else { "red" },
        h if h < 45.0 => if v < 0.6 { "brown" } else { "orange" },
        h if h < 70.0 => "yellow",
        h if h < 160.0 => "green",
        h if h < 200.0 => "teal",
        h if h < 255.0 => "blue",
        h if h < 290.0 => "purple",
        _ => "pink",
    }
}

/// Level overlays, parsed once and shared by every level's port matching.
type Overlays = Mutex<HashMap<u32, Option<Arc<LevelOverlay>>>>;

fn overlay(root: &Path, cache: &Overlays, l: u32) -> Option<Arc<LevelOverlay>> {
    if let Some(o) = cache.lock().unwrap().get(&l) { return o.clone(); }
    let o = crate::disc_source::level_file(root, l, "overlay.bin").ok().and_then(|b| LevelOverlay::parse(&b).ok()).map(Arc::new);
    cache.lock().unwrap().insert(l, o.clone());
    o
}

/// Thumbnail edge in pixels.
const THUMB_SIZE: u32 = 96;

/// The thumbnails being written: `<data>/catalog/thumbs/<kind>/<content hash>.png`, each rendered once (the first
/// thread to meet the content claims it; a file left by an earlier build is the same content, so it is kept).
struct Thumbs {
    dir: std::path::PathBuf,
    claimed: Mutex<std::collections::HashSet<(AssetKind, u64)>>,
}

impl Thumbs {
    fn claim(&self, kind: AssetKind, hash: u64, rel: &str) -> bool {
        self.claimed.lock().unwrap().insert((kind, hash)) && !self.dir.join(rel).exists()
    }
    fn write(&self, rel: &str, rgba: &[u8]) {
        let path = self.dir.join(rel);
        let png = rc_extract::export::png::encode_rgba(THUMB_SIZE, THUMB_SIZE, rgba);
        if let Err(e) = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&path, png)) {
            eprintln!("catalog: thumbnail {}: {e}", path.display());
        }
    }
}

fn hash_parts(parts: &[u64]) -> u64 { rc_data::xxh64::xxh64(&parts.iter().flat_map(|p| p.to_le_bytes()).collect::<Vec<u8>>()) }

/// One level's finds: (content hash, entry), in kind then class order.
fn level_finds(root: &Path, l: u32, overlays: &Overlays, names: &UnitNames, thumbs: &Thumbs) -> Result<Vec<(u64, Entry)>> {
    let (core, data) = crate::map_load::level_core(root, l)?;
    let gs = crate::disc_source::level_file(root, l, "gs_ram.bin")?;
    let gameplay = rc_data::level_gameplay(root, l).with_context(|| format!("level {l:02}: gameplay"))?;
    let textures = rc_formats::texture::parse_textures(&core, &data, &gs).with_context(|| format!("level {l:02}: textures"))?;
    let tex: HashMap<(TextureTable, usize), u64> = textures.iter().map(|t| ((t.table, t.index), rc_data::xxh64::xxh64(&t.texture.rgba))).collect();
    let ports = match overlay(root, overlays, l) {
        Some(t) => LevelPorts::from_overlays(&t, &|r| overlay(root, overlays, r), &[crate::gameplay::EMITTER_UPDATE]),
        None => LevelPorts::by_class_number(),
    };
    let id = |kind, number| AssetId { game: Game::Rac1, level: l, kind, number };
    let count = |classes: Vec<i32>| classes.into_iter().fold(HashMap::new(), |mut m: HashMap<i32, u32>, c| { *m.entry(c).or_default() += 1; m });
    let moby_instances = rc_formats::gameplay::parse_moby_instances(&gameplay)?;
    let placed_mobys = count(moby_instances.iter().map(|m| m.o_class).collect());
    let placed_ties = count(rc_formats::tie::parse_tie_instances(&gameplay)?.iter().map(|t| t.o_class).collect());
    let placed_shrubs = count(rc_formats::shrub::parse_shrub_instances(&gameplay)?.iter().map(|s| s.o_class).collect());
    // A class's content: its blob, then the pixels of every texture slot it uses (slot 0xff: unused). A class
    // without a blob (logic only: triggers, directors) is unique to its level.
    // The gadget classes (the hand items' models): WAD streams of the core's gadget table, not `moby_class/` blocks.
    let gadgets: Vec<rc_formats::gadget::GadgetClass> = rc_formats::gadget::parse_gadget_classes(&core, &data).unwrap_or_default();
    let gadget_blob = |name: &str| gadgets.iter().find(|g| name == format!("moby_class/{:04}", g.moby.o_class)).map(|g| g.blob.as_slice());
    let content = |name: String, slots: &[u8], table: TextureTable| -> u64 {
        let blob = core.block(&data, &name).or_else(|| gadget_blob(&name));
        let mut parts = vec![blob.map_or(rc_data::xxh64::xxh64(format!("{l}/{name}").as_bytes()), rc_data::xxh64::xxh64)];
        parts.extend(slots.iter().filter(|&&s| s != 0xff).map(|&s| tex.get(&(table, s as usize)).copied().unwrap_or(0)));
        hash_parts(&parts)
    };
    let used = |slots: &[u8]| slots.iter().filter(|&&s| s != 0xff).count() as u32;
    let mut out = Vec::new();

    // The classes with a creature reaction table (they take hits and react: the enemies and critters).
    let creatures = crate::gameplay::own_level_reactions(l);
    // The classes with their own collision.
    let solid: std::collections::HashSet<i32> = rc_formats::moby_collision::parse_level(&core, &data).map(|v| v.into_iter().map(|(c, _)| c).collect()).unwrap_or_default();
    let mut models: HashMap<i32, rc_formats::moby::LevelMobyClass> =
        rc_formats::moby::parse_level_mobys(&core, &data).with_context(|| format!("level {l:02}: moby classes"))?.into_iter().map(|c| (c.o_class, c)).collect();
    for g in &gadgets { models.entry(g.moby.o_class).or_insert_with(|| g.moby.clone()); }
    for e in &core.moby_classes {
        let o = e.o_class;
        let mut entry = Entry::new(id(AssetKind::Moby, o));
        entry.placed = placed_mobys.get(&o).copied().unwrap_or(0);
        entry.textures = used(&e.textures);
        if let Some(m) = models.get(&o) {
            // The box at the first placement's scale (the editor copies that instance).
            let scale = moby_instances.iter().find(|i| i.o_class == o).map_or(1.0, |i| i.scale);
            entry.bounds = bounds(&rc_extract::thumbs::moby_tris(m), scale);
            entry.joints = m.class.header.joint_count as u32;
            entry.sequences = m.class.sequence_pointers.iter().filter(|&&p| p != 0).count() as u32;
        }
        let known = known_name(o);
        let oc = o as i16;
        entry.behaviour = Some(match (ports.get(oc), ports.external(oc), ports.level_update(oc)) {
            (Some(u), _, _) => {
                (entry.port, entry.name, entry.about) = port_name(u, names);
                Behaviour::Ported
            }
            (None, Some(_), _) => {
                (entry.port, entry.name) = ("Emitter".into(), "particle emitter".into());
                Behaviour::Ported
            }
            (None, None, Some(f)) if f != 0 => Behaviour::Unported,
            _ => Behaviour::Static,
        });
        if let Some((n, group)) = known {
            entry.name = n.into();
            entry.group = group.into();
            // Gear is held, worn or flown by the hero code (its class-table entry, if any, is not what moves it): a
            // behaviour badge ("scenery", "unported") would mislead.
            if group == rc_map::catalog::GROUP_GEAR { entry.behaviour = None; }
        } else if ["bolt", "crate", "pickup", "nanotech", "ammo"].iter().any(|w| entry.port.to_lowercase().contains(w) || entry.name.contains(w)) {
            entry.group = rc_map::catalog::GROUP_PICKUPS.into();
        }
        entry.kind = kind_of(&entry, creatures.contains_key(&oc)).into();
        entry.collision = solid.contains(&o);
        out.push((content(format!("moby_class/{o:04}"), &e.textures, TextureTable::Moby), entry));
    }
    for e in &core.tie_classes {
        let mut entry = Entry::new(id(AssetKind::Tie, e.o_class));
        entry.placed = placed_ties.get(&e.o_class).copied().unwrap_or(0);
        entry.textures = used(&e.textures);
        out.push((content(format!("tie_class/{:04}", e.o_class), &e.textures, TextureTable::Tie), entry));
    }
    for e in &core.shrub_classes {
        let o = e.base.o_class;
        let mut entry = Entry::new(id(AssetKind::Shrub, o));
        entry.placed = placed_shrubs.get(&o).copied().unwrap_or(0);
        entry.textures = used(&e.base.textures);
        out.push((content(format!("shrub_class/{o:04}"), &e.base.textures, TextureTable::Shrub), entry));
    }
    if let Some(sky) = core.block(&data, "sky") {
        let mut entry = Entry::new(id(AssetKind::Sky, 0));
        entry.name = format!("{} sky", rc_map::LEVEL_NAMES[l as usize]);
        out.push((rc_data::xxh64::xxh64(sky), entry));
    }
    for t in textures.iter().filter(|t| t.table == TextureTable::Tfrag) {
        let mut entry = Entry::new(id(AssetKind::TfragTexture, t.index as i32));
        entry.name = format!("{}×{}", t.texture.width, t.texture.height);
        out.push((tex[&(t.table, t.index)], entry));
    }

    // Thumbnails: each new content of this level drawn from the level's own classes and textures.
    let ties: HashMap<i32, rc_formats::tie::LevelTieClass> = rc_formats::tie::parse_level_ties(&core, &data).map_or_else(|_| HashMap::new(), |v| v.into_iter().map(|c| (c.o_class, c)).collect());
    let shrubs: HashMap<i32, rc_formats::shrub::LevelShrubClass> = rc_formats::shrub::parse_level_shrubs(&core, &data).map_or_else(|_| HashMap::new(), |v| v.into_iter().map(|c| (c.o_class, c)).collect());
    let texs: HashMap<(TextureTable, usize), &rc_formats::texture::Texture> = textures.iter().map(|t| ((t.table, t.index), &t.texture)).collect();
    let lookup = |tb: TextureTable, i: usize| texs.get(&(tb, i)).copied();
    let means: HashMap<(TextureTable, usize), [f32; 3]> = textures
        .iter()
        .map(|t| {
            let px = t.texture.rgba.chunks(4).filter(|p| p[3] >= 0x20);
            let (mut sum, mut n) = ([0f32; 3], 0f32);
            for p in px { for k in 0..3 { sum[k] += p[k] as f32 / 255.0; } n += 1.0; }
            ((t.table, t.index), sum.map(|v| v / n.max(1.0)))
        })
        .collect();
    let mean = |tb: TextureTable, i: usize| means.get(&(tb, i)).copied();
    let planet = rc_map::LEVEL_NAMES[l as usize];
    // Names and tags from the model (every find: the entry that stays after the dedupe is any level's).
    for (_, e) in &mut out {
        use rc_extract::thumbs as th;
        let tris = match e.id.kind {
            AssetKind::Moby => models.get(&e.id.number).map(th::moby_tris),
            AssetKind::Tie => ties.get(&e.id.number).map(th::tie_tris),
            AssetKind::Shrub => shrubs.get(&e.id.number).map(th::shrub_tris),
            _ => None,
        };
        let Some((size, shape, colour)) = tris.as_deref().and_then(|t| describe(t, &mean)) else { continue };
        let word = match e.id.kind {
            AssetKind::Tie => shape,
            AssetKind::Shrub if matches!(colour, "green" | "teal" | "yellow") => "plant",
            AssetKind::Shrub => "detail",
            _ => "object",
        };
        if e.name.is_empty() { e.name = format!("{planet} {size} {word}"); }
        e.tags = vec![planet.to_lowercase(), size.into(), shape.into(), colour.into()];
    }
    for (hash, e) in &mut out {
        let kind = e.id.kind;
        e.thumb = format!("thumbs/{}/{hash:016x}.png", kind.name());
        if !thumbs.claim(kind, *hash, &e.thumb) { continue; }
        use rc_extract::thumbs as th;
        let img = match kind {
            AssetKind::Moby => models.get(&e.id.number).and_then(|m| th::render(&th::moby_tris(m), &lookup, THUMB_SIZE)),
            AssetKind::Tie => ties.get(&e.id.number).and_then(|t| th::render(&th::tie_tris(t), &lookup, THUMB_SIZE)),
            AssetKind::Shrub => shrubs.get(&e.id.number).and_then(|t| th::render(&th::shrub_tris(t), &lookup, THUMB_SIZE)),
            // A sky is seen from inside: its largest texture (the clouds or the backdrop).
            AssetKind::Sky => rc_formats::sky::sky_block(&core, &data).ok().flatten().and_then(|b| {
                let sky = rc_formats::sky::parse_sky_block(b).ok()?;
                let t = rc_formats::sky::parse_sky_textures(b, &sky).ok()?;
                t.iter().map(|t| &t.texture).max_by_key(|t| t.width * t.height).and_then(|t| th::texture_thumb(t, THUMB_SIZE))
            }),
            AssetKind::TfragTexture => lookup(TextureTable::Tfrag, e.id.number as usize).and_then(|t| th::texture_thumb(t, THUMB_SIZE)),
        };
        if let Some(rgba) = img { thumbs.write(&e.thumb, &rgba); }
    }
    Ok(out)
}

/// Builds the catalog of every level the data folder holds.
pub fn build(root: &Path) -> Result<Catalog> {
    let names = unit_names();
    let thumbs = Thumbs { dir: root.join("catalog"), claimed: Mutex::new(Default::default()) };
    let overlays: Overlays = Mutex::new(HashMap::new());
    let levels: Vec<u32> = (0..19).filter(|&l| crate::disc_source::level_file(root, l, "core_index.bin").is_ok()).collect();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 8);
    let next = Mutex::new(levels.iter().copied());
    let results: Mutex<Vec<(u32, Result<Vec<(u64, Entry)>>)>> = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let Some(l) = next.lock().unwrap().next() else { break };
                let t0 = Instant::now();
                let r = level_finds(root, l, &overlays, &names, &thumbs);
                println!("catalog: level {l:02} {}: {} in {:.1} s", rc_map::LEVEL_NAMES[l as usize], r.as_ref().map_or("failed".into(), |v| format!("{} assets", v.len())), t0.elapsed().as_secs_f64());
                // The level's lumps are not needed again (each level is read once).
                rc_data::global().forget_level(root, l);
                results.lock().unwrap().push((l, r));
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|r| r.0);
    let mut finds = Vec::new();
    for (l, r) in results { finds.extend(r.with_context(|| format!("level {l:02}"))?); }
    // Kind first, then level and number: the browser's tabs, each in level order.
    finds.sort_by_key(|(_, e)| (e.id.kind, e.id.level, e.id.number));
    let mut entries = catalog::dedupe(finds);
    // Classes without geometry (logic-only mobys) have no thumbnail.
    for e in &mut entries { if !thumbs.dir.join(&e.thumb).exists() { e.thumb.clear(); } }
    Ok(Catalog { rerac: env!("CARGO_PKG_VERSION").to_string(), entries })
}

/// The catalog page (`index.html`): a browser over the entries and their thumbnails, the catalog embedded (a page opened
/// from disk cannot fetch the JSON beside it).
const VIEWER: &str = include_str!("catalog_viewer.html");

/// Writes `catalog.json` and `index.html` into `dir`.
pub fn write(dir: &Path, c: &Catalog) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let json = c.to_json();
    std::fs::write(dir.join("catalog.json"), &json)?;
    std::fs::write(dir.join("index.html"), VIEWER.replace("/*CATALOG*/", &format!("const CATALOG = {};", json.replace("</", "<\\/"))))
}

/// The spawn test's verdicts as run here (`catalog_live.tsv`: asset id, `live` or `needs-work`, its note), so a catalog
/// built on a PC that never ran `--spawn-test` knows which objects work on maps.
fn shipped_verdicts() -> HashMap<&'static str, (catalog::Live, &'static str)> {
    include_str!("catalog_live.tsv")
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let mut f = l.splitn(3, '\t');
            let (id, live, note) = (f.next()?, f.next()?, f.next().unwrap_or(""));
            let live = match live { "live" => catalog::Live::Live, "needs-work" => catalog::Live::NeedsWork, _ => return None };
            Some((id, (live, note)))
        })
        .collect()
}

/// `--build-catalog`: builds and writes `<data>/catalog/catalog.json`, prints a summary, returns the exit code.
pub fn build_and_write(root: &Path) -> i32 {
    let t0 = Instant::now();
    let mut c = match build(root) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: catalog not built: {e:#}");
            return 1;
        }
    };
    let dir = root.join("catalog");
    let path = dir.join("catalog.json");
    // The spawn test's verdicts of the catalog this one replaces stay (same asset, same content: same thumbnail).
    if let Some(old) = std::fs::read_to_string(&path).ok().and_then(|t| Catalog::parse(&t).ok()) {
        for e in &mut c.entries {
            if let Some(o) = old.find(&e.id).filter(|o| o.thumb == e.thumb && o.live.is_some()) { (e.live, e.live_note) = (o.live, o.live_note.clone()); }
        }
    }
    // The rest from the verdicts shipped with the build (a first catalog on another PC: the editor's "Live only").
    let shipped = shipped_verdicts();
    for e in c.entries.iter_mut().filter(|e| e.live.is_none()) {
        if let Some((live, note)) = shipped.get(e.id.to_string().as_str()) { (e.live, e.live_note) = (Some(*live), note.to_string()); }
    }
    if let Err(e) = write(&dir, &c) {
        eprintln!("error: writing {}: {e}", path.display());
        return 1;
    }
    let mobys: Vec<&Entry> = c.entries.iter().filter(|e| e.id.kind == AssetKind::Moby).collect();
    let by = |b: Behaviour| mobys.iter().filter(|e| e.behaviour == Some(b)).count();
    println!(
        "catalog: {} entries ({}) in {:.1} s -> {}",
        c.entries.len(),
        c.counts().iter().map(|(k, n)| format!("{} {n}", k.name())).collect::<Vec<_>>().join(", "),
        t0.elapsed().as_secs_f64(),
        path.display()
    );
    println!("catalog: mobys {} ported, {} static, {} unported", by(Behaviour::Ported), by(Behaviour::Static), by(Behaviour::Unported));
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_names_follow_the_ports() {
        let n = unit_names();
        assert_eq!(n.modules.len(), units::PORTS.len());
        // U301 is two units (the pack biters and Gaspar's meteor shower): the module tells them apart.
        let biter = units::PORTS.iter().position(|p| p.classes.contains(&193) && p.level == 9).expect("the pack biter's port");
        assert_eq!(port_name(ClassUpdate::Unit(biter as u16), &n).1, "pack biter");
        assert_eq!(n.about.get("eudora_brawler").map(String::as_str), Some("Eudora's brawler bots"));
    }

    #[test]
    fn shipped_verdicts_read() {
        let v = shipped_verdicts();
        assert!(v.len() > 500, "{} verdicts", v.len());
        assert!(v.values().filter(|x| x.0 == catalog::Live::Live).count() > 500);
        assert!(v.keys().all(|k| k.starts_with("rac1/L") && k.contains("/moby/")));
    }
}

#[cfg(test)]
mod describe_tests {
    use super::*;
    use rc_extract::thumbs::Tri;

    #[test]
    fn names_colours_and_shapes() {
        assert_eq!(colour_name([0.2, 0.6, 0.2]), "green");
        assert_eq!(colour_name([0.2, 0.3, 0.8]), "blue");
        assert_eq!(colour_name([0.5, 0.5, 0.52]), "grey");
        assert_eq!(colour_name([0.45, 0.28, 0.15]), "brown");
        let tri = |h: f32| Tri { p: [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, h]], uv: [[0.0; 2]; 3], tex: None };
        assert_eq!(describe(&[tri(12.0)], &|_, _| None), Some(("large", "column", "grey")));
        assert_eq!(describe(&[tri(0.1)], &|_, _| None), Some(("small", "platform", "grey")));
        assert_eq!(describe(&[], &|_, _| None), None);
    }
}
