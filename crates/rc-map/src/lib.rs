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

pub mod floor;
pub mod json;
pub mod patch;

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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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

/// What an asset id names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AssetKind {
    /// A moby class (`number` = o_class).
    Moby,
    /// A texture of the level's tfrag table (`number` = table index).
    TfragTexture,
}

impl AssetKind {
    pub fn name(self) -> &'static str {
        match self {
            AssetKind::Moby => "moby",
            AssetKind::TfragTexture => "tfrag-tex",
        }
    }
}

/// `game/Lnn/kind/number`: a game asset on one source level, e.g. `rac1/L09/moby/193`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
        let kind = match kind {
            "moby" => AssetKind::Moby,
            "tfrag-tex" => AssetKind::TfragTexture,
            _ => return err(format!("asset id {s:?}: unknown kind {kind:?} (moby, tfrag-tex)")),
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
}

/// A placed object: a moby class of some level, copied from one of its instances there.
#[derive(Clone, Debug, PartialEq)]
pub struct Object {
    pub asset: AssetId,
    pub position: [f32; 3],
    pub yaw: f32,
    /// The source level's instance whose record and pvars are the template (None: the class's first instance).
    pub template: Option<usize>,
}

/// One map file.
#[derive(Clone, Debug, PartialEq)]
pub struct MapDef {
    pub name: String,
    pub host_level: u32,
    pub start: Start,
    /// Below this height Ratchet dies (level settings +0x28); None keeps the host's.
    pub death_height: Option<f32>,
    /// Item ids Ratchet owns from the start (docs/plan/gadgets.md: 29 Grindboots, 12 Swingshot, …).
    pub loadout: Vec<usize>,
    pub abilities: Abilities,
    pub floors: Vec<Floor>,
    pub rails: Vec<Rail>,
    pub objects: Vec<Object>,
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
        let mut floors = Vec::new();
        for (i, f) in v.get("terrain").and_then(Value::as_arr).unwrap_or(&[]).iter().enumerate() {
            let what = format!("terrain[{i}]");
            match f.get("kind").and_then(Value::as_str) {
                Some("floor") => {}
                k => return err(format!("{what}: kind {k:?}: only \"floor\" is supported")),
            }
            let min = f.get("min").and_then(Value::as_f32s::<2>).ok_or_else(|| MapError(format!("{what}.min: expected [x, y]")))?;
            let max = f.get("max").and_then(Value::as_f32s::<2>).ok_or_else(|| MapError(format!("{what}.max: expected [x, y]")))?;
            let height = f.get("height").and_then(Value::as_f32).ok_or_else(|| MapError(format!("{what}.height: expected a number")))?;
            let texture = f.get("texture").and_then(Value::as_str).map(AssetId::parse).transpose()?;
            if texture.is_some_and(|t| t.kind != AssetKind::TfragTexture) { return err(format!("{what}.texture: expected a tfrag-tex asset")); }
            let tile = f.get("tile").and_then(Value::as_f32).unwrap_or(8.0);
            let floor = Floor { min, max, height, texture, tile };
            floor::check(&floor).map_err(|e| MapError(format!("{what}: {e}")))?;
            floors.push(floor);
        }
        let mut rails = Vec::new();
        for (i, r) in v.get("rails").and_then(Value::as_arr).unwrap_or(&[]).iter().enumerate() {
            let pts = r.get("points").and_then(Value::as_arr).ok_or_else(|| MapError(format!("rails[{i}].points: missing")))?;
            let points = pts.iter().enumerate().map(|(k, p)| f3(Some(p), &format!("rails[{i}].points[{k}]"))).collect::<Result<Vec<_>>>()?;
            if points.len() < 2 { return err(format!("rails[{i}]: needs at least 2 points")); }
            rails.push(Rail { points });
        }
        let mut objects = Vec::new();
        for (i, o) in v.get("objects").and_then(Value::as_arr).unwrap_or(&[]).iter().enumerate() {
            let what = format!("objects[{i}]");
            let asset = AssetId::parse(o.get("asset").and_then(Value::as_str).ok_or_else(|| MapError(format!("{what}.asset: missing")))?)?;
            if asset.kind != AssetKind::Moby { return err(format!("{what}.asset: expected a moby asset, got {asset}")); }
            if asset.number == 0 { return err(format!("{what}: Ratchet (class 0) is placed by \"start\"")); }
            let template = o.get("template").map(|t| t.as_i64().filter(|&t| t >= 0).map(|t| t as usize).ok_or_else(|| MapError(format!("{what}.template: expected an instance index")))).transpose()?;
            objects.push(Object { asset, position: f3(o.get("position"), &format!("{what}.position"))?, yaw: o.get("yaw").and_then(Value::as_f32).unwrap_or(0.0), template });
        }
        Ok(MapDef { name, host_level, start, death_height, loadout, abilities, floors, rails, objects })
    }

    /// The moby classes the map takes from levels other than its host: (source level, o_class), sorted, distinct.
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
            {"asset": "rac1/L09/moby/193", "position": [180, 170, 60], "yaw": 3.1, "template": 52},
            {"asset": "rac1/L09/moby/193", "position": [185, 170, 60]}
        ]
    }"#;

    #[test]
    fn parses_a_map() {
        let m = MapDef::parse(MAP).unwrap();
        assert_eq!((m.name.as_str(), m.host_level, m.loadout.as_slice()), ("Test", 1, &[29usize][..]));
        assert_eq!(m.abilities, Abilities { rails: Some(true), cables: None });
        assert_eq!(m.floors[0].texture, Some(AssetId { game: Game::Rac1, level: 1, kind: AssetKind::TfragTexture, number: 3 }));
        assert_eq!(m.objects[1].template, Some(52));
        assert_eq!(m.imports(), vec![(9, 193)]);
        assert_eq!(m.objects[1].asset.to_string(), "rac1/L09/moby/193");
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
