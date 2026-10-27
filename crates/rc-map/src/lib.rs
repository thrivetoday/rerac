//! RAC Maker custom maps (docs/plan/rac_maker.md; the design doc is linked from there).
//!
//! A map is a JSON file the editor writes and the engine plays with `rerac --map <file>`. It holds only the map's own
//! work (terrain, layout, rails, settings) and **references** to game assets ([`AssetId`], e.g. `rac1/L09/moby/193`):
//! the models, textures and behaviours come from the player's own extracted data, never from the map.
//!
//! A map plays on a **host level** (Novalis by default): the engine loads the host as usual, then replaces its world
//! content with the map's ([`patch`]: the gameplay sections; [`floor`]: the collision). Objects from other levels are
//! **imports** ([`MapDef::imports`]): their classes are loaded from their own level's data next to the host's.
//!
//! Phase 0 scope: flat floors, grind rails, objects copied from a template instance of their source level (its record
//! and pvars), a start point, a loadout and the hero's rail / cable switches.

pub mod catalog;
pub mod floor;
pub mod json;
pub mod patch;
pub mod coast;
pub mod logic;
pub mod terrain;
pub mod walkway;

use json::Value;

/// The map file format version this build reads (`"maker_format"`).
pub const MAKER_FORMAT: i64 = 1;

/// Game coordinates the collision queries cover: every axis in `[0, 1024)` (rc_game::collision_query).
pub const WORLD_SIZE: f32 = 1024.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapError(pub String);

impl std::fmt::Display for MapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) }
}

impl std::error::Error for MapError {}

pub type Result<T> = std::result::Result<T, MapError>;

pub(crate) fn err<T>(msg: impl Into<String>) -> Result<T> { Err(MapError(msg.into())) }

/// The games asset ids can name. Only RAC1 has data in ReRAC today; the others are reserved so map files and the
/// catalog keep one id scheme when they arrive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Game {
    Rac1,
}

impl Game {
    pub fn name(self) -> &'static str {
        match self {
            Game::Rac1 => "rac1",
        }
    }
    fn parse(s: &str) -> Result<Game> {
        match s {
            "rac1" => Ok(Game::Rac1),
            "gc" | "uya" | "dl" => err(format!("asset game {s:?} is not supported by this build (RAC1 only)")),
            _ => err(format!("unknown asset game {s:?}")),
        }
    }
}

/// The RAC1 levels by index, as players know them (the catalog and the editor show these).
pub const LEVEL_NAMES: [&str; 19] = [
    "Veldin", "Novalis", "Aridia", "Kerwan", "Eudora", "Rilgar", "Blarg Station", "Umbris", "Batalia", "Gaspar",
    "Orxon", "Pokitaru", "Hoven", "Gemlik Base", "Oltanis", "Quartu", "Kalebo III", "Drek's Fleet", "Veldin (finale)",
];

/// What an asset id names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AssetKind {
    /// A moby class (`number` = o_class): objects with behaviour (enemies, crates, platforms, pickups, …).
    Moby,
    /// A tie class (`number` = o_class): large static models placed many times (buildings, pipes, rocks).
    Tie,
    /// A shrub class (`number` = o_class): small detail and plants, with far billboards.
    Shrub,
    /// The level's sky (`number` = 0).
    Sky,
    /// A texture of the level's tfrag table (`number` = table index).
    TfragTexture,
}

impl AssetKind {
    pub const ALL: [AssetKind; 5] = [AssetKind::Moby, AssetKind::Tie, AssetKind::Shrub, AssetKind::Sky, AssetKind::TfragTexture];

    pub fn name(self) -> &'static str {
        match self {
            AssetKind::Moby => "moby",
            AssetKind::Tie => "tie",
            AssetKind::Shrub => "shrub",
            AssetKind::Sky => "sky",
            AssetKind::TfragTexture => "tfrag-tex",
        }
    }

    pub fn parse(s: &str) -> Option<AssetKind> { AssetKind::ALL.into_iter().find(|k| k.name() == s) }
}

/// `game/Lnn/kind/number`: a game asset on one source level, e.g. `rac1/L09/moby/193`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AssetId {
    pub game: Game,
    pub level: u32,
    pub kind: AssetKind,
    pub number: i32,
}

impl AssetId {
    pub fn parse(s: &str) -> Result<AssetId> {
        let parts: Vec<&str> = s.split('/').collect();
        let [game, level, kind, number] = parts[..] else { return err(format!("asset id {s:?}: expected game/Lnn/kind/number")) };
        let game = Game::parse(game)?;
        let level = level.strip_prefix('L').and_then(|l| l.parse::<u32>().ok()).filter(|&l| l < 19);
        let Some(level) = level else { return err(format!("asset id {s:?}: level must be L00..L18")) };
        let Some(kind) = AssetKind::parse(kind) else {
            return err(format!("asset id {s:?}: unknown kind {kind:?} (moby, tie, shrub, sky, tfrag-tex)"));
        };
        let Ok(number) = number.parse::<i32>() else { return err(format!("asset id {s:?}: bad number")) };
        Ok(AssetId { game, level, kind, number })
    }
}

impl std::fmt::Display for AssetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/L{:02}/{}/{}", self.game.name(), self.level, self.kind.name(), self.number)
    }
}

/// Where Ratchet starts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Start {
    pub position: [f32; 3],
    pub yaw: f32,
}

/// The hero's map ability profile: `None` keeps the host level's rule (rc_game::hero::Abilities).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Abilities {
    pub rails: Option<bool>,
    pub cables: Option<bool>,
}

/// A flat, square-celled floor: `min`/`max` are multiples of the collision cell (4 units), `height` a multiple of
/// 1/64 (the packed collision vertex precision).
#[derive(Clone, Debug, PartialEq)]
pub struct Floor {
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub height: f32,
    /// The texture drawn on it (None: a plain colour).
    pub texture: Option<AssetId>,
    /// World units per texture repeat.
    pub tile: f32,
}

/// A grind rail through its points (an open spline).
#[derive(Clone, Debug, PartialEq)]
pub struct Rail {
    pub points: Vec<[f32; 3]>,
    /// A smooth curve through the points (else straight lines between them).
    pub smooth: bool,
    /// How it looks: a planet's grind rail repeated along it (`"look"`: a key of the engine's rail looks); None: a
    /// plain beam.
    pub look: Option<String>,
    /// Ridden only from its first point to its last (`"one_way"`); else both ways (the game's rails are one way: a
    /// map's two-way rail is the rail and its reverse, the one Ratchet heads along taken).
    pub one_way: bool,
}

