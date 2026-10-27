//! The asset catalog: every asset of every level the player's data holds, deduplicated, with what the editor's
//! asset browser shows (name, where it appears, how often the game places it, whether its behaviour is ported). The
//! engine builds it (`rerac --build-catalog`, crate rc-engine `catalog`) into `<data>/catalog/catalog.json`; the
//! editor reads it. Format version [`CATALOG_FORMAT`]; unknown keys are ignored, so newer catalogs still read.
//!
//! **Deduplication.** Assets with the same content (the class blob and the pixels of the textures it uses) are one
//! entry: its `id` is the first level that has it, `also` lists the others. A crate class that every level carries
//! is one entry with 18 more ids.

use crate::json::{self, Value};
use crate::{err, AssetId, AssetKind, MapError, Result};

pub const CATALOG_FORMAT: i64 = 1;

/// Ratchet, Clank, his ship and the models of his items (worn or held, moved by the hero code: not placed objects).
pub const GROUP_GEAR: &str = "gear";
/// Crates, bolts and pickups.
pub const GROUP_PICKUPS: &str = "pickups";

/// What a moby is for the editor's type filter ([`Entry::kind`]), and its label there.
pub const KIND_ENEMY: &str = "enemy";
pub const KIND_PICKUP: &str = "pickup";
pub const KIND_PLATFORM: &str = "platform";
pub const KIND_SCENERY: &str = "scenery";
pub const KIND_EFFECT: &str = "effect";
pub const KIND_OTHER: &str = "other";
pub const KINDS: [(&str, &str); 6] = [
    (KIND_ENEMY, "Enemies & creatures"),
    (KIND_PICKUP, "Crates & pickups"),
    (KIND_PLATFORM, "Platforms & machines"),
    (KIND_SCENERY, "Scenery"),
    (KIND_EFFECT, "Effects & pieces"),
    (KIND_OTHER, "Other"),
];

/// Whether a moby class does something in the port (the catalog's badge; a spawn test on a map refines it later).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Behaviour {
    /// Its level's class table names an update the port runs.
    Ported,
    /// No update function: scenery that only stands and animates.
    Static,
    /// An update the port does not have yet.
    Unported,
}

impl Behaviour {
    pub fn name(self) -> &'static str {
        match self {
            Behaviour::Ported => "ported",
            Behaviour::Static => "static",
            Behaviour::Unported => "unported",
        }
    }
    fn parse(s: &str) -> Option<Behaviour> { [Behaviour::Ported, Behaviour::Static, Behaviour::Unported].into_iter().find(|b| b.name() == s) }
}

/// The spawn test's verdict on a moby class (rc-engine `spawn_test`: placed on a custom map next to Ratchet and run).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Live {
    /// It ran its update for the whole test without reaching unported code or failing.
    Live,
    /// It reached unported code, crashed the run, or did not run (see `live_note`).
    NeedsWork,
}

impl Live {
    pub fn name(self) -> &'static str {
        match self {
            Live::Live => "live",
            Live::NeedsWork => "needs-work",
        }
    }
    fn parse(s: &str) -> Option<Live> { [Live::Live, Live::NeedsWork].into_iter().find(|b| b.name() == s) }
}

/// One catalog entry.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// Where it is first found (the lowest level).
    pub id: AssetId,
    /// The same asset on other levels.
    pub also: Vec<AssetId>,
    /// A short name (the port's name for mobys; empty when nothing names it yet).
    pub name: String,
    /// A longer description, when one is known.
    pub about: String,
    /// Instances the game's levels place, over every level it is on.
    pub placed: u32,
    /// Mobys only.
    pub behaviour: Option<Behaviour>,
    /// The port that runs it (mobys with [`Behaviour::Ported`]), e.g. `U301` or `Crate`.
    pub port: String,
    /// Model facts: joints and animation sequences (mobys), textures used.
    pub joints: u32,
    pub sequences: u32,
    pub textures: u32,
    /// Mobys: the model's box in world units around its origin, `[min x, y, z, max x, y, z]` (game axes, z up), at the
    /// scale its first placement in the game gives it (the editor's template). Stacking and spacing use it.
    pub bounds: Option<[f32; 6]>,
    /// Its thumbnail, relative to the catalog folder (`thumbs/<kind>/<content hash>.png`; empty: none).
    pub thumb: String,
    /// Words to filter by (`tall`, `large`, `blue`, …).
    pub tags: Vec<String>,
    /// The browser's group: [`GROUP_GEAR`], [`GROUP_PICKUPS`], or empty.
    pub group: String,
    /// Mobys: what it is for the type filter ([`KINDS`]; empty: not sorted).
    pub kind: String,
    /// Mobys: the class has its own collision (a collision blob); without one the editor can make it solid from its
    /// model.
    pub collision: bool,
    /// The spawn test's verdict (mobys it tested) and what it saw.
    pub live: Option<Live>,
    pub live_note: String,
}