impl Rail {
    /// The line Ratchet grinds: the points, or the smooth curve through them.
    pub fn line(&self) -> Vec<[f32; 3]> { if self.smooth { walkway::curve(&self.points) } else { self.points.clone() } }
}

/// A placed object: a moby class of some level, copied from one of its instances there.
#[derive(Clone, Debug, PartialEq)]
pub struct Object {
    pub asset: AssetId,
    pub position: [f32; 3],
    pub yaw: f32,
    /// The source level's instance whose record and pvars are the template (None: the class's first instance).
    pub template: Option<usize>,
    /// Its moby group (instance +0x48: crates of one group stack on each other and fall when one under them breaks;
    /// the editor groups touching crates). None: alone.
    pub group: Option<u8>,
    /// The path it moves along (a mover's route, an enemy's patrol), its points in order (empty: its template's,
    /// which names a spline of its source level the map does not have).
    pub path: Vec<[f32; 3]>,
    /// A mover's travel time along its path, seconds (None: the template's).
    pub time: Option<f32>,
    /// Solid from its model: its triangles baked into the map's collision (for classes without collision of their
    /// own, whose level geometry made them solid on the disc). None: the editor decides.
    pub solid: Option<bool>,
    /// Its name, for the map's rules (crate::logic): unique on the map.
    pub name: Option<String>,
    /// Hidden until a rule shows it.
    pub hidden: bool,
}

/// One map file.
#[derive(Clone, Debug, PartialEq)]
pub struct MapDef {
    pub name: String,
    pub host_level: u32,
    pub start: Start,
    /// Ratchet's ship (`"ship"`): the level starts with it landing there and ends when he boards it after the goal;
    /// his start is then at its hatch (the engine places him). None: no ship, the start as given.
    pub ship: Option<Start>,
    /// Below this height Ratchet dies (level settings +0x28); None keeps the host's.
    pub death_height: Option<f32>,
    /// Item ids Ratchet owns from the start (docs/plan/gadgets.md: 29 Grindboots, 12 Swingshot, …).
    pub loadout: Vec<usize>,
    pub abilities: Abilities,
    /// A death brings everything back as at the first load (`"reset_on_death"`, default true: handy while building a
    /// map). False keeps the game's rule: what was broken or killed this visit stays gone after a death.
    pub reset_on_death: bool,
    pub floors: Vec<Floor>,
    /// Sculpted terrain (`"kind": "patch"` in `"terrain"`).
    pub patches: Vec<terrain::Patch>,
    /// Walkways and bridges (`"kind": "walkway"` in `"terrain"`).
    pub walkways: Vec<walkway::Walkway>,
    /// Checkpoints, the goal, death zones, camera zones, rule triggers (crate::logic).
    pub zones: Vec<logic::Zone>,
    /// When/then rules (crate::logic).
    pub rules: Vec<logic::Rule>,
    /// The sky, light, fog and song (`"world"`).
    pub world: World,
    pub rails: Vec<Rail>,
    pub objects: Vec<Object>,
}

/// The world of a map (`"world"`): another planet's sky and light, a fog, the song.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct World {
    /// The sky (and its background colour and fog) of this level; None: the host's.
    pub sky: Option<u32>,
    /// The song: (level, its start track: the start, then the loop after it); None: the host's first song.
    pub music: Option<(u32, u32)>,
    /// No song at all.
    pub silent: bool,
    /// The fog over the sky level's (its colour, where it starts and where it is full).
    pub fog: Option<Fog>,
    /// The sky moved: raised (degrees, + up) and turned about the up axis (degrees).
    pub sky_raise: f32,
    pub sky_turn: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fog {
    pub color: [u8; 3],
    pub near: f32,
    pub far: f32,
}

/// An edge (`"edge"` of a patch or a walkway).
fn parse_edge(e: &Value, what: &str) -> Result<terrain::Edge> {
    let kind = if let Some(id) = e.get("piece").and_then(Value::as_str) {
        terrain::EdgeKind::Piece(AssetId::parse(id)?)
    } else if let Some(w) = e.get("wall") {
        let texture = w.get("texture").and_then(Value::as_str).map(AssetId::parse).transpose()?;
        terrain::EdgeKind::Wall { texture, tile: w.get("tile").and_then(Value::as_f32).unwrap_or(4.0) }
    } else {
        terrain::EdgeKind::Invisible
    };
    let gaps = e.get("gaps").and_then(Value::as_arr).unwrap_or(&[]).iter()
        .map(|g| g.as_f32s::<3>().ok_or_else(|| MapError(format!("{what}.edge.gaps: expected [x, y, radius]"))))
        .collect::<Result<Vec<[f32; 3]>>>()?;
    Ok(terrain::Edge {
        kind,
        height: e.get("height").and_then(Value::as_f32).unwrap_or(1.2),
        gaps,
        step: e.get("step").and_then(Value::as_f32).unwrap_or(0.0).max(0.0),
        inset: e.get("inset").and_then(Value::as_f32).unwrap_or(0.0).clamp(0.0, 4.0),
    })
}

/// A number as the map file writes it (always with a decimal point).
fn num(v: f32) -> String { let s = format!("{v}"); if s.contains('.') || s.contains('e') || s.contains("inf") || s.contains("NaN") { s } else { format!("{s}.0") } }

/// An edge as the map file writes it (`, "edge": {...}`).
fn edge_json(e: &terrain::Edge) -> String {
    let q = |s: &str| format!("{:?}", s);
    let kind = match &e.kind {
        terrain::EdgeKind::Invisible => String::new(),
        terrain::EdgeKind::Piece(t) => format!("\"piece\": {}, ", q(&t.to_string())),
        terrain::EdgeKind::Wall { texture, tile } => {
            let tex = texture.map(|t| format!("\"texture\": {}, ", q(&t.to_string()))).unwrap_or_default();
            format!("\"wall\": {{ {tex}\"tile\": {} }}, ", num(*tile))
        }
    };
    let gaps = if e.gaps.is_empty() { String::new() } else {
        format!(", \"gaps\": [{}]", e.gaps.iter().map(|g| format!("[{}, {}, {}]", num(g[0]), num(g[1]), num(g[2]))).collect::<Vec<_>>().join(", "))
    };
    let fit = if e.step > 0.0 { format!(", \"step\": {}, \"inset\": {}", num(e.step), num(e.inset)) } else { String::new() };
    format!(",\n      \"edge\": {{ {kind}\"height\": {}{fit}{gaps} }}", num(e.height))
}

/// A surface byte (`"surface"`, plain ground without one).
fn parse_surface(f: &Value, what: &str) -> Result<u8> {
    f.get("surface").map_or(Ok(floor::GROUND), |s| s.as_i64().filter(|s| (0..=0x7f).contains(s)).map(|s| s as u8).ok_or_else(|| MapError(format!("{what}.surface: expected 0..127"))))
}

fn in_world(p: &[f32]) -> bool { p.iter().all(|&c| c.is_finite() && (0.0..WORLD_SIZE).contains(&c)) }

fn f3(v: Option<&Value>, what: &str) -> Result<[f32; 3]> {
    let p = v.and_then(Value::as_f32s::<3>).ok_or_else(|| MapError(format!("{what}: expected [x, y, z]")))?;
    if !in_world(&p) { return err(format!("{what}: {p:?} is outside the world (0..1024 on every axis)")); }
    Ok(p)
}

impl MapDef {
    /// Reads a map file's text.
    pub fn parse(text: &str) -> Result<MapDef> {
        let v = json::parse(text).map_err(MapError)?;
        let fmt = v.get("maker_format").and_then(Value::as_i64);
        if fmt != Some(MAKER_FORMAT) { return err(format!("maker_format {fmt:?}: this build reads format {MAKER_FORMAT}")); }
        if let Some(g) = v.get("game") {
            Game::parse(g.as_str().ok_or_else(|| MapError("game: expected a string".into()))?)?;
        }
        let name = v.get("name").and_then(Value::as_str).unwrap_or("Untitled map").to_string();
        let host_level = match v.get("host_level") {
            None => 1,
            Some(h) => h.as_i64().filter(|&l| (0..19).contains(&l)).ok_or_else(|| MapError("host_level: expected 0..18".into()))? as u32,
        };
        let s = v.get("start").ok_or_else(|| MapError("start: missing".into()))?;
        let start = Start { position: f3(s.get("position"), "start.position")?, yaw: s.get("yaw").and_then(Value::as_f32).unwrap_or(0.0) };
        let ship = v.get("ship").map(|s| -> Result<Start> { Ok(Start { position: f3(s.get("position"), "ship.position")?, yaw: s.get("yaw").and_then(Value::as_f32).unwrap_or(0.0) }) }).transpose()?;
        let death_height = v.get("death_height").map(|d| d.as_f32().ok_or_else(|| MapError("death_height: expected a number".into()))).transpose()?;
        let loadout = match v.get("loadout") {
            None => Vec::new(),
            Some(l) => l
                .as_arr()
                .ok_or_else(|| MapError("loadout: expected an array of item ids".into()))?
                .iter()
                .map(|i| i.as_i64().filter(|&i| (0..64).contains(&i)).map(|i| i as usize).ok_or_else(|| MapError(format!("loadout: bad item id {i:?}"))))
                .collect::<Result<_>>()?,
        };
        let abilities = match v.get("abilities") {
            None => Abilities::default(),
            Some(a) => Abilities { rails: a.get("rails").and_then(Value::as_bool), cables: a.get("cables").and_then(Value::as_bool) },
        };
        let reset_on_death = match v.get("reset_on_death") {
            None => true,
            Some(r) => r.as_bool().ok_or_else(|| MapError("reset_on_death: expected true or false".into()))?,
        };
        let mut floors = Vec::new();
        let mut patches = Vec::new();
        let mut walkways = Vec::new();
        for (i, f) in v.get("terrain").and_then(Value::as_arr).unwrap_or(&[]).iter().enumerate() {
            let what = format!("terrain[{i}]");
            let texture = f.get("texture").and_then(Value::as_str).map(AssetId::parse).transpose()?;
            if texture.is_some_and(|t| t.kind != AssetKind::TfragTexture) { return err(format!("{what}.texture: expected a tfrag-tex asset")); }
            let tile = f.get("tile").and_then(Value::as_f32).unwrap_or(8.0);
            match f.get("kind").and_then(Value::as_str) {
                Some("floor") => {}
                Some("patch") => {
                    let origin = f3(f.get("origin"), &format!("{what}.origin"))?;
                    let spacing = f.get("spacing").and_then(Value::as_f32).unwrap_or(2.0);
                    let size = f.get("size").and_then(Value::as_f32s::<2>).filter(|s| s.iter().all(|&c| c.fract() == 0.0 && c >= 1.0 && c <= 65535.0))
                        .ok_or_else(|| MapError(format!("{what}.size: expected [cells along x, cells along y]")))?;
                    let heights = f.get("heights").and_then(Value::as_arr).ok_or_else(|| MapError(format!("{what}.heights: missing")))?
                        .iter().map(|h| h.as_f32().ok_or_else(|| MapError(format!("{what}.heights: expected numbers")))).collect::<Result<Vec<f32>>>()?;
                    let depth = f.get("depth").and_then(Value::as_f32).unwrap_or(0.0);
                    let mut layers = Vec::new();
                    for (k, l) in f.get("layers").and_then(Value::as_arr).unwrap_or(&[]).iter().enumerate() {
                        let texture = AssetId::parse(l.get("texture").and_then(Value::as_str).ok_or_else(|| MapError(format!("{what}.layers[{k}].texture: missing")))?)?;
                        if texture.kind != AssetKind::TfragTexture { return err(format!("{what}.layers[{k}].texture: expected a tfrag-tex asset")); }
                        let weights = l.get("weights").and_then(Value::as_arr).ok_or_else(|| MapError(format!("{what}.layers[{k}].weights: missing")))?
                            .iter().map(|w| w.as_i64().filter(|w| (0..=255).contains(w)).map(|w| w as u8).ok_or_else(|| MapError(format!("{what}.layers[{k}].weights: expected 0..255"))))
                            .collect::<Result<Vec<u8>>>()?;
                        layers.push(terrain::PaintLayer { texture, tile: l.get("tile").and_then(Value::as_f32).unwrap_or(8.0), weights });
                    }
                    let mask = match f.get("mask").and_then(Value::as_arr) {
                        None => Vec::new(),
                        Some(m) => m.iter().map(|v| v.as_f32().map(|v| v / 8.0).ok_or_else(|| MapError(format!("{what}.mask: expected numbers")))).collect::<Result<Vec<f32>>>()?,
                    };
                    let edge = f.get("edge").map(|e| parse_edge(e, &what)).transpose()?;
                    let surface = parse_surface(f, &what)?;
                    let shade = f.get("shade").and_then(Value::as_arr).unwrap_or(&[]).iter()
                        .map(|c| c.as_i64().filter(|c| (0..=0xffffff).contains(c)).map(|c| c as u32).ok_or_else(|| MapError(format!("{what}.shade: expected colours 0..0xffffff"))))
                        .collect::<Result<Vec<u32>>>()?;
                    let mut scatter = Vec::new();
                    for (k, s) in f.get("scatter").and_then(Value::as_arr).unwrap_or(&[]).iter().enumerate() {
                        let asset = AssetId::parse(s.get("asset").and_then(Value::as_str).ok_or_else(|| MapError(format!("{what}.scatter[{k}].asset: missing")))?)?;
                        let at = s.get("at").and_then(Value::as_f32s::<2>).ok_or_else(|| MapError(format!("{what}.scatter[{k}].at: expected [x, y]")))?;
                        scatter.push(terrain::Scatter { asset, at, yaw: s.get("yaw").and_then(Value::as_f32).unwrap_or(0.0), scale: s.get("scale").and_then(Value::as_f32).unwrap_or(1.0) });
                    }
                    let roof = match f.get("roof") {
                        None => None,
                        Some(r) => Some(terrain::Roof {
                            height: r.get("height").and_then(Value::as_f32).unwrap_or(6.0),
                            texture: r.get("texture").and_then(Value::as_str).map(AssetId::parse).transpose()?,
                            tile: r.get("tile").and_then(Value::as_f32).unwrap_or(8.0),
                        }),
                    };
                    let mut p = terrain::Patch { origin, spacing, size: [size[0] as u16, size[1] as u16], heights, texture, tile, depth, layers, mask, edge, roof, surface, shade, scatter };
                    p.quantize_mask();
                    p.quantize();
                    p.check().map_err(|e| MapError(format!("{what}: {e}")))?;
                    patches.push(p);
                    continue;
                }
                Some("walkway") => {
                    let pts = f.get("points").and_then(Value::as_arr).ok_or_else(|| MapError(format!("{what}.points: missing")))?;
                    let points = pts.iter().enumerate().map(|(k, p)| f3(Some(p), &format!("{what}.points[{k}]"))).collect::<Result<Vec<_>>>()?;
                    let w = walkway::Walkway {
                        points,
                        width: f.get("width").and_then(Value::as_f32).unwrap_or(3.0),
                        depth: f.get("depth").and_then(Value::as_f32).unwrap_or(1.0),
                        texture,
                        tile: f.get("tile").and_then(Value::as_f32).unwrap_or(4.0),
                        surface: parse_surface(f, &what)?,
                        edge: f.get("edge").map(|e| parse_edge(e, &what)).transpose()?,
                        cut: f.get("cut").and_then(Value::as_bool).unwrap_or(false),
                    };
                    w.check().map_err(|e| MapError(format!("{what}: {e}")))?;
                    walkways.push(w);
                    continue;
                }
                k => return err(format!("{what}: kind {k:?}: expected \"floor\", \"patch\" or \"walkway\"")),
            }
            let min = f.get("min").and_then(Value::as_f32s::<2>).ok_or_else(|| MapError(format!("{what}.min: expected [x, y]")))?;
            let max = f.get("max").and_then(Value::as_f32s::<2>).ok_or_else(|| MapError(format!("{what}.max: expected [x, y]")))?;
            let height = f.get("height").and_then(Value::as_f32).ok_or_else(|| MapError(format!("{what}.height: expected a number")))?;
            let floor = Floor { min, max, height, texture, tile };
            floor::check(&floor).map_err(|e| MapError(format!("{what}: {e}")))?;
            floors.push(floor);
        }
        let mut rails = Vec::new();
        for (i, r) in v.get("rails").and_then(Value::as_arr).unwrap_or(&[]).iter().enumerate() {
            let pts = r.get("points").and_then(Value::as_arr).ok_or_else(|| MapError(format!("rails[{i}].points: missing")))?;
            let points = pts.iter().enumerate().map(|(k, p)| f3(Some(p), &format!("rails[{i}].points[{k}]"))).collect::<Result<Vec<_>>>()?;
            if points.len() < 2 { return err(format!("rails[{i}]: needs at least 2 points")); }
            rails.push(Rail { points, smooth: r.get("smooth").and_then(Value::as_bool).unwrap_or(false), look: r.get("look").and_then(Value::as_str).map(str::to_string), one_way: r.get("one_way").and_then(Value::as_bool).unwrap_or(false) });
        }
        let mut objects = Vec::new();
        for (i, o) in v.get("objects").and_then(Value::as_arr).unwrap_or(&[]).iter().enumerate() {
            let what = format!("objects[{i}]");
            let asset = AssetId::parse(o.get("asset").and_then(Value::as_str).ok_or_else(|| MapError(format!("{what}.asset: missing")))?)?;
            if asset.kind != AssetKind::Moby { return err(format!("{what}.asset: expected a moby asset, got {asset}")); }
            if asset.number == 0 { return err(format!("{what}: Ratchet (class 0) is placed by \"start\"")); }
            let template = o.get("template").map(|t| t.as_i64().filter(|&t| t >= 0).map(|t| t as usize).ok_or_else(|| MapError(format!("{what}.template: expected an instance index")))).transpose()?;
            let group = o.get("group").map(|g| g.as_i64().filter(|g| (0..=126).contains(g)).map(|g| g as u8).ok_or_else(|| MapError(format!("{what}.group: expected 0..126")))).transpose()?;
            let path = o.get("path").and_then(Value::as_arr).unwrap_or(&[]).iter().enumerate().map(|(k, p)| f3(Some(p), &format!("{what}.path[{k}]"))).collect::<Result<Vec<_>>>()?;
            if path.len() == 1 { return err(format!("{what}.path: needs at least 2 points")); }
            let time = o.get("time").map(|t| t.as_f32().filter(|&t| t > 0.0).ok_or_else(|| MapError(format!("{what}.time: expected seconds above 0")))).transpose()?;
            let solid = o.get("solid").and_then(Value::as_bool);
            let name = o.get("name").and_then(Value::as_str).map(str::to_string).filter(|n| !n.trim().is_empty());
            let hidden = o.get("hidden").and_then(Value::as_bool).unwrap_or(false);
            objects.push(Object { asset, position: f3(o.get("position"), &format!("{what}.position"))?, yaw: o.get("yaw").and_then(Value::as_f32).unwrap_or(0.0), template, group, path, time, solid, name, hidden });
        }
        let mut zones = Vec::new();
        for (i, z) in v.get("zones").and_then(Value::as_arr).unwrap_or(&[]).iter().enumerate() {
            let what = format!("zones[{i}]");
            let kind = match z.get("kind").and_then(Value::as_str) {
                Some("trigger") => logic::ZoneKind::Trigger,
                Some("checkpoint") => logic::ZoneKind::Checkpoint,
                Some("goal") => logic::ZoneKind::Goal,
                Some("death") => logic::ZoneKind::Death,
                Some("camera") => logic::ZoneKind::Camera { eye: f3(z.get("eye"), &format!("{what}.eye"))? },
                k => return err(format!("{what}.kind {k:?}: expected one of {:?}", logic::ZoneKind::NAMES)),
            };
            let size = z.get("size").and_then(Value::as_f32s::<3>).ok_or_else(|| MapError(format!("{what}.size: expected [x, y, z]")))?;
            let zone = logic::Zone {
                name: z.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                kind,
                centre: f3(z.get("centre"), &format!("{what}.centre"))?,
                half: size.map(|s| s / 2.0),
                yaw: z.get("yaw").and_then(Value::as_f32).unwrap_or(0.0),
            };
            zone.check().map_err(|e| MapError(format!("{what}: {e}")))?;
            zones.push(zone);
        }
        let mut rules = Vec::new();
        for (i, r) in v.get("rules").and_then(Value::as_arr).unwrap_or(&[]).iter().enumerate() {
            let what = format!("rules[{i}]");
            let w = r.get("when").ok_or_else(|| MapError(format!("{what}.when: missing")))?;
            let s = |k: &str| w.get(k).and_then(Value::as_str).map(str::to_string);
            let when = if let Some(z) = s("enters") { logic::When::Enters(z) } else if let Some(z) = s("cleared") { logic::When::Cleared(z) } else if let Some(o) = s("destroyed") { logic::When::Destroyed(o) } else {
                return err(format!("{what}.when: expected enters, cleared or destroyed"));
            };
            let mut then = Vec::new();
            for (k, t) in r.get("then").and_then(Value::as_arr).unwrap_or(&[]).iter().enumerate() {
                let s = |key: &str| t.get(key).and_then(Value::as_str).map(str::to_string);
                then.push(if let Some(o) = s("remove") { logic::Then::Remove(o) } else if let Some(o) = s("show") { logic::Then::Show(o) } else if let Some(m) = s("message") { logic::Then::Message(m) } else if let Some(z) = s("checkpoint") { logic::Then::Checkpoint(z) } else if t.get("goal").is_some() { logic::Then::Goal } else {
                    return err(format!("{what}.then[{k}]: expected remove, show, message, checkpoint or goal"));
                });
            }
            rules.push(logic::Rule { when, then, cutscene: r.get("cutscene").and_then(Value::as_bool).unwrap_or(false) });
        }
        let mut world = World::default();
        if let Some(w) = v.get("world") {
            let level = |x: Option<&Value>, what: &str| -> Result<Option<u32>> {
                match x.and_then(Value::as_i64) {
                    None => Ok(None),
                    Some(l) if (0..19).contains(&l) => Ok(Some(l as u32)),
                    Some(l) => err(format!("world.{what}: level {l} must be 0..18")),
                }
            };
            world.sky = level(w.get("sky"), "sky")?;
            world.music = match w.get("music").and_then(Value::as_arr) {
                Some([l, t]) => match (l.as_i64(), t.as_i64()) {
                    (Some(l), Some(t)) if (0..19).contains(&l) && (0..14).contains(&t) => Some((l as u32, t as u32)),
                    _ => return err("world.music: expected [level 0..18, start track 0..13]"),
                },
                Some(_) => return err("world.music: expected [level, start track]"),
                None => None,
            };
            if let Some(f) = w.get("fog") {
                let c = f.get("color").and_then(Value::as_f32s::<3>).ok_or_else(|| MapError("world.fog.color: expected [r, g, b]".into()))?;
                world.fog = Some(Fog {
                    color: c.map(|x| x.clamp(0.0, 255.0) as u8),
                    near: f.get("near").and_then(Value::as_f32).unwrap_or(30.0),
                    far: f.get("far").and_then(Value::as_f32).unwrap_or(200.0),
                });
            }
            world.silent = w.get("silent").and_then(Value::as_bool).unwrap_or(false);
            world.sky_raise = w.get("sky_raise").and_then(Value::as_f32).unwrap_or(0.0).clamp(-45.0, 45.0);
            world.sky_turn = w.get("sky_turn").and_then(Value::as_f32).unwrap_or(0.0).rem_euclid(360.0);
        }
        Ok(MapDef { name, host_level, start, ship, death_height, loadout, abilities, reset_on_death, floors, patches, walkways, zones, rules, world, rails, objects })
    }