impl Entry {
    pub fn new(id: AssetId) -> Entry {
        Entry { id, also: Vec::new(), name: String::new(), about: String::new(), placed: 0, behaviour: None, port: String::new(), joints: 0, sequences: 0, textures: 0, bounds: None, thumb: String::new(), tags: Vec::new(), group: String::new(), kind: String::new(), collision: false, live: None, live_note: String::new() }
    }
    /// Every id it is known by.
    pub fn ids(&self) -> impl Iterator<Item = &AssetId> { std::iter::once(&self.id).chain(&self.also) }
}

/// The catalog.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Catalog {
    /// The ReRAC version that built it (rebuilt when it changes: ports and readers grow).
    pub rerac: String,
    pub entries: Vec<Entry>,
}

fn quote(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

impl Catalog {
    /// The entry that has `id` (as its first id or in `also`).
    pub fn find(&self, id: &AssetId) -> Option<&Entry> { self.entries.iter().find(|e| e.ids().any(|i| i == id)) }

    /// Entry counts per kind, in [`AssetKind::ALL`] order.
    pub fn counts(&self) -> Vec<(AssetKind, usize)> {
        AssetKind::ALL.iter().map(|&k| (k, self.entries.iter().filter(|e| e.id.kind == k).count())).collect()
    }

    /// The catalog as JSON: one entry per line (diffable), default fields left out.
    pub fn to_json(&self) -> String {
        let mut o = format!(
            "{{\n  \"catalog_format\": {CATALOG_FORMAT},\n  \"game\": \"rac1\",\n  \"rerac\": {},\n  \"levels\": [{}],\n  \"entries\": [\n",
            quote(&self.rerac),
            crate::LEVEL_NAMES.iter().map(|n| quote(n)).collect::<Vec<_>>().join(", ")
        );
        for (k, e) in self.entries.iter().enumerate() {
            let mut f = vec![format!("\"id\": {}", quote(&e.id.to_string()))];
            if !e.name.is_empty() { f.push(format!("\"name\": {}", quote(&e.name))); }
            if !e.about.is_empty() { f.push(format!("\"about\": {}", quote(&e.about))); }
            if let Some(b) = e.behaviour { f.push(format!("\"behaviour\": \"{}\"", b.name())); }
            if !e.port.is_empty() { f.push(format!("\"port\": {}", quote(&e.port))); }
            f.push(format!("\"placed\": {}", e.placed));
            for (key, v) in [("joints", e.joints), ("sequences", e.sequences), ("textures", e.textures)] {
                if v != 0 { f.push(format!("\"{key}\": {v}")); }
            }
            if let Some(b) = e.bounds { f.push(format!("\"bounds\": [{}]", b.map(|v| v.to_string()).join(", "))); }
            if !e.thumb.is_empty() { f.push(format!("\"thumb\": {}", quote(&e.thumb))); }
            if !e.group.is_empty() { f.push(format!("\"group\": {}", quote(&e.group))); }
            if !e.kind.is_empty() { f.push(format!("\"kind\": {}", quote(&e.kind))); }
            if e.collision { f.push("\"collision\": true".into()); }
            if !e.tags.is_empty() { f.push(format!("\"tags\": [{}]", e.tags.iter().map(|t| quote(t)).collect::<Vec<_>>().join(", "))); }
            if let Some(l) = e.live { f.push(format!("\"live\": \"{}\"", l.name())); }
            if !e.live_note.is_empty() { f.push(format!("\"live_note\": {}", quote(&e.live_note))); }
            if !e.also.is_empty() { f.push(format!("\"also\": [{}]", e.also.iter().map(|a| quote(&a.to_string())).collect::<Vec<_>>().join(", "))); }
            o.push_str(&format!("    {{{}}}{}\n", f.join(", "), if k + 1 < self.entries.len() { "," } else { "" }));
        }
        o.push_str("  ]\n}\n");
        o
    }

    /// Reads a catalog file's text.
    pub fn parse(text: &str) -> Result<Catalog> {
        let v = json::parse(text).map_err(MapError)?;
        let fmt = v.get("catalog_format").and_then(Value::as_i64);
        if fmt != Some(CATALOG_FORMAT) { return err(format!("catalog_format {fmt:?}: this build reads format {CATALOG_FORMAT}")); }
        let s = |e: &Value, k: &str| e.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let n = |e: &Value, k: &str| e.get(k).and_then(Value::as_i64).unwrap_or(0).clamp(0, u32::MAX as i64) as u32;
        let mut entries = Vec::new();
        for (i, e) in v.get("entries").and_then(Value::as_arr).unwrap_or(&[]).iter().enumerate() {
            let id = AssetId::parse(e.get("id").and_then(Value::as_str).ok_or_else(|| MapError(format!("entries[{i}].id: missing")))?)?;
            let also = e.get("also").and_then(Value::as_arr).unwrap_or(&[]).iter().map(|a| AssetId::parse(a.as_str().unwrap_or(""))).collect::<Result<Vec<_>>>()?;
            let behaviour = e.get("behaviour").and_then(Value::as_str).and_then(Behaviour::parse);
            entries.push(Entry {
                id, also, name: s(e, "name"), about: s(e, "about"), placed: n(e, "placed"), behaviour, port: s(e, "port"),
                joints: n(e, "joints"), sequences: n(e, "sequences"), textures: n(e, "textures"), bounds: e.get("bounds").and_then(|b| b.as_f32s::<6>()), thumb: s(e, "thumb"),
                tags: e.get("tags").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(|t| t.as_str().map(str::to_string)).collect(),
                group: s(e, "group"),
                kind: s(e, "kind"),
                collision: e.get("collision").and_then(Value::as_bool).unwrap_or(false),
                live: e.get("live").and_then(Value::as_str).and_then(Live::parse),
                live_note: s(e, "live_note"),
            });
        }
        Ok(Catalog { rerac: v.get("rerac").and_then(Value::as_str).unwrap_or("").to_string(), entries })
    }
}

/// Merges per-level finds into catalog entries: `finds` are (content hash, entry) in level order; an entry whose
/// hash was seen for the same kind joins the first one (its id goes to `also`, its placements add up).
pub fn dedupe(finds: impl IntoIterator<Item = (u64, Entry)>) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    let mut seen: std::collections::HashMap<(AssetKind, u64), usize> = std::collections::HashMap::new();
    for (hash, e) in finds {
        match seen.get(&(e.id.kind, hash)) {
            Some(&k) => {
                let first = &mut out[k];
                first.also.push(e.id);
                first.placed += e.placed;
                if first.name.is_empty() { first.name = e.name; }
                if first.about.is_empty() { first.about = e.about; }
            }
            None => {
                seen.insert((e.id.kind, hash), out.len());
                out.push(e);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Game;

    fn id(level: u32, kind: AssetKind, number: i32) -> AssetId { AssetId { game: Game::Rac1, level, kind, number } }

    #[test]
    fn dedupes_by_kind_and_hash() {
        let mut a = Entry::new(id(1, AssetKind::Moby, 500));
        a.placed = 3;
        let mut b = Entry::new(id(9, AssetKind::Moby, 500));
        b.placed = 4;
        b.name = "crate".into();
        let c = Entry::new(id(9, AssetKind::Tie, 500));
        let d = Entry::new(id(9, AssetKind::Moby, 193));
        let out = dedupe([(7, a), (7, b), (7, c), (8, d)]);
        assert_eq!(out.len(), 3);
        assert_eq!((out[0].placed, out[0].also.as_slice(), out[0].name.as_str()), (7, &[id(9, AssetKind::Moby, 500)][..], "crate"));
    }

    #[test]
    fn json_round_trip() {
        let mut e = Entry::new(id(9, AssetKind::Moby, 193));
        e.name = "pack biter".into();
        e.about = "the pack biters of Gaspar and \"Quartu\"".into();
        e.behaviour = Some(Behaviour::Ported);
        e.port = "U301".into();
        e.placed = 125;
        e.joints = 30;
        e.bounds = Some([-0.5, -0.75, 0.0, 0.5, 0.75, 1.25]);
        e.also = vec![id(15, AssetKind::Moby, 193)];
        e.thumb = "thumbs/moby/00000000000000c1.png".into();
        e.tags = vec!["creature".into(), "small".into()];
        e.group = GROUP_PICKUPS.into();
        e.kind = KIND_PICKUP.into();
        e.collision = true;
        e.live = Some(Live::NeedsWork);
        e.live_note = "reached unported code \"x\"".into();
        let c = Catalog { rerac: "0.3.6".into(), entries: vec![e, Entry::new(id(0, AssetKind::Sky, 0))] };
        let back = Catalog::parse(&c.to_json()).unwrap();
        assert_eq!(back, c);
        assert_eq!(back.find(&id(15, AssetKind::Moby, 193)).map(|e| e.port.as_str()), Some("U301"));
        assert_eq!(back.counts()[0], (AssetKind::Moby, 1));
    }
}