    /// A new map: an 80 × 80 ground patch at height 60 on `host_level`, Ratchet in its middle, the rails on, nothing placed.
    pub fn new_default(name: &str, host_level: u32) -> MapDef {
        MapDef {
            name: name.to_string(),
            host_level,
            start: Start { position: [168.0, 144.0, 60.5], yaw: 0.0 },
            ship: None,
            death_height: Some(40.0),
            loadout: vec![29],
            abilities: Abilities { rails: Some(true), cables: None },
            reset_on_death: true,
            floors: Vec::new(),
            // The ground: an 80 × 80 terrain patch (sculptable) of Novalis's grass (tfrag texture 8).
            patches: vec![{
                let mut g = terrain::Patch::flat([128.0, 104.0, 60.0], 2.0, [40, 40]);
                g.texture = Some(AssetId { game: Game::Rac1, level: 1, kind: AssetKind::TfragTexture, number: 8 });
                g
            }],
            walkways: Vec::new(),
            zones: Vec::new(),
            rules: Vec::new(),
            world: World::default(),
            rails: Vec::new(),
            objects: Vec::new(),
        }
    }

    /// The map file's text ([`MapDef::parse`] reads it back).
    pub fn to_json(&self) -> String {
        let n = |v: f32| { let s = format!("{v}"); if s.contains('.') || s.contains('e') || s.contains("inf") || s.contains("NaN") { s } else { format!("{s}.0") } };
        let v3 = |p: [f32; 3]| format!("[{}, {}, {}]", n(p[0]), n(p[1]), n(p[2]));
        let q = |s: &str| format!("{:?}", s);
        let mut o = format!("{{
  \"maker_format\": {MAKER_FORMAT},
  \"name\": {},
  \"game\": \"rac1\",
  \"host_level\": {},
", q(&self.name), self.host_level);
        o += &format!("  \"start\": {{ \"position\": {}, \"yaw\": {} }},
", v3(self.start.position), n(self.start.yaw));
        if let Some(sh) = &self.ship { o += &format!("  \"ship\": {{ \"position\": {}, \"yaw\": {} }},\n", v3(sh.position), n(sh.yaw)); }
        if let Some(d) = self.death_height { o += &format!("  \"death_height\": {},
", n(d)); }
        o += &format!("  \"loadout\": [{}],
", self.loadout.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", "));
        let ab = [("rails", self.abilities.rails), ("cables", self.abilities.cables)].iter().filter_map(|(k, v)| v.map(|b| format!("\"{k}\": {b}"))).collect::<Vec<_>>();
        o += &format!("  \"abilities\": {{ {} }},
", ab.join(", "));
        o += &format!("  \"reset_on_death\": {},
", self.reset_on_death);
        let mut floors: Vec<String> = self.floors.iter().map(|f| {
            let tex = f.texture.map(|t| format!(", \"texture\": {}", q(&t.to_string()))).unwrap_or_default();
            format!("    {{ \"kind\": \"floor\", \"min\": [{}, {}], \"max\": [{}, {}], \"height\": {}{tex}, \"tile\": {} }}", n(f.min[0]), n(f.min[1]), n(f.max[0]), n(f.max[1]), n(f.height), n(f.tile))
        }).collect();
        floors.extend(self.patches.iter().map(|p| {
            let tex = p.texture.map(|t| format!(", \"texture\": {}", q(&t.to_string()))).unwrap_or_default();
            // The heights row by row (one line per row of points).
            let rows: Vec<String> = p.heights.chunks(p.points().0).map(|r| r.iter().map(|&h| n(h)).collect::<Vec<_>>().join(", ")).collect();
            let layers: Vec<String> = p.layers.iter().map(|l| {
                let w: Vec<String> = l.weights.chunks(p.points().0).map(|r| r.iter().map(|w| w.to_string()).collect::<Vec<_>>().join(",")).collect();
                format!("{{ \"texture\": {}, \"tile\": {}, \"weights\": [
          {}
        ] }}", q(&l.texture.to_string()), n(l.tile), w.join(",
          "))
            }).collect();
            let layers = if layers.is_empty() { String::new() } else { format!(",
      \"layers\": [
        {}
      ]", layers.join(",
        ")) };
            // The land values in eighths, row by row.
            let mask = if p.mask.is_empty() {
                String::new()
            } else {
                let rows: Vec<String> = p.mask.chunks(p.points().0).map(|r| r.iter().map(|v| ((v * 8.0).round() as i32).to_string()).collect::<Vec<_>>().join(",")).collect();
                format!(",\n      \"mask\": [\n        {}\n      ]", rows.join(",\n        "))
            };
            let edge = p.edge.as_ref().map_or(String::new(), edge_json);
            let surface = if p.surface == floor::GROUND { String::new() } else { format!(",\n      \"surface\": {}", p.surface) };
            let shade = if p.shade.is_empty() { String::new() } else {
                let rows: Vec<String> = p.shade.chunks(p.points().0).map(|r| r.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(",")).collect();
                format!(",\n      \"shade\": [\n        {}\n      ]", rows.join(",\n        "))
            };
            let scatter = if p.scatter.is_empty() { String::new() } else {
                let items: Vec<String> = p.scatter.iter().map(|s| format!("{{ \"asset\": {}, \"at\": [{}, {}], \"yaw\": {}, \"scale\": {} }}", q(&s.asset.to_string()), n(s.at[0]), n(s.at[1]), n(s.yaw), n(s.scale))).collect();
                format!(",\n      \"scatter\": [\n        {}\n      ]", items.join(",\n        "))
            };
            let roof = p.roof.as_ref().map_or(String::new(), |r| {
                let tex = r.texture.map(|t| format!(", \"texture\": {}", q(&t.to_string()))).unwrap_or_default();
                format!(",\n      \"roof\": {{ \"height\": {}{tex}, \"tile\": {} }}", n(r.height), n(r.tile))
            });
            let layers = layers + &mask + &edge + &roof + &surface + &shade + &scatter;
            format!(
                "    {{ \"kind\": \"patch\", \"origin\": {}, \"spacing\": {}, \"size\": [{}, {}]{tex}, \"tile\": {}, \"depth\": {},
      \"heights\": [
        {}
      ]{layers} }}",
                v3(p.origin), n(p.spacing), p.size[0], p.size[1], n(p.tile), n(p.depth), rows.join(",
        ")
            )
        }));
        floors.extend(self.walkways.iter().map(|w| {
            let tex = w.texture.map(|t| format!(", \"texture\": {}", q(&t.to_string()))).unwrap_or_default();
            let surface = if w.surface == floor::GROUND { String::new() } else { format!(", \"surface\": {}", w.surface) };
            let edge = w.edge.as_ref().map_or(String::new(), edge_json) + if w.cut { ", \"cut\": true" } else { "" };
            format!("    {{ \"kind\": \"walkway\", \"points\": [{}], \"width\": {}, \"depth\": {}{tex}, \"tile\": {}{surface}{edge} }}",
                w.points.iter().map(|&p| v3(p)).collect::<Vec<_>>().join(", "), n(w.width), n(w.depth), n(w.tile))
        }));
        o += &format!("  \"terrain\": [
{}
  ],
", floors.join(",
"));
        let rails: Vec<String> = self.rails.iter().map(|r| format!("    {{ \"points\": [{}]{} }}", r.points.iter().map(|&p| v3(p)).collect::<Vec<_>>().join(", "), format!("{}{}", if r.smooth { ", \"smooth\": true" } else { "" }, r.look.as_ref().map(|l| format!(", \"look\": {}", q(l))).unwrap_or_default() + if r.one_way { ", \"one_way\": true" } else { "" }))).collect();
        o += &format!("  \"rails\": [
{}
  ],
", rails.join(",
"));
        let objects: Vec<String> = self.objects.iter().map(|ob| {
            let t = ob.template.map(|t| format!(", \"template\": {t}")).unwrap_or_default()
                + &ob.group.map(|g| format!(", \"group\": {g}")).unwrap_or_default()
                + &if ob.path.is_empty() { String::new() } else { format!(", \"path\": [{}]", ob.path.iter().map(|&p| v3(p)).collect::<Vec<_>>().join(", ")) }
                + &ob.time.map(|t| format!(", \"time\": {}", n(t))).unwrap_or_default()
                + &ob.solid.map(|s| format!(", \"solid\": {s}")).unwrap_or_default()
                + &ob.name.as_ref().map(|nm| format!(", \"name\": {}", q(nm))).unwrap_or_default()
                + if ob.hidden { ", \"hidden\": true" } else { "" };
            format!("    {{ \"asset\": {}, \"position\": {}, \"yaw\": {}{t} }}", q(&ob.asset.to_string()), v3(ob.position), n(ob.yaw))
        }).collect();
        let zones: Vec<String> = self.zones.iter().map(|z| {
            let eye = if let logic::ZoneKind::Camera { eye } = z.kind { format!(", \"eye\": {}", v3(eye)) } else { String::new() };
            format!("    {{ \"name\": {}, \"kind\": \"{}\", \"centre\": {}, \"size\": {}, \"yaw\": {}{eye} }}", q(&z.name), z.kind.name(), v3(z.centre), v3(z.half.map(|h| h * 2.0)), n(z.yaw))
        }).collect();
        if !zones.is_empty() { o += &format!("  \"zones\": [\n{}\n  ],\n", zones.join(",\n")); }
        let rules: Vec<String> = self.rules.iter().map(|r| {
            let when = match &r.when {
                logic::When::Enters(z) => format!("{{ \"enters\": {} }}", q(z)),
                logic::When::Cleared(z) => format!("{{ \"cleared\": {} }}", q(z)),
                logic::When::Destroyed(o) => format!("{{ \"destroyed\": {} }}", q(o)),
            };
            let then: Vec<String> = r.then.iter().map(|t| match t {
                logic::Then::Remove(o) => format!("{{ \"remove\": {} }}", q(o)),
                logic::Then::Show(o) => format!("{{ \"show\": {} }}", q(o)),
                logic::Then::Message(m) => format!("{{ \"message\": {} }}", q(m)),
                logic::Then::Checkpoint(z) => format!("{{ \"checkpoint\": {} }}", q(z)),
                logic::Then::Goal => "{ \"goal\": true }".to_string(),
            }).collect();
            format!("    {{ \"when\": {when}, \"then\": [{}]{} }}", then.join(", "), if r.cutscene { ", \"cutscene\": true" } else { "" })
        }).collect();
        if !rules.is_empty() { o += &format!("  \"rules\": [\n{}\n  ],\n", rules.join(",\n")); }
        if self.world != World::default() {
            let w = &self.world;
            let mut f: Vec<String> = Vec::new();
            if let Some(l) = w.sky { f.push(format!("\"sky\": {l}")); }
            if let Some((l, t)) = w.music { f.push(format!("\"music\": [{l}, {t}]")); }
            if w.silent { f.push("\"silent\": true".into()); }
            if w.sky_raise != 0.0 { f.push(format!("\"sky_raise\": {}", n(w.sky_raise))); }
            if w.sky_turn != 0.0 { f.push(format!("\"sky_turn\": {}", n(w.sky_turn))); }
            if let Some(g) = &w.fog { f.push(format!("\"fog\": {{ \"color\": [{}, {}, {}], \"near\": {}, \"far\": {} }}", g.color[0], g.color[1], g.color[2], n(g.near), n(g.far))); }
            o += &format!("  \"world\": {{ {} }},\n", f.join(", "));
        }
        o += &format!("  \"objects\": [
{}
  ]
}}
", objects.join(",
"));
        o
    }

    /// The moby classes the map takes from levels other than its host: (source level, o_class), sorted, distinct.
    /// The world's level settings words a map writes (offsets in the level settings section): with a sky level, its
    /// background colour and fog (`sky_settings`: that level's words 0x00..0x24), then the map's own fog over them.
    pub fn settings_words(&self, sky_settings: Option<[u32; 10]>) -> Vec<(usize, u32)> {
        let mut out: Vec<(usize, u32)> = sky_settings.map_or_else(Vec::new, |w| (0..10).map(|k| (k * 4, w[k])).collect());
        if let Some(f) = &self.world.fog {
            out.retain(|(o, _)| !(0x0c..=0x1c).contains(o));
            out.extend([(0x0c, f.color[0] as u32), (0x10, f.color[1] as u32), (0x14, f.color[2] as u32), (0x18, f.near.to_bits()), (0x1c, f.far.max(f.near + 1.0).to_bits())]);
        }
        out
    }

    /// The walkways as built ([`walkway::Walkway::pieces`]: a `cut` one only where it is off the islands' land).
    pub fn walkway_pieces(&self) -> Vec<walkway::Walkway> {
        let land = |x: f32, y: f32| self.patches.iter().filter_map(|p| p.height_at(x, y)).reduce(f32::max);
        self.walkways.iter().flat_map(|w| w.pieces(land)).collect()
    }

    pub fn imports(&self) -> Vec<(u32, i32)> {
        let mut v: Vec<(u32, i32)> = self.objects.iter().filter(|o| o.asset.level != self.host_level).map(|o| (o.asset.level, o.asset.number)).collect();
        v.sort();
        v.dedup();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAP: &str = r#"{
        "maker_format": 1, "name": "Test", "game": "rac1", "host_level": 1,
        "start": {"position": [160, 140, 60.5], "yaw": 1.5},
        "death_height": 40, "loadout": [29], "abilities": {"rails": true},
        "terrain": [{"kind": "floor", "min": [128, 112], "max": [208, 192], "height": 60, "texture": "rac1/L01/tfrag-tex/3"}],
        "rails": [{"points": [[150, 150, 62], [190, 150, 66]]}],
        "objects": [
            {"asset": "rac1/L01/moby/500", "position": [170, 130, 60]},
            {"asset": "rac1/L09/moby/193", "position": [180, 170, 60], "yaw": 3.1, "template": 52, "group": 2, "path": [[180, 170, 60], [184, 170, 68]], "time": 4.5},
            {"asset": "rac1/L09/moby/193", "position": [185, 170, 60]}
        ]
    }"#;

    #[test]
    fn patches_round_trip() {
        let mut d = MapDef::new_default("islands", 1);
        let mut p = terrain::Patch::flat([150.0, 120.0, 58.5], 2.0, [3, 2]);
        p.heights[5] = 2.25;
        p.depth = 12.0;
        p.texture = Some(AssetId::parse("rac1/L01/tfrag-tex/13").unwrap());
        let mut w = vec![0u8; 12];
        w[4] = 200;
        p.layers.push(terrain::PaintLayer { texture: AssetId::parse("rac1/L01/tfrag-tex/8").unwrap(), tile: 4.0, weights: w });
        d.patches.push(p);
        let mut island = terrain::Patch::island([170.0, 150.0, 70.0], 2.0, 6.0, |x, y| 6.0 - (x * x + y * y).sqrt());
        island.edge = Some(terrain::Edge { kind: terrain::EdgeKind::Piece(AssetId::parse("rac1/L02/tie/495").unwrap()), height: 1.5, gaps: vec![[200.0, 194.0, 2.0]], step: 4.25, inset: 1.0625 });
        island.roof = Some(terrain::Roof { height: 7.5, texture: Some(AssetId::parse("rac1/L01/tfrag-tex/8").unwrap()), tile: 8.0 });
        island.surface = 7;
        island.shade = vec![0xffffff; island.heights.len()];
        island.shade[30] = 0x806040;
        island.scatter.push(terrain::Scatter { asset: AssetId::parse("rac1/L01/shrub/12").unwrap(), at: [170.5, 150.25], yaw: 1.5, scale: 1.25 });
        d.patches.push(island);
        let mut bridge = walkway::Walkway::new(vec![[160.0, 140.0, 66.0], [170.0, 146.0, 68.0], [180.0, 150.0, 70.0]], 3.0);
        bridge.texture = Some(AssetId::parse("rac1/L01/tfrag-tex/13").unwrap());
        bridge.surface = 1;
        bridge.edge = Some(terrain::Edge { kind: terrain::EdgeKind::Wall { texture: None, tile: 2.0 }, height: 1.0, gaps: Vec::new(), step: 0.0, inset: 0.0 });
        d.walkways.push(bridge);
        let back = MapDef::parse(&d.to_json()).unwrap();
        assert_eq!(back, d);
        assert!(MapDef::parse(&d.to_json().replace("\"spacing\": 2.0", "\"spacing\": 3.0")).is_err());
    }

    #[test]
    fn parses_a_map() {
        let m = MapDef::parse(MAP).unwrap();
        assert_eq!((m.name.as_str(), m.host_level, m.loadout.as_slice()), ("Test", 1, &[29usize][..]));
        assert_eq!(m.abilities, Abilities { rails: Some(true), cables: None });
        assert!(m.reset_on_death);
        assert!(!MapDef::parse(&MAP.replace("\"death_height\": 40", "\"death_height\": 40, \"reset_on_death\": false")).unwrap().reset_on_death);
        assert_eq!(m.floors[0].texture, Some(AssetId { game: Game::Rac1, level: 1, kind: AssetKind::TfragTexture, number: 3 }));
        assert_eq!(m.objects[1].template, Some(52));
        assert_eq!((m.objects[0].group, m.objects[1].group), (None, Some(2)));
        assert_eq!((m.objects[1].path.len(), m.objects[1].time), (2, Some(4.5)));
        assert_eq!(m.imports(), vec![(9, 193)]);
        assert_eq!(m.objects[1].asset.to_string(), "rac1/L09/moby/193");
    }

    #[test]
    fn writes_and_reads_back() {
        let m = MapDef::parse(MAP).unwrap();
        assert_eq!(MapDef::parse(&m.to_json()).unwrap(), m);
        let d = MapDef::new_default("New \"map\"", 1);
        assert_eq!(MapDef::parse(&d.to_json()).unwrap(), d);
        // Zones, rules, named and hidden objects, smooth rails.
        let mut l = m.clone();
        l.rails = vec![Rail { points: vec![[150.0, 150.0, 62.0], [160.0, 152.0, 64.0], [170.0, 150.0, 63.0]], smooth: true, look: Some("kerwan".into()), one_way: true }];
        l.objects[0].name = Some("Gate".into());
        l.objects[0].hidden = true;
        l.zones = vec![
            logic::Zone { name: "Arena".into(), kind: logic::ZoneKind::Trigger, centre: [160.0, 150.0, 63.0], half: [5.0, 5.0, 2.0], yaw: 0.5 },
            logic::Zone { name: "View".into(), kind: logic::ZoneKind::Camera { eye: [150.0, 140.0, 70.0] }, centre: [160.0, 150.0, 63.0], half: [3.0, 3.0, 2.0], yaw: 0.0 },
        ];
        l.rules = vec![logic::Rule {
            cutscene: true,
            when: logic::When::Cleared("Arena".into()),
            then: vec![logic::Then::Show("Gate".into()), logic::Then::Message("Open \"now\"".into()), logic::Then::Checkpoint("Arena".into()), logic::Then::Goal],
        }];
        assert_eq!(MapDef::parse(&l.to_json()).unwrap(), l);
        // The world, the ship, a cut path.
        l.world = World { sky: Some(3), music: Some((5, 2)), silent: false, fog: Some(Fog { color: [10, 20, 30], near: 40.0, far: 200.0 }), sky_raise: -8.0, sky_turn: 90.0 };
        l.ship = Some(Start { position: [150.0, 140.0, 60.0], yaw: 1.5 });
        assert_eq!(MapDef::parse(&l.to_json()).unwrap(), l);
    }

    #[test]
    fn rejects_bad_maps() {
        let bad = |from: &str, to: &str| MapDef::parse(&MAP.replace(from, to)).unwrap_err().0;
        assert!(bad("\"maker_format\": 1", "\"maker_format\": 2").contains("maker_format"));
        assert!(bad("[160, 140, 60.5]", "[160, 1400, 60.5]").contains("outside the world"));
        assert!(bad("rac1/L09/moby/193\", \"position\": [180", "uya/L09/moby/193\", \"position\": [180").contains("not supported"));
        assert!(bad("\"min\": [128, 112]", "\"min\": [129, 112]").contains("multiples of 4"));
        assert!(bad("rac1/L01/moby/500", "rac1/L01/moby/0").contains("start"));
    }
}
