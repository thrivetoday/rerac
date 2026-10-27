//! The RAC Maker editor: `rerac --data-dir <data> --edit <map.json>` (docs/plan/rac_maker.md, Phase 2).
//!
//! The editor is the game itself with the tick frozen: the map loads exactly as `--map` plays it (crate::map_load), on
//! the fly camera, with egui panels (`bevy_egui`) on the window over the game frame. Every change edits the working
//! map ([`Editor::def`]), hands it to the loader (`map_load::set`) and reloads the host level in the background
//! (crate::level_switch), so what is drawn is what plays. A reload that fails (a class clash, the moby texture table
//! full) undoes the change and says why.
//!
//! **At once, before the reload is in.** Each object carries an editor id ([`Editor::ids`]), and the loaded level's
//! instances are known by the ids they were loaded from ([`Editor::loaded_ids`]): a moved, turned or removed object
//! is redrawn or hidden on the spot (`gameplay::editor_show`), and a new one shows as its box and thumbnail until the
//! reload draws it.
//!
//! **Sizes.** The catalog gives each moby its box (`Entry::bounds`). A click on an object's top stacks the new one
//! on it, on its side puts it next to it; anything placed or dragged into another object is pushed out of it.
//!
//! **Terrain** (the Terrain tab, Phase 3): patches of height points (`rc_map::terrain::Patch`) made with "New patch",
//! sculpted by dragging on them with a brush (raise, lower, smooth, flatten; Shift turns raise into lower), given a
//! texture, a tile and a cliff depth in the inspector. A stroke redraws the patch every frame (crate::map_load's
//! renderer follows `map_load::set`) and the reload that brings the collision runs when the stroke ends.
//!
//! **Controls.** Right mouse held: look; WASD / Space / Q: fly (crate::fly_cam). Left click: select an object (or
//! place the armed asset); drag a selected object to move it on its height; R / Shift+R: turn it 15°; Delete:
//! remove it. Ctrl+Z / Ctrl+Y: undo / redo; Ctrl+S: save; F5: play the map from its start (the tick runs, the play
//! camera follows Ratchet; it waits for a load still running) and F5 again: back to editing (the map reloads).

use crate::fly_cam::FlyCam;
use crate::game_camera::CameraSource;
use crate::tfrag_render::game_to_bevy;
use bevy::asset::RenderAssetUsages;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::{egui, EguiContexts, EguiGlobalSettings, EguiPlugin, EguiPrimaryContextPass, EguiTextureHandle, PrimaryEguiContext};
use rc_map::catalog::{Behaviour, Catalog, Entry, Live, GROUP_GEAR, KINDS};
use rc_map::{AssetId, AssetKind, MapDef, Object, LEVEL_NAMES};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

mod menu;

/// Editing (the tick frozen, the fly camera) or playing the map.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorMode {
    Editing,
    Playing,
}

/// The game tick's run condition: always outside the editor, only while playing in it.
pub fn playing(mode: Option<Res<EditorMode>>) -> bool { mode.is_none_or(|m| *m == EditorMode::Playing) }

/// What the panels hold this frame, for the fly camera (crate::fly_cam): the cursor shows while editing, and the
/// wheel and keys are the panels' while the pointer is over them or a text field has focus.
#[derive(Resource, Default)]
pub struct UiFocus {
    pub editing: bool,
    pub pointer: bool,
    pub keyboard: bool,
    /// The editor takes the mouse wheel (over an island handle: the anchors' reach).
    pub wheel: bool,
    /// The panels' rectangles (window logical pixels = egui points): the pointer is over the panels when it is in
    /// one (egui's own hover test leaves out the background layer the panels are drawn in).
    panels: Vec<egui::Rect>,
}

impl UiFocus {
    fn covers(&self, at: Vec2) -> bool { self.panels.iter().any(|r| r.contains(egui::pos2(at.x, at.y))) }
}

/// The crate classes (rc_game `crate_::CLASSES`): touching ones share a group, so a stack falls when a crate under it
/// breaks.
const CRATES: [i32; 5] = [500, 501, 502, 505, 511];

/// Undo steps kept.
const UNDO: usize = 200;
/// The box of an asset the catalog has none for (game units around its origin).
const DEFAULT_BOX: [f32; 6] = [-0.5, -0.5, 0.0, 0.5, 0.5, 1.0];
/// Objects closer than this are touching, not overlapping.
const TOUCH: f32 = 0.02;

/// Ratchet's items the loadout can grant (item id, name: docs/plan/gadgets.md).
/// Where an enemy's sight range comes from.
#[derive(Clone, Copy)]
enum Range {
    /// A template pvar (f32 at this offset).
    Pvar(usize),
    /// A constant of the class's code.
    Const(f32),
    None,
}

/// Per enemy class: its wake range (where it notices Ratchet) and its attack range, by the ported classes
/// (rc_game moby_update::classes; most measure flat from where they stand or their home, within a few units of height).
const SIGHT: &[(i32, Range, Range)] = {
    use Range::{Const as C, None as N, Pvar as P};
    &[
        (459, P(0x1a8), P(0x1a0)), (1023, P(0x1e4), C(3.3)), (1382, C(64.0), C(1.8)), (749, N, C(2.0)), (1454, C(30.0), N),
        (238, P(0x2ac), C(1.5)), (572, P(0x224), C(2.5)), (865, P(0x224), C(2.5)), (866, P(0x224), C(2.5)), (573, P(0x22c), N),
        (574, P(0x26c), C(3.5)), (217, P(0x43c), C(4.5)), (340, P(0x140), C(2.0)), (427, N, C(14.0)), (563, P(0x3e8), C(3.0)),
        (623, P(0x29c), C(3.0)), (625, C(20.0), N), (827, P(0x1dc), C(2.0)), (1048, C(30.0), C(2.5)), (577, C(16.0), C(1.5)),
        (193, N, C(1.5)), (333, C(20.0), N), (253, C(20.0), N), (1202, C(30.0), C(4.0)), (1231, C(24.0), N), (1246, P(0x23c), C(12.0)),
        (294, P(0x230), C(5.5)), (326, N, C(40.0)), (1041, N, C(20.0)), (29, C(20.0), N), (63, P(0x1fc), C(1.5)),
        (1262, P(0x19c), C(4.0)), (44, C(64.0), N), (1445, C(16.0), C(1.25)), (580, P(0x24c), C(12.0)), (612, P(0x184), P(0x180)),
    ]
};

const ITEMS: [(usize, &str); 30] = [
    (2, "Heli-Pack"), (3, "Thruster-Pack"), (4, "Hydro-Pack"), (5, "Sonic Summoner"), (6, "O2 Mask"), (7, "Pilot's Helmet"),
    (9, "Suck Cannon"), (10, "Bomb Glove"), (11, "Devastator"), (12, "Swingshot"), (13, "Visibomb Gun"), (14, "Taunter"),
    (15, "Blaster"), (16, "Pyrocitor"), (17, "Mine Glove"), (18, "Walloper"), (19, "Tesla Claw"), (20, "Glove of Doom"),
    (21, "Morph-o-Ray"), (22, "Hydrodisplacer"), (23, "R.Y.N.O."), (24, "Drone Device"), (25, "Decoy Glove"),
    (26, "Trespasser"), (27, "Metal Detector"), (28, "Magneboots"), (29, "Grindboots"), (30, "Hoverboard"),
    (31, "Hologuise"), (32, "Gadgetron PDA"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reload {
    Idle,
    /// A change is waiting for the running load to finish.
    Wanted,
    /// Loading; the level generation before it.
    Loading(u64),
}

/// What the viewport's clicks do: pick and place objects, or sculpt terrain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Objects,
    Terrain,
    /// The Maps tab (files, recent maps, backups): clicks in the world select objects.
    Maps,
    /// Zones and when/then rules (rc_map::logic).
    Logic,
    /// The sky, fog and song (rc_map::World), and the radio.
    World,
}

/// A terrain brush.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Brush {
    Raise,
    Lower,
    /// Up to one flat height (the stroke's start + [`Editor::plateau`]; inverted: a flat-bottomed pit).
    Plateau,
    Smooth,
    Flatten,
    /// The chosen texture painted over the patch ([`Editor::paint`]; inverted: rubbed out).
    Paint,
    /// Land added to the patch's shape (inverted: cut away: bays, holes, gaps).
    Land,
    /// The top shaded with a colour ([`Editor::shade`]; inverted: back to unshaded).
    Shade,
    /// Plants and details strewn over the land ([`Editor::scatter`]; inverted: picked up).
    Scatter,
    /// The edge taken out under the brush (inverted: put back).
    Gaps,
}

const BRUSHES: [(Brush, &str, &str); 10] = [
    (Brush::Raise, "Raise", "Hills (inverted: dips)"),
    (Brush::Lower, "Lower", "Dips and valleys (inverted: hills)"),
    (Brush::Plateau, "Plateau", "Lifts the ground to one flat height: platforms and ledges (inverted: flat-bottomed pits)"),
    (Brush::Smooth, "Smooth", "Softens bumps and edges"),
    (Brush::Flatten, "Flatten", "Levels to the height where the stroke started"),
    (Brush::Paint, "Paint", "Paints the texture picked below over the patch (inverted: rubs it out)"),
    (Brush::Land, "Land", "Grows the land out to where you paint (inverted: cuts it away: bays, holes, gaps)"),
    (Brush::Shade, "Shade", "Tints the ground with a colour: shadows under trees, burnt or mossy patches (inverted: clears it)"),
    (Brush::Scatter, "Decor", "Strews the plant, rock or detail picked below over the land (inverted: picks them up)"),
    (Brush::Gaps, "Edge gaps", "Takes the island's railing or wall out under the brush (inverted: puts it back)"),
];

/// Each type's subtypes in the asset browser, by words of the asset's name (the first that matches; the last of a
/// type takes the rest).
const SUBTYPES: [(&str, &[(&str, &[&str])]); 5] = [
    (rc_map::catalog::KIND_ENEMY, &[
        ("Bosses", &["boss", "sea beast", "umbris beast"]),
        ("Turrets & cannons", &["turret", "cannon", "launcher", "mine", "lobber", "thrower", "flamer", "spawner"]),
        ("Flyers & ships", &["drone", "copter", "gunship", "dropship", "fighter", "flying", "jet", "ship", "hover"]),
        ("Robots & troopers", &["trooper", "robot", "bot", "sentry", "guard", "brawler", "gunner", "grenadier", "commando", "crew", "walker", "tank", "runner", "racer", "hopper", "logger", "crawler"]),
        ("Creatures", &[]),
    ]),
    (rc_map::catalog::KIND_PICKUP, &[
        ("Crates", &["crate"]),
        ("Breakables", &["breakable"]),
        ("Bolts", &["bolt"]),
        ("Ammo & health", &[]),
    ]),
    (rc_map::catalog::KIND_PLATFORM, &[
        ("Hazards", &["flame", "laser", "energy", "spout", "fan"]),
        ("Switches & pads", &["switch", "button", "pad", "crank", "target", "lever", "teleporter"]),
        ("Doors & gates", &["door", "gate", "hatch", "airlock", "lock", "barrier", "field", "flap"]),
        ("Lifts & movers", &[]),
    ]),
    (rc_map::catalog::KIND_SCENERY, &[
        ("Plants", &["grass", "plant", "tree", "flower", "bush", "vine", "leaf", "palm", "cactus", "shrub", "weed", "mushroom"]),
        ("Lights", &["lamp", "light", "glow"]),
        ("Rocks", &["rock", "boulder", "stone", "asteroid", "crystal"]),
        ("Signs & screens", &["sign", "hologram", "logo", "flag", "poster", "screen", "billboard", "monitor"]),
        ("Walls & barriers", &["wall", "barricade", "fence", "barrier"]),
        ("Props", &[]),
    ]),
    (rc_map::catalog::KIND_EFFECT, &[
        ("Fire & smoke", &["fire", "flame", "smoke", "explosion", "spark", "steam", "burn"]),
        ("Water", &["water", "splash", "drip", "bubble", "wave"]),
        ("Pieces & other", &[]),
    ]),
];

/// An asset's subtype within its type (None: the type has no subtypes).
fn subtype(kind: &str, name: &str) -> Option<&'static str> {
    let subs = SUBTYPES.iter().find(|(k, _)| *k == kind)?.1;
    let n = name.to_lowercase();
    subs.iter().find(|(_, words)| words.is_empty() || words.iter().any(|w| n.contains(w))).map(|(s, _)| *s)
}

/// A shade colour's preset swatches: (name, colour).
const SHADES: [(&str, [u8; 3]); 6] = [
    ("Shadow", [110, 110, 125]),
    ("Dark", [70, 70, 80]),
    ("Warm", [255, 200, 150]),
    ("Moss", [150, 210, 120]),
    ("Burnt", [120, 85, 60]),
    ("Snow", [235, 240, 255]),
];

/// An island's outline shape (the island builder).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Round,
    Square,
    Blob,
}

/// The island builder's settings: shape, across (game units), height above the ground clicked, cliff depth, edge,
/// texture; `seed` varies the blob.
#[derive(Clone, Debug)]
struct IslandSpec {
    shape: Shape,
    size: f32,
    height: f32,
    depth: f32,
    edge: Option<rc_map::terrain::Edge>,
    texture: Option<AssetId>,
    seed: u32,
}

/// The next random number (xorshift).
fn next(seed: &mut u32) -> u32 {
    let mut x = *seed;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *seed = x;
    x
}

/// The Terrain tab's modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerrainMode {
    /// Clicks select (objects, islands, paths), nothing is built.
    Select,
    /// Click to put an island there.
    Build,
    /// Drag the selected island's handles: its coastline anchors, its middle (move), its height arrow.
    Shape,
    /// Click the selected island's edge to open or close a stretch of it.
    Openings,
    /// The brushes.
    Sculpt,
    /// Walkways and bridges: click their points.
    Paths,
    /// Grind rails: click their points.
    Rails,
}

/// A handle of the selected island (Shape mode), where it is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
enum IslandHandle {
    /// A point on the coastline: dragged, the coast around it follows (out or in), fading over the reach.
    Anchor([f32; 3]),
    /// The middle: dragged, the whole island moves over the ground.
    Move([f32; 3]),
    /// The arrow's tip above the middle: dragged up or down, the island rises or sinks.
    Lift([f32; 3]),
}

/// A drag of an island handle: the island before it (each frame starts again from it), where the drag took hold.
struct ShapeDrag {
    patch: usize,
    base: rc_map::terrain::Patch,
    handle: IslandHandle,
    grab: [f32; 3],
    start: Snap,
}

/// The railings and walls an edge can be made of: game pieces (level scenery and objects of every planet), each one
/// checked to be a railing, fence or wall by its model.
/// (id, name, a wall: the Walls category; else Railings.)
const EDGE_PIECES: [(&str, &str, bool); 28] = [
    ("rac1/L02/tie/495", "Aridia truss railing", false),
    ("rac1/L03/tie/1377", "Kerwan iron fence", false),
    ("rac1/L02/tie/576", "Aridia chain-link fence", false),
    ("rac1/L08/tie/404", "Batalia guard rail", false),
    ("rac1/L07/tie/1356", "Umbris handrail", false),
    ("rac1/L00/tie/2041", "Veldin rail bars", false),
    ("rac1/L00/tie/2217", "Veldin curved bars", false),
    ("rac1/L18/moby/936", "Veldin barricade fence", false),
    ("rac1/L01/tie/631", "Novalis low stone wall", false),
    ("rac1/L04/tie/239", "Eudora low rock wall", false),
    ("rac1/L16/tie/2294", "Kalebo concrete barrier", false),
    ("rac1/L11/moby/1247", "Pokitaru panel", false),
    ("rac1/L01/moby/729", "Novalis rock wall", true),
    ("rac1/L01/tie/458", "Novalis rock face", true),
    ("rac1/L01/tie/565", "Novalis mossy rocks", true),
    ("rac1/L03/tie/979", "Kerwan rock face", true),
    ("rac1/L03/tie/832", "Kerwan stone wall", true),
    ("rac1/L02/tie/533", "Aridia brick wall", true),
    ("rac1/L02/tie/590", "Aridia metal wall", true),
    ("rac1/L02/tie/1418", "Aridia panel wall", true),
    ("rac1/L02/tie/2268", "Aridia window wall", true),
    ("rac1/L08/tie/339", "Batalia metal wall", true),
    ("rac1/L08/tie/362", "Batalia hedge", true),
    ("rac1/L10/tie/2368", "Orxon stone wall", true),
    ("rac1/L12/tie/180", "Hoven blue wall", true),
    ("rac1/L12/tie/179", "Hoven teal wall", true),
    ("rac1/L16/tie/2153", "Kalebo window wall", true),
    ("rac1/L16/tie/2155", "Kalebo plain wall", true),
];

/// An edge piece's name (its id when it is not one of [`EDGE_PIECES`]).
fn piece_name(id: AssetId) -> String {
    EDGE_PIECES.iter().find(|(k, _, _)| AssetId::parse(k).is_ok_and(|a| a == id)).map_or_else(|| id.to_string(), |(_, n, _)| n.to_string())
}

/// A patch's coastline anchors: points along each outline loop, evenly about a 32nd of the whole coast apart.
fn anchors(p: &rc_map::terrain::Patch) -> Vec<[f32; 3]> {
    let total: f32 = p.loops().iter().map(|l| (0..l.len()).map(|k| { let (a, b) = (l[k], l[(k + 1) % l.len()]); ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt() }).sum::<f32>()).sum();
    let step = (total / 32.0).clamp(1.5, 8.0);
    crate::map_load::piece_spans(p, step).into_iter().map(|s| s.0).collect()
}

/// The selected island's handles: its anchors, the move handle at its middle, the height arrow's tip above it.
fn island_handles(p: &rc_map::terrain::Patch) -> Vec<IslandHandle> {
    let c = p.centre();
    let mut v: Vec<IslandHandle> = anchors(p).into_iter().map(|a| IslandHandle::Anchor([a[0], a[1], a[2] + 0.15])).collect();
    v.push(IslandHandle::Move([c[0], c[1], c[2] + 0.4]));
    v.push(IslandHandle::Lift([c[0], c[1], c[2] + 4.0]));
    v
}

/// The handle under the pointer (the nearest on screen within 16 pixels; the move and height handles first).
fn handle_at(p: &rc_map::terrain::Patch, window: &Window, frame: &crate::display::GameFrame, cam: (&Camera, &GlobalTransform)) -> Option<IslandHandle> {
    let cursor = window.cursor_position()?;
    let at = |h: &IslandHandle| match *h { IslandHandle::Anchor(q) | IslandHandle::Move(q) | IslandHandle::Lift(q) => q };
    let mut best: Option<(f32, IslandHandle)> = None;
    for h in island_handles(p).into_iter().rev() {
        let Some(sp) = to_window(window, frame, cam, at(&h)) else { continue };
        let d = sp.distance(cursor) - if matches!(h, IslandHandle::Anchor(_)) { 0.0 } else { 4.0 };
        if d < 16.0 && best.is_none_or(|b| d < b.0) { best = Some((d, h)); }
    }
    best.map(|b| b.1)
}

impl IslandSpec {
    /// The outline's distance from the centre at angle `a`.
    fn reach(&self, a: f32) -> f32 {
        let r = self.size / 2.0;
        match self.shape {
            Shape::Round => r,
            Shape::Square => r / a.cos().abs().max(a.sin().abs()),
            Shape::Blob => {
                let (p1, p2) = ((self.seed % 97) as f32 * 0.37, (self.seed % 89) as f32 * 0.61);
                r * (1.0 + 0.18 * (3.0 * a + p1).sin() + 0.1 * (5.0 * a + p2).sin())
            }
        }
    }
    /// The land value at an offset from the centre (above 0 inside).
    fn land(&self, x: f32, y: f32) -> f32 {
        let d = (x * x + y * y).sqrt();
        match self.shape {
            Shape::Square => self.size / 2.0 - x.abs().max(y.abs()),
            _ => self.reach(y.atan2(x)) - d,
        }
    }
    /// The grid spacing for its size (at most 128 cells across).
    fn spacing(&self) -> f32 { if self.size <= 60.0 { 1.0 } else if self.size <= 240.0 { 2.0 } else { 4.0 } }
}

/// A map and its objects' editor ids (an undo step).
#[derive(Clone)]
struct Snap {
    def: MapDef,
    ids: Vec<u64>,
}

/// The editor's state.
#[derive(Resource)]
pub struct Editor {
    /// The working map (what the next reload loads, what Save writes).
    def: MapDef,
    /// Per object of `def`: its editor id (kept through moves, undo and reloads; new for every new object).
    ids: Vec<u64>,
    next_id: u64,
    path: PathBuf,
    /// The map as last loaded successfully (a failed reload goes back to it), and the ids its instances were loaded
    /// from (`loaded_ids[k]` is static instance `k + 1`; instance 0 is Ratchet).
    loaded: Snap,
    /// What the running load is loading.
    loading: Option<Snap>,
    undo: Vec<Snap>,
    redo: Vec<Snap>,
    saved: bool,
    selected: Option<usize>,
    /// The asset a click in the world places (index into the catalog's entries).
    armed: Option<usize>,
    /// Moving the selected object: the pick point's offset from it (game units).
    drag: Option<[f32; 3]>,
    drag_start: Option<Snap>,
    reload: Reload,
    /// A change came while loading: load again once that load is in.
    reload_again: bool,
    /// Play was asked for while a load ran: it starts when the load is in (editing is locked until then).
    play_pending: bool,
    /// An inspector drag is changing a value: one undo step for the whole drag.
    coalescing: bool,
    status: String,
    catalog: Option<Arc<Catalog>>,
    /// The catalog's placeable mobys (index into `catalog.entries`).
    placeable: Vec<usize>,
    filter: String,
    planet: Option<u32>,
    /// The type filter (a `catalog::KINDS` key; None: every type) and its subtype ([`SUBTYPES`]).
    kind: Option<&'static str>,
    sub: Option<&'static str>,
    live_only: bool,
    thumbs: HashMap<String, Option<egui::TextureId>>,
    /// Per level: the classes its gameplay file places (a placed asset's template is copied from there).
    placed_by_level: HashMap<u32, Vec<i32>>,
    camera_set: bool,
    /// Frames per second, smoothed (the top bar).
    fps: f32,
    tool: Tool,
    brush: Brush,
    /// The brush's radius (game units) and strength (1: raise 6 units a second at the centre).
    radius: f32,
    strength: f32,
    /// The selected terrain patch.
    patch: Option<usize>,
    /// A stroke in progress: the height Flatten levels to (the first point's). The reload waits for its end.
    stroke: Option<f32>,
    /// Where the brush is over the terrain (the marker).
    brush_at: Option<[f32; 3]>,
    /// The facing of the next placed object (R / Shift+R while placing).
    place_yaw: f32,
    terrain_mode: TerrainMode,
    island: IslandSpec,
    /// Where a click would build the island (its centre on the ground, the marker).
    island_at: Option<[f32; 3]>,
    /// Shape mode: how far a coastline anchor's pull reaches (the mouse wheel over a handle), the handle under the
    /// pointer, the drag going on.
    reach: f32,
    shape_hover: Option<IslandHandle>,
    shape_drag: Option<ShapeDrag>,
    /// Openings mode: the edge stretch under the pointer (its ends, open), and a drag opening (true) or closing
    /// stretches, with the map before it (one undo step).
    gap_hover: Option<Stretch>,
    gap_paint: Option<(bool, Snap)>,
    /// The island just built: the Build settings' edge, texture and cliff depth change it too while it is selected.
    last_built: Option<usize>,
    /// The edge picker shows the walls (else the railings).
    edge_walls: bool,
    /// The Shade brush's colour, the Scatter brush's piece and size, the random numbers' state.
    shade: [u8; 3],
    scatter: Option<AssetId>,
    scatter_size: f32,
    seed: u32,
    /// Paths mode: the walkway selected (its settings show; clicks add points to it while `walk_drawing`), the first
    /// point of a path not made yet, the settings a new one gets, the point under the pointer, a point being dragged
    /// (with the map before it), where a click would put the next point.
    walk: Option<usize>,
    walk_drawing: bool,
    walk_first: Option<[f32; 3]>,
    walk_spec: rc_map::walkway::Walkway,
    walk_lift: f32,
    walk_hover: Option<usize>,
    walk_drag: Option<(usize, Snap)>,
    walk_at: Option<[f32; 3]>,
    /// Rails mode, as Paths: the rail selected, drawing, its first point, how high new points go, smooth or not, the
    /// point under the pointer, a point dragged, where the next point goes.
    rail: Option<usize>,
    rail_drawing: bool,
    rail_first: Option<[f32; 3]>,
    rail_lift: f32,
    rail_smooth: bool,
    rail_hover: Option<usize>,
    rail_drag: Option<(usize, Snap)>,
    /// The selected rail's point picked (its arrows move it along x, y, up) and an arrow drag (axis, where the grab
    /// was along it, the point then, the map before).
    rail_point: Option<usize>,
    rail_axis: Option<(usize, f32, [f32; 3], Snap)>,
    rail_at: Option<[f32; 3]>,
    /// The collision drawn over the world (the top bar's Collision), cached for the map drawn last.
    show_collision: bool,
    collision_view: Option<(usize, Vec<([[f32; 3]; 3], Color)>)>,
    /// What the last save found wrong (the Maps tab lists it).
    warnings: Vec<String>,
    /// The main menu (open at the start).
    menu: menu::MainMenu,
    /// Logic tab: the zone selected, the kind a click places (armed), a zone being dragged (grab offset, map before).
    zone: Option<usize>,
    zone_new: Option<&'static str>,
    zone_drag: Option<([f32; 2], Snap)>,
    /// Logic tab, a zone kind armed: the zone a click would place where the pointer is (drawn as a preview).
    zone_ghost: Option<rc_map::logic::Zone>,
    /// The help panel's search.
    help_query: String,
    /// A view button pressed (Top, Front, ...): the camera moves to look at the selection (or what it faces) from there.
    view_snap: Option<&'static str>,
    /// The radio while editing: the song playing (an index into `map_load::songs`) and a change asked for; its song
    /// list open (top bar).
    radio: Option<usize>,
    radio_menu: bool,
    /// The look new rails get (`map_load::RAIL_LOOKS` key; None: the plain beam).
    rail_look: Option<String>,
    /// The planets' globe pictures (World tab tiles), made once each.
    globes: HashMap<u32, Option<egui::TextureId>>,
    /// The World tab's song list: the planet whose songs show.
    song_planet: Option<u32>,
    /// The thumbnail: its camera to be remembered (Set thumbnail), a retake asked for (a save), and the camera to put
    /// back after a retake (transform, yaw, pitch).
    thumb_record: bool,
    thumb_retake: bool,
    thumb_restore: Option<(Transform, f32, f32)>,
    /// World tab: a point light asked for (placed where the camera looks), the light picked, the sky layer counts.
    light_add: bool,
    light_sel: Option<usize>,
    sky_shells: HashMap<u32, usize>,
    radio_want: Option<Option<usize>>,
    /// Set thumbnail: the file the next frames' capture goes to (the markers hidden for it), and frames to wait.
    thumb_shot: Option<(PathBuf, u8)>,
    /// Brush options: a square footprint, always inverted (as Shift), the Plateau's height, the Paint's texture.
    square: bool,
    invert: bool,
    plateau: f32,
    paint: Option<AssetId>,
    /// The help panel (and which of its topics).
    help: Option<usize>,
    settings: Settings,
    /// The last backup of unsaved work (every 5 minutes).
    last_backup: std::time::Instant,
    /// The floor whose texture the map inspector is showing (index into `def.floors`).
    floor_edit: Option<usize>,
    /// A gizmo being dragged on the selected object (an arrow, the turning ring, a path point).
    gizmo: Option<Gizmo>,
    /// Per asset: the template's sight ranges (the robot trooper's wake and attack ranges), read once.
    ranges: HashMap<AssetId, Option<(f32, f32)>>,
    /// Where the pointer was pressed on the selected object: it moves only once the pointer has left that spot (a
    /// click selects without moving it).
    press: Option<Vec2>,
    /// Objects (by editor id) the game moved away from where they were placed when the level loaded (their class
    /// places itself from its home level's data), and where they went.
    moved_away: HashMap<u64, [f32; 3]>,
}

/// A gizmo drag: along world axis `axis` (0 x, 1 y, 2 up) from where the ray's closest point was (`t0`) and the
/// object's position then; the turning ring from the angle grabbed and the facing then; a path point (its index).
#[derive(Clone, Copy, Debug)]
enum Gizmo {
    Axis { axis: usize, t0: f32, from: [f32; 3] },
    Ring { a0: f32, yaw0: f32 },
    Point { k: usize },
}

/// The editor's handles (move arrows, the turning ring, path and rail points): drawn over everything, so a big model
/// never hides them.
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct Handles;

fn handles_on_top(mut store: ResMut<GizmoConfigStore>) {
    let (config, _) = store.config_mut::<Handles>();
    config.depth_bias = -1.0;
    config.line.width = 2.5;
}

/// Where the selected object's move arrows start: just above its top (inside a big model they were hidden).
fn gizmo_centre(pos: [f32; 3], local: [f32; 6]) -> [f32; 3] { [pos[0], pos[1], pos[2] + local[5].max(0.5) + 0.3] }

/// The selected object's gizmo size: its arrows' length and its ring's radius (game units).
fn gizmo_size(local: [f32; 6]) -> (f32, f32) {
    let r = (local[3] - local[0]).max(local[4] - local[1]).max(1.0);
    (r * 0.5 + 1.6, r * 0.5 + 0.9)
}

/// The closest approach of a ray to the line `c + s·axis`: (distance between them, `s` there, the ray's `t` there).
fn ray_line((o, d): ([f32; 3], [f32; 3]), c: [f32; 3], axis: [f32; 3]) -> (f32, f32, f32) {
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let w = [o[0] - c[0], o[1] - c[1], o[2] - c[2]];
    let (a, b, cc, dd, e) = (dot(d, d), dot(d, axis), dot(axis, axis), dot(d, w), dot(axis, w));
    let den = a * cc - b * b;
    if den.abs() < 1e-6 { return (f32::MAX, 0.0, 0.0); }
    let (t, s_) = ((b * e - cc * dd) / den, (a * e - b * dd) / den);
    let p = [o[0] + d[0] * t, o[1] + d[1] * t, o[2] + d[2] * t];
    let q = [c[0] + axis[0] * s_, c[1] + axis[1] * s_, c[2] + axis[2] * s_];
    (((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt(), s_, t)
}

const AXES: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
/// A picked rail or path point's arrows (game units).
const POINT_ARROW: f32 = 1.6;
const AXIS_COLORS: [Color; 3] = [Color::srgb(1.0, 0.3, 0.3), Color::srgb(0.35, 1.0, 0.4), Color::srgb(0.35, 0.6, 1.0)];

/// The editor's own settings (`%LOCALAPPDATA%\rerac\maker\settings.txt`): the recent maps, and whether the help has
/// been seen (it opens by itself the first time).
#[derive(Default)]
struct Settings {
    recent: Vec<PathBuf>,
    help_seen: bool,
    /// The main menu's order of the maps (file names), as the cards were dragged.
    order: Vec<String>,
    /// A save takes the map's picture again from where it was set (off: `thumb_on_save=0`).
    no_thumb_on_save: bool,
}

/// The editor's folder: settings and the guide.
fn maker_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("rerac").join("maker")
}

/// The maps folder: `Documents\RAC Maker`.
fn maps_dir() -> PathBuf {
    std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Documents").join("RAC Maker")
}

impl Settings {
    fn load() -> Settings {
        let mut s = Settings::default();
        let Ok(t) = std::fs::read_to_string(maker_dir().join("settings.txt")) else { return s };
        for line in t.lines() {
            if let Some(p) = line.strip_prefix("recent=") { s.recent.push(PathBuf::from(p)); }
            if line == "help_seen=1" { s.help_seen = true; }
            if let Some(f) = line.strip_prefix("order=") { s.order.push(f.to_string()); }
            if line == "thumb_on_save=0" { s.no_thumb_on_save = true; }
        }
        s
    }
    fn store(&self) {
        let mut t = String::new();
        if self.help_seen { t += "help_seen=1\n"; }
        for r in &self.recent { t += &format!("recent={}\n", r.display()); }
        for f in &self.order { t += &format!("order={f}\n"); }
        if self.no_thumb_on_save { t += "thumb_on_save=0\n"; }
        let _ = std::fs::create_dir_all(maker_dir()).and_then(|_| std::fs::write(maker_dir().join("settings.txt"), t));
    }
    /// `path` first in the recent list (at most 10).
    fn opened(&mut self, path: &std::path::Path) {
        self.recent.retain(|r| r != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(10);
        self.store();
    }
}

/// A time as `YYYY-MM-DD HH:MM:SS` (UTC) from seconds since 1970 (the civil-from-days algorithm), and the same
/// without separators for file names.
fn stamp(secs: u64) -> (String, String) {
    let (days, rem) = ((secs / 86400) as i64, secs % 86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let (h, mi, se) = (rem / 3600, rem / 60 % 60, rem % 60);
    (format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{se:02} UTC"), format!("{y:04}{m:02}{d:02}-{h:02}{mi:02}{se:02}"))
}

fn now_secs() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) }

/// The backups of map `path`: `<maps folder>\backups\<file name>\<time>[-auto].json`.
fn backup_dir(path: &std::path::Path) -> PathBuf {
    maps_dir().join("backups").join(path.file_stem().map_or("map".into(), |s| s.to_string_lossy().into_owned()))
}

/// The backups of `path`, newest first: (file, label).
fn backups(path: &std::path::Path) -> Vec<(PathBuf, String)> {
    let mut v: Vec<(PathBuf, String)> = std::fs::read_dir(backup_dir(path))
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect::<Vec<PathBuf>>())
        .unwrap_or_default()
        .into_iter()
        .map(|p: PathBuf| {
            let n = p.file_stem().map_or(String::new(), |s| s.to_string_lossy().into_owned());
            let auto = n.ends_with("-auto");
            let t = n.trim_end_matches("-auto");
            let label = if t.len() == 15 { format!("{}-{}-{} {}:{}:{} UTC", &t[0..4], &t[4..6], &t[6..8], &t[9..11], &t[11..13], &t[13..15]) } else { t.to_string() };
            (p, if auto { format!("{label} (unsaved work)") } else { label })
        })
        .collect();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v
}

/// The help topics (the Help panel's tabs, as the game's own help pages): title, lines.
const HELP: [(&str, &[&str]); 11] = [
    ("Start here", &[
        "The editor is the game itself, paused: what you see is what you will play.",
        "Left panel: Objects to place things, Terrain to build ground, Logic for checkpoints, the goal and rules, World for the sky and song, Maps for your files and backups.",
        "The top bar holds the tools as icons (hover one for its name): Select, Place, Build, Shape, Openings, Sculpt, Paths, Rails.",
        "View: Top, Front, Back, Left, Right, 3/4 (top bar, right): look at the selected object from that side. The radio is there too: pick a song while you edit.",
        "Help has a search box: type a word to find every line about it.",
        "The main menu (RAC MAKER · Menu, top left) lists your maps with their pictures, versions and backups.",
        "Right panel: what is selected (an object, a terrain patch), or the map's settings with nothing selected: start point, loadout, floors.",
        "Every change shows at once and reloads the level in the background in about half a second.",
        "The full guide (every feature and key) opens with Open the full guide, at the top of this panel.",
    ]),
    ("Camera", &[
        "Hold the right mouse button and move the mouse to look around.",
        "W A S D fly, Space goes up, Q goes down. Hold Shift to fly five times faster.",
        "The mouse wheel over the world changes the flying speed: the top bar shows it, and the reset button next to it puts it back.",
        "Camera feels slow? You scrolled down over the world: scroll up, or press the reset.",
    ]),
    ("Placing objects", &[
        "Objects tab: filter by type and planet; Live only hides objects that do not work away from their home planet yet.",
        "Click an object, then click in the world. The green box shows where it lands: on the ground, on top of what you point at, or next to it.",
        "R / Shift+R turns it before you place it. Esc stops placing.",
        "Select an object: its position and facing on the right, and Add another (on top, left, right, in front, behind) for rows and stacks.",
        "Crates that touch are linked: break the bottom one and the ones above fall.",
        "Solid (on the right): collision from the object's shape. Walls, rocks and other pieces get it by themselves; leave it off for things that move or break.",
        "A red line from an object: the game moves it somewhere else when the level loads (its home planet's data). It cannot stay where it is put yet.",
    ]),
    ("Moving & turning", &[
        "Drag a selected object to slide it along the ground; it stops against other objects.",
        "R / Shift+R turn it 15 degrees. Delete removes it.",
        "Type exact numbers on the right: click a number, type, Enter.",
        "Placed objects (Objects tab, at the bottom) lists everything on the map: click to select, x to delete.",
    ]),
    ("Terrain", &[
        "The toolbar over the world: Select, Place, Build, Shape, Openings, Sculpt, Paths. Esc drops what you hold and comes back to Select.",
        "Select: click an island, a path or an object; its settings show on the right (texture, surface, edge, roof, cliff depth).",
        "Build islands: pick a shape, size, height and edge. The green outline shows the island a click builds; click anywhere, on the ground or in the air.",
        "Openings: click a stretch of an island's edge to take it out, a red one to put it back; or brush them out with Sculpt > Edge gaps.",
        "Shape: drag the white dots on an island's coast out or in (the wheel over a dot: how much follows). Green diamond: move it. Blue arrow: raise it.",
        "Sculpt: Raise, Lower, Plateau, Smooth, Flatten, Paint, Land, Shade (tint), Decor (plants, rocks, details), Edge gaps. Shift: the opposite.",
        "Paths: click where a walkway starts and where it bends, Enter when done. Railings it crosses open by themselves. Drag points; the wheel raises one.",
        "Rails: grind rails, drawn like paths (click, click, Enter). Smooth curves them; the wheel over a point raises it. Look: a planet's rail (Kerwan, Batalia, Oltanis, Kalebo, Veldin) or plain.",
        "Paths: Stop where it meets land (on for new paths) builds a bridge only over the air, so nothing pokes through the islands.",
        "Rails: pick a point (in the world or in its list) to move it with its arrows or type where it goes. Rails go both ways unless One way is ticked.",
        "Islands pushed into each other lose their railing where they meet (same height); the file keeps your own openings.",
        "Collision (F6, also while playing) shows what Ratchet stands on. Saving checks for holes, a start over nothing and a too-high death height.",
    ]),
    ("Logic", &[
        "Zones: pick Checkpoint, Goal, Death zone, Camera or Trigger, then click in the world. Click a zone to select it, drag it to move it; size and turn on the right.",
        "Checkpoint: after a death Ratchet comes back to the last one he walked into. Goal: walking in finishes the map with your time.",
        "Death zone: walking in costs a life. Camera: while Ratchet is inside, the camera watches him from its purple ball (Use the current view as its camera).",
        "Rules: When (Ratchet enters a zone / all enemies in a zone are beaten / an object is destroyed) then (remove or show an object, a message, a checkpoint, the goal). Each rule runs once.",
        "Give objects a Name in their panel to use them in rules. Hidden until a rule shows it: an ambush, a bridge that appears.",
        "Show it with the camera (on a rule that removes or shows an object): the camera turns to it, it happens, then the view cuts back.",
        "In a camera zone the stick follows the zone's camera: push it towards where you want Ratchet to go on screen (grinding keeps the usual controls).",
    ]),
    ("Enemies", &[
        "Select an enemy: the blue circle is where it notices Ratchet, the red one where it attacks (for the enemies whose ranges are known).",
        "Robot troopers and the Veldin tank take a path: Square patrol or 8 ahead, then drag the points. Without one, a trooper guards where it stands.",
        "\"May vanish on maps\" in the object list: that enemy still uses its home planet's data and can disappear or wander off. Try it with Play first.",
        "Ammo crates refill the weapons in the loadout: tick a weapon in the map settings (Loadout), or the crate gives nothing.",
    ]),
    ("World", &[
        "World tab: another planet's sky (with its light and fog), your own fog, and the map's song.",
        "Under the sky tiles: raise moves the sky up or down, turn spins it round (where its sun and mountains are).",
        "Themes: a whole look in one click (sky, fog, light, colours, song): Toxic, Frozen, Volcanic, Night City, Desert Noon, Alien Dusk, Ruins.",
        "Light: your own sun (direction, height, colour), the ambient, and shade in hollows for your islands and paths. Colours: hue, saturation, brightness and a tint for the whole map (Objects too: also the enemies and pickups).",
        "Colours also recolours one ground texture everywhere it is used. Lights: add point lights where the camera looks (colour, reach, strength). Sky layers: hide or tint each layer of the sky.",
        "An island's or a path's panel has Colour: tint just it, or change its texture's colours everywhere. World changes show at once; the sky and objects' colours change when you let go of the slider.",
        "Some skies have twinkling and shooting stars and slowly drifting clouds: switch them off under the sky tiles (Stars and shooting stars, Clouds drift).",
        "An object's panel has Tint: colour that one object (a golden statue, a red crate).",
        "Song: Use picks what the map plays; the host's song or Silence also work. Press ▶ to listen while you edit (the radio), ■ to stop.",
        "Ratchet's ship (map settings, right panel with nothing selected): the level starts with it landing; after the goal, walk back to it and press △ to board and finish.",
        "With a ship, Ratchet starts by its hatch (the blue circle moves with it): make sure there is ground there and between him and the ship.",
    ]),
    ("Playing", &[
        "Play (F5) starts at the blue start circle with the loadout ticked in the map settings.",
        "F5 again stops: everything comes back and the camera returns to where you were.",
        "Set the start point, death height, rails and loadout on the right with nothing selected.",
        "Reaching a Goal shows your time; a few seconds later you are back in the editor.",
    ]),
    ("Files & backups", &[
        "Ctrl+S saves. A dot after the map's name means unsaved changes.",
        "New starts a fresh map in your maps folder (Documents\\RAC Maker).",
        "Maps tab: your recent maps and every map in the folder; click one to open it.",
        "Every save keeps a backup of the version it replaces; unsaved work is backed up every 5 minutes. Restore any of them from the Maps tab.",
        "Main menu: click a map for its picture, versions and backups. Save version keeps a named copy that is never pruned. Drag a card to put your maps in your own order.",
        "Set thumbnail (top bar): fly the camera to a nice view and press it; the picture shows in the main menu.",
        "+ picture (next to Save): each save takes the picture again from that same spot, with your new edits in it.",
    ]),
    ("All keys", &[
        "Right mouse + move: look around.",
        "W A S D, Space, Q: fly. Shift: faster. Wheel: speed.",
        "Left click: select, place, or sculpt (Terrain tab).",
        "R / Shift+R: turn (also while placing). Delete: remove. Esc: stop placing.",
        "Ctrl+Z / Ctrl+Y: undo / redo. Ctrl+S: save. F5: play / stop.",
    ]),
];

/// The full guide (docs/guide/rac_maker_guide.html), written to the editor's folder and opened in the browser.
const GUIDE: &str = include_str!("../../../docs/guide/rac_maker_guide.html");

fn open_guide() -> std::io::Result<()> {
    let path = maker_dir().join("guide.html");
    std::fs::create_dir_all(maker_dir())?;
    std::fs::write(&path, GUIDE)?;
    open_in_shell(&path)
}

/// Opens a file or folder the way Explorer would.
fn open_in_shell(path: &std::path::Path) -> std::io::Result<()> {
    std::process::Command::new("cmd").args(["/C", "start", ""]).arg(path).spawn().map(|_| ())
}

pub struct EditorPlugin;

impl Plugin for EditorPlugin {
    fn build(&self, app: &mut App) {
        if !crate::disc_source::edit_mode() { return; }
        let Some(def) = crate::map_load::original().map(Arc::new).or_else(crate::map_load::active) else { return };
        let path = crate::map_load::path().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("map.json"));
        let root = crate::level_load::extracted_root();
        let catalog = std::fs::read_to_string(root.join("catalog/catalog.json")).ok().and_then(|t| Catalog::parse(&t).ok()).map(Arc::new);
        let placeable = catalog.as_ref().map_or_else(Vec::new, |c| {
            c.entries.iter().enumerate().filter(|(_, e)| e.id.kind == AssetKind::Moby && !e.thumb.is_empty() && e.group != GROUP_GEAR && e.behaviour.is_some() && !crate::map_load::NOT_PLACEABLE.contains(&e.id.number)).map(|(i, _)| i).collect()
        });
        if catalog.is_none() { eprintln!("editor: no catalog at {}: run --build-catalog for the asset browser", root.join("catalog").display()); }
        let saved = path.exists();
        let mut settings = Settings::load();
        settings.opened(&path);
        let ids: Vec<u64> = (0..def.objects.len() as u64).collect();
        let loaded = Snap { def: (*def).clone(), ids: ids.clone() };
        // RC_EDITOR_TAB=objects|terrain|rails|logic|maps (tests and screenshots): straight into that tab, the menu closed.
        let start_tab = std::env::var("RC_EDITOR_TAB").ok();
        let (tool, terrain_mode) = match start_tab.as_deref() {
            Some("terrain") => (Tool::Terrain, TerrainMode::Select),
            Some("rails") => (Tool::Terrain, TerrainMode::Rails),
            Some("logic") => (Tool::Logic, TerrainMode::Select),
            Some("maps") => (Tool::Maps, TerrainMode::Select),
            Some("world") => (Tool::World, TerrainMode::Select),
            _ => (Tool::Objects, TerrainMode::Select),
        };
        let mut main_menu = menu::MainMenu::opened();
        main_menu.open = start_tab.is_none();
        app.add_plugins(EguiPlugin::default())
            .insert_resource(EguiGlobalSettings { auto_create_primary_context: false, ..default() })
            .insert_resource(EditorMode::Editing)
            .insert_resource(UiFocus { editing: true, ..default() })
            .insert_resource(Editor {
                def: (*def).clone(), next_id: ids.len() as u64, ids, path, loaded, loading: None, undo: Vec::new(), redo: Vec::new(), saved,
                selected: None, armed: None, drag: None, drag_start: None, reload: Reload::Idle, reload_again: false, play_pending: false,
                coalescing: false, status: "Right mouse: look · WASD: fly · click: select · F5: play".into(),
                catalog, placeable, filter: String::new(), planet: None, kind: None, live_only: true, thumbs: HashMap::new(),
                placed_by_level: HashMap::new(), camera_set: false, fps: 0.0,
                tool, brush: Brush::Raise, radius: 4.0, strength: 1.0, patch: None, stroke: None, brush_at: None,
                place_yaw: 0.0, terrain_mode, island_at: None, reach: 8.0, shape_hover: None, shape_drag: None, gap_hover: None, gap_paint: None, last_built: None, edge_walls: false,
                shade: SHADES[0].1, sub: None, scatter: None, scatter_size: 1.0, seed: 0x2545_f491,
                walk: None, walk_drawing: false, walk_first: None, walk_spec: rc_map::walkway::Walkway { cut: true, ..rc_map::walkway::Walkway::new(vec![[0.0; 3], [1.0, 0.0, 0.0]], 4.0) }, walk_lift: 0.0,
                walk_hover: None, walk_drag: None, walk_at: None,
                rail: None, rail_drawing: false, rail_first: None, rail_lift: 1.0, rail_smooth: true, rail_hover: None, rail_drag: None, rail_point: None, rail_axis: None, rail_at: None, show_collision: false, collision_view: None, warnings: Vec::new(),
                menu: main_menu, thumb_shot: None, zone: (tool == Tool::Logic && !def.zones.is_empty()).then_some(0), zone_new: None, zone_drag: None, zone_ghost: None, help_query: String::new(), view_snap: None, radio: None, radio_menu: false, radio_want: None, rail_look: Some("kerwan".into()), globes: HashMap::new(), song_planet: None, light_add: false, light_sel: None, sky_shells: HashMap::new(), thumb_record: false, thumb_retake: false, thumb_restore: None,
                island: IslandSpec { shape: Shape::Round, size: 24.0, height: 8.0, depth: 8.0, edge: None, texture: Some(AssetId { game: rc_map::Game::Rac1, level: 1, kind: AssetKind::TfragTexture, number: 8 }), seed: 1 }, square: false, invert: false, plateau: 3.0, paint: None, help: (!settings.help_seen).then_some(0), settings,
                last_backup: std::time::Instant::now(), floor_edit: None, gizmo: None, ranges: HashMap::new(), press: None, moved_away: HashMap::new(),
            })
            .add_systems(Startup, (group_on_open, handles_on_top))
            .init_gizmo_group::<Handles>()
            .add_systems(Last, backup_on_close)
            .init_resource::<Previews>()
            .add_systems(Update, (attach_egui, start_camera, viewport, keys, drive_reload, keep_view, live_preview, markers, preview_models).chain())
            .init_resource::<crate::audio_out::HeldLevelAudio>()
            .add_systems(Update, (shoot_thumbnail, snap_view, radio))
            .add_systems(EguiPrimaryContextPass, panels);
        println!("editor: editing {} (F5 plays, Ctrl+S saves)", crate::map_load::path().map_or("?".into(), |p| p.display().to_string()));
    }
}

/// Set thumbnail: two frames after the request (the markers gone), the game frame (the free camera's view, no
/// panels) saved as the map's thumbnail, at most 640 × 360.
fn shoot_thumbnail(mut commands: Commands, mut ed: ResMut<Editor>, frame: Res<crate::display::GameFrame>) {
    let Some((path, wait)) = ed.thumb_shot.clone() else { return };
    if wait == u8::MAX { return; }
    if wait > 0 {
        ed.thumb_shot = Some((path, wait - 1));
        return;
    }
    ed.thumb_shot = Some((path.clone(), u8::MAX));
    use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
    commands.spawn(Screenshot::image(frame.image.clone())).observe(move |shot: On<ScreenshotCaptured>, mut ed: ResMut<Editor>| {
        ed.thumb_shot = None;
        ed.status = match shot.image.clone().try_into_dynamic() {
            Ok(img) => match img.thumbnail(640, 360).to_rgb8().save(&path) {
                Ok(()) => "Thumbnail set: the main menu shows this view".into(),
                Err(e) => format!("Thumbnail not saved: {e}"),
            },
            Err(e) => format!("Thumbnail not taken: {e:?}"),
        };
    });
}

/// The window closing with unsaved changes: they are kept as a backup (the Maps tab restores it).
fn backup_on_close(ed: Res<Editor>, mut closed: MessageReader<bevy::window::WindowCloseRequested>, mut exits: MessageReader<AppExit>) {
    let closing = closed.read().count() + exits.read().count() > 0;
    if closing && !ed.saved {
        ed.backup(&ed.def.to_json(), true);
        println!("editor: unsaved changes kept as a backup in {}", backup_dir(&ed.path).display());
    }
}

/// A map opened from its file: its crates grouped (a file written by hand, or before groups, has none); a change
/// reloads it.
fn group_on_open(mut ed: ResMut<Editor>) {
    let before = ed.def.objects.clone();
    ed.group_crates();
    ed.fill_solid();
    let mut fitted = false;
    {
        let def = &mut ed.def;
        crate::map_load::fit_edges(def);
        fitted = fit_path_openings(def) || fitted;
    }
    if ed.def.objects != before || fitted { ed.reload = Reload::Wanted; }
}

/// The openings where paths cross islands' edges cut to the paths' inner width (maps from before cut whole pieces,
/// wider than the path). Whether anything changed.
fn fit_path_openings(def: &mut MapDef) -> bool {
    let paths = def.walkways.clone();
    let mut fitted = false;
    for p in &mut def.patches {
        let Some(e) = p.edge.as_mut() else { continue };
        let n = e.gaps.clone();
        e.gaps.retain(|g| paths.iter().all(|w| w.height_at(g[0], g[1]).is_none()));
        if e.gaps != n {
            let land = p.clone();
            p.open_where(|x, y| paths.iter().any(|w| w.inside(x, y, w.edge.as_ref().map_or(0.0, |e| e.inset)) && w.height_at(x, y).is_some_and(|h| land.height_at(x, y).is_none_or(|g| (h - g).abs() < 4.0))));
            fitted = true;
        }
    }
    if fitted { println!("editor: openings where paths cross fitted to the paths"); }
    fitted
}

/// One planet's moby models and textures, for the editor's previews (read on a worker thread, once per planet).
struct LevelModels {
    classes: HashMap<i32, rc_formats::moby::LevelMobyClass>,
    textures: Vec<rc_formats::texture::LevelTexture>,
}

enum Loaded {
    Loading(std::thread::JoinHandle<Option<LevelModels>>),
    Ready(Arc<LevelModels>),
    Failed,
}

/// The model previews: the object about to be placed, and placed objects the level has not loaded yet, drawn as
/// their real models (the catalog thumbnails' triangles, textured) instead of a box.
#[derive(Resource, Default)]
struct Previews {
    levels: HashMap<u32, Loaded>,
    models: HashMap<AssetId, Option<Vec<(Handle<Mesh>, Handle<StandardMaterial>)>>>,
    /// What is drawn: per preview its asset, place, facing and entities.
    shown: Vec<(AssetId, [f32; 3], f32, Vec<Entity>)>,
}

impl Previews {
    /// The model of `id` (None while its planet loads or when it has none).
    fn model(&mut self, id: AssetId, meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>, images: &mut Assets<Image>) -> Option<Vec<(Handle<Mesh>, Handle<StandardMaterial>)>> {
        if let Some(m) = self.models.get(&id) { return m.clone(); }
        let level = id.level;
        let state = self.levels.entry(level).or_insert_with(|| {
            Loaded::Loading(std::thread::spawn(move || {
                let root = crate::level_load::extracted_root();
                let (core, data) = crate::map_load::level_core(&root, level).ok()?;
                let gs = crate::disc_source::level_file(&root, level, "gs_ram.bin").ok()?;
                let classes = rc_formats::moby::parse_level_mobys(&core, &data).ok()?.into_iter().map(|c| (c.o_class, c)).collect();
                let textures = rc_formats::texture::parse_textures(&core, &data, &gs).ok()?;
                Some(LevelModels { classes, textures })
            }))
        });
        if let Loaded::Loading(h) = state {
            if !h.is_finished() { return None; }
            let Loaded::Loading(h) = std::mem::replace(state, Loaded::Failed) else { return None };
            if let Ok(Some(m)) = h.join() { *state = Loaded::Ready(Arc::new(m)); }
        }
        let Loaded::Ready(lm) = state else {
            self.models.insert(id, None);
            return None;
        };
        let lm = lm.clone();
        let built = lm.classes.get(&id.number).map(|class| model_parts(class, &lm.textures, meshes, materials, images)).filter(|v| !v.is_empty());
        self.models.insert(id, built.clone());
        built
    }
}

/// A moby class's bind-pose model as meshes, one per texture (Bevy axes, about its origin), unlit like the game's.
fn model_parts(
    class: &rc_formats::moby::LevelMobyClass,
    textures: &[rc_formats::texture::LevelTexture],
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) -> Vec<(Handle<Mesh>, Handle<StandardMaterial>)> {
    use bevy::mesh::{Indices, PrimitiveTopology};
    let tris = rc_extract::thumbs::moby_tris(class);
    let mut groups: HashMap<Option<(rc_formats::texture::TextureTable, usize)>, Vec<&rc_extract::thumbs::Tri>> = HashMap::new();
    for t in &tris { groups.entry(t.tex).or_default().push(t); }
    let mut out = Vec::new();
    for (tex, tris) in groups {
        let mut positions = Vec::new();
        let mut uvs = Vec::new();
        let mut normals = Vec::new();
        for t in tris {
            let p = t.p.map(|q| game_to_bevy(q));
            let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or(Vec3::Y);
            // Both faces (the model's own winding is not the renderer's).
            for &k in &[0usize, 1, 2, 0, 2, 1] {
                positions.push(p[k].to_array());
                uvs.push(t.uv[k]);
                normals.push(n.to_array());
            }
        }
        let n = positions.len() as u32;
        let mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
            .with_inserted_indices(Indices::U32((0..n).collect()));
        let image = tex.and_then(|(table, index)| textures.iter().find(|t| t.table == table && t.index == index)).map(|t| {
            let t = &t.texture;
            // PS2 alpha is 0..0x80.
            let rgba: Vec<u8> = t.rgba.chunks(4).flat_map(|p| [p[0], p[1], p[2], (p[3] as u16 * 2).min(255) as u8]).collect();
            let mut img = Image::new(
                bevy::render::render_resource::Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 },
                bevy::render::render_resource::TextureDimension::D2,
                rgba,
                bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::RENDER_WORLD,
            );
            img.sampler = ImageSampler::Descriptor(bevy::image::ImageSamplerDescriptor {
                address_mode_u: bevy::image::ImageAddressMode::Repeat,
                address_mode_v: bevy::image::ImageAddressMode::Repeat,
                ..bevy::image::ImageSamplerDescriptor::linear()
            });
            images.add(img)
        });
        let material = materials.add(StandardMaterial {
            base_color: if image.is_some() { Color::WHITE } else { Color::srgb(0.7, 0.7, 0.7) },
            base_color_texture: image,
            unlit: true,
            alpha_mode: AlphaMode::Mask(0.4),
            cull_mode: None,
            ..default()
        });
        out.push((meshes.add(mesh), material));
    }
    out
}

/// Draws the previews: the armed asset where a click would place it (turned by R), and every object the level has
/// not loaded yet. The thumbnails stand in while a planet's models load.
#[allow(clippy::too_many_arguments)]
fn preview_models(
    mut commands: Commands,
    mut ed: ResMut<Editor>,
    mut previews: ResMut<Previews>,
    mode: Res<EditorMode>,
    focus: Res<UiFocus>,
    windows: Query<&Window, With<PrimaryWindow>>,
    frame: Res<crate::display::GameFrame>,
    cams: Query<(&Camera, &GlobalTransform), With<FlyCam>>,
    mut transforms: Query<&mut Transform>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let mut want: Vec<(AssetId, [f32; 3], f32)> = Vec::new();
    if *mode == EditorMode::Editing {
        if let (Some(e), false, Ok(window), Ok(cam)) = (ed.armed, focus.pointer || ed.play_pending, windows.single(), cams.single()) {
            if let Some(asset) = ed.placeable_id(e) {
                let local = ed.local_box(&asset);
                if let Some(p) = cursor_ray(window, &frame, cam).and_then(|r| ed.placement(r, local)) { want.push((asset, p, ed.place_yaw)); }
            }
        }
        for (i, o) in ed.def.objects.iter().enumerate() {
            if !ed.loaded.ids.contains(&ed.ids[i]) { want.push((o.asset, o.position, o.yaw)); }
        }
    }
    // The same assets in the same order: only moved. Else drawn anew.
    let same = previews.shown.len() == want.len() && previews.shown.iter().zip(&want).all(|(a, b)| a.0 == b.0);
    if same {
        for (k, w) in want.iter().enumerate() {
            let (pos, yaw) = (w.1, w.2);
            let shown = &mut previews.shown[k];
            if shown.1 == pos && shown.2 == yaw { continue; }
            (shown.1, shown.2) = (pos, yaw);
            for &e in &shown.3 {
                if let Ok(mut t) = transforms.get_mut(e) { *t = Transform::from_translation(game_to_bevy(pos)).with_rotation(Quat::from_rotation_y(yaw)); }
            }
        }
        // A model that was still loading: try again next time the set changes (the thumbnail stands in).
        if previews.shown.iter().all(|s| !s.3.is_empty()) || want.is_empty() { return; }
    }
    for (_, _, _, es) in previews.shown.drain(..) { for e in es { commands.entity(e).despawn(); } }
    for (asset, pos, yaw) in want {
        let parts = previews.model(asset, &mut meshes, &mut materials, &mut images).unwrap_or_default();
        let t = Transform::from_translation(game_to_bevy(pos)).with_rotation(Quat::from_rotation_y(yaw));
        let es = parts.into_iter().map(|(m, mat)| commands.spawn((Mesh3d(m), MeshMaterial3d(mat), t)).id()).collect();
        previews.shown.push((asset, pos, yaw, es));
    }
    let _ = &mut ed;
}

/// egui draws on the window: its context goes on the present camera (crate::display), not the world camera that
/// draws into the game frame.
fn attach_egui(mut commands: Commands, cams: Query<Entity, (With<crate::display::PresentCamera>, Without<PrimaryEguiContext>)>, mut done: Local<bool>) {
    if *done { return; }
    for e in &cams {
        commands.entity(e).insert(PrimaryEguiContext);
        *done = true;
    }
}

fn to_game(v: Vec3) -> [f32; 3] { [v.x, -v.z, v.y] }

/// The radio while editing (the World tab): a song asked for is read and played; after a level reload (a new audio
/// system) the song playing is put back. Playing the map stops it (the map's own song plays then).
#[allow(clippy::type_complexity)]
fn radio(
    mut ed: ResMut<Editor>,
    mode: Res<EditorMode>,
    audio: Option<ResMut<crate::audio_out::AudioOut>>,
    mut held: ResMut<crate::audio_out::HeldLevelAudio>,
    mut loaded: Local<Option<(usize, Arc<rc_game::audio::music::MusicTrack>, Arc<rc_game::audio::music::MusicTrack>)>>,
    mut was: Local<Option<EditorMode>>,
) {
    let Some(mut audio) = audio else { return };
    // The level's own sound data, before the radio puts its songs in (the boot's: later loads hold theirs).
    if held.0.is_none() && loaded.is_none() { held.0 = Some(audio.system().data.clone()); }
    let changed = *was != Some(*mode);
    *was = Some(*mode);
    // Play starts: a fresh audio system on the level's sound data (the map's song; the music starts as the level's
    // does, after the ship's landing when there is one).
    if changed && *mode == EditorMode::Playing {
        if let Some(data) = held.0.clone() { audio.swap_level(data); }
        return;
    }
    if *mode != EditorMode::Editing { return; }
    // Back from play: the play's music stops (the radio, when on, plays again below).
    if changed && ed.radio.is_none() && ed.radio_want.is_none() {
        audio.system().play_song(None);
        return;
    }
    let want = ed.radio_want.take();
    if let Some(w) = want { ed.radio = w; }
    let Some(k) = ed.radio else {
        if want.is_some() { audio.system().play_song(None); }
        return;
    };
    let playing = loaded.as_ref().is_some_and(|l| l.0 == k && audio.system().song_slot(0).is_some_and(|m| Arc::ptr_eq(m, &l.1)));
    if playing && want.is_none() { return; }
    let root = crate::level_load::extracted_root();
    let Some(&(l, t, ref name)) = crate::map_load::songs(&root).get(k) else { return };
    if loaded.as_ref().is_none_or(|x| x.0 != k) {
        let read = |n: u32| crate::disc_source::level_file(&root, l, &format!("music/{n:03}.bin")).ok().and_then(|b| rc_game::audio::music::MusicTrack::from_vag(&b).ok()).map(Arc::new);
        let (Some(a), Some(b)) = (read(t), read(t + 1)) else {
            ed.status = format!("The song {name} does not read");
            ed.radio = None;
            return;
        };
        *loaded = Some((k, a, b));
    }
    let (_, a, b) = loaded.as_ref().unwrap();
    audio.system().play_song(Some((a.clone(), b.clone())));
    if want.is_some() { ed.status = format!("Radio: {name}"); }
}

/// A planet's globe as the ship's menu spins it (`global/unknown_0820/NNN.bin`: the surface, its shading ring and the
/// disc mask, each 128 × 128 with the first one's palette, laid over black; the stations' pictures alone): RGBA.
fn planet_globe(level: u32) -> Option<(u32, u32, Vec<u8>)> {
    let root = crate::level_load::extracted_root();
    let raw = std::fs::read(root.join(format!("global/unknown_0820/{level:03}.bin"))).ok()?;
    let bytes = rc_formats::wad::decompress(&raw).ok()?;
    let file = rc_game::map::MapFile::parse(bytes)?;
    let pal = file.palette().to_vec();
    let layers: Vec<rc_formats::texture::Texture> = (0..3).filter_map(|k| file.pif(k).and_then(|p| rc_formats::hud::decode_indexed8_raw(p.pixels, p.width, p.height, &pal).ok())).collect();
    let first = layers.first()?;
    let (w, h) = (first.width, first.height);
    let station = [6, 13, 17].contains(&level);
    let mut out = vec![0u8; (w * h * 4) as usize];
    for px in out.chunks_mut(4) { px[3] = 255; }
    for (k, l) in layers.iter().enumerate() {
        if (station && k > 0) || l.width != w || l.height != h { continue; }
        for (o, s) in out.chunks_mut(4).zip(l.rgba.chunks(4)) {
            // PS2 alpha is 0..0x80.
            let a = (s[3] as f32 * 2.0).min(255.0) / 255.0;
            for c in 0..3 { o[c] = (o[c] as f32 * (1.0 - a) + s[c] as f32 * a) as u8; }
        }
    }
    Some((w, h, out))
}

/// A planet's globe as an egui texture (made once).
fn globe_tex(ed: &mut Editor, contexts: &mut EguiContexts, images: &mut Assets<Image>, level: u32) -> Option<egui::TextureId> {
    if let Some(t) = ed.globes.get(&level) { return *t; }
    let id = planet_globe(level).map(|(w, h, rgba)| {
        let img = Image::new(
            bevy::render::render_resource::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            bevy::render::render_resource::TextureDimension::D2,
            rgba,
            bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::RENDER_WORLD,
        );
        contexts.add_image(EguiTextureHandle::Strong(images.add(img)))
    });
    ed.globes.insert(level, id);
    id
}

/// A level's sky picture: its catalog thumbnail (`rac1/LNN/sky/0`).
fn sky_tex(ed: &mut Editor, contexts: &mut EguiContexts, images: &mut Assets<Image>, level: u32) -> Option<egui::TextureId> {
    // The space levels' catalog pictures are a planet or a sun sprite: their starfield instead (the sky texture with
    // the most opaque pixels, then the largest), made once.
    if [6, 13, 17].contains(&level) {
        if let Some(t) = ed.globes.get(&(100 + level)) { return *t; }
        let root = crate::level_load::extracted_root();
        let id = map_load_level_sky(&root, level).map(|t| {
            let img = Image::new(
                bevy::render::render_resource::Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 },
                bevy::render::render_resource::TextureDimension::D2,
                t.rgba.chunks(4).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
                bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::RENDER_WORLD,
            );
            contexts.add_image(EguiTextureHandle::Strong(images.add(img)))
        });
        ed.globes.insert(100 + level, id);
        return id;
    }
    let key = format!("rac1/L{level:02}/sky/0");
    let rel = ed.catalog.as_ref()?.entries.iter().find(|e| e.id.to_string() == key).map(|e| e.thumb.clone())?;
    thumb(ed, contexts, images, &rel, &mut 4)
}

/// A space level's sky picture for a tile: its starfield (the largest, most see-through sky texture: the others are
/// planet and sun sprites) over the colour of its sky dome above the horizon.
fn map_load_level_sky(root: &std::path::Path, level: u32) -> Option<rc_formats::texture::Texture> {
    let (core, data) = crate::map_load::level_core(root, level).ok()?;
    let block = rc_formats::sky::sky_block(&core, &data).ok()??;
    let sky = rc_formats::sky::parse_sky_block(block).ok()?;
    let textures = rc_formats::sky::parse_sky_textures(block, &sky).ok()?;
    let clear = |t: &rc_formats::texture::Texture| t.rgba.chunks(4).filter(|p| p[3] < 64).count() * 100 / (t.rgba.len() / 4).max(1);
    let stars = textures.into_iter().map(|t| t.texture).max_by_key(|t| (t.width * t.height, clear(t)))?;
    // The plain dome's colour above the horizon (its vertices' average).
    let (mut sum, mut n) = ([0u32; 3], 0u32);
    for shell in sky.shells.iter().filter(|s| !s.textured()) {
        for c in &shell.clusters {
            for v in rc_formats::sky::sky_gs_vertices(shell, c) {
                if v.position[2] < 0 { continue; }
                for k in 0..3 { sum[k] += v.rgba[k] as u32; }
                n += 1;
            }
        }
    }
    let bg = if n > 0 { sum.map(|v| (v / n).min(255) as f32) } else { [0.0; 3] };
    let rgba = stars.rgba.chunks(4).flat_map(|p| {
        let a = p[3] as f32 / 255.0;
        [0, 1, 2].map(|k| (bg[k] * (1.0 - a) + p[k] as f32 * a) as u8).into_iter().chain([255])
    }).collect();
    Some(rc_formats::texture::Texture { width: stars.width, height: stars.height, rgba })
}

/// How many layers (shells) a level's sky has (read once).
fn sky_layer_count(ed: &mut Editor, level: u32) -> usize {
    if let Some(&n) = ed.sky_shells.get(&level) { return n; }
    let root = crate::level_load::extracted_root();
    let n = crate::map_load::level_core(&root, level).ok().and_then(|(core, data)| {
        let block = rc_formats::sky::sky_block(&core, &data).ok()??;
        rc_formats::sky::parse_sky_block(block).ok().map(|s| s.shells.len())
    }).unwrap_or(0);
    ed.sky_shells.insert(level, n);
    n
}

/// A square tile: a picture (or a spark without one) and a name under it, the full text on hover. Clicked: true.
fn picture_tile(ui: &mut egui::Ui, tex: Option<egui::TextureId>, label: &str, hint: &str, on: bool, size: f32) -> bool {
    let (rect, r) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    let orange = egui::Color32::from_rgb(0xf0, 0x8a, 0x24);
    let fill = if on { egui::Color32::from_rgb(0x4a, 0x35, 0x18) } else if r.hovered() { egui::Color32::from_rgb(0x3a, 0x4d, 0x75) } else { egui::Color32::from_rgb(0x2a, 0x37, 0x55) };
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 8.0, fill);
    if on { painter.rect_stroke(rect.shrink(1.0), 8.0, egui::Stroke::new(2.0, orange), egui::StrokeKind::Inside); }
    let font = egui::FontId::proportional(if size < 60.0 { 10.5 } else { 12.0 });
    let galley = painter.layout(label.to_string(), font, if on { orange } else { egui::Color32::from_gray(230) }, size - 6.0);
    let img = (size - galley.size().y - 12.0).min(size * 0.7);
    let img_rect = egui::Rect::from_center_size(egui::pos2(rect.center().x, rect.top() + 4.0 + img / 2.0), egui::vec2(img, img));
    match tex {
        Some(t) => egui::Image::new((t, egui::vec2(img, img))).corner_radius(4.0).paint_at(ui, img_rect),
        None => { painter.circle_stroke(img_rect.center(), img * 0.3, egui::Stroke::new(2.0, egui::Color32::from_gray(150))); }
    }
    painter.galley(egui::pos2(rect.center().x - galley.size().x / 2.0, rect.bottom() - 4.0 - galley.size().y), galley, egui::Color32::WHITE);
    r.on_hover_text(hint).clicked()
}

/// Square tiles in rows filling the width: `n` of them, `per_row` a row; `draw(ui, k, size)` draws tile k.
fn tile_rows(ui: &mut egui::Ui, n: usize, per_row: usize, mut draw: impl FnMut(&mut egui::Ui, usize, f32)) {
    let gap = 6.0;
    let size = ((ui.available_width() - gap * (per_row as f32 - 1.0)) / per_row as f32).floor().clamp(44.0, 110.0);
    for row in 0..n.div_ceil(per_row) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for k in row * per_row..((row + 1) * per_row).min(n) { draw(ui, k, size); }
        });
        ui.add_space(2.0);
    }
}

/// An island's or a path's colour (its panel): its own tint, and its texture's colours wherever that texture is used
/// (`World::recolour`). The texture's colours change the map at once (drawn again live).
fn colour_section(ui: &mut egui::Ui, ed: &mut Editor, tint: &mut Option<[u8; 3]>, texture: Option<AssetId>, what: &str) {
    egui::CollapsingHeader::new(egui::RichText::new("Colour").strong()).id_salt(("colour", what.to_string())).default_open(true).show(ui, |ui| {
        ui.horizontal(|ui| {
            let mut on = tint.is_some();
            if ui.checkbox(&mut on, format!("Tint this {what}")).on_hover_text(format!("Colours this {what} only (its light): darker, warmer, mossy...")).changed() {
                *tint = on.then_some([255, 220, 180]);
            }
            if let Some(c) = tint.as_mut() { ui.color_edit_button_srgb(c); }
        });
        let Some(id) = texture else { return };
        let k = ed.def.world.recolour.iter().position(|r| r.0 == id);
        let mut on = k.is_some();
        if ui.checkbox(&mut on, "Its texture's colours, everywhere").on_hover_text("Changes this texture on every island and path that uses it (World tab > Colours lists them all)").changed() {
            ed.checkpoint();
            if on { ed.def.world.recolour.push((id, rc_map::Palette::default())); } else if let Some(k) = k { ed.def.world.recolour.remove(k); }
            crate::map_load::set(ed.def.clone());
            ed.changed();
        }
        if let Some(k) = ed.def.world.recolour.iter().position(|r| r.0 == id) {
            let mut pal = ed.def.world.recolour[k].1;
            ui.add(egui::Slider::new(&mut pal.hue, -180.0..=180.0).suffix("°").text("hue"));
            ui.add(egui::Slider::new(&mut pal.saturation, 0.0..=2.0).text("saturation"));
            ui.add(egui::Slider::new(&mut pal.brightness, 0.3..=1.7).text("brightness"));
            ui.horizontal(|ui| {
                ui.color_edit_button_srgb(&mut pal.tint);
                ui.add(egui::Slider::new(&mut pal.tint_amount, 0.0..=1.0).text("tint"));
            });
            pal.hue = pal.hue.round();
            pal.saturation = (pal.saturation * 20.0).round() / 20.0;
            pal.brightness = (pal.brightness * 20.0).round() / 20.0;
            pal.tint_amount = (pal.tint_amount * 20.0).round() / 20.0;
            if pal != ed.def.world.recolour[k].1 {
                if !ed.coalescing { ed.checkpoint(); }
                ed.coalescing = true;
                ed.def.world.recolour[k].1 = pal;
                crate::map_load::set(ed.def.clone());
                ed.changed();
            }
        }
    });
}

/// A theme (design doc: "Theme presets"): a sky, a fog, a light, a palette and a song in one click.
struct Theme {
    name: &'static str,
    about: &'static str,
    sky: u32,
    fog: ([u8; 3], f32, f32),
    sun: (f32, f32, [u8; 3]),
    ambient: [u8; 3],
    shade: f32,
    /// Hue, saturation, brightness, tint, tint amount.
    palette: (f32, f32, f32, [u8; 3], f32),
    song: (u32, u32),
}

const THEMES: [Theme; 7] = [
    Theme { name: "Toxic", about: "Green haze, sickly light, Orxon's sky", sky: 10, fog: ([60, 140, 40], 20.0, 160.0), sun: (200.0, 40.0, [200, 255, 160]), ambient: [70, 110, 60], shade: 0.5, palette: (50.0, 1.3, 0.95, [180, 255, 150], 0.3), song: (10, 0) },
    Theme { name: "Frozen", about: "Pale blue, bright and cold, Kalebo's sky", sky: 16, fog: ([200, 215, 235], 40.0, 260.0), sun: (240.0, 35.0, [230, 240, 255]), ambient: [150, 160, 180], shade: 0.3, palette: (160.0, 0.45, 1.15, [200, 230, 255], 0.4), song: (16, 0) },
    Theme { name: "Volcanic", about: "Red smoke and ember light, Gaspar's sky", sky: 9, fog: ([120, 40, 20], 20.0, 200.0), sun: (300.0, 30.0, [255, 170, 110]), ambient: [90, 50, 40], shade: 0.6, palette: (-20.0, 1.2, 0.85, [255, 150, 100], 0.35), song: (9, 0) },
    Theme { name: "Night City", about: "Dark blue night, cool light, Aridia's sky", sky: 2, fog: ([10, 15, 40], 40.0, 260.0), sun: (60.0, 50.0, [120, 140, 200]), ambient: [40, 50, 80], shade: 0.5, palette: (0.0, 0.9, 0.7, [150, 170, 255], 0.4), song: (3, 2) },
    Theme { name: "Desert Noon", about: "High sun, warm sand, Pokitaru's sky", sky: 11, fog: ([230, 210, 170], 60.0, 320.0), sun: (180.0, 70.0, [255, 240, 210]), ambient: [140, 130, 110], shade: 0.4, palette: (15.0, 1.1, 1.1, [255, 230, 170], 0.3), song: (2, 0) },
    Theme { name: "Alien Dusk", about: "Pink and violet evening, Veldin's sky", sky: 0, fog: ([120, 60, 110], 30.0, 220.0), sun: (270.0, 15.0, [255, 140, 120]), ambient: [70, 50, 90], shade: 0.5, palette: (-60.0, 1.2, 0.95, [255, 170, 220], 0.3), song: (4, 0) },
    Theme { name: "Ruins", about: "Grey, dusty and still, Hoven's sky", sky: 12, fog: ([150, 150, 140], 30.0, 200.0), sun: (210.0, 45.0, [220, 210, 190]), ambient: [100, 100, 95], shade: 0.7, palette: (0.0, 0.6, 0.9, [220, 210, 180], 0.3), song: (8, 0) },
];

/// The World tab: the sky (another planet's, with its light and fog), the fog, the map's song, and the radio.
fn world_panel(ui: &mut egui::Ui, ed: &mut Editor, contexts: &mut EguiContexts, images: &mut Assets<Image>) {
    let orange = egui::Color32::from_rgb(0xf0, 0x8a, 0x24);
    let mut w = ed.def.world.clone();
    egui::ScrollArea::vertical().id_salt("world_tab").show(ui, |ui| {
        egui::CollapsingHeader::new(egui::RichText::new("Themes").strong().size(16.0)).id_salt("world_themes").default_open(true).show(ui, |ui| {
            ui.label(egui::RichText::new("A whole look in one click: sky, fog, light, colours and song. Change any of it after.").weak().small());
            let texs: Vec<Option<egui::TextureId>> = THEMES.iter().map(|t| globe_tex(ed, contexts, images, t.sky)).collect();
            tile_rows(ui, THEMES.len(), 4, |ui, k, size| {
                let t = &THEMES[k];
                if picture_tile(ui, texs[k], t.name, t.about, false, size) {
                    w.sky = Some(t.sky);
                    w.fog = Some(rc_map::Fog { color: t.fog.0, near: t.fog.1, far: t.fog.2 });
                    w.light = Some(rc_map::Light { sun_azimuth: t.sun.0, sun_elevation: t.sun.1, sun: t.sun.2, ambient: t.ambient, shade: t.shade });
                    let (hue, saturation, brightness, tint, tint_amount) = t.palette;
                    w.palette = Some(rc_map::Palette { hue, saturation, brightness, tint, tint_amount, objects: w.palette.is_some_and(|p| p.objects) });
                    w.music = Some(t.song);
                    w.silent = false;
                    ed.status = format!("Theme {}: change any part of it below", t.name);
                }
            });
            if ui.small_button("Back to the planet's own look").on_hover_text("No sky, fog, light or colour changes (the song stays)").clicked() {
                w.sky = None;
                w.fog = None;
                w.light = None;
                w.palette = None;
                w.sky_raise = 0.0;
                w.sky_turn = 0.0;
            }
        });
        ui.add_space(4.0);
        egui::CollapsingHeader::new(egui::RichText::new("Sky and light").strong().size(16.0)).id_salt("world_sky").default_open(true).show(ui, |ui| {
            ui.label(egui::RichText::new("Another planet's sky, with its background colour and fog.").weak().small());
            let host = ed.def.host_level;
            let levels: Vec<u32> = std::iter::once(host).chain((0..19u32).filter(|&l| l != host)).collect();
            let texs: Vec<Option<egui::TextureId>> = levels.iter().map(|&l| sky_tex(ed, contexts, images, l)).collect();
            tile_rows(ui, levels.len(), 4, |ui, k, size| {
                let l = levels[k];
                let on = if k == 0 { w.sky.is_none_or(|s| s == host) } else { w.sky == Some(l) };
                let name = LEVEL_NAMES[l as usize];
                let label = if k == 0 { format!("{name} (host)") } else { name.to_string() };
                if picture_tile(ui, texs[k], &label, &format!("{name}'s sky"), on, size) { w.sky = if k == 0 { None } else { Some(l) }; }
            });
            ui.add(egui::Slider::new(&mut w.sky_raise, -45.0..=45.0).suffix("°").text("raise")).on_hover_text("Moves the sky up (or down, below 0): its horizon higher or lower");
            ui.add(egui::Slider::new(&mut w.sky_turn, 0.0..=359.0).suffix("°").text("turn")).on_hover_text("Turns the sky round: where its sun, planets and mountains are");
            if (w.sky_raise != 0.0 || w.sky_turn != 0.0) && ui.small_button("Reset").clicked() { w.sky_raise = 0.0; w.sky_turn = 0.0; }
            // The sky's own animation: stars and shooting stars (some skies), layers that turn (drifting clouds).
            let sky_level = w.sky.unwrap_or(host);
            if rc_game::sky_stars::level_stars(sky_level).is_some() {
                let mut stars = !w.no_sky_stars;
                if ui.checkbox(&mut stars, "Stars and shooting stars").on_hover_text("This sky's twinkling stars and the ones that fly across it").changed() { w.no_sky_stars = !stars; }
            }
            if !crate::sky_render::level_rotations(sky_level).is_empty() {
                let mut turning = !w.sky_still;
                if ui.checkbox(&mut turning, "Clouds drift").on_hover_text("This sky's layers turn slowly (clouds, rings); off: they stand still").changed() { w.sky_still = !turning; }
            }
            // The sky's layers (shells): each one shown or hidden, and tinted.
            let n = sky_layer_count(ed, w.sky.unwrap_or(host));
            if n > 0 {
                egui::CollapsingHeader::new("Sky layers").id_salt("world_sky_layers").default_open(false).show(ui, |ui| {
                    ui.label(egui::RichText::new("The sky is drawn in layers (a dome, clouds, a sun, mountains...): hide or tint each.").weak().small());
                    if w.sky_layers.len() < n { w.sky_layers.resize(n, rc_map::SkyLayer::default()); }
                    w.sky_layers.truncate(n);
                    for (k, l) in w.sky_layers.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            let mut shown = !l.hidden;
                            if ui.checkbox(&mut shown, format!("Layer {}", k + 1)).changed() { l.hidden = !shown; }
                            ui.color_edit_button_srgb(&mut l.tint).on_hover_text("Its colour (white: as it is)");
                        });
                    }
                    if w.sky_layers.iter().all(|l| *l == rc_map::SkyLayer::default()) { w.sky_layers.clear(); }
                });
            }
            w.sky_raise = w.sky_raise.round();
            w.sky_turn = w.sky_turn.round();
        });
        ui.add_space(4.0);
        egui::CollapsingHeader::new(egui::RichText::new("Fog").strong().size(16.0)).id_salt("world_fog").default_open(false).show(ui, |ui| {
            let mut own = w.fog.is_some();
            if ui.checkbox(&mut own, "Own fog").on_hover_text("A colour the distance fades into, over the sky's own fog").changed() {
                w.fog = own.then_some(rc_map::Fog { color: [150, 170, 200], near: 40.0, far: 220.0 });
            }
            if let Some(f) = w.fog.as_mut() {
                ui.horizontal(|ui| {
                    ui.label("Colour");
                    ui.color_edit_button_srgb(&mut f.color);
                });
                ui.add(egui::Slider::new(&mut f.near, 0.0..=400.0).text("starts at"));
                ui.add(egui::Slider::new(&mut f.far, 10.0..=800.0).text("full at"));
                if f.far < f.near + 1.0 { f.far = f.near + 1.0; }
                f.near = f.near.round();
                f.far = f.far.round();
            }
        });
        ui.add_space(4.0);
        egui::CollapsingHeader::new(egui::RichText::new("Light").strong().size(16.0)).id_salt("world_light").default_open(false).show(ui, |ui| {
            ui.label(egui::RichText::new("How your islands, paths and pieces are shaded (the game's objects keep their own light).").weak().small());
            let mut own = w.light.is_some();
            if ui.checkbox(&mut own, "Own light").changed() { w.light = own.then(rc_map::Light::default); }
            if let Some(l) = w.light.as_mut() {
                ui.add(egui::Slider::new(&mut l.sun_azimuth, 0.0..=359.0).suffix("°").text("sun direction")).on_hover_text("Where the sun is, round the compass");
                ui.add(egui::Slider::new(&mut l.sun_elevation, 0.0..=90.0).suffix("°").text("sun height")).on_hover_text("Low: long, warm side light; high: noon");
                ui.horizontal(|ui| {
                    ui.label("Sun");
                    ui.color_edit_button_srgb(&mut l.sun);
                    ui.label("Ambient");
                    ui.color_edit_button_srgb(&mut l.ambient).on_hover_text("The light in the shade");
                });
                ui.add(egui::Slider::new(&mut l.shade, 0.0..=1.0).text("shade hollows")).on_hover_text("Darkens dips and the feet of slopes (ambient occlusion)");
                l.sun_azimuth = l.sun_azimuth.round();
                l.sun_elevation = l.sun_elevation.round();
                l.shade = (l.shade * 20.0).round() / 20.0;
            }
        });
        ui.add_space(4.0);
        egui::CollapsingHeader::new(egui::RichText::new("Colours").strong().size(16.0)).id_salt("world_colours").default_open(false).show(ui, |ui| {
            ui.label(egui::RichText::new("Shift the whole map's colours: hue, saturation, brightness and a tint.").weak().small());
            let mut own = w.palette.is_some();
            if ui.checkbox(&mut own, "Recolour").changed() { w.palette = own.then(rc_map::Palette::default); }
            if let Some(pal) = w.palette.as_mut() {
                ui.add(egui::Slider::new(&mut pal.hue, -180.0..=180.0).suffix("°").text("hue"));
                ui.add(egui::Slider::new(&mut pal.saturation, 0.0..=2.0).text("saturation"));
                ui.add(egui::Slider::new(&mut pal.brightness, 0.3..=1.7).text("brightness"));
                ui.horizontal(|ui| {
                    ui.label("Tint");
                    ui.color_edit_button_srgb(&mut pal.tint);
                    ui.add(egui::Slider::new(&mut pal.tint_amount, 0.0..=1.0).text("amount"));
                });
                ui.checkbox(&mut pal.objects, "Objects too").on_hover_text("Also the enemies, crates, pickups and Ratchet (their textures): off keeps them easy to tell apart");
                pal.hue = pal.hue.round();
                pal.saturation = (pal.saturation * 20.0).round() / 20.0;
                pal.brightness = (pal.brightness * 20.0).round() / 20.0;
                pal.tint_amount = (pal.tint_amount * 20.0).round() / 20.0;
            }
            // Material groups: one ground texture of the map recoloured wherever it is.
            let mut used: Vec<AssetId> = ed.def.patches.iter().filter_map(|p| p.texture).chain(ed.def.patches.iter().flat_map(|p| p.layers.iter().map(|l| l.texture))).chain(ed.def.walkways.iter().filter_map(|w| w.texture)).chain(ed.def.floors.iter().filter_map(|f| f.texture)).collect();
            used.sort_by_key(|a| (a.level, a.number));
            used.dedup();
            if !used.is_empty() {
                ui.add_space(4.0);
                ui.label(egui::RichText::new("One ground texture, everywhere it is used").strong());
                for id in used {
                    let k = w.recolour.iter().position(|r| r.0 == id);
                    let tex = ed.entry(&id).map(|e| e.thumb.clone()).and_then(|t| thumb(ed, contexts, images, &t, &mut 4));
                    ui.horizontal(|ui| {
                        if let Some(t) = tex { ui.add(egui::Image::new((t, egui::vec2(28.0, 28.0)))); }
                        let mut on = k.is_some();
                        if ui.checkbox(&mut on, id.to_string()).changed() {
                            if on { w.recolour.push((id, rc_map::Palette::default())); } else if let Some(k) = k { w.recolour.remove(k); }
                        }
                    });
                    if let Some(k) = w.recolour.iter().position(|r| r.0 == id) {
                        let pal = &mut w.recolour[k].1;
                        ui.indent(("recolour", k), |ui| {
                            ui.add(egui::Slider::new(&mut pal.hue, -180.0..=180.0).suffix("°").text("hue"));
                            ui.add(egui::Slider::new(&mut pal.saturation, 0.0..=2.0).text("saturation"));
                            ui.add(egui::Slider::new(&mut pal.brightness, 0.3..=1.7).text("brightness"));
                            ui.horizontal(|ui| {
                                ui.color_edit_button_srgb(&mut pal.tint);
                                ui.add(egui::Slider::new(&mut pal.tint_amount, 0.0..=1.0).text("tint"));
                            });
                        });
                        pal.hue = pal.hue.round();
                        pal.saturation = (pal.saturation * 20.0).round() / 20.0;
                        pal.brightness = (pal.brightness * 20.0).round() / 20.0;
                        pal.tint_amount = (pal.tint_amount * 20.0).round() / 20.0;
                    }
                }
            }
        });
        ui.add_space(4.0);
        egui::CollapsingHeader::new(egui::RichText::new("Lights").strong().size(16.0)).id_salt("world_lights").default_open(false).show(ui, |ui| {
            ui.label(egui::RichText::new("Point lights: they light your ground and the objects near them (a lamp, lava glow, a crystal).").weak().small());
            if ui.button("+ Add a light where the camera looks").clicked() { ed.light_add = true; }
            let mut drop = None;
            for (k, l) in w.lights.iter_mut().enumerate() {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.selectable_label(ed.light_sel == Some(k), egui::RichText::new(format!("Light {}", k + 1)).strong()).on_hover_text("Pick it: it shows in the world").clicked() {
                            ed.light_sel = if ed.light_sel == Some(k) { None } else { Some(k) };
                        }
                        ui.color_edit_button_srgb(&mut l.colour);
                        if ui.small_button("x").on_hover_text("Delete this light").clicked() { drop = Some(k); }
                    });
                    ui.horizontal(|ui| {
                        for (a, v) in l.position.iter_mut().enumerate() {
                            ui.add(egui::DragValue::new(v).speed(0.1).range(0.0..=1023.0)).on_hover_text(["x", "y", "height"][a]);
                        }
                    });
                    ui.add(egui::Slider::new(&mut l.radius, 2.0..=60.0).text("reach"));
                    ui.add(egui::Slider::new(&mut l.intensity, 0.1..=3.0).text("strength"));
                    l.radius = l.radius.round();
                    l.intensity = (l.intensity * 10.0).round() / 10.0;
                    for v in l.position.iter_mut() { *v = (*v * 4.0).round() / 4.0; }
                });
            }
            if let Some(k) = drop {
                w.lights.remove(k);
                ed.light_sel = None;
            }
            if w.lights.len() > 4 { ui.label(egui::RichText::new("Only the first 4 light the objects (the game has a few light slots); all of them light the ground.").weak().small()); }
        });
        ui.add_space(4.0);
        egui::CollapsingHeader::new(egui::RichText::new("Song").strong().size(16.0)).id_salt("world_song").default_open(true).show(ui, |ui| {
            ui.label(egui::RichText::new("What plays on the map. Pick a planet for its songs; ▶ listens (the radio).").weak().small());
            let root = crate::level_load::extracted_root();
            let songs = crate::map_load::songs(&root);
            let now = match (w.silent, w.music) {
                (true, _) => "Silence".to_string(),
                (false, None) => "The host's song".to_string(),
                (false, Some(m)) => songs.iter().find(|s| (s.0, s.1) == m).map_or("?".into(), |s| s.2.clone()),
            };
            ui.label(egui::RichText::new(format!("This map plays: {now}")).color(orange));
            ui.horizontal(|ui| {
                if ui.selectable_label(w.music.is_none() && !w.silent, "The host's song").clicked() { w.music = None; w.silent = false; }
                if ui.selectable_label(w.silent, "Silence").clicked() { w.music = None; w.silent = true; }
            });
            let planets: Vec<u32> = (0..19u32).filter(|l| songs.iter().any(|s| s.0 == *l)).collect();
            let texs: Vec<Option<egui::TextureId>> = planets.iter().map(|&l| globe_tex(ed, contexts, images, l)).collect();
            let mut pick = ed.song_planet;
            tile_rows(ui, planets.len(), 5, |ui, k, size| {
                let l = planets[k];
                let uses = w.music.is_some_and(|m| m.0 == l);
                if picture_tile(ui, texs[k], LEVEL_NAMES[l as usize], &format!("{}'s songs", LEVEL_NAMES[l as usize]), pick == Some(l) || (pick.is_none() && uses), size) {
                    pick = if pick == Some(l) { None } else { Some(l) };
                }
            });
            ed.song_planet = pick;
            if let Some(l) = pick {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(format!("{}'s songs", LEVEL_NAMES[l as usize])).strong());
                egui::Grid::new("songs").num_columns(3).striped(true).show(ui, |ui| {
                    for (k, (sl, t, name)) in songs.iter().enumerate().filter(|(_, s)| s.0 == l) {
                        let on = w.music == Some((*sl, *t));
                        if ui.small_button(if ed.radio == Some(k) { "■" } else { "▶" }).on_hover_text("Listen (the radio)").clicked() {
                            ed.radio_want = Some(if ed.radio == Some(k) { None } else { Some(k) });
                        }
                        let short = name.split(": ").nth(1).unwrap_or(name);
                        ui.label(if on { egui::RichText::new(short).color(orange).strong() } else { egui::RichText::new(short) });
                        if ui.add_enabled(!on, egui::Button::new("Use").small()).on_hover_text("This map plays it").clicked() {
                            w.music = Some((*sl, *t));
                            w.silent = false;
                        }
                        ui.end_row();
                    }
                });
            }
        });
    });
    if w != ed.def.world {
        if !ed.coalescing { ed.checkpoint(); }
        ed.coalescing = true;
        ed.def.world = w;
        // Live: the map's light, colours, lights and fog redrawn now; the sky and the objects' colours come with the
        // reload when the slider is let go.
        crate::map_load::set(ed.def.clone());
        ed.changed();
    }
}

/// A view button (crate::editor panels): the camera looks at the selected object (else the ground it faces, else 20
/// ahead) from that side, at a distance that fits the object.
fn snap_view(mut ed: ResMut<Editor>, mut cams: Query<(&mut Transform, &mut FlyCam)>) {
    // Set thumbnail: where the camera stands is kept in the map (a save takes the picture again from there).
    if std::mem::take(&mut ed.thumb_record) {
        if let Ok((t, fly)) = cams.single() {
            let p = t.translation;
            let view = Some([p.x, p.y, p.z, fly.yaw, fly.pitch].map(|v| (v * 1000.0).round() / 1000.0));
            if ed.def.thumb_view != view {
                ed.def.thumb_view = view;
                ed.saved = false;
            }
        }
    }
    // A retake: the camera to the picture's spot, the shot, then the camera back where it was.
    if std::mem::take(&mut ed.thumb_retake) {
        if let (Some(v), Ok((mut t, mut fly))) = (ed.def.thumb_view, cams.single_mut()) {
            ed.thumb_restore = Some((*t, fly.yaw, fly.pitch));
            t.translation = Vec3::new(v[0], v[1], v[2]);
            t.rotation = Quat::from_euler(EulerRot::YXZ, v[3], v[4], 0.0);
            (fly.yaw, fly.pitch) = (v[3], v[4]);
            ed.thumb_shot = Some((menu::thumb_path(&ed.path), 3));
        }
    } else if ed.thumb_shot.is_none() {
        if let (Some((rt, yaw, pitch)), Ok((mut t, mut fly))) = (ed.thumb_restore.take(), cams.single_mut()) {
            *t = rt;
            (fly.yaw, fly.pitch) = (yaw, pitch);
            ed.status = "Saved; the map's picture taken again from its spot".into();
        }
    }
    // A point light asked for in the World tab: on the ground the camera looks at, 2 units up (else 12 ahead).
    if std::mem::take(&mut ed.light_add) {
        if let Ok((t, _)) = cams.single() {
            let (eye, fwd) = (to_game(t.translation), to_game(t.forward().as_vec3()));
            let at = terrain_hit(&ed.def, (eye, fwd)).map(|h| [h.1[0], h.1[1], h.1[2] + 2.0]).unwrap_or([eye[0] + fwd[0] * 12.0, eye[1] + fwd[1] * 12.0, eye[2] + fwd[2] * 12.0]);
            ed.checkpoint();
            ed.def.world.lights.push(rc_map::PointLight { position: at.map(|v| (v * 4.0).round() / 4.0), colour: [255, 200, 130], radius: 14.0, intensity: 1.2 });
            ed.light_sel = Some(ed.def.world.lights.len() - 1);
            ed.status = "Light placed where the camera looks: colour, reach and place in the World tab".into();
            ed.changed();
        }
    }
    let Some(view) = ed.view_snap.take() else { return };
    let Ok((mut t, mut fly)) = cams.single_mut() else { return };
    let eye = to_game(t.translation);
    let fwd = to_game(t.forward().as_vec3());
    let (focus, size) = match ed.selected.filter(|&i| i < ed.def.objects.len()) {
        Some(i) => {
            let b = ed.box_of(i);
            ([(b[0] + b[3]) / 2.0, (b[1] + b[4]) / 2.0, (b[2] + b[5]) / 2.0], (b[3] - b[0]).max(b[4] - b[1]).max(b[5] - b[2]))
        }
        None => (terrain_hit(&ed.def, (eye, fwd)).map(|h| h.1).unwrap_or([eye[0] + fwd[0] * 20.0, eye[1] + fwd[1] * 20.0, eye[2] + fwd[2] * 20.0]), 6.0),
    };
    let d = (size * 2.5).clamp(8.0, 60.0);
    // Where the camera stands, from the focus (game axes: x east, y north, z up).
    let off = match view {
        "Top" => [0.0, -0.02 * d, d * 1.4],
        "Front" => [0.0, -d, d * 0.15],
        "Back" => [0.0, d, d * 0.15],
        "Left" => [-d, 0.0, d * 0.15],
        "Right" => [d, 0.0, d * 0.15],
        _ => [-d * 0.7, -d * 0.7, d * 0.7],
    };
    let at = [focus[0] + off[0], focus[1] + off[1], focus[2] + off[2]];
    *t = Transform::from_translation(game_to_bevy(at)).looking_at(game_to_bevy(focus), Vec3::Y);
    let speed = fly.speed;
    *fly = FlyCam::from_transform(&t, speed);
    ed.status = format!("{view} view");
}

/// The fly camera above and behind the start, looking at it (once).
fn start_camera(mut ed: ResMut<Editor>, mut cams: Query<(&mut Transform, &mut FlyCam)>, mut source: Option<ResMut<CameraSource>>) {
    if ed.camera_set { return; }
    let Ok((mut t, mut fly)) = cams.single_mut() else { return };
    let s = ed.def.start.position;
    let target = game_to_bevy(s);
    *t = Transform::from_translation(game_to_bevy([s[0] - 18.0, s[1] - 18.0, s[2] + 14.0])).looking_at(target, Vec3::Y);
    *fly = FlyCam::from_transform(&t, fly.speed.clamp(14.0, 24.0));
    if let Some(src) = source.as_deref_mut() { *src = CameraSource::Fly; }
    ed.camera_set = true;
}

// ------------------------------------------------------------------------------------------------
// Boxes.

/// An object's box in world units: its asset's local box turned by its yaw, as an axis-aligned box
/// `[min x, y, z, max x, y, z]` around the world.
fn world_box(local: [f32; 6], pos: [f32; 3], yaw: f32) -> [f32; 6] {
    let (s, c) = yaw.sin_cos();
    let mut b = [f32::MAX, f32::MAX, pos[2] + local[2], f32::MIN, f32::MIN, pos[2] + local[5]];
    for (x, y) in [(local[0], local[1]), (local[3], local[1]), (local[0], local[4]), (local[3], local[4])] {
        let (wx, wy) = (pos[0] + x * c - y * s, pos[1] + x * s + y * c);
        b[0] = b[0].min(wx);
        b[1] = b[1].min(wy);
        b[3] = b[3].max(wx);
        b[4] = b[4].max(wy);
    }
    b
}

fn overlaps(a: &[f32; 6], b: &[f32; 6]) -> bool { (0..3).all(|k| a[k] < b[k + 3] - TOUCH && b[k] < a[k + 3] - TOUCH) }

/// Where a ray enters a box: the distance along it and the face's outward normal (axis 0..3, sign).
fn ray_box((o, d): ([f32; 3], [f32; 3]), b: &[f32; 6]) -> Option<(f32, usize, f32)> {
    let (mut near, mut far, mut face) = (f32::MIN, f32::MAX, (2, 1.0));
    for k in 0..3 {
        if d[k].abs() < 1e-8 {
            if o[k] < b[k] || o[k] > b[k + 3] { return None; }
            continue;
        }
        let (t0, t1) = ((b[k] - o[k]) / d[k], (b[k + 3] - o[k]) / d[k]);
        let (lo, hi, sign) = if t0 < t1 { (t0, t1, -1.0) } else { (t1, t0, 1.0) };
        if lo > near {
            near = lo;
            face = (k, sign);
        }
        far = far.min(hi);
    }
    (near <= far && far > 0.0 && near > 0.0).then_some((near, face.0, face.1))
}

impl Editor {
    fn snap(&self) -> Snap { Snap { def: self.def.clone(), ids: self.ids.clone() } }
    fn restore(&mut self, s: Snap) -> Snap {
        self.last_built = None;
        let now = self.snap();
        (self.def, self.ids) = (s.def, s.ids);
        now
    }
    /// Records the map before a change (undo), clears the redo list.
    fn checkpoint(&mut self) {
        self.undo.push(self.snap());
        if self.undo.len() > UNDO { self.undo.remove(0); }
        self.redo.clear();
    }
    /// The change is made: reload it.
    fn changed(&mut self) {
        self.saved = false;
        match self.reload {
            Reload::Idle => self.reload = Reload::Wanted,
            Reload::Loading(_) => self.reload_again = true,
            Reload::Wanted => {}
        }
    }
    fn add_object(&mut self, o: Object) -> usize {
        self.def.objects.push(o);
        self.ids.push(self.next_id);
        self.next_id += 1;
        self.def.objects.len() - 1
    }
    fn remove_object(&mut self, i: usize) -> Object {
        self.ids.remove(i);
        self.def.objects.remove(i)
    }
    fn undo(&mut self) {
        if let Some(s) = self.undo.pop() {
            let now = self.restore(s);
            self.redo.push(now);
            self.selected = None;
            self.changed();
            self.status = "Undone".into();
        }
    }
    fn redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            let now = self.restore(s);
            self.undo.push(now);
            self.selected = None;
            self.changed();
            self.status = "Redone".into();
        }
    }
    /// Writes `def` as a backup of this map (`auto`: unsaved work, else the version a save replaces), keeping 30.
    fn backup(&self, text: &str, auto: bool) {
        let dir = backup_dir(&self.path);
        let name = format!("{}{}.json", stamp(now_secs()).1, if auto { "-auto" } else { "" });
        if std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join(name), text)).is_err() { return; }
        for (old, _) in backups(&self.path).into_iter().skip(30) { let _ = std::fs::remove_file(old); }
    }
    /// Opens map file `path` (another map: its host level loads), keeping a backup of unsaved work first.
    fn open(&mut self, path: PathBuf) {
        if !self.saved { self.backup(&self.def.to_json(), true); }
        let def = match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| MapDef::parse(&t).map_err(|e| e.to_string())) {
            Ok(d) => d,
            Err(e) => {
                self.status = format!("Could not open {}: {e}", path.display());
                return;
            }
        };
        self.switch_to(path, def, true);
        self.status = format!("Opened {}", self.path.display());
    }
    /// `def` as the map being edited, saved as `path` (`saved`: it is that file's content).
    fn switch_to(&mut self, path: PathBuf, mut def: MapDef, saved: bool) {
        self.last_built = None;
        crate::map_load::fit_edges(&mut def);
        fit_path_openings(&mut def);
        self.ids = (0..def.objects.len() as u64).map(|i| i + self.next_id).collect();
        self.next_id += def.objects.len() as u64;
        // Nothing of the new map is loaded yet: every object shows as new until its reload.
        self.loaded = Snap { def: def.clone(), ids: Vec::new() };
        self.def = def;
        self.path = path;
        self.saved = saved;
        self.undo.clear();
        self.redo.clear();
        (self.selected, self.armed, self.patch, self.floor_edit) = (None, None, None, None);
        self.settings.opened(&self.path.clone());
        self.camera_set = false;
        self.changed();
        self.saved = saved;
    }
    /// A new map in the maps folder (`Map N.json`), on the same planet.
    fn new_map(&mut self) {
        let dir = maps_dir();
        let path = (1..).map(|n| dir.join(format!("Map {n}.json"))).find(|p| !p.exists()).unwrap_or_else(|| dir.join("Map.json"));
        if !self.saved { self.backup(&self.def.to_json(), true); }
        let name = path.file_stem().map_or("New map".into(), |s| s.to_string_lossy().into_owned());
        let def = MapDef::new_default(&name, self.def.host_level);
        self.switch_to(path, def, false);
        self.save();
        self.status = format!("New map {}", self.path.display());
    }
    fn save(&mut self) {
        self.group_crates();
        self.fill_solid();
        // The version on disk becomes a backup first.
        if let Ok(old) = std::fs::read_to_string(&self.path) {
            if old != self.def.to_json() { self.backup(&old, false); }
        }
        let r = self.path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&self.path, self.def.to_json()));
        self.warnings = map_warnings(&self.def);
        self.status = match r {
            Ok(()) if !self.warnings.is_empty() => {
                self.saved = true;
                format!("Saved, with {} warnings (Maps tab)", self.warnings.len())
            }
            Ok(()) => {
                self.saved = true;
                format!("Saved {}", self.path.display())
            }
            Err(e) => format!("Not saved: {e}"),
        };
        // The map's picture taken again from where it was set (it shows the edits since).
        if self.saved && self.def.thumb_view.is_some() && !self.settings.no_thumb_on_save && self.thumb_shot.is_none() { self.thumb_retake = true; }
    }
    /// An enemy's wake and attack ranges ([`SIGHT`]: a template pvar or the class's constant; 0 = none): the circles of
    /// where it notices Ratchet and where it attacks. None for classes not mapped.
    fn sight(&mut self, id: AssetId) -> Option<(f32, f32)> {
        let &(_, wake, attack) = SIGHT.iter().find(|s| s.0 == id.number)?;
        if let Some(r) = self.ranges.get(&id) { return *r; }
        let pvar = matches!(wake, Range::Pvar(_)) || matches!(attack, Range::Pvar(_));
        let root = crate::level_load::extracted_root();
        let p = pvar.then(|| rc_data::level_gameplay(&root, id.level).ok().and_then(|g| rc_map::patch::TemplateRecord::first_of_class(&g, id.number).ok()).and_then(|t| t.pvar)).flatten();
        let get = |r: Range| match r {
            Range::Pvar(o) => p.as_ref().and_then(|p| p.get(o..o + 4)).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).filter(|v| v.is_finite() && *v >= 0.0 && *v < 1000.0),
            Range::Const(v) => Some(v),
            Range::None => Some(0.0),
        };
        // The amoeboids' 0 means 15.
        let w = get(wake).map(|v| if v == 0.0 && [572, 865, 866].contains(&id.number) { 15.0 } else { v });
        let r = w.zip(get(attack));
        self.ranges.insert(id, r);
        r
    }
    /// The catalog entry of an object's asset.
    fn entry(&self, id: &AssetId) -> Option<&Entry> { self.catalog.as_deref().and_then(|c| c.find(id)) }
    fn name_of(&self, id: &AssetId) -> String {
        self.entry(id).filter(|e| !e.name.is_empty()).map_or_else(|| id.to_string(), |e| e.name.clone())
    }
    /// An asset's box around its origin.
    fn local_box(&self, id: &AssetId) -> [f32; 6] { self.entry(id).and_then(|e| e.bounds).unwrap_or(DEFAULT_BOX) }
    fn box_of(&self, i: usize) -> [f32; 6] {
        let o = &self.def.objects[i];
        world_box(self.local_box(&o.asset), o.position, o.yaw)
    }
    /// The id to place an entry as: on the host when it carries the same class, else the first level that places it
    /// (its instance is the template).
    fn placeable_id(&mut self, e: usize) -> Option<AssetId> {
        let entry = self.catalog.as_ref()?.entries.get(e)?.clone();
        let host = self.def.host_level;
        let root = crate::level_load::extracted_root();
        let mut places = |l: u32, o: i32| -> bool {
            self.placed_by_level
                .entry(l)
                .or_insert_with(|| {
                    rc_data::level_gameplay(&root, l).ok().and_then(|g| rc_formats::gameplay::parse_moby_instances(&g).ok()).map_or_else(Vec::new, |v| v.iter().map(|m| m.o_class).collect())
                })
                .contains(&o)
        };
        let ids: Vec<AssetId> = entry.ids().copied().collect();
        ids.iter().find(|i| i.level == host && places(i.level, i.number)).or_else(|| ids.iter().find(|i| places(i.level, i.number))).copied()
    }
    /// `pos` moved out of every other object a box `local` (turned by `yaw`) there would overlap: pushed the
    /// shortest way sideways (or up onto it when that is shorter), a few rounds.
    fn clear_of(&self, skip: Option<usize>, local: [f32; 6], mut pos: [f32; 3], yaw: f32) -> [f32; 3] {
        for _ in 0..8 {
            let b = world_box(local, pos, yaw);
            let Some(o) = (0..self.def.objects.len()).filter(|&j| Some(j) != skip).map(|j| self.box_of(j)).find(|o| overlaps(&b, o)) else { break };
            // The four ways out sideways, and up on top.
            let moves = [(0, o[3] - b[0]), (0, o[0] - b[3]), (1, o[4] - b[1]), (1, o[1] - b[4]), (2, o[5] - b[2])];
            let (k, d) = moves.into_iter().min_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).unwrap_or((0, 0.0));
            pos[k] += d;
        }
        pos
    }
    /// Where a click places a new object of box `local` (yaw 0): on the top of the object the ray hits there, next
    /// to it on the side the ray hits, else on the floor under the ray (out of anything there).
    fn placement(&self, ray: ([f32; 3], [f32; 3]), local: [f32; 6]) -> Option<[f32; 3]> {
        let local = world_box(local, [0.0; 3], self.place_yaw);
        let lift = -local[2].min(0.0);
        let hit = (0..self.def.objects.len()).filter_map(|j| ray_box(ray, &self.box_of(j)).map(|h| (j, h))).min_by(|a, b| a.1 .0.total_cmp(&b.1 .0));
        if let Some((j, (_, axis, sign))) = hit {
            let (o, b) = (&self.def.objects[j], self.box_of(j));
            let p = match (axis, sign > 0.0) {
                (2, true) => Some([o.position[0], o.position[1], b[5] + lift]),
                (2, false) => None,
                (k, up) => {
                    let mut p = o.position;
                    p[k] = if up { b[k + 3] - local[k] } else { b[k] - local[k + 3] };
                    Some(p)
                }
            };
            if let Some(p) = p { return Some(self.clear_of(None, local, p, 0.0)); }
        }
        let p = terrain_hit(&self.def, ray).map(|h| h.1).or_else(|| on_plane(ray, ground(&self.def, ray.0[0], ray.0[1], f32::MAX)))?;
        let z = ground(&self.def, p[0], p[1], p[2] + 0.5) + lift;
        Some(self.clear_of(None, local, [p[0], p[1], z], 0.0))
    }
    /// Applies the brush for `dt` seconds at `at` (game units) to every patch under it, and hands the map to the
    /// renderer (the collision follows at the stroke's end).
    fn sculpt(&mut self, at: [f32; 3], dt: f32, invert: bool) {
        let (r, rate, square) = (self.radius, self.strength, self.square);
        let invert = invert != self.invert;
        let brush = match (self.brush, invert) {
            (Brush::Raise, true) => Brush::Lower,
            (Brush::Lower, true) => Brush::Raise,
            (b, _) => b,
        };
        let start = self.stroke.unwrap_or(at[2]);
        let top = if invert { start - self.plateau } else { start + self.plateau };
        let paint = self.paint;
        let (shade, scatter, scatter_size) = (self.shade, self.scatter, self.scatter_size);
        let mut seed = self.seed;
        for p in &mut self.def.patches {
            if brush == Brush::Gaps {
                if p.edge.is_some() { p.open_near([at[0], at[1]], r, invert); }
                continue;
            }
            if brush == Brush::Scatter {
                let max = p.max();
                if at[0] + r < p.origin[0] || at[0] - r > max[0] || at[1] + r < p.origin[1] || at[1] - r > max[1] { continue; }
                let inside = |x: f32, y: f32| { let (dx, dy) = ((x - at[0]).abs(), (y - at[1]).abs()); if square { dx.max(dy) < r } else { dx * dx + dy * dy < r * r } };
                if invert {
                    p.scatter.retain(|s| !inside(s.at[0], s.at[1]));
                    continue;
                }
                let Some(asset) = scatter else { continue };
                // About `strength` × 8 a second, each where there is land and nothing within its own size.
                let mut tries = (rate * 8.0 * dt + (next(&mut seed) as f32 / u32::MAX as f32)).floor() as usize;
                while tries > 0 && p.scatter.len() < 4000 {
                    tries -= 1;
                    let (u, v) = (next(&mut seed) as f32 / u32::MAX as f32 * 2.0 - 1.0, next(&mut seed) as f32 / u32::MAX as f32 * 2.0 - 1.0);
                    let (x, y) = (at[0] + u * r, at[1] + v * r);
                    if !inside(x, y) || p.height_at(x, y).is_none() { continue; }
                    let scale = scatter_size * (0.8 + 0.4 * next(&mut seed) as f32 / u32::MAX as f32);
                    if p.scatter.iter().any(|s| (s.at[0] - x).powi(2) + (s.at[1] - y).powi(2) < (0.6 * scale).powi(2)) { continue; }
                    let yaw = next(&mut seed) as f32 / u32::MAX as f32 * std::f32::consts::TAU;
                    let q = |v: f32| (v * 64.0).round() / 64.0;
                    p.scatter.push(rc_map::terrain::Scatter { asset, at: [q(x), q(y)], yaw: (yaw * 100.0).round() / 100.0, scale: (scale * 100.0).round() / 100.0 });
                }
                continue;
            }
            if brush == Brush::Land {
                let max = p.max();
                if at[0] + r < p.origin[0] || at[0] - r > max[0] || at[1] + r < p.origin[1] || at[1] - r > max[1] { continue; }
                // A patch without a shape gets one: all land, its border water.
                if p.mask.is_empty() {
                    p.mask = vec![rc_map::terrain::LAND_MAX; p.heights.len()];
                    p.quantize_mask();
                }
                let (nx, ny) = p.points();
                for j in 0..ny {
                    for i in 0..nx {
                        let (x, y) = (p.origin[0] + i as f32 * p.spacing, p.origin[1] + j as f32 * p.spacing);
                        let (dx, dy) = ((x - at[0]).abs(), (y - at[1]).abs());
                        let d = if square { dx.max(dy) } else { (dx * dx + dy * dy).sqrt() };
                        if d >= r { continue; }
                        let w = (1.0 - (d / r).powi(2)).powi(2);
                        let k = p.at(i, j);
                        let step = rate * 16.0 * dt * w;
                        p.mask[k] = if invert { p.mask[k] - step } else { p.mask[k] + step };
                    }
                }
                p.quantize_mask();
                continue;
            }
            let (nx, ny) = p.points();
            let max = p.max();
            if at[0] + r < p.origin[0] || at[0] - r > max[0] || at[1] + r < p.origin[1] || at[1] - r > max[1] { continue; }
            let before = p.heights.clone();
            // The paint layer (made on first use), unless the texture is the patch's own (painting it rubs out the rest).
            let layer = match (brush, paint) {
                (Brush::Paint, Some(t)) if Some(t) != p.texture => Some(match p.layers.iter().position(|l| l.texture == t) {
                    Some(k) => k,
                    None => {
                        p.layers.push(rc_map::terrain::PaintLayer { texture: t, tile: p.tile, weights: vec![0; nx * ny] });
                        p.layers.len() - 1
                    }
                }),
                _ => None,
            };
            for j in 0..ny {
                for i in 0..nx {
                    let (x, y) = (p.origin[0] + i as f32 * p.spacing, p.origin[1] + j as f32 * p.spacing);
                    let (dx, dy) = ((x - at[0]).abs(), (y - at[1]).abs());
                    let d = if square { dx.max(dy) } else { (dx * dx + dy * dy).sqrt() };
                    if d >= r { continue; }
                    // A soft brush: full in the middle, nothing at the edge (the Plateau: full out to 70 % of it).
                    let w = (1.0 - (d / r).powi(2)).powi(2);
                    let k = p.at(i, j);
                    let h = before[k];
                    let blend = (rate * dt * 8.0).min(1.0) * w;
                    if brush == Brush::Shade {
                        if p.shade.is_empty() { p.shade = vec![0xffffff; nx * ny]; }
                        let goal = if invert { [255u8; 3] } else { shade };
                        let c = p.shade[k];
                        let ch = |c: u32, s: u32, g: u8| { let v = ((c >> s) & 0xff) as f32; (v + (g as f32 - v) * (blend * 0.6)).round().clamp(0.0, 255.0) as u32 };
                        p.shade[k] = ch(c, 16, goal[0]) << 16 | ch(c, 8, goal[1]) << 8 | ch(c, 0, goal[2]);
                        continue;
                    }
                    if brush == Brush::Paint {
                        let amount = (rate * dt * 600.0 * w).round() as i32;
                        for (n, l) in p.layers.iter_mut().enumerate() {
                            let mine = Some(n) == layer;
                            let v = l.weights[k] as i32;
                            l.weights[k] = match (mine, invert) {
                                (true, false) => v + amount,
                                (true, true) => v - amount,
                                // Under the new paint (or all of them when painting the patch's own texture back).
                                (false, false) => v - amount,
                                (false, true) => v,
                            }
                            .clamp(0, 255) as u8;
                        }
                        continue;
                    }
                    p.heights[k] = match brush {
                        Brush::Raise => h + rate * 6.0 * dt * w,
                        Brush::Lower => h - rate * 6.0 * dt * w,
                        Brush::Plateau => {
                            let wf = ((1.0 - d / r) / 0.3).min(1.0);
                            let goal = top - p.origin[2];
                            let step = rate * 12.0 * dt * wf;
                            if invert { if h > goal { (h - step).max(goal) } else { h } } else if h < goal { (h + step).min(goal) } else { h }
                        }
                        Brush::Smooth => {
                            let n = |a: usize, b: usize| before[p.at(a, b)];
                            let avg = (n(i.saturating_sub(1), j) + n((i + 1).min(nx - 1), j) + n(i, j.saturating_sub(1)) + n(i, (j + 1).min(ny - 1))) / 4.0;
                            h + (avg - h) * blend
                        }
                        Brush::Flatten => h + (start - p.origin[2] - h) * blend,
                        Brush::Paint | Brush::Land | Brush::Shade | Brush::Scatter | Brush::Gaps => h,
                    };
                    // Inside the world.
                    p.heights[k] = p.heights[k].clamp(1.0 - p.origin[2], 1000.0 - p.origin[2]);
                }
            }
            // A layer painted out everywhere goes; so does a shade back to white everywhere.
            p.layers.retain(|l| l.weights.iter().any(|&w| w > 0));
            if p.shade.iter().all(|&c| c == 0xffffff) { p.shade.clear(); }
            p.quantize();
        }
        self.seed = seed;
        self.saved = false;
        crate::map_load::set(self.def.clone());
    }
    /// Builds the island ([`Editor::island`]) centred on ground point `at`: its top `height` above it, selected.
    fn build_island(&mut self, at: [f32; 3]) {
        let spec = self.island.clone();
        let top = [at[0], at[1], (at[2] + spec.height.max(0.25)).clamp(1.0, 1000.0)];
        let mut p = rc_map::terrain::Patch::island(top, spec.spacing(), spec.size / 2.0, |x, y| spec.land(x, y));
        p.texture = spec.texture;
        p.depth = spec.depth;
        p.tile = 8.0;
        p.edge = spec.edge.clone();
        if let Err(e) = p.check() {
            self.status = format!("No island there: {e}");
            return;
        }
        self.checkpoint();
        self.def.patches.push(p);
        self.patch = Some(self.def.patches.len() - 1);
        self.last_built = self.patch;
        // The next blob differs.
        self.island.seed = self.island.seed.wrapping_mul(1103515245).wrapping_add(12345);
        self.status = format!("Island built: {:.0} units across, {:.0} up. Shape: drag its coast, move it, lift it", spec.size, spec.height);
        self.changed();
    }
    /// Each object's `solid` decided where the map leaves it open: on for the scenery, platforms and other pieces that
    /// stay put (on the disc most of them are solid through the level geometry around them: a walk into the shells'
    /// wall 709, the big wall 729, the hinged bridge 746 or an item display 1461 goes straight through, a crate stops
    /// Ratchet), off for the enemies, the pickups, the effects, the path classes and whatever moves by its name (baked
    /// collision stays where it was put).
    fn fill_solid(&mut self) {
        const MOVES: [&str; 12] = ["hinge", "lift", "elevator", "door", "slid", "crank", "spinner", "rotat", "mover", "path", "swing", "turn"];
        for i in 0..self.def.objects.len() {
            if self.def.objects[i].solid.is_some() { continue; }
            let id = self.def.objects[i].asset;
            let solid = !crate::map_load::PATH_CLASSES.contains(&id.number)
                && self.entry(&id).is_some_and(|e| {
                    let name = e.name.to_lowercase();
                    [rc_map::catalog::KIND_SCENERY, rc_map::catalog::KIND_PLATFORM, rc_map::catalog::KIND_OTHER].contains(&e.kind.as_str()) && !MOVES.iter().any(|w| name.contains(w))
                });
            self.def.objects[i].solid = Some(solid);
        }
    }
    /// The crates' groups: every set of touching crates (stacked or side by side) is one group, numbered after the
    /// groups other objects carry; a lone crate has none. Before every load and save.
    fn group_crates(&mut self) {
        let crates: Vec<usize> = (0..self.def.objects.len()).filter(|&i| CRATES.contains(&self.def.objects[i].asset.number)).collect();
        let boxes: Vec<[f32; 6]> = crates.iter().map(|&i| self.box_of(i)).collect();
        let touch = |a: &[f32; 6], b: &[f32; 6]| (0..3).all(|k| a[k] <= b[k + 3] + 0.05 && b[k] <= a[k + 3] + 0.05);
        // Union-find over the crates.
        let mut root: Vec<usize> = (0..crates.len()).collect();
        fn find(r: &mut [usize], i: usize) -> usize { if r[i] == i { i } else { let p = find(r, r[i]); r[i] = p; p } }
        for a in 0..crates.len() {
            for b in a + 1..crates.len() {
                if touch(&boxes[a], &boxes[b]) {
                    let (ra, rb) = (find(&mut root, a), find(&mut root, b));
                    root[ra] = rb;
                }
            }
        }
        let mut next = self.def.objects.iter().enumerate().filter(|(i, _)| !crates.contains(i)).filter_map(|(_, o)| o.group).max().map_or(0, |g| g + 1);
        let mut numbers: HashMap<usize, u8> = HashMap::new();
        for k in 0..crates.len() {
            let r = find(&mut root, k);
            let alone = (0..crates.len()).filter(|&j| find(&mut root, j) == r).count() < 2;
            self.def.objects[crates[k]].group = if alone || next > 126 {
                None
            } else {
                Some(*numbers.entry(r).or_insert_with(|| {
                    next += 1;
                    next - 1
                }))
            };
        }
    }
    /// `pos` lowered (or raised) onto what is under a box `local` there: the floor, or the highest object top below
    /// it whose footprint it shares (the game drops a crate left in the air).
    fn settle(&self, skip: Option<usize>, local: [f32; 6], pos: [f32; 3], yaw: f32) -> [f32; 3] {
        let b = world_box(local, pos, yaw);
        let mut support = ground(&self.def, pos[0], pos[1], b[2] + 0.5);
        for j in (0..self.def.objects.len()).filter(|&j| Some(j) != skip) {
            let o = self.box_of(j);
            let shares = b[0] < o[3] - TOUCH && o[0] < b[3] - TOUCH && b[1] < o[4] - TOUCH && o[1] < b[4] - TOUCH;
            if shares && o[5] <= b[2] + TOUCH { support = support.max(o[5]); }
        }
        [pos[0], pos[1], support - local[2].min(0.0)]
    }
    /// A copy of object `i` next to it along `dir` (a unit world axis; z up: on top), past any copies already there.
    fn add_beside(&mut self, i: usize, dir: [f32; 3]) {
        let mut copy = self.def.objects[i].clone();
        let b = self.box_of(i);
        let step = [b[3] - b[0], b[4] - b[1], b[5] - b[2]];
        let local = self.local_box(&copy.asset);
        for _ in 0..64 {
            for k in 0..3 { copy.position[k] += dir[k] * step[k]; }
            // Beside it: standing on what is there (a copy beside a stacked crate goes down to the floor).
            let at = if dir[2] == 0.0 { self.settle(None, local, copy.position, copy.yaw) } else { copy.position };
            if !(0..self.def.objects.len()).any(|j| overlaps(&world_box(local, at, copy.yaw), &self.box_of(j))) {
                copy.position = at;
                break;
            }
        }
        self.checkpoint();
        let name = self.name_of(&copy.asset);
        self.selected = Some(self.add_object(copy));
        self.status = format!("Added {name}");
        self.changed();
    }
}

/// The pick ray in game space from the cursor (window → game frame → world camera).
fn cursor_ray(window: &Window, frame: &crate::display::GameFrame, cam: (&Camera, &GlobalTransform)) -> Option<([f32; 3], [f32; 3])> {
    let c = window.cursor_position()?;
    let win = Vec2::new(window.width(), window.height());
    let shown = crate::display::fit(frame.size, win);
    let off = (win - shown) / 2.0;
    let px = (c - off) * frame.size.as_vec2() / shown;
    if px.x < 0.0 || px.y < 0.0 || px.x > frame.size.x as f32 || px.y > frame.size.y as f32 { return None; }
    let ray = cam.0.viewport_to_world(cam.1, px).ok()?;
    Some((to_game(ray.origin), to_game(*ray.direction)))
}

/// A game-space point on the window (logical pixels: egui's points), the inverse of [`cursor_ray`].
fn to_window(window: &Window, frame: &crate::display::GameFrame, cam: (&Camera, &GlobalTransform), p: [f32; 3]) -> Option<Vec2> {
    let px = cam.0.world_to_viewport(cam.1, game_to_bevy(p)).ok()?;
    let win = Vec2::new(window.width(), window.height());
    let shown = crate::display::fit(frame.size, win);
    Some((win - shown) / 2.0 + px * shown / frame.size.as_vec2())
}

/// Where the ray meets the horizontal plane at height `z` (game units).
fn on_plane((o, d): ([f32; 3], [f32; 3]), z: f32) -> Option<[f32; 3]> {
    if d[2].abs() < 1e-5 { return None; }
    let t = (z - o[2]) / d[2];
    (t > 0.0).then(|| [o[0] + d[0] * t, o[1] + d[1] * t, z])
}

/// The object whose box the ray enters first.
fn pick(ed: &Editor, ray: ([f32; 3], [f32; 3])) -> Option<usize> {
    (0..ed.def.objects.len()).filter_map(|j| ray_box(ray, &ed.box_of(j)).map(|h| (j, h.0))).min_by(|a, b| a.1.total_cmp(&b.1)).map(|h| h.0)
}

/// The ground under (x, y) at or below `below`: the highest floor or patch top there (a cliff top above is not it),
/// else the lowest one, else the start's height.
fn ground(def: &MapDef, x: f32, y: f32, below: f32) -> f32 {
    let floors = def.floors.iter().filter(|f| (f.min[0]..f.max[0]).contains(&x) && (f.min[1]..f.max[1]).contains(&y)).map(|f| f.height);
    let tops: Vec<f32> = floors.chain(def.patches.iter().filter_map(|p| p.height_at(x, y))).chain(def.walkway_pieces().iter().filter_map(|w| w.height_at(x, y))).collect();
    tops.iter().copied().filter(|&z| z <= below).fold(None, |m: Option<f32>, z| Some(m.map_or(z, |m| m.max(z))))
        .or_else(|| tops.iter().copied().reduce(f32::min))
        .unwrap_or(def.start.position[2] - 0.5)
}

/// Where a ray first meets a triangle (Möller–Trumbore; both sides): the distance along it.
fn ray_tri((o, d): ([f32; 3], [f32; 3]), t: &[[f32; 3]; 3]) -> Option<f32> {
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let (e1, e2) = (sub(t[1], t[0]), sub(t[2], t[0]));
    let p = cross(d, e2);
    let det = dot(e1, p);
    if det.abs() < 1e-8 { return None; }
    let s = sub(o, t[0]);
    let u = dot(s, p) / det;
    if !(0.0..=1.0).contains(&u) { return None; }
    let q = cross(s, e1);
    let v = dot(d, q) / det;
    if v < 0.0 || u + v > 1.0 { return None; }
    let t = dot(e2, q) / det;
    (t > 0.0).then_some(t)
}

/// Where the ray first meets the terrain (a patch's top or a floor): the distance, the point, and the patch (None: a
/// floor).
fn terrain_hit(def: &MapDef, ray: ([f32; 3], [f32; 3])) -> Option<(f32, [f32; 3], Option<usize>)> {
    let (o, d) = ray;
    let mut best: Option<(f32, Option<usize>)> = None;
    for (k, p) in def.patches.iter().enumerate() {
        let max = p.max();
        let top = p.heights.iter().fold(f32::MIN, |m, &h| m.max(h)) + p.origin[2];
        let lowest = p.heights.iter().fold(f32::MAX, |m, &h| m.min(h)) + p.origin[2];
        if ray_box(ray, &[p.origin[0], p.origin[1], lowest - 0.01, max[0], max[1], top + 0.01]).is_none()
            && !(o[0] >= p.origin[0] && o[0] <= max[0] && o[1] >= p.origin[1] && o[1] <= max[1] && o[2] >= lowest && o[2] <= top)
        {
            continue;
        }
        for t in p.top() {
            if let Some(h) = ray_tri(ray, &t) {
                if best.is_none_or(|b| h < b.0) { best = Some((h, Some(k))); }
            }
        }
    }
    for w in &def.walkway_pieces() {
        for t in w.top_tris() {
            if let Some(h) = ray_tri(ray, &t) {
                if best.is_none_or(|b| h < b.0) { best = Some((h, None)); }
            }
        }
    }
    for f in &def.floors {
        if let Some(q) = on_plane(ray, f.height) {
            let h = ((q[0] - o[0]).powi(2) + (q[1] - o[1]).powi(2) + (q[2] - o[2]).powi(2)).sqrt();
            if (f.min[0]..f.max[0]).contains(&q[0]) && (f.min[1]..f.max[1]).contains(&q[1]) && best.is_none_or(|b| h < b.0) { best = Some((h, None)); }
        }
    }
    best.map(|(t, k)| (t, [o[0] + d[0] * t, o[1] + d[1] * t, o[2] + d[2] * t], k))
}

/// Editing input is taken (not playing, not waiting for play to start).
fn editing(ed: &Editor, mode: EditorMode) -> bool { mode == EditorMode::Editing && !ed.play_pending }

#[allow(clippy::too_many_arguments)]
fn viewport(
    mut ed: ResMut<Editor>,
    mode: Res<EditorMode>,
    focus: Res<UiFocus>,
    mut contexts: EguiContexts,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    windows: Query<&Window, With<PrimaryWindow>>,
    frame: Res<crate::display::GameFrame>,
    cams: Query<(&Camera, &GlobalTransform), With<FlyCam>>,
    scroll: Res<bevy::input::mouse::AccumulatedMouseScroll>,
) {
    if !editing(&ed, *mode) { return; }
    let over_panels = windows.single().ok().and_then(|w| w.cursor_position()).is_some_and(|c| focus.covers(c));
    let (Ok(window), Ok(cam)) = (windows.single(), cams.single()) else { return };
    let Some(ray) = cursor_ray(window, &frame, cam) else { return };
    // The panels' answer from last frame, and egui's own for where the pointer is now (a click may come with a jump).
    let over_ui = over_panels || focus.pointer || contexts.ctx_mut().is_ok_and(|c| c.is_pointer_over_egui());
    if ed.tool == Tool::Terrain && ed.terrain_mode == TerrainMode::Shape {
        ed.island_at = None;
        ed.brush_at = None;
        shape_island(&mut ed, ray, window, &frame, cam, over_ui, &buttons, &keys, scroll.delta.y);
        return;
    }
    ed.shape_hover = None;
    if ed.tool == Tool::Terrain && ed.terrain_mode == TerrainMode::Rails {
        ed.island_at = None;
        ed.brush_at = None;
        draw_rails(&mut ed, ray, window, &frame, cam, over_ui, &buttons, scroll.delta.y);
        return;
    }
    ed.rail_at = None;
    ed.rail_hover = None;
    if ed.tool == Tool::Terrain && ed.terrain_mode == TerrainMode::Paths {
        ed.island_at = None;
        ed.brush_at = None;
        ed.gap_hover = None;
        draw_paths(&mut ed, ray, window, &frame, cam, over_ui, &buttons, scroll.delta.y);
        return;
    }
    ed.walk_at = None;
    ed.walk_hover = None;
    if ed.tool == Tool::Terrain && ed.terrain_mode == TerrainMode::Openings {
        ed.island_at = None;
        ed.brush_at = None;
        open_edges(&mut ed, ray, window, &frame, cam, over_ui, &buttons);
        return;
    }
    ed.gap_hover = None;
    if ed.tool == Tool::Terrain && ed.terrain_mode == TerrainMode::Build {
        // The island builder: where the pointer meets the ground (else the start's height): a click builds it there.
        let at = terrain_hit(&ed.def, ray).map(|h| h.1).or_else(|| on_plane(ray, ed.def.start.position[2] - 0.5));
        ed.island_at = at.filter(|_| !over_ui);
        ed.brush_at = None;
        if buttons.just_pressed(MouseButton::Left) && !over_ui {
            if let Some(at) = at { ed.build_island(at); }
        }
        return;
    }
    ed.island_at = None;
    if ed.tool == Tool::Terrain && ed.terrain_mode == TerrainMode::Sculpt {
        let hit = terrain_hit(&ed.def, ray);
        ed.brush_at = hit.filter(|h| h.2.is_some()).map(|h| h.1);
        if buttons.just_pressed(MouseButton::Left) && !over_ui {
            if let Some((_, at, Some(k))) = hit {
                ed.checkpoint();
                ed.patch = Some(k);
                ed.stroke = Some(at[2]);
            }
        }
        if buttons.pressed(MouseButton::Left) && ed.stroke.is_some() {
            if let Some(at) = ed.brush_at {
                let invert = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
                ed.sculpt(at, time.delta_secs().min(0.05), invert);
            }
        }
        if !buttons.pressed(MouseButton::Left) && ed.stroke.take().is_some() {
            ed.status = "Sculpted".into();
            ed.changed();
        }
        return;
    }
    ed.brush_at = None;
    if ed.tool == Tool::Logic {
        logic_clicks(&mut ed, ray, over_ui, &buttons);
        return;
    }
    if buttons.just_pressed(MouseButton::Left) && !over_ui {
        if let Some(e) = ed.armed {
            let Some(asset) = ed.placeable_id(e) else {
                ed.status = "No level places this asset: it cannot be copied".into();
                return;
            };
            let Some(p) = ed.placement(ray, ed.local_box(&asset)) else { return };
            ed.checkpoint();
            let yaw = ed.place_yaw;
            // A mover gets a lift path to start with (the template's names a spline of its home planet).
            // A robot trooper patrols a short beat where it stands (without a path it walks a Novalis route away).
            let path = match asset.number {
                726 => vec![p, [p[0], p[1], p[2] + 8.0]],
                459 => vec![p, [p[0] + 6.0 * yaw.cos(), p[1] + 6.0 * yaw.sin(), p[2]]],
                _ => Vec::new(),
            };
            ed.selected = Some(ed.add_object(Object { asset, position: p, yaw, template: None, group: None, path, time: None, solid: None, name: None, hidden: false, tint: None }));
            ed.status = format!("Placed {}", ed.name_of(&asset));
            ed.changed();
        } else if let Some(g) = ed.selected.and_then(|i| grab_gizmo(&ed, i, ray, window, &frame, cam)) {
            // The selected object's gizmo first: an arrow, the ring, a path point.
            ed.gizmo = Some(g);
            ed.drag_start = Some(ed.snap());
        } else {
            ed.selected = pick(&ed, ray);
            // No object there: the path or island under the pointer (its settings on the right), else nothing.
            if ed.selected.is_some() {
                (ed.patch, ed.walk) = (None, None);
            } else {
                match terrain_hit(&ed.def, ray) {
                    Some((_, _, Some(k))) => {
                        (ed.patch, ed.walk) = (Some(k), None);
                        ed.status = format!("Patch {} selected: its settings are on the right", k + 1);
                    }
                    Some((_, h, None)) => {
                        ed.walk = ed.def.walkways.iter().position(|w| w.height_at(h[0], h[1]).is_some_and(|z| (z - h[2]).abs() < 0.25));
                        ed.patch = None;
                        if let Some(k) = ed.walk { ed.status = format!("Path {} selected: its settings are on the right", k + 1); }
                    }
                    None => (ed.patch, ed.walk) = (None, None),
                }
            }
            if let Some(i) = ed.selected {
                let p = ed.def.objects[i].position;
                // The grab offset on the surface the drag follows (the ground under the pointer).
                let at = terrain_hit(&ed.def, ray).map(|h| h.1).or_else(|| on_plane(ray, p[2]));
                ed.drag = at.map(|h| [p[0] - h[0], p[1] - h[1], 0.0]);
                ed.drag_start = Some(ed.snap());
                ed.press = window.cursor_position();
            }
        }
    }
    if let Some(id) = ed.selected.and_then(|i| ed.def.objects.get(i)).map(|o| o.asset) { ed.sight(id); }
    if buttons.pressed(MouseButton::Left) {
        if let (Some(i), Some(g)) = (ed.selected, ed.gizmo) {
            drag_gizmo(&mut ed, i, g, ray);
        } else if let (Some(i), Some(off), true) = (ed.selected, ed.drag, ed.press.is_none_or(|p| window.cursor_position().is_some_and(|c| c.distance(p) > 5.0))) {
            ed.press = None;
            // Along the ground under the pointer (onto a hill, up a step), never into it or into other objects.
            let o = ed.def.objects[i].clone();
            let local = ed.local_box(&o.asset);
            let at = terrain_hit(&ed.def, ray).map(|h| h.1).or_else(|| on_plane(ray, o.position[2]));
            if let Some(h) = at {
                let want = [h[0] + off[0], h[1] + off[1], h[2]];
                let p = ed.settle(Some(i), local, [want[0], want[1], h[2] + 0.5], o.yaw);
                let p = ed.clear_of(Some(i), local, p, o.yaw);
                let d = [p[0] - o.position[0], p[1] - o.position[1], p[2] - o.position[2]];
                let ob = &mut ed.def.objects[i];
                ob.position = p;
                // Its path moves with it.
                for q in &mut ob.path { for k in 0..3 { q[k] += d[k]; } }
            }
        }
    }
    if buttons.just_released(MouseButton::Left) {
        ed.press = None;
        let gizmo = ed.gizmo.take();
        if let (true, Some(start)) = (ed.drag.take().is_some() || gizmo.is_some(), ed.drag_start.take()) {
            if start.def.objects != ed.def.objects {
                ed.undo.push(start);
                ed.redo.clear();
                ed.status = match gizmo { Some(Gizmo::Ring { .. }) => "Turned".into(), Some(Gizmo::Point { .. }) => "Path changed".into(), _ => "Moved".into() };
                ed.changed();
            }
        }
    }
}

/// A stretch of an edge (a piece's length of coast): its ends, its middle on the coast, half its length along it,
/// whether it is an opening.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Stretch {
    a: [f32; 3],
    b: [f32; 3],
    mid: [f32; 3],
    half: f32,
    open: bool,
}

/// Paths mode. With a path selected, its points are handles: drag one across, the wheel over it raises or lowers it.
/// A click elsewhere puts the next point there (the ground under the pointer plus [`Editor::walk_lift`], else the
/// height of the last point): the first two clicks make a new path, then each click adds a bend until Enter. A click
/// on a path selects it.
#[allow(clippy::too_many_arguments)]
fn draw_paths(ed: &mut Editor, ray: ([f32; 3], [f32; 3]), window: &Window, frame: &crate::display::GameFrame, cam: (&Camera, &GlobalTransform), over_ui: bool, buttons: &ButtonInput<MouseButton>, wheel: f32) {
    let k = ed.walk.filter(|&k| k < ed.def.walkways.len());
    // The point under the pointer.
    if ed.walk_drag.is_none() {
        ed.walk_hover = None;
        if let (Some(k), false, Some(cursor)) = (k, over_ui, window.cursor_position()) {
            let mut best: Option<(f32, usize)> = None;
            for (i, q) in ed.def.walkways[k].points.iter().enumerate() {
                let Some(sp) = to_window(window, frame, cam, *q) else { continue };
                let d = sp.distance(cursor);
                if d < 16.0 && best.is_none_or(|b| d < b.0) { best = Some((d, i)); }
            }
            ed.walk_hover = best.map(|b| b.1);
        }
    }
    // The wheel over a point: up or down half a unit a notch.
    if let (Some(k), Some(i), true) = (k, ed.walk_hover.or(ed.walk_drag.as_ref().map(|d| d.0)), wheel != 0.0) {
        if ed.walk_drag.is_none() { ed.checkpoint(); }
        let mut w = ed.def.walkways[k].clone();
        w.points[i][2] = (w.points[i][2] + 0.5 * wheel.signum()).clamp(1.0, 1000.0);
        if w.check().is_ok() {
            ed.def.walkways[k] = w;
            crate::map_load::set(ed.def.clone());
            if ed.walk_drag.is_none() { ed.changed(); }
        }
    }
    // Where a click would put the next point.
    let last = ed.walk_first.or_else(|| k.filter(|_| ed.walk_drawing).and_then(|k| ed.def.walkways[k].points.last().copied()));
    let target = terrain_hit(&ed.def, ray).map(|h| [h.1[0], h.1[1], h.1[2] + ed.walk_lift]).or_else(|| on_plane(ray, last.map_or(ed.def.start.position[2] - 0.5, |l| l[2])));
    let target = target.map(|t| [(t[0] * 64.0).round() / 64.0, (t[1] * 64.0).round() / 64.0, (t[2].clamp(1.0, 1000.0) * 64.0).round() / 64.0]);
    ed.walk_at = target.filter(|_| !over_ui && ed.walk_hover.is_none() && ed.walk_drag.is_none());
    if buttons.just_pressed(MouseButton::Left) && !over_ui {
        if let (Some(i), Some(k)) = (ed.walk_hover, k) {
            ed.walk_drag = Some((i, ed.snap()));
            let _ = k;
        } else if let Some(t) = target {
            match (ed.walk_first.take(), k.filter(|_| ed.walk_drawing)) {
                // Another bend of the path being drawn.
                (None, Some(k)) => {
                    let mut w = ed.def.walkways[k].clone();
                    w.points.push(t);
                    match w.check() {
                        Ok(()) => {
                            ed.checkpoint();
                            ed.def.walkways[k] = w;
                            ed.status = format!("Path {}: {} points (Enter: done)", k + 1, ed.def.walkways[k].points.len());
                            open_for_path(ed, k);
                            ed.changed();
                        }
                        Err(e) => ed.status = format!("Not there: {e}"),
                    }
                }
                // The second point: the path is made.
                (Some(first), _) => {
                    let mut w = ed.walk_spec.clone();
                    w.points = vec![first, t];
                    match w.check() {
                        Ok(()) => {
                            ed.checkpoint();
                            ed.def.walkways.push(w);
                            ed.walk = Some(ed.def.walkways.len() - 1);
                            ed.walk_drawing = true;
                            ed.status = "Path made: click for more bends, Enter when done".into();
                            open_for_path(ed, ed.def.walkways.len() - 1);
                            ed.changed();
                        }
                        Err(e) => { ed.walk_first = Some(first); ed.status = format!("Not there: {e}"); }
                    }
                }
                // A click on a path selects it; elsewhere the first point of a new one.
                (None, None) => {
                    let on = ed.def.walkways.iter().position(|w| w.height_at(t[0], t[1]).is_some_and(|h| (h - (t[2] - ed.walk_lift)).abs() < 0.6));
                    match on {
                        Some(j) => {
                            ed.walk = Some(j);
                            ed.status = format!("Path {} selected: drag its points, the wheel over one raises it", j + 1);
                        }
                        None => {
                            ed.walk = None;
                            ed.walk_first = Some(t);
                            ed.status = "First point: click where the path goes next".into();
                        }
                    }
                }
            }
        }
    }
    if let (Some((i, _)), Some(k)) = (&ed.walk_drag, k) {
        let i = *i;
        if buttons.pressed(MouseButton::Left) {
            let z = ed.def.walkways[k].points[i][2];
            if let Some(h) = on_plane(ray, z) {
                let mut w = ed.def.walkways[k].clone();
                w.points[i] = [(h[0] * 64.0).round() / 64.0, (h[1] * 64.0).round() / 64.0, z];
                if w.check().is_ok() && w != ed.def.walkways[k] {
                    ed.def.walkways[k] = w;
                    crate::map_load::set(ed.def.clone());
                }
            }
        } else if let Some((_, start)) = ed.walk_drag.take() {
            if start.def.walkways != ed.def.walkways {
                ed.undo.push(start);
                ed.redo.clear();
                ed.status = "Path changed".into();
                open_for_path(ed, k);
                ed.changed();
            }
        }
    }
}

/// Opens the islands' edges where path `k` runs over them (as high as the barrier, give or take a little): every
/// stretch it crosses becomes an opening. The status says how many (Openings mode closes them again).
fn open_for_path(ed: &mut Editor, k: usize) {
    let before = ed.def.patches.clone();
    let Some(w) = ed.def.walkways.get(k).cloned() else { return };
    let mut opened = 0;
    for p in &mut ed.def.patches {
        let Some(e) = p.edge.as_mut() else { continue };
        let reach = e.height + 1.0;
        // The openings this path made before (centred on it) go: they are cut again to its width.
        e.gaps.retain(|g| w.height_at(g[0], g[1]).is_none());
        let land = p.clone();
        // Inside the path's own railings (so the island's railing comes right up to them).
        let margin = w.edge.as_ref().map_or(0.0, |e| e.inset);
        opened += p.open_where(|x, y| w.inside(x, y, margin) && w.height_at(x, y).is_some_and(|h| land.height_at(x, y).is_none_or(|g| h > g - 1.0 && h < g + reach)));
    }
    if ed.def.patches != before {
        crate::map_load::set(ed.def.clone());
        ed.status = format!("Opened {opened} stretches of railing or wall where the path crosses (Openings mode: click one to close it)");
    }
}

/// Rails mode, as Paths: with a rail selected its points are handles (drag across, the wheel up or down); a click puts
/// the next point at the ground under the pointer plus [`Editor::rail_lift`] (else the last point's height): two clicks
/// make a rail, then each adds a bend until Enter. A click near a rail selects it.
#[allow(clippy::too_many_arguments)]
fn draw_rails(ed: &mut Editor, ray: ([f32; 3], [f32; 3]), window: &Window, frame: &crate::display::GameFrame, cam: (&Camera, &GlobalTransform), over_ui: bool, buttons: &ButtonInput<MouseButton>, wheel: f32) {
    let k = ed.rail.filter(|&k| k < ed.def.rails.len());
    let cursor = window.cursor_position();
    if ed.rail_drag.is_none() {
        ed.rail_hover = None;
        if let (Some(k), false, Some(c)) = (k, over_ui, cursor) {
            ed.rail_hover = ed.def.rails[k].points.iter().enumerate()
                .filter_map(|(i, q)| to_window(window, frame, cam, *q).map(|sp| (sp.distance(c), i)))
                .filter(|(d, _)| *d < 16.0)
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|x| x.1);
        }
    }
    if let (Some(k), Some(i), true) = (k, ed.rail_hover.or(ed.rail_drag.as_ref().map(|d| d.0)), wheel != 0.0) {
        if ed.rail_drag.is_none() { ed.checkpoint(); }
        let z = &mut ed.def.rails[k].points[i][2];
        *z = (*z + 0.25 * wheel.signum()).clamp(1.0, 1000.0);
        crate::map_load::set(ed.def.clone());
        if ed.rail_drag.is_none() { ed.changed(); }
    }
    // The picked point's arrows: a press on one moves the point along it (x, y or up) while held.
    let picked = k.and_then(|k| ed.rail_point.filter(|&i| i < ed.def.rails[k].points.len()).map(|i| (k, i)));
    if let (Some((k, i)), true, false) = (picked, buttons.just_pressed(MouseButton::Left), over_ui) {
        let q = ed.def.rails[k].points[i];
        let cam_at = to_game(cam.1.translation());
        let tolerance = (((q[0] - cam_at[0]).powi(2) + (q[1] - cam_at[1]).powi(2) + (q[2] - cam_at[2]).powi(2)).sqrt() * 0.025).max(0.12);
        for (axis, dir) in AXES.iter().enumerate() {
            let (d, s_, t) = ray_line(ray, q, *dir);
            if t > 0.0 && d < tolerance && (0.3..=POINT_ARROW + 0.3).contains(&s_) {
                ed.rail_axis = Some((axis, s_, q, ed.snap()));
                break;
            }
        }
    }
    if let Some((axis, t0, from, _)) = ed.rail_axis {
        if let (Some((k, i)), true) = (picked, buttons.pressed(MouseButton::Left)) {
            let (_, s_, _) = ray_line(ray, from, AXES[axis]);
            let mut q = from;
            q[axis] = ((from[axis] + s_ - t0) * 64.0).round() / 64.0;
            q[axis] = q[axis].clamp(if axis == 2 { 1.0 } else { 0.0 }, 1023.0);
            if ed.def.rails[k].points[i] != q {
                ed.def.rails[k].points[i] = q;
                crate::map_load::set(ed.def.clone());
            }
        } else if let Some((_, _, _, start)) = ed.rail_axis.take() {
            if start.def.rails != ed.def.rails {
                ed.undo.push(start);
                ed.redo.clear();
                ed.status = "Rail point moved".into();
                ed.changed();
            }
        }
        return;
    }
    let last = ed.rail_first.or_else(|| k.filter(|_| ed.rail_drawing).and_then(|k| ed.def.rails[k].points.last().copied()));
    let target = terrain_hit(&ed.def, ray).map(|h| [h.1[0], h.1[1], h.1[2] + ed.rail_lift]).or_else(|| on_plane(ray, last.map_or(ed.def.start.position[2] + ed.rail_lift, |l| l[2])));
    let target = target.map(|t| [(t[0] * 64.0).round() / 64.0, (t[1] * 64.0).round() / 64.0, (t[2].clamp(1.0, 1000.0) * 64.0).round() / 64.0]);
    ed.rail_at = target.filter(|_| !over_ui && ed.rail_hover.is_none() && ed.rail_drag.is_none());
    if buttons.just_pressed(MouseButton::Left) && !over_ui {
        if ed.rail_hover.is_some() && k.is_some() {
            ed.rail_drag = Some((ed.rail_hover.unwrap(), ed.snap()));
            ed.rail_point = ed.rail_hover;
        } else if let Some(t) = target {
            match (ed.rail_first.take(), k.filter(|_| ed.rail_drawing)) {
                (None, Some(k)) => {
                    ed.checkpoint();
                    ed.def.rails[k].points.push(t);
                    ed.status = format!("Rail {}: {} points (Enter: done)", k + 1, ed.def.rails[k].points.len());
                    ed.changed();
                }
                (Some(first), _) => {
                    if ((t[0] - first[0]).powi(2) + (t[1] - first[1]).powi(2) + (t[2] - first[2]).powi(2)).sqrt() < 0.5 {
                        ed.rail_first = Some(first);
                    } else {
                        ed.checkpoint();
                        ed.def.rails.push(rc_map::Rail { points: vec![first, t], smooth: ed.rail_smooth, look: ed.rail_look.clone(), one_way: false });
                        ed.rail = Some(ed.def.rails.len() - 1);
                        ed.rail_drawing = true;
                        ed.status = "Rail made: click for more bends, Enter when done (turn Grind rails on in the map settings)".into();
                        if ed.def.abilities.rails != Some(true) { ed.def.abilities.rails = Some(true); }
                        ed.changed();
                    }
                }
                (None, None) => {
                    // A click near a rail (on screen) selects it; elsewhere the first point of a new one.
                    let near = cursor.and_then(|c| {
                        ed.def.rails.iter().enumerate().filter_map(|(j, r)| {
                            r.line().windows(2).filter_map(|s| {
                                let (a, b) = (to_window(window, frame, cam, s[0])?, to_window(window, frame, cam, s[1])?);
                                let ab = b - a;
                                let tt = ((c - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                                Some((a + ab * tt).distance(c))
                            }).reduce(f32::min).map(|d| (d, j))
                        }).filter(|(d, _)| *d < 12.0).min_by(|a, b| a.0.total_cmp(&b.0)).map(|x| x.1)
                    });
                    match near {
                        Some(j) => {
                            ed.rail = Some(j);
                            ed.rail_point = None;
                            ed.status = format!("Rail {} selected: drag its points, the wheel over one raises it", j + 1);
                        }
                        None => {
                            ed.rail = None;
                            ed.rail_first = Some(t);
                            ed.status = "First point: click where the rail goes next".into();
                        }
                    }
                }
            }
        }
    }
    if let (Some((i, _)), Some(k)) = (&ed.rail_drag, k) {
        let i = *i;
        if buttons.pressed(MouseButton::Left) {
            let z = ed.def.rails[k].points[i][2];
            if let Some(h) = on_plane(ray, z) {
                let q = [(h[0] * 64.0).round() / 64.0, (h[1] * 64.0).round() / 64.0, z];
                if ed.def.rails[k].points[i] != q {
                    ed.def.rails[k].points[i] = q;
                    crate::map_load::set(ed.def.clone());
                }
            }
        } else if let Some((_, start)) = ed.rail_drag.take() {
            if start.def.rails != ed.def.rails {
                ed.undo.push(start);
                ed.redo.clear();
                ed.status = "Rail changed".into();
                ed.changed();
            }
        }
    }
}

/// The Rails mode's panel.
fn rails_panel(ui: &mut egui::Ui, ed: &mut Editor) {
    ui.label(egui::RichText::new("Click where a grind rail starts, then where it bends; Enter when done. Ratchet jumps on and grinds it (Grindboots in the loadout, Grind rails on in the map settings).").weak());
    ui.label(egui::RichText::new("Drag a point to move it, the mouse wheel over it raises or lowers it, Delete over it takes it out.").weak().small());
    ui.add(egui::Slider::new(&mut ed.rail_lift, 0.0..=20.0).text("points above the ground"));
    if ed.rail_drawing || ed.rail_first.is_some() {
        if ui.button("Done (Enter)").clicked() { ed.rail_drawing = false; ed.rail_first = None; }
    }
    let k = ed.rail.filter(|&k| k < ed.def.rails.len());
    let mut smooth = k.map_or(ed.rail_smooth, |k| ed.def.rails[k].smooth);
    if ui.checkbox(&mut smooth, "Smooth curve").on_hover_text("A smooth curve through the points (else straight lines between them)").changed() {
        match k {
            Some(k) => { ed.checkpoint(); ed.def.rails[k].smooth = smooth; ed.changed(); }
            None => ed.rail_smooth = smooth,
        }
    }
    if let Some(k) = k {
        let mut one = ed.def.rails[k].one_way;
        if ui.checkbox(&mut one, "One way").on_hover_text("Ratchet grinds it only from its first point to its last (else both ways)").changed() {
            ed.checkpoint();
            ed.def.rails[k].one_way = one;
            ed.changed();
        }
    }
    // The look: a planet's rail repeated along it, or the plain beam.
    let mut look = k.map_or(ed.rail_look.clone(), |k| ed.def.rails[k].look.clone());
    ui.label(egui::RichText::new("Look").strong());
    ui.horizontal_wrapped(|ui| {
        if ui.selectable_label(look.is_none(), "Plain").on_hover_text("A plain orange beam").clicked() { look = None; }
        for (key, name, ..) in crate::map_load::RAIL_LOOKS {
            if ui.selectable_label(look.as_deref() == Some(key), name).on_hover_text("That planet's grind rail, repeated along this one").clicked() { look = Some(key.to_string()); }
        }
    });
    match k {
        Some(k) if look != ed.def.rails[k].look => { ed.checkpoint(); ed.def.rails[k].look = look; ed.changed(); }
        None => ed.rail_look = look,
        _ => {}
    }
    if ed.def.abilities.rails != Some(true) {
        ui.label(egui::RichText::new("Grind rails are off for this map (map settings, right panel with nothing selected).").color(egui::Color32::from_rgb(0xff, 0xb3, 0x47)));
        if ui.button("Turn grind rails on").clicked() { ed.checkpoint(); ed.def.abilities.rails = Some(true); ed.changed(); }
    }
    if let Some(k) = k {
        ui.separator();
        ui.label(egui::RichText::new(format!("Rail {}: its points", k + 1)).strong());
        ui.label(egui::RichText::new("Pick a point to move it with its arrows in the world, or type where it goes.").weak().small());
        let mut pts = ed.def.rails[k].points.clone();
        let mut drop = None;
        egui::Grid::new("rail_points").num_columns(5).striped(true).show(ui, |ui| {
            for (i, q) in pts.iter_mut().enumerate() {
                if ui.selectable_label(ed.rail_point == Some(i), format!("{}", i + 1)).on_hover_text("Pick it: its arrows show in the world").clicked() {
                    ed.rail_point = if ed.rail_point == Some(i) { None } else { Some(i) };
                }
                for (a, v) in q.iter_mut().enumerate() {
                    ui.add(egui::DragValue::new(v).speed(0.1).range(if a == 2 { 1.0..=1000.0 } else { 0.0..=1023.0 })).on_hover_text(["x", "y", "height"][a]);
                }
                if ui.add_enabled(ed.def.rails[k].points.len() > 2, egui::Button::new("x").small()).on_hover_text("Take this point out").clicked() { drop = Some(i); }
                ui.end_row();
            }
        });
        if let Some(i) = drop {
            pts.remove(i);
            ed.rail_point = None;
        }
        for q in &mut pts { for v in q.iter_mut() { *v = (*v * 64.0).round() / 64.0; } }
        if pts != ed.def.rails[k].points {
            if !ed.coalescing { ed.checkpoint(); }
            ed.coalescing = true;
            ed.def.rails[k].points = pts;
            ed.changed();
        }
    }
    ui.separator();
    ui.label(egui::RichText::new(format!("Rails on this map ({})", ed.def.rails.len())).strong());
    let mut remove = None;
    for j in 0..ed.def.rails.len() {
        let label = format!("Rail {}: {} points", j + 1, ed.def.rails[j].points.len());
        ui.horizontal(|ui| {
            if ui.selectable_label(ed.rail == Some(j), label).clicked() { ed.rail = Some(j); ed.rail_drawing = false; ed.rail_point = None; }
            if ui.small_button("x").on_hover_text("Delete this rail").clicked() { remove = Some(j); }
        });
    }
    if let Some(j) = remove {
        ed.checkpoint();
        ed.def.rails.remove(j);
        ed.rail = None;
        ed.rail_drawing = false;
        ed.status = format!("Rail {} deleted (Ctrl+Z brings it back)", j + 1);
        ed.changed();
    }
}

/// The tools in the top bar (a painted icon each, the name on hover): Select (Esc), Place, the terrain tools.
fn tool_icons(ui: &mut egui::Ui, ed: &mut Editor) {
    let current = if ed.armed.is_some() { "Place" } else if ed.tool == Tool::Terrain {
        match ed.terrain_mode {
            TerrainMode::Select => "Select",
            TerrainMode::Build => "Build",
            TerrainMode::Shape => "Shape",
            TerrainMode::Openings => "Openings",
            TerrainMode::Sculpt => "Sculpt",
            TerrainMode::Paths => "Paths",
            TerrainMode::Rails => "Rails",
        }
    } else if ed.tool == Tool::Logic { "" } else { "Select" };
    const TOOLS: [(&str, &str); 8] = [
        ("Select", "Select (Esc): click objects, islands and paths"),
        ("Place", "Place objects: pick one in the Objects list, click in the world"),
        ("Build", "Build islands"),
        ("Shape", "Shape: drag an island's coast, move it, raise it"),
        ("Openings", "Openings: open or close stretches of an island's edge"),
        ("Sculpt", "Sculpt: hills, paint, shade, decor, edge gaps"),
        ("Paths", "Paths: walkways and bridges"),
        ("Rails", "Rails: grind rails"),
    ];
    for (name, hint) in TOOLS {
        let (rect, r) = ui.allocate_exact_size(egui::vec2(30.0, 24.0), egui::Sense::click());
        let on = current == name;
        let orange = egui::Color32::from_rgb(0xf0, 0x8a, 0x24);
        let fill = if on { orange } else if r.hovered() { egui::Color32::from_rgb(0x3a, 0x4d, 0x75) } else { egui::Color32::TRANSPARENT };
        ui.painter().rect_filled(rect, 5.0, fill);
        let ink = if !ui.is_enabled() { egui::Color32::from_gray(110) } else if on { egui::Color32::from_rgb(0x1a, 0x1f, 0x2e) } else { egui::Color32::from_gray(225) };
        draw_tool_icon(ui.painter(), rect.center(), name, ink);
        if r.on_hover_text(hint).clicked() {
            ed.armed = None;
            ed.stroke = None;
            match name {
                "Select" => {
                    if ed.tool == Tool::Terrain { ed.terrain_mode = TerrainMode::Select; }
                    if ed.tool == Tool::Logic { ed.tool = Tool::Objects; ed.zone_new = None; }
                }
                "Place" => { ed.tool = Tool::Objects; ed.status = "Pick an object in the list, then click in the world".into(); }
                _ => {
                    ed.tool = Tool::Terrain;
                    ed.terrain_mode = match name { "Build" => TerrainMode::Build, "Shape" => TerrainMode::Shape, "Openings" => TerrainMode::Openings, "Sculpt" => TerrainMode::Sculpt, "Rails" => TerrainMode::Rails, _ => TerrainMode::Paths };
                }
            }
        }
    }
}

/// A tool's icon, about 18 points across around `c`, drawn in `ink`.
fn draw_tool_icon(painter: &egui::Painter, c: egui::Pos2, name: &str, ink: egui::Color32) {
    let p = |x: f32, y: f32| egui::pos2(c.x + x, c.y + y);
    let line = egui::Stroke::new(1.6, ink);
    let poly = |pts: Vec<egui::Pos2>| egui::Shape::line(pts, line);
    let arc = |cx: f32, cy: f32, rx: f32, ry: f32, a0: f32, a1: f32| -> Vec<egui::Pos2> {
        (0..=16).map(|k| { let a = a0 + (a1 - a0) * k as f32 / 16.0; p(cx + rx * a.cos(), cy + ry * a.sin()) }).collect()
    };
    use std::f32::consts::{PI, TAU};
    match name {
        "Select" => {
            let pts = vec![p(-4.0, -8.0), p(-4.0, 6.0), p(-0.5, 2.5), p(2.0, 8.0), p(4.0, 7.0), p(1.5, 1.5), p(6.0, 1.5)];
            painter.add(egui::Shape::convex_polygon(pts[..3].iter().copied().chain([pts[6]]).collect(), ink, egui::Stroke::NONE));
            painter.add(egui::Shape::closed_line(pts, line));
        }
        "Place" => {
            // A box landing on the ground, an arrow down onto it.
            painter.rect_stroke(egui::Rect::from_min_max(p(-4.5, 0.0), p(4.5, 7.0)), 1.0, line, egui::StrokeKind::Middle);
            painter.line_segment([p(-8.0, 7.5), p(8.0, 7.5)], line);
            painter.line_segment([p(0.0, -9.0), p(0.0, -2.5)], line);
            painter.add(poly(vec![p(-3.0, -5.0), p(0.0, -2.0), p(3.0, -5.0)]));
        }
        "Build" => {
            // An island: its top, its cliffs, the rounded bottom.
            painter.add(egui::Shape::closed_line(arc(0.0, -3.0, 8.0, 3.5, 0.0, TAU), line));
            painter.line_segment([p(-8.0, -3.0), p(-6.0, 5.0)], line);
            painter.line_segment([p(8.0, -3.0), p(6.0, 5.0)], line);
            painter.add(poly(arc(0.0, 5.0, 6.0, 3.0, 0.0, PI)));
        }
        "Shape" => {
            // A coast with its handles.
            let pts = vec![p(-8.0, 1.0), p(-5.0, -6.0), p(1.0, -7.0), p(7.0, -3.0), p(6.0, 4.0), p(0.0, 7.0), p(-6.0, 6.0)];
            painter.add(egui::Shape::closed_line(pts.clone(), line));
            for q in [pts[1], pts[3], pts[5]] { painter.circle_filled(q, 2.2, ink); }
        }
        "Openings" => {
            // A railing with a gap.
            for (x0, x1) in [(-9.0, -3.0), (3.0, 9.0)] {
                painter.line_segment([p(x0, -4.0), p(x1, -4.0)], line);
                painter.line_segment([p(x0, 1.0), p(x1, 1.0)], line);
                for x in [x0 + 1.0, x1 - 1.0] { painter.line_segment([p(x, -6.0), p(x, 6.0)], line); }
            }
            painter.line_segment([p(-9.0, 7.5), p(9.0, 7.5)], egui::Stroke::new(1.0, ink));
        }
        "Sculpt" => {
            // A hill and the brush over it.
            let hill: Vec<egui::Pos2> = (0..=16).map(|k| { let x = -9.0 + 18.0 * k as f32 / 16.0; p(x, 7.0 - 9.0 * (-(x * x) / 18.0).exp()) }).collect();
            painter.add(poly(hill));
            painter.add(egui::Shape::closed_line(arc(0.0, -6.0, 4.0, 1.6, 0.0, TAU), line));
            painter.line_segment([p(0.0, -4.4), p(0.0, -1.0)], egui::Stroke::new(1.0, ink));
        }
        "Paths" => {
            // A road going away, its middle dashed.
            painter.line_segment([p(-8.0, 8.0), p(-2.0, -8.0)], line);
            painter.line_segment([p(8.0, 8.0), p(2.0, -8.0)], line);
            for (y0, y1) in [(5.0, 8.0), (-1.0, 2.0), (-6.0, -4.0)] { painter.line_segment([p(0.0, y0), p(0.0, y1)], egui::Stroke::new(1.3, ink)); }
        }
        "Rails" => {
            // A curved grind rail on its ties.
            let rail = arc(0.0, 12.0, 11.0, 16.0, -2.4, -0.74);
            for k in [2usize, 6, 10, 14] {
                let (a, b) = (rail[k - 1], rail[k + 1]);
                let d = (b - a).normalized();
                let n = egui::vec2(-d.y, d.x) * 3.5;
                painter.line_segment([rail[k] - n, rail[k] + n], egui::Stroke::new(1.2, ink));
            }
            painter.add(egui::Shape::line(rail, egui::Stroke::new(2.4, ink)));
        }
        _ => {}
    }
}

/// Where a ray meets a zone's box (its distance), in the box's own turned axes.
fn ray_zone((o, d): ([f32; 3], [f32; 3]), z: &rc_map::logic::Zone) -> Option<f32> {
    let (s, c) = z.yaw.sin_cos();
    let to = |v: [f32; 3]| [v[0] * c + v[1] * s, -v[0] * s + v[1] * c, v[2]];
    let lo = to([o[0] - z.centre[0], o[1] - z.centre[1], o[2] - z.centre[2]]);
    let ld = to(d);
    let (mut t0, mut t1) = (0.0f32, f32::MAX);
    for k in 0..3 {
        if ld[k].abs() < 1e-9 {
            if lo[k].abs() > z.half[k] { return None; }
            continue;
        }
        let (a, b) = ((-z.half[k] - lo[k]) / ld[k], (z.half[k] - lo[k]) / ld[k]);
        t0 = t0.max(a.min(b));
        t1 = t1.min(a.max(b));
    }
    (t0 <= t1).then_some(t0)
}

/// A new zone of `kind` standing on the ground point `g` (snapped to quarter units), with a free name.
fn new_zone(def: &MapDef, kind: &str, g: [f32; 3]) -> rc_map::logic::Zone {
    let half = match kind { "camera" => [6.0, 6.0, 4.0], "death" => [4.0, 4.0, 2.0], _ => [3.0, 3.0, 2.0] };
    let centre = [(g[0] * 4.0).round() / 4.0, (g[1] * 4.0).round() / 4.0, ((g[2] + half[2]) * 4.0).round() / 4.0];
    let kind_v = match kind {
        "checkpoint" => rc_map::logic::ZoneKind::Checkpoint,
        "goal" => rc_map::logic::ZoneKind::Goal,
        "death" => rc_map::logic::ZoneKind::Death,
        "camera" => rc_map::logic::ZoneKind::Camera { eye: [centre[0] - 10.0, centre[1] - 10.0, centre[2] + 8.0] },
        _ => rc_map::logic::ZoneKind::Trigger,
    };
    rc_map::logic::Zone { name: zone_name(def, kind), kind: kind_v, centre, half, yaw: 0.0 }
}

/// A free zone name for `kind` ("Checkpoint 2").
fn zone_name(def: &MapDef, kind: &str) -> String {
    let base = { let mut c = kind.chars(); c.next().map_or(String::new(), |f| f.to_uppercase().collect::<String>() + c.as_str()) };
    (1..).map(|n| format!("{base} {n}")).find(|n| def.zones.iter().all(|z| &z.name != n)).unwrap()
}

/// The Logic tab's clicks: a zone kind armed places one where the pointer meets the ground; a click on a zone selects
/// it and drags it over the ground; elsewhere, nothing selected.
fn logic_clicks(ed: &mut Editor, ray: ([f32; 3], [f32; 3]), over_ui: bool, buttons: &ButtonInput<MouseButton>) {
    let ground = terrain_hit(&ed.def, ray).map(|h| h.1).or_else(|| on_plane(ray, ed.def.start.position[2] - 0.5));
    // The zone a click would place, under the pointer.
    ed.zone_ghost = if over_ui { None } else { ed.zone_new.zip(ground).map(|(kind, g)| new_zone(&ed.def, kind, g)) };
    if buttons.just_pressed(MouseButton::Left) && !over_ui {
        let hit = ed.def.zones.iter().enumerate().filter_map(|(i, z)| ray_zone(ray, z).map(|t| (t, i))).min_by(|a, b| a.0.total_cmp(&b.0)).map(|x| x.1);
        if let (Some(kind), Some(g)) = (ed.zone_new, ground) {
            ed.checkpoint();
            let z = new_zone(&ed.def, kind, g);
            let name = z.name.clone();
            ed.zone_ghost = None;
            ed.def.zones.push(z);
            ed.zone = Some(ed.def.zones.len() - 1);
            ed.zone_new = None;
            ed.status = format!("{name} placed: size and settings on the right; drag it to move it");
            ed.changed();
        } else if let Some(i) = hit {
            ed.zone = Some(i);
            ed.selected = None;
            if let Some(g) = ground { ed.zone_drag = Some(([ed.def.zones[i].centre[0] - g[0], ed.def.zones[i].centre[1] - g[1]], ed.snap())); }
        } else {
            ed.zone = None;
        }
    }
    if let (Some(i), Some((off, _)), Some(g)) = (ed.zone, ed.zone_drag.as_ref(), ground) {
        if buttons.pressed(MouseButton::Left) && i < ed.def.zones.len() {
            let z = &mut ed.def.zones[i];
            let c = [((g[0] + off[0]) * 4.0).round() / 4.0, ((g[1] + off[1]) * 4.0).round() / 4.0, (g[2] + z.half[2]).round()];
            if c != z.centre {
                let d = [c[0] - z.centre[0], c[1] - z.centre[1], c[2] - z.centre[2]];
                z.centre = c;
                // A camera zone's eye moves with it.
                if let rc_map::logic::ZoneKind::Camera { eye } = &mut z.kind { for k in 0..3 { eye[k] += d[k]; } }
            }
        }
    }
    if !buttons.pressed(MouseButton::Left) {
        if let Some((_, start)) = ed.zone_drag.take() {
            if start.def.zones != ed.def.zones {
                ed.undo.push(start);
                ed.redo.clear();
                ed.status = "Zone moved".into();
                ed.changed();
            }
        }
    }
}

/// The colour a zone kind is drawn in.
fn zone_colour(k: &rc_map::logic::ZoneKind) -> Color {
    match k {
        rc_map::logic::ZoneKind::Checkpoint => Color::srgb(0.3, 0.6, 1.0),
        rc_map::logic::ZoneKind::Goal => Color::srgb(1.0, 0.85, 0.2),
        rc_map::logic::ZoneKind::Death => Color::srgb(1.0, 0.25, 0.25),
        rc_map::logic::ZoneKind::Camera { .. } => Color::srgb(0.75, 0.45, 1.0),
        rc_map::logic::ZoneKind::Trigger => Color::srgb(0.4, 1.0, 0.5),
    }
}

/// The Logic tab: zones to place, the zones, the rules.
fn logic_panel(ui: &mut egui::Ui, ed: &mut Editor) {
    ui.label(egui::RichText::new("Zones").strong().size(16.0));
    ui.label(egui::RichText::new("Pick a kind, then click in the world to place it. Click a zone to select it, drag it to move it.").weak().small());
    ui.horizontal_wrapped(|ui| {
        for (kind, label, hint) in [
            ("checkpoint", "Checkpoint", "Ratchet comes back here after a death once he has walked into it"),
            ("goal", "Goal", "Walking into it finishes the map"),
            ("death", "Death zone", "Walking into it costs a life (a pit, lava)"),
            ("camera", "Camera", "While Ratchet is inside, the camera stands at a spot you choose and watches him"),
            ("trigger", "Trigger", "Does nothing by itself: rules watch it (\"when Ratchet enters...\")"),
        ] {
            if ui.selectable_label(ed.zone_new == Some(kind), label).on_hover_text(hint).clicked() {
                ed.zone_new = if ed.zone_new == Some(kind) { None } else { Some(kind) };
                if ed.zone_new.is_some() { ed.status = format!("Click in the world to place a {label}"); }
            }
        }
    });
    let mut remove = None;
    for i in 0..ed.def.zones.len() {
        let z = &ed.def.zones[i];
        let label = format!("{}  ({})", z.name, z.kind.name());
        ui.horizontal(|ui| {
            if ui.selectable_label(ed.zone == Some(i), label).clicked() { ed.zone = Some(i); ed.selected = None; }
            if ui.small_button("x").on_hover_text("Delete this zone").clicked() { remove = Some(i); }
        });
    }
    if let Some(i) = remove {
        ed.checkpoint();
        ed.def.zones.remove(i);
        ed.zone = None;
        ed.changed();
    }
    ui.separator();
    ui.label(egui::RichText::new("Rules").strong().size(16.0));
    ui.label(egui::RichText::new("When something happens, then things happen (each rule once per run). Give objects a name in their panel to use them here.").weak().small());
    let zones: Vec<String> = ed.def.zones.iter().map(|z| z.name.clone()).collect();
    let objects: Vec<String> = ed.def.objects.iter().filter_map(|o| o.name.clone()).collect();
    let mut rules = ed.def.rules.clone();
    let mut drop_rule = None;
    for (r, rule) in rules.iter_mut().enumerate() {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("When").strong());
                use rc_map::logic::When;
                let kind = match &rule.when { When::Enters(_) => 0, When::Cleared(_) => 1, When::Destroyed(_) => 2 };
                for (k, label) in [(0, "Ratchet enters"), (1, "all enemies beaten in"), (2, "is destroyed:")] {
                    if ui.selectable_label(kind == k, label).clicked() && kind != k {
                        rule.when = match k { 0 => When::Enters(zones.first().cloned().unwrap_or_default()), 1 => When::Cleared(zones.first().cloned().unwrap_or_default()), _ => When::Destroyed(objects.first().cloned().unwrap_or_default()) };
                    }
                }
                if ui.small_button("x").on_hover_text("Delete this rule").clicked() { drop_rule = Some(r); }
            });
            ui.horizontal_wrapped(|ui| {
                use rc_map::logic::When;
                let (list, target) = match &mut rule.when { When::Enters(n) | When::Cleared(n) => (&zones, n), When::Destroyed(n) => (&objects, n) };
                if list.is_empty() { ui.label(egui::RichText::new("(none yet)").weak()); }
                for n in list { if ui.selectable_label(target == n, n).clicked() { *target = n.clone(); } }
            });
            ui.label(egui::RichText::new("then").strong());
            let mut drop_then = None;
            for (t, then) in rule.then.iter_mut().enumerate() {
                use rc_map::logic::Then;
                ui.horizontal_wrapped(|ui| {
                    if ui.small_button("x").clicked() { drop_then = Some(t); }
                    let word = if matches!(then, Then::Show(_)) { "show:" } else { "remove:" };
                    match then {
                        Then::Remove(n) | Then::Show(n) => {
                            ui.label(word);
                            if objects.is_empty() { ui.label(egui::RichText::new("(name an object first)").weak()); }
                            for o in &objects { if ui.selectable_label(n == o, o).clicked() { *n = o.clone(); } }
                        }
                        Then::Message(m) => { ui.label("message:"); ui.add(egui::TextEdit::singleline(m).desired_width(160.0)); }
                        Then::Checkpoint(z) => {
                            ui.label("checkpoint at:");
                            for o in &zones { if ui.selectable_label(z == o, o).clicked() { *z = o.clone(); } }
                        }
                        Then::Goal => { ui.label("the map is finished"); }
                    }
                });
            }
            if let Some(t) = drop_then { rule.then.remove(t); }
            ui.horizontal_wrapped(|ui| {
                use rc_map::logic::Then;
                let first_o = objects.first().cloned().unwrap_or_default();
                if ui.small_button("+ remove object").clicked() { rule.then.push(Then::Remove(first_o.clone())); }
                if ui.small_button("+ show object").clicked() { rule.then.push(Then::Show(first_o.clone())); }
                if ui.small_button("+ message").clicked() { rule.then.push(Then::Message("The gate is open!".into())); }
                if ui.small_button("+ checkpoint").clicked() { rule.then.push(Then::Checkpoint(zones.first().cloned().unwrap_or_default())); }
                if ui.small_button("+ goal").clicked() { rule.then.push(Then::Goal); }
            });
            if rule.then.iter().any(|t| matches!(t, rc_map::logic::Then::Remove(_) | rc_map::logic::Then::Show(_))) {
                ui.checkbox(&mut rule.cutscene, "Show it with the camera").on_hover_text("The camera turns to the object, it disappears (or appears), then the view cuts back to Ratchet");
            }
            let missing = rule.missing(&ed.def.zones, &ed.def.objects);
            if !missing.is_empty() { ui.label(egui::RichText::new(format!("Not on the map: {}", missing.join(", "))).color(egui::Color32::from_rgb(0xff, 0xb3, 0x47)).small()); }
        });
    }
    if let Some(r) = drop_rule { rules.remove(r); }
    if ui.button("+ New rule").clicked() {
        rules.push(rc_map::logic::Rule { when: rc_map::logic::When::Enters(zones.first().cloned().unwrap_or_default()), then: vec![rc_map::logic::Then::Message("Hello!".into())], cutscene: false });
    }
    if rules != ed.def.rules {
        if !ed.coalescing { ed.checkpoint(); }
        ed.coalescing = true;
        ed.def.rules = rules;
        ed.changed();
    }
}

/// The selected zone: its name, kind, size, facing; a camera zone's eye.
fn zone_inspector(ui: &mut egui::Ui, ed: &mut Editor, i: usize, cam: Option<&GlobalTransform>) {
    let mut z = ed.def.zones[i].clone();
    ui.heading(&z.name);
    ui.label(egui::RichText::new(match z.kind {
        rc_map::logic::ZoneKind::Checkpoint => "Checkpoint: Ratchet comes back here after a death once he has been in it.",
        rc_map::logic::ZoneKind::Goal => "Goal: walking in finishes the map (a banner, the jingle, then back to the editor).",
        rc_map::logic::ZoneKind::Death => "Death zone: walking in costs a life.",
        rc_map::logic::ZoneKind::Camera { .. } => "Camera: while Ratchet is inside, the camera stands at its eye (the purple ball) and watches him.",
        rc_map::logic::ZoneKind::Trigger => "Trigger: rules in the Logic tab watch it.",
    }).weak());
    egui::Grid::new("zone").num_columns(2).show(ui, |ui| {
        ui.label("Name");
        ui.add(egui::TextEdit::singleline(&mut z.name).desired_width(150.0));
        ui.end_row();
        // The box stands on its bottom: a taller box grows upwards.
        let (was_bottom, was_half) = (z.centre[2] - z.half[2], z.half[2]);
        let mut bottom = was_bottom;
        for (k, label) in ["Width", "Length", "Height"].iter().enumerate() {
            ui.label(*label);
            let mut size = z.half[k] * 2.0;
            if ui.add(egui::DragValue::new(&mut size).speed(0.1).range(0.25..=300.0)).changed() { z.half[k] = (size / 2.0 * 8.0).round() / 8.0; }
            ui.end_row();
        }
        ui.label("Turn");
        let mut deg = z.yaw.to_degrees();
        if ui.add(egui::Slider::new(&mut deg, 0.0..=359.0).suffix("°")).changed() { z.yaw = deg.to_radians(); }
        ui.end_row();
        ui.label("Bottom at").on_hover_text("The height of the box's floor (dragging the zone puts it on the ground)");
        ui.add(egui::DragValue::new(&mut bottom).speed(0.1).range(-100.0..=1000.0));
        ui.end_row();
        if bottom != was_bottom || z.half[2] != was_half { z.centre[2] = bottom + z.half[2]; }
    });
    if let rc_map::logic::ZoneKind::Camera { eye } = &mut z.kind {
        if ui.button("Use the current view as its camera").on_hover_text("Fly the editor camera to where the zone's camera should stand, then press this").clicked() {
            if let Some(t) = cam { *eye = to_game(t.translation()).map(|v| (v * 8.0).round() / 8.0); }
        }
        ui.label(egui::RichText::new(format!("Camera at {:.0}, {:.0}, {:.0}", eye[0], eye[1], eye[2])).weak().small());
    }
    if z.name.trim().is_empty() { z.name = ed.def.zones[i].name.clone(); }
    if z != ed.def.zones[i] && z.check().is_ok() {
        let old = ed.def.zones[i].name.clone();
        if !ed.coalescing { ed.checkpoint(); }
        ed.coalescing = true;
        // A rename follows into the rules.
        if z.name != old {
            for r in &mut ed.def.rules {
                use rc_map::logic::{Then, When};
                if let When::Enters(n) | When::Cleared(n) = &mut r.when { if *n == old { *n = z.name.clone(); } }
                for t in &mut r.then { if let Then::Checkpoint(n) = t { if *n == old { *n = z.name.clone(); } } }
            }
        }
        ed.def.zones[i] = z;
        ed.changed();
    }
    if ui.button("Delete zone").clicked() {
        ed.checkpoint();
        ed.def.zones.remove(i);
        ed.zone = None;
        ed.changed();
    }
}

/// The collision a map makes, as triangles coloured by what they are (the top bar's Collision view): walkable ground
/// by its surface, cliffs and walkway sides grey, edges yellow, rails orange.
fn collision_faces(def: &MapDef) -> Vec<([[f32; 3]; 3], Color)> {
    let colour = |s: u8| match s {
        7 => Color::srgb(0.4, 0.9, 1.0),
        1 => Color::srgb(1.0, 0.5, 0.1),
        8 => Color::srgb(1.0, 0.15, 0.15),
        2 => Color::srgb(0.8, 0.4, 1.0),
        _ => Color::srgb(0.3, 1.0, 0.4),
    };
    let (wall, edge) = (Color::srgba(0.75, 0.75, 0.8, 0.7), Color::srgb(1.0, 0.85, 0.2));
    let quad = |q: [[f32; 3]; 4]| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]];
    let mut out = Vec::new();
    for f in &def.floors {
        let (a, b, z) = (f.min, f.max, f.height);
        out.extend(quad([[a[0], a[1], z], [a[0], b[1], z], [b[0], b[1], z], [b[0], a[1], z]]).map(|t| (t, colour(rc_map::floor::GROUND))));
    }
    for p in &def.patches {
        out.extend(p.top().into_iter().map(|t| (t, colour(p.surface))));
        out.extend(p.walls(8.0).into_iter().flat_map(quad).map(|t| (t, wall)));
        out.extend(p.fence(8.0).into_iter().flat_map(quad).map(|t| (t, edge)));
        out.extend(p.roof_tris().into_iter().map(|t| (t, colour(rc_map::floor::GROUND))));
    }
    for w in &def.walkway_pieces() {
        out.extend(w.top_tris().into_iter().map(|t| (t, colour(w.surface))));
        out.extend(w.walls(8.0).into_iter().flat_map(quad).map(|t| (t, wall)));
        out.extend(w.fence(8.0).into_iter().flat_map(quad).map(|t| (t, edge)));
    }
    for r in &def.rails {
        for s in r.line().windows(2) { out.push(([s[0], s[1], s[1]], Color::srgb(1.0, 0.6, 0.1))); }
    }
    out
}

/// What is wrong with a map that a play would find: collision that did not fit (holes), a start over nothing, a death
/// height above ground Ratchet can stand on, objects over nothing.
fn map_warnings(def: &MapDef) -> Vec<String> {
    let mut out = Vec::new();
    let rails: Vec<Vec<[f32; 3]>> = def.rails.iter().map(|r| r.line()).collect();
    let (_, dropped) = rc_map::floor::collision_counted(&def.floors, &def.patches, &def.walkway_pieces(), &rails, &[], rc_map::floor::GROUND);
    if dropped > 0 { out.push(format!("{dropped} collision faces did not fit: there are holes. Make very steep or tall terrain smaller, or spread it out.")); }
    let pieces = def.walkway_pieces();
    let under = |x: f32, y: f32, z: f32| {
        def.floors.iter().any(|f| (f.min[0]..f.max[0]).contains(&x) && (f.min[1]..f.max[1]).contains(&y) && f.height <= z + 0.5)
            || def.patches.iter().any(|p| p.height_at(x, y).is_some_and(|h| h <= z + 0.5))
            || pieces.iter().any(|w| w.height_at(x, y).is_some_and(|h| h <= z + 0.5))
    };
    let s = def.start.position;
    if !under(s[0], s[1], s[2]) { out.push("The start has no ground under it: Ratchet falls at once.".into()); }
    if let Some(d) = def.death_height {
        let low = def.patches.iter().enumerate().filter_map(|(k, p)| p.top().iter().flatten().map(|v| v[2]).reduce(f32::min).map(|z| (k, z))).find(|&(_, z)| z <= d);
        if let Some((k, z)) = low { out.push(format!("Patch {} has ground at height {z:.1}, at or below the death height {d:.1}: Ratchet dies standing there.", k + 1)); }
        if s[2] <= d { out.push(format!("The start is at or below the death height {d:.1}.")); }
    }
    let floating = def.objects.iter().filter(|o| !under(o.position[0], o.position[1], o.position[2]) && o.path.is_empty()).count();
    if floating > 0 { out.push(format!("{floating} objects have nothing under them (they may fall, or float if that is how they work).")); }
    // Ammo crates and ammo pickups refill the weapons Ratchet owns: none in the loadout, they give nothing.
    let ammo = def.objects.iter().filter(|o| AMMO_PICKUPS.contains(&o.asset.number)).count();
    if ammo > 0 && !def.loadout.iter().any(|i| AMMO_WEAPONS.contains(i)) {
        out.push(format!("{ammo} ammo crates or pickups, but no weapon that uses ammo in the loadout (Map panel): they give nothing."));
    }
    // Readability (design doc "Guard rails"): the objects' colours shifted hard, or a fog thick enough to hide them.
    if let Some(p) = def.world.palette.filter(|p| p.objects) {
        if p.saturation < 0.6 || p.brightness < 0.65 || p.tint_amount > 0.5 || p.hue.abs() > 90.0 {
            out.push("Colours > Objects too shifts the enemies' and pickups' colours a lot: they may be hard to spot. Try it with Play, or leave Objects too off.".into());
        }
    }
    if let Some(f) = def.world.fog.filter(|f| f.far < 70.0) {
        out.push(format!("The fog is full at {:.0} units: enemies and ledges further away disappear in it.", f.far));
    }
    let vanish = def.objects.iter().filter(|o| crate::map_load::MAY_VANISH.contains(&o.asset.number)).count();
    if vanish > 0 { out.push(format!("{vanish} objects are marked \"may vanish on maps\": try them with Play before building around them.")); }
    // The logic: names used twice, rules naming what is not there, hidden objects nothing shows.
    let mut names: Vec<&str> = def.objects.iter().filter_map(|o| o.name.as_deref()).collect();
    names.sort_unstable();
    for w in names.windows(2) { if w[0] == w[1] { out.push(format!("Two objects are named \"{}\": rules use the first.", w[0])); } }
    let mut zones: Vec<&str> = def.zones.iter().map(|z| z.name.as_str()).collect();
    zones.sort_unstable();
    for w in zones.windows(2) { if w[0] == w[1] { out.push(format!("Two zones are named \"{}\": rules use the first.", w[0])); } }
    for (r, rule) in def.rules.iter().enumerate() {
        let missing = rule.missing(&def.zones, &def.objects);
        if !missing.is_empty() { out.push(format!("Rule {} names what is not on the map: {}.", r + 1, missing.join(", "))); }
    }
    let shown: Vec<&str> = def.rules.iter().flat_map(|r| r.then.iter()).filter_map(|t| if let rc_map::logic::Then::Show(n) = t { Some(n.as_str()) } else { None }).collect();
    let never = def.objects.iter().filter(|o| o.hidden && !o.name.as_deref().is_some_and(|n| shown.contains(&n))).count();
    if never > 0 { out.push(format!("{never} hidden objects are never shown by a rule: they never appear.")); }
    out
}

/// Ammo crates (511) and the ammo pickups: what they give depends on the weapons owned.
const AMMO_PICKUPS: [i32; 12] = [511, 204, 213, 214, 222, 223, 225, 226, 1006, 1438, 1447, 1449];
/// The loadout's weapons that use ammo (ITEMS).
const AMMO_WEAPONS: [usize; 11] = [10, 11, 13, 15, 16, 17, 19, 20, 23, 24, 25];

/// Every stretch of a patch's edge, open or not.
fn edge_stretches(p: &rc_map::terrain::Patch) -> Vec<Stretch> {
    let Some(e) = &p.edge else { return Vec::new() };
    p.loops()
        .into_iter()
        .map(rc_map::coast::Coast::new)
        .flat_map(|c| {
            e.spans(&c).into_iter().map(move |(s0, s1, open)| {
                let mid = c.point((s0 + s1) / 2.0);
                Stretch { a: c.point(s0), b: c.point(s1), mid, half: (s1 - s0) / 2.0, open }
            }).collect::<Vec<_>>()
        })
        .collect()
}

/// Openings mode: the stretch of the selected island's edge under the pointer (within 24 pixels of it on screen, at
/// its middle height); a click opens it (or closes an opening), a drag does the same to every stretch it passes; a
/// click away from the edge selects the island there.
#[allow(clippy::too_many_arguments)]
fn open_edges(ed: &mut Editor, ray: ([f32; 3], [f32; 3]), window: &Window, frame: &crate::display::GameFrame, cam: (&Camera, &GlobalTransform), over_ui: bool, buttons: &ButtonInput<MouseButton>) {
    let k = ed.patch.filter(|&k| k < ed.def.patches.len());
    ed.gap_hover = None;
    if let (Some(k), false, Some(cursor)) = (k, over_ui, window.cursor_position()) {
        let p = &ed.def.patches[k];
        let mid = p.edge.as_ref().map_or(0.5, |e| e.height.min(3.0) / 2.0);
        let mut best: Option<(f32, Stretch)> = None;
        for st in edge_stretches(p) {
            let (a, b) = (st.a, st.b);
            let (Some(sa), Some(sb)) = (to_window(window, frame, cam, [a[0], a[1], a[2] + mid]), to_window(window, frame, cam, [b[0], b[1], b[2] + mid])) else { continue };
            let ab = sb - sa;
            let t = ((cursor - sa).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
            let d = (sa + ab * t).distance(cursor);
            if d < 24.0 && best.is_none_or(|b| d < b.0) { best = Some((d, st)); }
        }
        ed.gap_hover = best.map(|b| b.1);
    }
    if buttons.just_pressed(MouseButton::Left) && !over_ui {
        match ed.gap_hover {
            Some(st) => ed.gap_paint = Some((!st.open, ed.snap())),
            None => {
                if let Some((_, _, Some(j))) = terrain_hit(&ed.def, ray) {
                    ed.patch = Some(j);
                    ed.status = if ed.def.patches[j].edge.is_some() { format!("Patch {} selected: click its edge", j + 1) } else { format!("Patch {} has no edge: choose one on the right", j + 1) };
                }
            }
        }
    }
    if let (Some((open, _)), Some(st), Some(k)) = (&ed.gap_paint, ed.gap_hover, k) {
        if buttons.pressed(MouseButton::Left) && st.open != *open {
            let open = *open;
            let (mx, my) = (st.mid[0], st.mid[1]);
            if let Some(e) = ed.def.patches[k].edge.as_mut() {
                if open {
                    // Exactly that stretch (the ones beside it are fitted up to it).
                    e.gaps.push([(mx * 64.0).round() / 64.0, (my * 64.0).round() / 64.0, ((st.half + 0.0625).max(0.5) * 64.0).round() / 64.0]);
                } else {
                    // Every opening that reaches into it.
                    e.gaps.retain(|g| ((mx - g[0]).powi(2) + (my - g[1]).powi(2)).sqrt() > g[2] + st.half - 0.05);
                }
                crate::map_load::set(ed.def.clone());
            }
        }
    }
    if !buttons.pressed(MouseButton::Left) {
        if let Some((open, start)) = ed.gap_paint.take() {
            if start.def.patches != ed.def.patches {
                ed.undo.push(start);
                ed.redo.clear();
                ed.status = if open { "Edge opened".into() } else { "Edge closed".into() };
                ed.changed();
            }
        }
    }
}

/// Shape mode: the selected island's handles. Hovered, the wheel sets the anchors' reach; a press on one starts a
/// drag (each frame redrawn from the island before it, one undo step, the collision on release); a press elsewhere
/// selects the island under the pointer. Ctrl snaps to the grid (moves and pulls) or half units (height).
#[allow(clippy::too_many_arguments)]
fn shape_island(
    ed: &mut Editor,
    ray: ([f32; 3], [f32; 3]),
    window: &Window,
    frame: &crate::display::GameFrame,
    cam: (&Camera, &GlobalTransform),
    over_ui: bool,
    buttons: &ButtonInput<MouseButton>,
    keys: &ButtonInput<KeyCode>,
    wheel: f32,
) {
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    if ed.shape_drag.is_none() {
        ed.shape_hover = if over_ui { None } else { ed.patch.and_then(|k| ed.def.patches.get(k)).and_then(|p| handle_at(p, window, frame, cam)) };
    }
    if (ed.shape_hover.is_some() || ed.shape_drag.is_some()) && wheel != 0.0 {
        ed.reach = (ed.reach * if wheel > 0.0 { 1.15 } else { 1.0 / 1.15 }).clamp(1.5, 64.0);
    }
    if buttons.just_pressed(MouseButton::Left) && !over_ui {
        match (ed.shape_hover, ed.patch) {
            (Some(h), Some(k)) if k < ed.def.patches.len() => {
                let grab = match h {
                    IslandHandle::Anchor(q) | IslandHandle::Move(q) => on_plane(ray, q[2]).unwrap_or(q),
                    IslandHandle::Lift(q) => [q[0], q[1], ray_line(ray, q, [0.0, 0.0, 1.0]).1],
                };
                ed.shape_drag = Some(ShapeDrag { patch: k, base: ed.def.patches[k].clone(), handle: h, grab, start: ed.snap() });
            }
            _ => {
                if let Some((_, _, Some(k))) = terrain_hit(&ed.def, ray) {
                    ed.patch = Some(k);
                    ed.status = format!("Patch {} selected: drag its handles", k + 1);
                }
            }
        }
    }
    if let Some(drag) = &ed.shape_drag {
        if buttons.pressed(MouseButton::Left) {
            let snap = |v: f32, step: f32| if ctrl { (v / step).round() * step } else { v };
            let base = &drag.base;
            let result = match drag.handle {
                IslandHandle::Anchor(a) => on_plane(ray, a[2]).map(|h| {
                    let d = [snap(h[0] - drag.grab[0], base.spacing), snap(h[1] - drag.grab[1], base.spacing)];
                    if d == [0.0, 0.0] { Ok(base.clone()) } else { base.warped([a[0], a[1]], d, ed.reach) }
                }),
                IslandHandle::Move(m) => on_plane(ray, m[2]).map(|h| {
                    // Its openings and scatter go with it.
                    let mut p = base.clone();
                    p.shift([snap(h[0] - drag.grab[0], base.spacing), snap(h[1] - drag.grab[1], base.spacing), 0.0]);
                    Ok(p)
                }),
                IslandHandle::Lift(l) => {
                    let s_ = ray_line(ray, l, [0.0, 0.0, 1.0]).1;
                    let mut p = base.clone();
                    p.origin[2] = ((base.origin[2] + snap(s_ - drag.grab[2], 0.5)) * 64.0).round() / 64.0;
                    Some(Ok(p))
                }
            };
            let k = drag.patch;
            match result {
                Some(Ok(p)) if p.check().is_ok() => {
                    if ed.def.patches.get(k) != Some(&p) {
                        ed.def.patches[k] = p;
                        crate::map_load::set(ed.def.clone());
                    }
                }
                Some(Ok(p)) => ed.status = format!("Not there: {}", p.check().err().unwrap_or_default()),
                Some(Err(e)) => ed.status = e,
                None => {}
            }
        } else if let Some(drag) = ed.shape_drag.take() {
            if drag.start.def.patches != ed.def.patches {
                ed.undo.push(drag.start);
                ed.redo.clear();
                ed.status = match drag.handle {
                    IslandHandle::Anchor(_) => "Coast reshaped".into(),
                    IslandHandle::Move(_) => "Island moved".into(),
                    IslandHandle::Lift(_) => "Island height changed".into(),
                };
                ed.changed();
            }
        }
    }
}

/// The selected object's gizmo under the ray, if any: a path point (within 14 pixels on screen), an arrow, the ring.
fn grab_gizmo(ed: &Editor, i: usize, ray: ([f32; 3], [f32; 3]), window: &Window, frame: &crate::display::GameFrame, cam: (&Camera, &GlobalTransform)) -> Option<Gizmo> {
    let o = &ed.def.objects[i];
    let cursor = window.cursor_position()?;
    for (k, q) in o.path.iter().enumerate() {
        if to_window(window, frame, cam, *q).is_some_and(|w| w.distance(cursor) < 14.0) { return Some(Gizmo::Point { k }); }
    }
    let local = ed.local_box(&o.asset);
    let (len, radius) = gizmo_size(local);
    let c = gizmo_centre(o.position, local);
    let cam_at = to_game(cam.1.translation());
    let dist = ((c[0] - cam_at[0]).powi(2) + (c[1] - cam_at[1]).powi(2) + (c[2] - cam_at[2]).powi(2)).sqrt();
    let tolerance = (dist * 0.025).max(0.15);
    for (axis, dir) in AXES.iter().enumerate() {
        let (d, s_, t) = ray_line(ray, c, *dir);
        if t > 0.0 && d < tolerance && (0.15..=len + 0.3).contains(&s_) { return Some(Gizmo::Axis { axis, t0: s_, from: o.position }); }
    }
    let h = on_plane(ray, o.position[2] + 0.05)?;
    let r = ((h[0] - o.position[0]).powi(2) + (h[1] - o.position[1]).powi(2)).sqrt();
    ((r - radius).abs() < tolerance * 1.5).then(|| Gizmo::Ring { a0: (h[1] - o.position[1]).atan2(h[0] - o.position[0]), yaw0: o.yaw })
}

/// A gizmo drag step on object `i`.
fn drag_gizmo(ed: &mut Editor, i: usize, g: Gizmo, ray: ([f32; 3], [f32; 3])) {
    let o = ed.def.objects[i].clone();
    let local = ed.local_box(&o.asset);
    match g {
        Gizmo::Axis { axis, t0, from } => {
            let c = gizmo_centre(from, local);
            let (_, s_, _) = ray_line(ray, c, AXES[axis]);
            let mut p = from;
            p[axis] += s_ - t0;
            // Never into the ground.
            let floor = ground(&ed.def, p[0], p[1], p[2] + 0.5) - local[2].min(0.0);
            p[2] = p[2].max(floor).clamp(0.0, 1023.0);
            p[0] = p[0].clamp(0.0, 1023.0);
            p[1] = p[1].clamp(0.0, 1023.0);
            let d = [p[0] - o.position[0], p[1] - o.position[1], p[2] - o.position[2]];
            let ob = &mut ed.def.objects[i];
            ob.position = p;
            for q in &mut ob.path { for k in 0..3 { q[k] += d[k]; } }
        }
        Gizmo::Ring { a0, yaw0 } => {
            if let Some(h) = on_plane(ray, o.position[2] + 0.05) {
                let a = (h[1] - o.position[1]).atan2(h[0] - o.position[0]);
                // In 15° steps.
                let step = 15f32.to_radians();
                ed.def.objects[i].yaw = ((yaw0 + a - a0) / step).round() * step;
                ed.def.objects[i].yaw = ed.def.objects[i].yaw.rem_euclid(std::f32::consts::TAU);
            }
        }
        Gizmo::Point { k } => {
            let z = o.path.get(k).map_or(o.position[2], |q| q[2]);
            if let Some(h) = on_plane(ray, z) {
                let ob = &mut ed.def.objects[i];
                ob.path[k] = [h[0].clamp(0.0, 1023.0), h[1].clamp(0.0, 1023.0), z];
                // The first point is where the object stands.
                if k == 0 { ob.position = [h[0], h[1], ob.position[2]]; }
            }
        }
    }
}

fn keys(mut ed: ResMut<Editor>, mut mode: ResMut<EditorMode>, keys: Res<ButtonInput<KeyCode>>, focus: Res<UiFocus>, mut source: Option<ResMut<CameraSource>>) {
    if keys.just_pressed(KeyCode::F5) { toggle_play(&mut ed, &mut mode, source.as_deref_mut()); }
    // The goal reached (crate::map_logic): back to the editor once its banner has shown.
    if *mode == EditorMode::Playing && crate::map_logic::goal_elapsed().is_some_and(|d| d.as_secs_f32() > 4.0) {
        crate::map_logic::reset();
        toggle_play(&mut ed, &mut mode, source.as_deref_mut());
        ed.status = "Goal reached! Back in the editor".into();
    }
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    // Ctrl+S always saves, also while a text or number field has the keyboard.
    if ctrl && keys.just_pressed(KeyCode::KeyS) && *mode == EditorMode::Editing { ed.save(); }
    // F6: the collision view, editing or playing.
    if keys.just_pressed(KeyCode::F6) {
        ed.show_collision = !ed.show_collision;
        ed.collision_view = None;
    }
    if !editing(&ed, *mode) || focus.keyboard { return; }
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    if ctrl && keys.just_pressed(KeyCode::KeyZ) { ed.undo(); }
    if ctrl && keys.just_pressed(KeyCode::KeyY) { ed.redo(); }
    let mut esc = keys.just_pressed(KeyCode::Escape);
    if ed.tool == Tool::Terrain && ed.terrain_mode == TerrainMode::Rails {
        let drawing = ed.rail_drawing || ed.rail_first.is_some();
        if (keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter) || esc) && drawing {
            ed.status = "Rail done".into();
            ed.rail_drawing = false;
            ed.rail_first = None;
            esc = false;
        }
        let k = ed.rail.filter(|&k| k < ed.def.rails.len());
        let take = if keys.just_pressed(KeyCode::Delete) { ed.rail_hover } else if keys.just_pressed(KeyCode::Backspace) && ed.rail_drawing { k.map(|k| ed.def.rails[k].points.len() - 1) } else { None };
        if let (Some(k), Some(i)) = (k, take) {
            if ed.def.rails[k].points.len() > 2 {
                ed.checkpoint();
                ed.def.rails[k].points.remove(i);
                ed.rail_hover = None;
                ed.changed();
            }
        }
    }
    if ed.tool == Tool::Terrain && ed.terrain_mode == TerrainMode::Paths {
        let drawing = ed.walk_drawing || ed.walk_first.is_some();
        if (keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter) || esc) && drawing {
            ed.status = "Path done".into();
            ed.walk_drawing = false;
            ed.walk_first = None;
            esc = false;
        }
        // Delete over a point takes it out (a path keeps two); Backspace takes the last one off the path being drawn.
        let k = ed.walk.filter(|&k| k < ed.def.walkways.len());
        let take = if keys.just_pressed(KeyCode::Delete) { ed.walk_hover } else if keys.just_pressed(KeyCode::Backspace) && ed.walk_drawing { k.map(|k| ed.def.walkways[k].points.len() - 1) } else { None };
        if let (Some(k), Some(i)) = (k, take) {
            if ed.def.walkways[k].points.len() > 2 {
                ed.checkpoint();
                ed.def.walkways[k].points.remove(i);
                ed.walk_hover = None;
                ed.status = "Point removed".into();
                ed.changed();
            }
        }
    }
    // Esc: the object being placed put down, else the terrain tool (back to Select), else the selection cleared.
    if esc {
        if ed.armed.is_some() {
            ed.armed = None;
            ed.status = "Placing cancelled".into();
        } else if ed.tool == Tool::Terrain && ed.terrain_mode != TerrainMode::Select {
            ed.terrain_mode = TerrainMode::Select;
            ed.stroke = None;
            ed.status = "Select: click an island, a path or an object".into();
        } else {
            (ed.selected, ed.patch, ed.walk) = (None, None, None);
        }
    }
    // While placing, R / Shift+R turn the object to place.
    if ed.armed.is_some() && keys.just_pressed(KeyCode::KeyR) && !ctrl {
        let step = if shift { -15f32 } else { 15.0 }.to_radians();
        ed.place_yaw = (ed.place_yaw + step).rem_euclid(std::f32::consts::TAU);
        ed.status = format!("Placing at {:.0}°", ed.place_yaw.to_degrees());
        return;
    }
    if let Some(i) = ed.selected {
        if keys.just_pressed(KeyCode::Delete) {
            ed.checkpoint();
            let gone = ed.remove_object(i);
            ed.selected = None;
            ed.status = format!("Removed {}", ed.name_of(&gone.asset));
            ed.changed();
        } else if keys.just_pressed(KeyCode::KeyR) && !ctrl {
            ed.checkpoint();
            let step = if shift { -15f32 } else { 15.0 }.to_radians();
            let y = &mut ed.def.objects[i].yaw;
            *y = (*y + step).rem_euclid(std::f32::consts::TAU);
            ed.changed();
        }
    }
}

/// F5: editing → playing (the tick runs on the loaded level, the play camera follows Ratchet), or back. Play asked
/// for while a load runs waits for it (F5 again: not after all).
fn toggle_play(ed: &mut Editor, mode: &mut EditorMode, source: Option<&mut CameraSource>) {
    if *mode == EditorMode::Editing && ed.reload != Reload::Idle {
        ed.play_pending = !ed.play_pending;
        ed.status = if ed.play_pending { "Play starts when the level has loaded…".into() } else { "Play cancelled".into() };
        return;
    }
    ed.play_pending = false;
    *mode = match *mode {
        EditorMode::Editing => {
            ed.armed = None;
            ed.drag = None;
            ed.status = "Playing — F5 to stop".into();
            if let Some(s) = source { *s = CameraSource::Play; }
            EditorMode::Playing
        }
        EditorMode::Playing => {
            ed.status = "Back to editing".into();
            // A game menu left open (the pause menu, the map) would stay over the view.
            crate::menu_render::request_leave();
            if let Some(s) = source { *s = CameraSource::Fly; }
            EditorMode::Editing
        }
    };
    // While editing the tick is frozen, so the loaded level is still as it loaded: play starts on it. Back to editing,
    // the map loads again (what play broke or moved comes back).
    if *mode == EditorMode::Editing {
        match ed.reload {
            Reload::Idle => ed.reload = Reload::Wanted,
            Reload::Loading(_) => ed.reload_again = true,
            Reload::Wanted => {}
        }
    }
}

/// Runs the reloads: hands the working map to the loader, loads the host in the background, swaps it in; a failed
/// load goes back to the last good map. Starts a pending play once the level is in.
#[allow(clippy::too_many_arguments)]
fn drive_reload(
    mut ed: ResMut<Editor>,
    mut mode: ResMut<EditorMode>,
    mut change: ResMut<crate::level_switch::LevelChange>,
    generation: Res<crate::level_switch::LevelGeneration>,
    mut state: Option<ResMut<crate::gameplay::Persistent>>,
    mut session: Option<ResMut<crate::gameplay::Session>>,
    mut source: Option<ResMut<CameraSource>>,
    mut started: Local<Option<std::time::Instant>>,
) {
    match ed.reload {
        Reload::Idle => {
            if ed.play_pending { toggle_play(&mut ed, &mut mode, source.as_deref_mut()); }
        }
        Reload::Wanted => {
            if !change.idle() || ed.drag.is_some() || ed.coalescing || ed.stroke.is_some() { return; }
            ed.group_crates();
            ed.fill_solid();
            crate::map_load::set(ed.def.clone());
            // The map's loadout, exactly (the game state outlives the reloads), and its gold bolts not collected yet.
            if let (Some(gs), Some(sess)) = (state.as_deref_mut(), session.as_deref_mut()) {
                let editable: Vec<usize> = ITEMS.iter().map(|i| i.0).collect();
                crate::gameplay::apply_loadout(&mut gs.0, &mut sess.0, &ed.def.loadout, &editable, ed.def.host_level);
                if let Some(l) = gs.0.levels.get_mut(ed.def.host_level as usize) { l.gold_bolts.iter_mut().for_each(|b| *b = 0); }
            }
            change.last_error = None;
            change.start_load(ed.def.host_level, Default::default());
            ed.loading = Some(ed.snap());
            ed.reload = Reload::Loading(generation.0);
            *started = Some(std::time::Instant::now());
        }
        Reload::Loading(g0) => {
            if change.loaded() { change.swap(); }
            if generation.0 != g0 {
                if let Some(s) = ed.loading.take() { ed.loaded = s; }
                if let Some(t) = started.take() { println!("editor: reload in {:.2} s", t.elapsed().as_secs_f64()); }
                ed.reload = if std::mem::take(&mut ed.reload_again) { Reload::Wanted } else { Reload::Idle };
            } else if change.idle() {
                // The swap ran and kept the old level: the load failed.
                let why = change.last_error.take().unwrap_or_else(|| "the level did not load".into());
                ed.status = format!("That change could not load ({}); undone", why.lines().last().unwrap_or(&why));
                let back = ed.loaded.clone();
                ed.restore(back);
                ed.loading = None;
                crate::map_load::set(ed.def.clone());
                ed.selected = None;
                ed.reload_again = false;
                ed.play_pending = false;
                ed.reload = Reload::Idle;
            }
        }
    }
}

/// The editing view is kept while playing (the play camera moves the same camera): Stop puts it back.
fn keep_view(mode: Res<EditorMode>, mut cams: Query<(&mut Transform, &mut FlyCam)>, mut kept: Local<Option<(Transform, f32, f32)>>) {
    if !mode.is_changed() { return; }
    let Ok((mut t, mut fly)) = cams.single_mut() else { return };
    match *mode {
        EditorMode::Playing => *kept = Some((*t, fly.yaw, fly.pitch)),
        EditorMode::Editing => {
            if let Some((view, yaw, pitch)) = kept.take() {
                *t = view;
                (fly.yaw, fly.pitch) = (yaw, pitch);
            }
        }
    }
}

/// While editing, the loaded level's instances follow the map at once: a moved or turned object is redrawn there,
/// a removed one hidden (the reload then makes it so in the game too).
fn live_preview(
    mut ed: ResMut<Editor>,
    mode: Res<EditorMode>,
    generation: Res<crate::level_switch::LevelGeneration>,
    play: Option<ResMut<crate::gameplay::Play>>,
    occl: Option<ResMut<crate::moby_render::MobyOcclusion>>,
    mut shown: Local<HashMap<usize, Option<([f32; 3], f32)>>>,
) {
    let fresh = generation.is_changed();
    if fresh { shown.clear(); }
    let (Some(mut play), Some(mut occl)) = (play, occl) else { return };
    // A fresh level: where the game put each object (a class that places itself from its home level's data moves
    // away from where it was put; crates only drop onto the ground).
    if play.is_added() {
        let mut away = HashMap::new();
        for (k, id) in ed.loaded.ids.iter().enumerate() {
            let Some(at) = crate::gameplay::editor_instance_at(&play, k + 1) else { continue };
            let p = ed.loaded.def.objects[k].position;
            let flat = ((at[0] - p[0]).powi(2) + (at[1] - p[1]).powi(2)).sqrt();
            if flat > 1.0 || at[2] - p[2] > 1.0 || p[2] - at[2] > 3.0 { away.insert(*id, at); }
        }
        if !away.is_empty() { println!("editor: {} objects moved away by the game at load", away.len()); }
        ed.moved_away = away;
    }
    if fresh { return; }
    if *mode != EditorMode::Editing || !play.is_added() && !ed.is_changed() && shown.is_empty() { return; }
    for (k, id) in ed.loaded.ids.iter().enumerate() {
        let ii = k + 1;
        let was = &ed.loaded.def.objects[k];
        let now = ed.ids.iter().position(|i| i == id).map(|j| &ed.def.objects[j]);
        let want = now.map(|o| (o.position, o.yaw));
        let same = now.is_some_and(|o| o.position == was.position && o.yaw == was.yaw);
        if same && !shown.contains_key(&ii) { continue; }
        if shown.get(&ii) == Some(&want) { continue; }
        crate::gameplay::editor_show(&mut play, &mut occl, ii, want);
        shown.insert(ii, want);
    }
}

/// Markers: the start, every object's box (the selected one highlighted, the ones not loaded yet dashed in
/// orange), and where the armed asset would go.
#[allow(clippy::too_many_arguments)]
fn markers(
    ed: Res<Editor>,
    play_cams: Query<&GlobalTransform, (With<Camera3d>, Without<FlyCam>)>,
    mode: Res<EditorMode>,
    focus: Res<UiFocus>,
    windows: Query<&Window, With<PrimaryWindow>>,
    frame: Res<crate::display::GameFrame>,
    cams: Query<(&Camera, &GlobalTransform), With<FlyCam>>,
    mut gizmos: Gizmos,
    mut handles: Gizmos<Handles>,
) {
    if ed.thumb_shot.is_some() { return; }
    // The collision near the camera, coloured by what it is (also while playing: F6).
    if ed.show_collision {
        if let Some((_, faces)) = &ed.collision_view {
            let eye = cams.single().ok().map(|c| to_game(c.1.translation())).or_else(|| play_cams.iter().next().map(|t| to_game(t.translation()))).unwrap_or(ed.def.start.position);
            for (t, c) in faces {
                let m = [(t[0][0] + t[1][0] + t[2][0]) / 3.0, (t[0][1] + t[1][1] + t[2][1]) / 3.0, (t[0][2] + t[1][2] + t[2][2]) / 3.0];
                if (m[0] - eye[0]).powi(2) + (m[1] - eye[1]).powi(2) + (m[2] - eye[2]).powi(2) > 45.0 * 45.0 { continue; }
                let v = t.map(|q| game_to_bevy([q[0], q[1], q[2] + 0.03]));
                gizmos.linestrip([v[0], v[1], v[2], v[0]], *c);
            }
        }
    }
    if *mode != EditorMode::Editing { return; }
    let s = ed.def.start.position;
    let flat = Quat::from_rotation_arc(Vec3::Z, Vec3::Y);
    gizmos.circle(Isometry3d::new(game_to_bevy([s[0], s[1], s[2] - 0.45]), flat), 0.8, Color::srgb(0.3, 0.9, 1.0));
    let draw_box = |gizmos: &mut Gizmos, local: [f32; 6], pos: [f32; 3], yaw: f32, color: Color| {
        let c = [(local[0] + local[3]) / 2.0, (local[1] + local[4]) / 2.0, (local[2] + local[5]) / 2.0];
        let size = Vec3::new(local[3] - local[0], local[5] - local[2], local[4] - local[1]).max(Vec3::splat(0.05));
        let rot = Quat::from_rotation_y(yaw);
        let at = game_to_bevy(pos) + rot * Vec3::new(c[0], c[2], -c[1]);
        gizmos.cube(Transform { translation: at, rotation: rot, scale: size }, color);
    };
    // Every object: a small circle on the ground (the models show what they are); the selected one: its box, its
    // gizmo (arrows to move, the ring to turn), its sight ranges and its path.
    for (i, ob) in ed.def.objects.iter().enumerate() {
        let local = ed.local_box(&ob.asset);
        let foot = game_to_bevy([ob.position[0], ob.position[1], ob.position[2] + 0.04]);
        if let Some(at) = ed.moved_away.get(&ed.ids[i]) {
            gizmos.line(foot, game_to_bevy(*at), Color::srgb(1.0, 0.3, 0.3));
            gizmos.sphere(Isometry3d::from_translation(game_to_bevy(*at)), 0.3, Color::srgb(1.0, 0.3, 0.3));
        }
        if ed.selected != Some(i) {
            gizmos.circle(Isometry3d::new(foot, flat), 0.35, Color::srgba(1.0, 1.0, 1.0, 0.5));
            if ob.path.len() >= 2 {
                gizmos.linestrip(ob.path.iter().map(|q| game_to_bevy(*q)), Color::srgba(0.4, 0.9, 1.0, 0.4));
            }
            continue;
        }
        draw_box(&mut gizmos, local, ob.position, ob.yaw, Color::srgba(1.0, 0.6, 0.1, 0.8));
        let (len, radius) = gizmo_size(local);
        let c = gizmo_centre(ob.position, local);
        for (k, dir) in AXES.iter().enumerate() {
            let a = game_to_bevy(c);
            let b = game_to_bevy([c[0] + dir[0] * len, c[1] + dir[1] * len, c[2] + dir[2] * len]);
            handles.arrow(a, b, AXIS_COLORS[k]);
        }
        handles.sphere(Isometry3d::from_translation(game_to_bevy(c)), 0.12, Color::WHITE);
        handles.circle(Isometry3d::new(game_to_bevy([ob.position[0], ob.position[1], ob.position[2] + 0.05]), flat), radius, Color::srgb(1.0, 0.85, 0.2));
        // Facing.
        let f = game_to_bevy([ob.position[0] + ob.yaw.cos() * radius, ob.position[1] + ob.yaw.sin() * radius, ob.position[2] + 0.05]);
        handles.line(foot, f, Color::srgb(1.0, 0.85, 0.2));
        if let Some((wake, range)) = ed.ranges.get(&ob.asset).copied().flatten() {
            if wake > 0.0 { gizmos.circle(Isometry3d::new(foot, flat), wake, Color::srgba(0.4, 0.7, 1.0, 0.8)); }
            if range > 0.0 { gizmos.circle(Isometry3d::new(foot, flat), range, Color::srgba(1.0, 0.35, 0.3, 0.8)); }
        }
        if ob.path.len() >= 2 {
            gizmos.linestrip(ob.path.iter().map(|q| game_to_bevy(*q)), Color::srgb(0.4, 0.9, 1.0));
            for (k, q) in ob.path.iter().enumerate() {
                let c = if k == 0 { Color::srgb(0.4, 1.0, 0.5) } else if k + 1 == ob.path.len() { Color::srgb(1.0, 0.4, 0.4) } else { Color::srgb(0.4, 0.9, 1.0) };
                handles.sphere(Isometry3d::from_translation(game_to_bevy(*q)), 0.3, c);
            }
        }
    }
    // The brush over the terrain, and the selected patch's rim.
    if ed.tool == Tool::Terrain {
        if let Some(at) = ed.brush_at {
            let c = if ed.stroke.is_some() { Color::srgb(1.0, 0.6, 0.1) } else { Color::srgb(0.4, 1.0, 0.5) };
            // The brush's edge laid on the ground (it follows hills and dips).
            let on_ground = |x: f32, y: f32| game_to_bevy([x, y, ground(&ed.def, x, y, at[2] + ed.radius) + 0.12]);
            let r = ed.radius;
            let pts: Vec<Vec3> = if ed.square {
                (0..=64).map(|k| {
                    let t = k as f32 / 16.0;
                    let (side, f) = ((t.floor() as i32).min(3), t.fract() * 2.0 - 1.0);
                    let (x, y) = match side { 0 => (f * r, -r), 1 => (r, f * r), 2 => (-f * r, r), _ => (-r, -f * r) };
                    on_ground(at[0] + x, at[1] + y)
                }).collect()
            } else {
                (0..=64).map(|k| { let a = k as f32 / 64.0 * std::f32::consts::TAU; on_ground(at[0] + a.cos() * r, at[1] + a.sin() * r) }).collect()
            };
            gizmos.linestrip(pts, c);
            gizmos.circle(Isometry3d::new(game_to_bevy([at[0], at[1], at[2] + 0.1]), flat), 0.2, c);
        }
        // The selected island's coast (not while drawing rails or paths: they are not about it).
        if let Some(p) = ed.patch.and_then(|k| ed.def.patches.get(k)).filter(|_| !matches!(ed.terrain_mode, TerrainMode::Rails | TerrainMode::Paths)) {
            // Its coast.
            for lp in p.loops() {
                let pts: Vec<Vec3> = lp.iter().map(|q| game_to_bevy([q[0], q[1], q[2] + 0.06])).collect();
                gizmos.linestrip(pts.iter().copied().chain(pts.first().copied()), Color::srgb(1.0, 0.6, 0.1));
            }
            if ed.terrain_mode == TerrainMode::Openings {
                // The edge's stretches: standing in yellow, openings in red, the one under the pointer in white.
                let h = p.edge.as_ref().map_or(1.0, |e| e.height.min(4.0));
                for st in edge_stretches(p) {
                    let (a, b, open) = (st.a, st.b, st.open);
                    let hot = ed.gap_hover.is_some_and(|g| g.a == a && g.b == b);
                    let c = if hot { Color::WHITE } else if open { Color::srgb(1.0, 0.3, 0.3) } else { Color::srgb(1.0, 0.85, 0.2) };
                    let lift = |q: [f32; 3], z: f32| game_to_bevy([q[0], q[1], q[2] + z]);
                    gizmos.line(lift(a, 0.12), lift(b, 0.12), c);
                    gizmos.line(lift(a, h), lift(b, h), c);
                    if hot || open {
                        gizmos.line(lift(a, 0.12), lift(a, h), c);
                        gizmos.line(lift(b, 0.12), lift(b, h), c);
                    }
                    if open { gizmos.line(lift(a, 0.12), lift(b, h), c); gizmos.line(lift(b, 0.12), lift(a, h), c); }
                }
            }
            if ed.terrain_mode == TerrainMode::Shape {
                let dragged = ed.shape_drag.as_ref().map(|d| d.handle);
                let hot = dragged.or(ed.shape_hover);
                for h in island_handles(p) {
                    let on = hot.is_some_and(|x| std::mem::discriminant(&x) == std::mem::discriminant(&h) && match (x, h) { (IslandHandle::Anchor(a), IslandHandle::Anchor(b)) => (a[0] - b[0]).abs() + (a[1] - b[1]).abs() < 0.01, _ => true });
                    match h {
                        IslandHandle::Anchor(q) => {
                            let c = if on { Color::srgb(1.0, 0.9, 0.2) } else { Color::srgb(1.0, 1.0, 1.0) };
                            gizmos.sphere(Isometry3d::from_translation(game_to_bevy(q)), if on { 0.35 } else { 0.22 }, c);
                        }
                        IslandHandle::Move(q) => {
                            let c = if on { Color::srgb(1.0, 0.9, 0.2) } else { Color::srgb(0.4, 1.0, 0.5) };
                            gizmos.rect(Isometry3d::new(game_to_bevy(q), flat * Quat::from_rotation_z(std::f32::consts::FRAC_PI_4)), Vec2::splat(if on { 1.3 } else { 1.0 }), c);
                            gizmos.circle(Isometry3d::new(game_to_bevy(q), flat), 0.25, c);
                        }
                        IslandHandle::Lift(q) => {
                            let c = if on { Color::srgb(1.0, 0.9, 0.2) } else { Color::srgb(0.35, 0.6, 1.0) };
                            gizmos.arrow(game_to_bevy([q[0], q[1], q[2] - 3.4]), game_to_bevy(q), c);
                            gizmos.sphere(Isometry3d::from_translation(game_to_bevy(q)), if on { 0.4 } else { 0.28 }, c);
                        }
                    }
                }
                // The anchor's reach: how much coast its pull carries.
                if let Some(IslandHandle::Anchor(a)) = hot {
                    let centre = match &ed.shape_drag { Some(d) => d.grab, None => a };
                    gizmos.circle(Isometry3d::new(game_to_bevy([centre[0], centre[1], a[2]]), flat), ed.reach, Color::srgba(1.0, 0.9, 0.2, 0.6));
                }
            }
        }
    }
    // The point lights (World tab): a ball in their colour and their reach on the ground, the picked one brighter.
    if ed.tool == Tool::World {
        for (k, l) in ed.def.world.lights.iter().enumerate() {
            let c = Color::srgb_u8(l.colour[0], l.colour[1], l.colour[2]);
            let on = ed.light_sel == Some(k);
            handles.sphere(Isometry3d::from_translation(game_to_bevy(l.position)), if on { 0.5 } else { 0.3 }, if on { Color::WHITE } else { c });
            let g = ground(&ed.def, l.position[0], l.position[1], l.position[2]);
            gizmos.circle(Isometry3d::new(game_to_bevy([l.position[0], l.position[1], g + 0.1]), Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)), l.radius, c.with_alpha(if on { 0.9 } else { 0.4 }));
            gizmos.line(game_to_bevy(l.position), game_to_bevy([l.position[0], l.position[1], g]), c.with_alpha(0.5));
        }
    }
    // The zone a click would place, at the pointer (pulsing), then the zones: their boxes in their kind's colour (faint
    // outside the Logic tab), a camera zone's eye.
    if let Some(z) = ed.zone_ghost.as_ref().filter(|_| ed.tool == Tool::Logic && ed.zone_new.is_some()) {
        let k = z.corners().map(game_to_bevy);
        let c = zone_colour(&z.kind);
        for (a, b) in [(0, 1), (1, 3), (3, 2), (2, 0), (4, 5), (5, 7), (7, 6), (6, 4), (0, 4), (1, 5), (2, 6), (3, 7)] { gizmos.line(k[a], k[b], c); }
        // Its floor crossed, so the spot it stands on reads at a glance.
        gizmos.line(k[0], k[3], c.with_alpha(0.6));
        gizmos.line(k[1], k[2], c.with_alpha(0.6));
    }
    for (i, z) in ed.def.zones.iter().enumerate() {
        let logic = ed.tool == Tool::Logic;
        let c = zone_colour(&z.kind);
        let c = if ed.zone == Some(i) && logic { Color::WHITE } else if logic { c } else { c.with_alpha(0.25) };
        let k = z.corners().map(game_to_bevy);
        for (a, b) in [(0, 1), (1, 3), (3, 2), (2, 0), (4, 5), (5, 7), (7, 6), (6, 4), (0, 4), (1, 5), (2, 6), (3, 7)] { gizmos.line(k[a], k[b], c); }
        if let (true, rc_map::logic::ZoneKind::Camera { eye }) = (logic, &z.kind) {
            gizmos.sphere(Isometry3d::from_translation(game_to_bevy(*eye)), 0.5, c);
            gizmos.line(game_to_bevy(*eye), game_to_bevy(z.centre), c.with_alpha(0.5));
        }
    }
    // Rails: the selected rail's points, where the next point goes.
    if let (Tool::Terrain, TerrainMode::Rails) = (ed.tool, ed.terrain_mode) {
        if let Some(r) = ed.rail.and_then(|k| ed.def.rails.get(k)) {
            gizmos.linestrip(r.line().iter().map(|q| game_to_bevy([q[0], q[1], q[2] + 0.05])), Color::srgb(1.0, 0.85, 0.2));
            if let Some(q) = ed.rail_point.and_then(|i| r.points.get(i)) {
                for (k, dir) in AXES.iter().enumerate() {
                    handles.arrow(game_to_bevy(*q), game_to_bevy([q[0] + dir[0] * POINT_ARROW, q[1] + dir[1] * POINT_ARROW, q[2] + dir[2] * POINT_ARROW]), AXIS_COLORS[k]);
                }
            }
            for (i, q) in r.points.iter().enumerate() {
                let hot = ed.rail_hover == Some(i) || ed.rail_drag.as_ref().is_some_and(|d| d.0 == i) || ed.rail_point == Some(i);
                gizmos.sphere(Isometry3d::from_translation(game_to_bevy(*q)), if hot { 0.4 } else { 0.25 }, if hot { Color::srgb(1.0, 0.9, 0.2) } else { Color::WHITE });
                let g = ground(&ed.def, q[0], q[1], q[2] - 0.3);
                if q[2] - g > 0.3 { gizmos.line(game_to_bevy(*q), game_to_bevy([q[0], q[1], g]), Color::srgba(1.0, 1.0, 1.0, 0.35)); }
            }
        }
        if let Some(t) = ed.rail_at {
            let from = ed.rail_first.or_else(|| ed.rail.filter(|_| ed.rail_drawing).and_then(|k| ed.def.rails.get(k)).and_then(|r| r.points.last().copied()));
            if let Some(f) = from { gizmos.line(game_to_bevy(f), game_to_bevy(t), Color::srgb(1.0, 0.85, 0.2)); }
            gizmos.sphere(Isometry3d::from_translation(game_to_bevy(t)), 0.25, Color::srgb(1.0, 0.85, 0.2));
        }
    }
    // Paths: the selected path's line and points, where the next point goes.
    if let (Tool::Terrain, TerrainMode::Paths) = (ed.tool, ed.terrain_mode) {
        if let Some(w) = ed.walk.and_then(|k| ed.def.walkways.get(k)) {
            let c = w.centre();
            gizmos.linestrip(c.iter().map(|q| game_to_bevy([q[0], q[1], q[2] + 0.08])), Color::srgb(0.4, 0.9, 1.0));
            let (l, r) = w.sides();
            for side in [l, r] { gizmos.linestrip(side.iter().map(|q| game_to_bevy([q[0], q[1], q[2] + 0.08])), Color::srgba(0.4, 0.9, 1.0, 0.5)); }
            for (i, q) in w.points.iter().enumerate() {
                let hot = ed.walk_hover == Some(i) || ed.walk_drag.as_ref().is_some_and(|d| d.0 == i);
                gizmos.sphere(Isometry3d::from_translation(game_to_bevy(*q)), if hot { 0.45 } else { 0.3 }, if hot { Color::srgb(1.0, 0.9, 0.2) } else { Color::WHITE });
                // Down to the ground: how high it is.
                let g = ground(&ed.def, q[0], q[1], q[2] - 0.6);
                if q[2] - g > 0.6 { gizmos.line(game_to_bevy(*q), game_to_bevy([q[0], q[1], g]), Color::srgba(1.0, 1.0, 1.0, 0.35)); }
            }
        }
        if let Some(t) = ed.walk_at {
            let from = ed.walk_first.or_else(|| ed.walk.filter(|_| ed.walk_drawing).and_then(|k| ed.def.walkways.get(k)).and_then(|w| w.points.last().copied()));
            if let Some(f) = from { gizmos.line(game_to_bevy(f), game_to_bevy(t), Color::srgb(0.4, 1.0, 0.5)); }
            gizmos.sphere(Isometry3d::from_translation(game_to_bevy(t)), 0.3, Color::srgb(0.4, 1.0, 0.5));
            let half = ed.walk.and_then(|k| ed.def.walkways.get(k)).map_or(ed.walk_spec.width, |w| w.width) / 2.0;
            gizmos.circle(Isometry3d::new(game_to_bevy(t), flat), half, Color::srgba(0.4, 1.0, 0.5, 0.5));
        }
    }
    // The island the builder would make: its outline at the top, its cliffs down to the bottom, its edge.
    if let (Tool::Terrain, TerrainMode::Build, Some(at)) = (ed.tool, ed.terrain_mode, ed.island_at) {
        let i = &ed.island;
        let top = at[2] + i.height.max(0.25);
        let ring = |z: f32| -> Vec<Vec3> { (0..=72).map(|k| { let a = k as f32 / 72.0 * std::f32::consts::TAU; let r = i.reach(a); game_to_bevy([at[0] + a.cos() * r, at[1] + a.sin() * r, z]) }).collect() };
        let c = Color::srgb(0.4, 1.0, 0.5);
        gizmos.linestrip(ring(top), c);
        if i.depth > 0.0 {
            gizmos.linestrip(ring(top - i.depth), Color::srgba(0.4, 1.0, 0.5, 0.45));
            for k in 0..12 {
                let a = k as f32 / 12.0 * std::f32::consts::TAU;
                let r = i.reach(a);
                let (x, y) = (at[0] + a.cos() * r, at[1] + a.sin() * r);
                gizmos.line(game_to_bevy([x, y, top]), game_to_bevy([x, y, top - i.depth]), Color::srgba(0.4, 1.0, 0.5, 0.45));
            }
        }
        if let Some(e) = &i.edge { gizmos.linestrip(ring(top + e.height), Color::srgb(1.0, 0.85, 0.2)); }
        // Where it stands, and the centre up to its top.
        gizmos.line(game_to_bevy(at), game_to_bevy([at[0], at[1], top]), Color::srgba(1.0, 1.0, 1.0, 0.6));
        gizmos.circle(Isometry3d::new(game_to_bevy([at[0], at[1], at[2] + 0.05]), flat), 0.5, Color::srgba(1.0, 1.0, 1.0, 0.6));
    }
    // The armed asset where a click would put it.
    if let (Some(e), false, Ok(window), Ok(cam)) = (ed.armed, focus.pointer || ed.play_pending, windows.single(), cams.single()) {
        let local = ed.catalog.as_ref().and_then(|c| c.entries.get(e)).and_then(|e| e.bounds).unwrap_or(DEFAULT_BOX);
        if let Some(p) = cursor_ray(window, &frame, cam).and_then(|r| ed.placement(r, local)) {
            draw_box(&mut gizmos, local, p, ed.place_yaw, Color::srgba(0.4, 1.0, 0.5, 0.35));
        }
    }
}

// ------------------------------------------------------------------------------------------------
// The panels.

/// The HUD look: the launcher's navy and orange (docs: the editor styled like the game's own HUD and menus).
fn theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    let navy = egui::Color32::from_rgb(0x1b, 0x27, 0x40);
    let navy2 = egui::Color32::from_rgb(0x22, 0x31, 0x4f);
    let line = egui::Color32::from_rgb(0x2f, 0x42, 0x68);
    let orange = egui::Color32::from_rgb(0xf0, 0x8a, 0x24);
    v.panel_fill = navy;
    v.window_fill = navy;
    v.extreme_bg_color = egui::Color32::from_rgb(0x13, 0x1c, 0x2e);
    v.faint_bg_color = navy2;
    v.selection.bg_fill = orange;
    v.selection.stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(0x1a, 0x12, 0x05));
    v.hyperlink_color = egui::Color32::from_rgb(0xff, 0xb3, 0x47);
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, line);
    v.widgets.inactive.weak_bg_fill = navy2;
    v.widgets.inactive.bg_fill = navy2;
    v.widgets.hovered.weak_bg_fill = line;
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, orange);
    v.widgets.active.weak_bg_fill = orange;
    v.override_text_color = Some(egui::Color32::from_rgb(0xe8, 0xee, 0xf9));
    ctx.set_visuals(v);
    // Rows are clicked as a whole: labels must not take the click for text selection.
    ctx.global_style_mut(|s| s.interaction.selectable_labels = false);
}

/// An entry's thumbnail as an egui texture (loaded on first use).
fn thumb(ed: &mut Editor, contexts: &mut EguiContexts, images: &mut Assets<Image>, rel: &str, budget: &mut u32) -> Option<egui::TextureId> {
    if let Some(t) = ed.thumbs.get(rel) { return *t; }
    if *budget == 0 { return None; }
    *budget -= 1;
    let path = crate::level_load::extracted_root().join("catalog").join(rel);
    let id = std::fs::read(&path).ok().and_then(|b| {
        Image::from_buffer(&b, ImageType::Extension("png"), CompressedImageFormats::NONE, true, ImageSampler::linear(), RenderAssetUsages::RENDER_WORLD).ok()
    }).map(|img| contexts.add_image(EguiTextureHandle::Strong(images.add(img))));
    ed.thumbs.insert(rel.to_string(), id);
    id
}

fn badge(ui: &mut egui::Ui, e: &Entry) {
    let (text, color) = match (e.live, e.behaviour) {
        (Some(Live::Live), _) => ("Live", egui::Color32::from_rgb(0x5f, 0xd4, 0x8a)),
        (Some(Live::NeedsWork), _) => ("Needs work", egui::Color32::from_rgb(0xf2, 0xc9, 0x4c)),
        (_, Some(Behaviour::Ported)) => ("Ported", egui::Color32::from_rgb(0x5f, 0xd4, 0x8a)),
        (_, Some(Behaviour::Static)) => ("Scenery", egui::Color32::from_rgb(0x8f, 0xa4, 0xc8)),
        _ => ("Unported", egui::Color32::from_rgb(0xef, 0x6b, 0x6b)),
    };
    ui.label(egui::RichText::new(text).small().strong().color(color));
    if e.id.kind == AssetKind::Moby && crate::map_load::MAY_VANISH.contains(&e.id.number) {
        ui.label(egui::RichText::new("may vanish on maps").small().color(egui::Color32::from_rgb(0xff, 0xb3, 0x47)))
            .on_hover_text("Seen to disappear, hide or wander off on a custom map: it still needs its home level's data. Phase 4 work in progress.");
    }
}

#[allow(clippy::too_many_arguments)]
fn panels(
    mut contexts: EguiContexts,
    mut ed: ResMut<Editor>,
    mut mode: ResMut<EditorMode>,
    mut focus: ResMut<UiFocus>,
    mut images: ResMut<Assets<Image>>,
    mut source: Option<ResMut<CameraSource>>,
    time: Res<Time>,
    windows: Query<&Window, With<PrimaryWindow>>,
    frame: Res<crate::display::GameFrame>,
    cams: Query<(&Camera, &GlobalTransform), With<FlyCam>>,
    mut fly: Query<&mut FlyCam>,
    previews: Res<Previews>,
    mut themed: Local<bool>,
    mut exit: MessageWriter<AppExit>,
    mut last_pick: Local<(Option<usize>, Option<usize>, Option<usize>, Option<usize>, Option<usize>)>,
    mut loading_since: Local<Option<std::time::Instant>>,
) -> Result {
    let ctx = contexts.ctx_mut()?.clone();
    if !*themed {
        theme(&ctx);
        *themed = true;
    }
    let ed = &mut *ed;
    // Selecting something while the help is up shows its settings instead.
    let picked = (ed.selected, ed.patch, ed.walk, ed.zone, ed.rail);
    if picked != *last_pick {
        if ed.help.is_some() && (picked.0.is_some() || picked.1.is_some() || picked.2.is_some() || picked.3.is_some() || picked.4.is_some()) { ed.help = None; }
        *last_pick = picked;
    }
    if !ed.saved && ed.last_backup.elapsed().as_secs() >= 300 {
        ed.last_backup = std::time::Instant::now();
        ed.backup(&ed.def.to_json(), true);
    }
    let dt = time.delta_secs();
    if dt > 0.0 { ed.fps = if ed.fps == 0.0 { 1.0 / dt } else { ed.fps * 0.95 + 0.05 / dt }; }
    // The main menu over everything (the world dimmed behind it).
    if ed.menu.open && *mode == EditorMode::Editing {
        menu::main_menu(&ctx, ed, &mut contexts, &mut images, &mut exit);
        *focus = UiFocus { editing: true, pointer: true, keyboard: ctx.egui_wants_keyboard_input(), wheel: true, panels: vec![ctx.viewport_rect()] };
        return Ok(());
    }
    // Taking the thumbnail: no panels for those frames either (the capture is the game frame; this keeps the pointer still).
    if ed.thumb_shot.is_some() { return Ok(()); }
    let mut root = egui::Ui::new(ctx.clone(), "rac_maker".into(), egui::UiBuilder::new().layer_id(egui::LayerId::background()).max_rect(ctx.viewport_rect()));
    if ed.coalescing && !ctx.input(|i| i.pointer.any_down()) && ctx.memory(|m| m.focused().is_none()) { ed.coalescing = false; }
    let orange = egui::Color32::from_rgb(0xf0, 0x8a, 0x24);
    let can_edit = editing(ed, *mode);
    // The collision view: built again for each new version of the map (the def's address and size as its key).
    if ed.show_collision {
        let key = ed.def.patches.iter().map(|p| p.heights.len() + p.mask.len() + p.edge.as_ref().map_or(0, |e| e.gaps.len() * 7 + e.height as usize)).sum::<usize>()
            ^ (ed.def.walkways.iter().map(|w| w.points.len() * 31 + (w.width * 64.0) as usize).sum::<usize>() << 1)
            ^ ed.undo.len() << 20
            ^ ed.def.patches.iter().map(|p| (p.origin[0] * 64.0 + p.origin[1] * 7.0 + p.origin[2] * 3.0) as usize + p.surface as usize).sum::<usize>();
        if ed.collision_view.as_ref().is_none_or(|v| v.0 != key) || ed.stroke.is_some() || ed.shape_drag.is_some() || ed.walk_drag.is_some() {
            ed.collision_view = Some((key, collision_faces(&ed.def)));
        }
    }
    let mut rects = Vec::new();
    // The top bar.
    let top = egui::Panel::top("rac_maker_top").show(&mut root, |ui| -> Option<egui::Rect> {
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new(egui::RichText::new("RAC MAKER · Menu").strong().color(orange)).frame(false)).on_hover_text("The main menu: your maps, versions, backups").clicked() {
                ed.menu.reopen();
            }
            ui.separator();
            ui.label(format!("{}{}", ed.def.name, if ed.saved { "" } else { " •" }));
            ui.separator();
            if *mode == EditorMode::Editing {
                ui.add_enabled_ui(can_edit, |ui| {
                    if ui.button("New").on_hover_text("A new map in your maps folder (Documents\\RAC Maker)").clicked() { ed.new_map(); }
                    if ui.button("Save").on_hover_text("Ctrl+S (the old version is kept as a backup)").clicked() { ed.save(); }
                    let mut thumb = !ed.settings.no_thumb_on_save;
                    if ui.checkbox(&mut thumb, egui::RichText::new("+ picture").small())
                        .on_hover_text(if ed.def.thumb_view.is_some() { "Saving also takes the map's picture again, from where you pressed Set thumbnail (with your new edits in it)" } else { "Saving also takes the map's picture again: press Set thumbnail once to choose where from" })
                        .changed()
                    {
                        ed.settings.no_thumb_on_save = !thumb;
                        ed.settings.store();
                    }
                    if ui.add_enabled(!ed.undo.is_empty(), egui::Button::new("Undo")).on_hover_text("Ctrl+Z").clicked() { ed.undo(); }
                    if ui.add_enabled(!ed.redo.is_empty(), egui::Button::new("Redo")).on_hover_text("Ctrl+Y").clicked() { ed.redo(); }
                });
                ui.separator();
            }
            let play = match (*mode, ed.play_pending) {
                (EditorMode::Playing, _) => "■ Stop (F5)",
                (_, true) => "▶ Starting… (F5: cancel)",
                _ => "▶ Play (F5)",
            };
            if ui.add(egui::Button::new(egui::RichText::new(play).strong()).fill(orange)).clicked() {
                toggle_play(ed, &mut mode, source.as_deref_mut());
            }
            if ui.button("Set thumbnail").on_hover_text("The main menu's picture of this map: what the camera sees now (fly it to a nice view first)").clicked() {
                ed.thumb_shot = Some((menu::thumb_path(&ed.path), 2));
                ed.thumb_record = true;
            }
            if ui.selectable_label(ed.show_collision, "Collision (F6)").on_hover_text("Shows what Ratchet stands on and bumps into near the camera, also while playing: green ground, blue slippery, orange hot, red deadly, purple magnetic, grey cliffs, yellow edges").clicked() {
                ed.show_collision = !ed.show_collision;
                ed.collision_view = None;
            }
            // The tools, as icons after a divider: Select (Esc), Place, the terrain tools.
            if *mode == EditorMode::Editing {
                ui.separator();
                ui.add_enabled_ui(can_edit, |ui| tool_icons(ui, ed));
            }
            if !matches!(ed.reload, Reload::Idle) {
                ui.separator();
                ui.spinner();
                ui.label("loading…");
            }
            // A narrow window keeps Help (after the rest when there is no room on the right) and drops the rest.
            let room = ui.available_width();
            let mut radio_rect: Option<egui::Rect> = None;
            let layout = if room > 70.0 { egui::Layout::right_to_left(egui::Align::Center) } else { egui::Layout::left_to_right(egui::Align::Center) };
            if room <= 70.0 { ui.separator(); }
            ui.with_layout(layout, |ui| {
                if room > 300.0 {
                    ui.label(egui::RichText::new(format!("{:.0} fps", ed.fps)).weak());
                    ui.separator();
                }
                let help = ed.help.is_some();
                if ui.selectable_label(help, egui::RichText::new("Help").strong()).clicked() {
                    ed.help = if help { None } else { Some(0) };
                    if help && !ed.settings.help_seen {
                        ed.settings.help_seen = true;
                        ed.settings.store();
                    }
                }
                if let Some(mut fly) = fly.single_mut().ok().filter(|_| room > 230.0) {
                    ui.separator();
                    if ui.small_button("⟲").on_hover_text("Reset the camera speed").clicked() { fly.speed = 16.0; }
                    ui.label(format!("Camera speed {:.0}", fly.speed)).on_hover_text("The mouse wheel over the world changes it; Shift flies 5× faster");
                }
                // The views (right to left here): look at the selection from above, the front, ...
                if room > 520.0 && *mode == EditorMode::Editing {
                    ui.separator();
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for (name, hint) in [
                        ("3/4", "From above at an angle"),
                        ("Right", "From the right (east, looking west)"),
                        ("Left", "From the left (west, looking east)"),
                        ("Back", "From the back (north, looking south)"),
                        ("Front", "From the front (south, looking north)"),
                        ("Top", "Look straight down"),
                    ] {
                        if ui.add(egui::Button::new(egui::RichText::new(name).small())).on_hover_text(format!("{hint}, at the selected object (or what the camera faces)")).clicked() {
                            ed.view_snap = Some(name);
                        }
                    }
                    ui.label(egui::RichText::new("View").weak().small());
                    ui.spacing_mut().item_spacing.x = 8.0;
                }
                // The radio: the song playing and the button that opens the song list.
                if room > 700.0 && *mode == EditorMode::Editing {
                    ui.separator();
                    let root = crate::level_load::extracted_root();
                    let songs = crate::map_load::songs(&root);
                    let playing = ed.radio.and_then(|k| songs.get(k)).map(|s| s.2.clone());
                    if playing.is_some() && ui.small_button("■").on_hover_text("Stop the radio").clicked() { ed.radio_want = Some(None); }
                    let label = playing.map_or("Radio: off".to_string(), |n| format!("Radio: {n}"));
                    let r = ui.selectable_label(ed.radio_menu, egui::RichText::new(label).color(if ed.radio.is_some() { orange } else { egui::Color32::from_gray(220) }));
                    if r.clicked() { ed.radio_menu = !ed.radio_menu; }
                    radio_rect = Some(r.rect);
                }
            });
            radio_rect
        }).inner
    });
    rects.push(top.response.rect);
    if let (true, Some(at)) = (ed.radio_menu, top.inner) {
        let root = crate::level_load::extracted_root();
        let songs = crate::map_load::songs(&root);
        let list = egui::Area::new(egui::Id::new("rac_maker_radio")).fixed_pos(egui::pos2(at.left(), at.bottom() + 4.0)).order(egui::Order::Foreground).show(&ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(260.0);
                if ui.button("Stop the radio").clicked() { ed.radio_want = Some(None); ed.radio_menu = false; }
                egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                    let mut planet = u32::MAX;
                    for (k, (l, _, name)) in songs.iter().enumerate() {
                        if *l != planet {
                            planet = *l;
                            ui.label(egui::RichText::new(LEVEL_NAMES[*l as usize]).strong().color(orange));
                        }
                        let short = name.split(": ").nth(1).unwrap_or(name);
                        if ui.selectable_label(ed.radio == Some(k), format!("   {short}")).clicked() {
                            ed.radio_want = Some(Some(k));
                            ed.radio_menu = false;
                        }
                    }
                });
            });
        });
        rects.push(list.response.rect);
        // A click anywhere else closes it.
        if ctx.input(|i| i.pointer.any_pressed()) && ctx.pointer_latest_pos().is_some_and(|p| !list.response.rect.contains(p) && !at.contains(p)) { ed.radio_menu = false; }
    }
    // A load running: a card with a spinner, a bar and the stage, under the top bar in the middle (the world stays
    // usable; the edits made meanwhile load next).
    // Fast reloads (most edits) show only the top bar's spinner: the card comes after a second and a half.
    let since = (!matches!(ed.reload, Reload::Idle)).then(|| *loading_since.get_or_insert_with(std::time::Instant::now));
    if !since.is_some_and(|t| t.elapsed().as_secs_f32() < 1.5) && since.is_some() {
        let since = since.unwrap();
        let step = crate::level_load::LOAD_STEP.load(std::sync::atomic::Ordering::Relaxed).min(crate::level_load::LOAD_STAGES.len());
        let n = crate::level_load::LOAD_STAGES.len();
        let at = egui::pos2(ctx.viewport_rect().center().x - 160.0, top.response.rect.bottom() + 54.0);
        egui::Area::new(egui::Id::new("rac_maker_loading")).fixed_pos(at).order(egui::Order::Foreground).interactable(false).show(&ctx, |ui| {
            egui::Frame::popup(ui.style()).inner_margin(12.0).show(ui, |ui| {
                ui.set_width(320.0);
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(22.0).color(orange));
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("Loading the level").strong());
                        let what = if step < n { crate::level_load::LOAD_STAGES[step] } else { "putting it in place" };
                        ui.label(egui::RichText::new(format!("{what} · {:.0} s", since.elapsed().as_secs_f32())).weak().small());
                    });
                });
                ui.add_space(4.0);
                ui.add(egui::ProgressBar::new(((step as f32 + 0.5) / (n as f32 + 1.0)).min(1.0)).desired_height(8.0).fill(orange).animate(true));
            });
        });
        ctx.request_repaint();
    } else if since.is_none() {
        *loading_since = None;
    }
    rects.push(egui::Panel::bottom("rac_maker_status").show(&mut root, |ui| { ui.label(&ed.status); }).response.rect);
    if *mode == EditorMode::Editing {
        let left = egui::Panel::left("rac_maker_assets").default_size(300.0).show(&mut root, |ui| {
            ui.add_enabled_ui(can_edit, |ui| {
                ui.horizontal(|ui| {
                    for (t, label) in [(Tool::Objects, "Objects"), (Tool::Terrain, "Terrain"), (Tool::Logic, "Logic"), (Tool::World, "World"), (Tool::Maps, "Maps")] {
                        if ui.selectable_label(ed.tool == t, egui::RichText::new(label).heading()).clicked() && ed.tool != t {
                            ed.tool = t;
                            ed.armed = None;
                            ed.selected = None;
                        }
                    }
                });
                ui.separator();
                match ed.tool {
                    Tool::Objects => {
                        placed_list(ui, ed);
                        ui.separator();
                        asset_browser(ui, ed, &mut contexts, &mut images);
                    }
                    Tool::Terrain => terrain_panel(ui, ed, cams.single().ok(), &mut contexts, &mut images),
                    Tool::Maps => maps_panel(ui, ed),
                    Tool::Logic => logic_panel(ui, ed),
                    Tool::World => world_panel(ui, ed, &mut contexts, &mut images),
                }
            });
        });
        let right = egui::Panel::right("rac_maker_inspector").default_size(if ed.help.is_some() { 560.0 } else { 280.0 }).show(&mut root, |ui| {
            if let Some(t) = ed.help {
                help_panel(ui, ed, t);
                return;
            }
            // What is selected: an object, else an island, else a path (its settings are on the left in Paths), else the map.
            ui.add_enabled_ui(can_edit, |ui| {
                let (k, w) = (ed.patch.filter(|&k| k < ed.def.patches.len()), ed.walk.filter(|&k| k < ed.def.walkways.len()));
                let paths = ed.tool == Tool::Terrain && ed.terrain_mode == TerrainMode::Paths;
                let zone = ed.zone.filter(|&z| ed.tool == Tool::Logic && z < ed.def.zones.len());
                match (ed.selected, k, w) {
                    (None, _, _) if zone.is_some() => zone_inspector(ui, ed, zone.unwrap(), cams.single().ok().map(|c| c.1)),
                    (None, Some(k), _) => patch_inspector(ui, ed, k, &mut contexts, &mut images),
                    (None, None, Some(_)) if !paths => {
                        egui::ScrollArea::vertical().id_salt("walk_inspector").show(ui, |ui| walk_settings(ui, ed, &mut contexts, &mut images));
                    }
                    _ => inspector(ui, ed, &mut contexts, &mut images, cams.single().ok().map(|c| c.1)),
                }
            });
        });
        rects.extend([left.response.rect, right.response.rect]);
        // The objects not loaded yet: their thumbnail where they stand, until the reload draws them.
        if let (Ok(window), Ok(cam)) = (windows.single(), cams.single()) {
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Background, egui::Id::new("rac_maker_ghosts")));
            let cam_at = to_game(cam.1.translation());
            for i in 0..ed.def.objects.len() {
                if ed.loaded.ids.contains(&ed.ids[i]) || previews.models.get(&ed.def.objects[i].asset).is_some_and(|m| m.is_some()) { continue; }
                let o = ed.def.objects[i].clone();
                let b = ed.local_box(&o.asset);
                let mid = [o.position[0], o.position[1], o.position[2] + (b[2] + b[5]) / 2.0];
                let Some(at) = to_window(window, &frame, cam, mid) else { continue };
                let Some(rel) = ed.entry(&o.asset).map(|e| e.thumb.clone()) else { continue };
                let Some(tex) = thumb(ed, &mut contexts, &mut images, &rel, &mut 2) else { continue };
                let dist = ((mid[0] - cam_at[0]).powi(2) + (mid[1] - cam_at[1]).powi(2) + (mid[2] - cam_at[2]).powi(2)).sqrt().max(1.0);
                let size = (1200.0 * (b[5] - b[2]).max(0.5) / dist).clamp(24.0, 160.0);
                let rect = egui::Rect::from_center_size(egui::pos2(at.x, at.y), egui::vec2(size, size));
                painter.image(tex, rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::from_white_alpha(200));
            }
        }
    }
    let over_panels = ctx.pointer_latest_pos().is_some_and(|p| rects.iter().any(|r| r.contains(p)));
    *focus = UiFocus {
        editing: can_edit,
        pointer: over_panels || ctx.is_pointer_over_egui() || ctx.egui_wants_pointer_input(),
        keyboard: ctx.egui_wants_keyboard_input(),
        wheel: ed.tool == Tool::Terrain
            && ((ed.terrain_mode == TerrainMode::Shape && (ed.shape_hover.is_some() || ed.shape_drag.is_some()))
                || (ed.terrain_mode == TerrainMode::Paths && (ed.walk_hover.is_some() || ed.walk_drag.is_some()))
                || (ed.terrain_mode == TerrainMode::Rails && (ed.rail_hover.is_some() || ed.rail_drag.is_some()))),
        panels: rects,
    };
    Ok(())
}

/// The help panel: the topics as tabs down the left (as the game's own help pages), the topic's lines on the right.
fn help_panel(ui: &mut egui::Ui, ed: &mut Editor, topic: usize) {
    let orange = egui::Color32::from_rgb(0xf0, 0x8a, 0x24);
    ui.horizontal(|ui| {
        ui.heading(egui::RichText::new("HELP").strong().color(orange));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Close").clicked() {
                ed.help = None;
                ed.settings.help_seen = true;
                ed.settings.store();
            }
            if ui.button("Open the full guide").on_hover_text("In your browser, with every feature and key").clicked() {
                ed.status = match open_guide() { Ok(()) => "The guide opened in your browser".into(), Err(e) => format!("Could not open the guide: {e}") };
            }
        });
    });
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut ed.help_query).hint_text("Search the help…").desired_width(ui.available_width() - 30.0));
        if !ed.help_query.is_empty() && ui.small_button("x").on_hover_text("Clear the search").clicked() { ed.help_query.clear(); }
    });
    ui.separator();
    // A search: every line (of any topic) holding all its words, under its topic's name.
    let words: Vec<String> = ed.help_query.to_lowercase().split_whitespace().map(str::to_string).collect();
    let hits = |line: &str| { let l = line.to_lowercase(); words.iter().all(|w| l.contains(w.as_str())) };
    let bullet = |ui: &mut egui::Ui, line: &str| {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("•").color(orange));
            ui.label(highlighted(ui, line, &words));
        });
        ui.add_space(2.0);
    };
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(150.0);
            for (k, (title, lines)) in HELP.iter().enumerate() {
                let found = !words.is_empty() && (hits(title) || lines.iter().any(|l| hits(l)));
                let text = egui::RichText::new(*title).strong();
                let text = if !words.is_empty() && !found { text.weak() } else { text };
                if ui.selectable_label(k == topic && words.is_empty(), text).clicked() {
                    ed.help = Some(k);
                    ed.help_query.clear();
                }
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().id_salt("help_text").show(ui, |ui| {
            ui.vertical(|ui| {
                if words.is_empty() {
                    let (title, lines) = HELP[topic.min(HELP.len() - 1)];
                    ui.label(egui::RichText::new(title).heading().color(orange));
                    ui.add_space(4.0);
                    for line in lines { bullet(ui, line); }
                    return;
                }
                let mut any = false;
                for (k, (title, lines)) in HELP.iter().enumerate() {
                    let found: Vec<&str> = lines.iter().copied().filter(|l| hits(title) || hits(l)).collect();
                    if found.is_empty() { continue; }
                    any = true;
                    if ui.add(egui::Label::new(egui::RichText::new(*title).heading().color(orange)).sense(egui::Sense::click())).on_hover_text("Open this topic").clicked() {
                        ed.help = Some(k);
                        ed.help_query.clear();
                    }
                    for line in found { bullet(ui, line); }
                    ui.add_space(6.0);
                }
                if !any { ui.label(egui::RichText::new("Nothing found. Try another word, or Open the full guide.").weak()); }
            });
        });
    });
}

/// `line` with every piece of it that one of `words` (lower case) matches marked (the help's search).
fn highlighted(ui: &egui::Ui, line: &str, words: &[String]) -> egui::text::LayoutJob {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let plain = egui::TextFormat { font_id: font.clone(), color: ui.visuals().text_color(), ..Default::default() };
    let marked = egui::TextFormat { font_id: font, color: egui::Color32::BLACK, background: egui::Color32::from_rgb(0xf0, 0xb0, 0x40), ..Default::default() };
    // Which bytes are matched (case-insensitive; the lower-case text keeps the byte offsets of these ASCII words).
    let lower = line.to_lowercase();
    let mut hit = vec![false; line.len()];
    if lower.len() == line.len() {
        for w in words.iter().filter(|w| !w.is_empty()) {
            let mut from = 0;
            while let Some(k) = lower[from..].find(w.as_str()) {
                let at = from + k;
                for h in &mut hit[at..at + w.len()] { *h = true; }
                from = at + w.len();
            }
        }
    }
    let mut job = egui::text::LayoutJob::default();
    let mut start = 0;
    for i in 1..=line.len() {
        if i == line.len() || (line.is_char_boundary(i) && hit[i] != hit[start]) {
            if !line.is_char_boundary(i) { continue; }
            job.append(&line[start..i], 0.0, if hit[start] { marked.clone() } else { plain.clone() });
            start = i;
        }
    }
    job.wrap.max_width = ui.available_width();
    job
}

/// The Maps tab: the map's file, new, recent maps, the maps folder, the backups.
fn maps_panel(ui: &mut egui::Ui, ed: &mut Editor) {
    if !ed.warnings.is_empty() {
        ui.label(egui::RichText::new("Found at the last save").strong().color(egui::Color32::from_rgb(0xff, 0xb3, 0x47)));
        for w in &ed.warnings { ui.label(format!("• {w}")); }
        ui.separator();
    }
    ui.label(egui::RichText::new("This map").strong());
    ui.label(egui::RichText::new(ed.path.display().to_string()).small());
    ui.horizontal(|ui| {
        if ui.button("Save").clicked() { ed.save(); }
        if ui.button("New map").clicked() { ed.new_map(); }
        if ui.button("Open maps folder").clicked() {
            let _ = std::fs::create_dir_all(maps_dir());
            let _ = open_in_shell(&maps_dir());
        }
    });
    ui.separator();
    let mut open = None;
    egui::ScrollArea::vertical().id_salt("maps_lists").show(ui, |ui| {
        ui.label(egui::RichText::new("Recent").strong());
        for r in ed.settings.recent.clone() {
            if r == ed.path { continue; }
            let name = r.file_stem().map_or("?".into(), |s| s.to_string_lossy().into_owned());
            if ui.selectable_label(false, name).on_hover_text(r.display().to_string()).clicked() { open = Some(r); }
        }
        ui.add_space(6.0);
        ui.label(egui::RichText::new("In your maps folder").strong());
        let mut files: Vec<PathBuf> = std::fs::read_dir(maps_dir())
            .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect())
            .unwrap_or_default();
        files.sort();
        if files.is_empty() { ui.label(egui::RichText::new("None yet: New or Save puts maps here.").weak()); }
        for f in files {
            let name = f.file_stem().map_or("?".into(), |s| s.to_string_lossy().into_owned());
            if ui.selectable_label(f == ed.path, name).clicked() && f != ed.path { open = Some(f); }
        }
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Backups of this map").strong());
        let list = backups(&ed.path);
        if list.is_empty() { ui.label(egui::RichText::new("None yet: every save keeps one.").weak()); }
        for (file, label) in list.into_iter().take(30) {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(label).small());
                if ui.small_button("Restore").on_hover_text("Puts this version back (unsaved; Undo takes it back)").clicked() {
                    match std::fs::read_to_string(&file).map_err(|e| e.to_string()).and_then(|t| MapDef::parse(&t).map_err(|e| e.to_string())) {
                        Ok(def) => {
                            ed.checkpoint();
                            ed.ids = (0..def.objects.len() as u64).map(|i| i + ed.next_id).collect();
                            ed.next_id += def.objects.len() as u64;
                            ed.def = def;
                            (ed.selected, ed.patch) = (None, None);
                            ed.status = "Backup restored (Save keeps it)".into();
                            ed.changed();
                        }
                        Err(e) => ed.status = format!("That backup does not read: {e}"),
                    }
                }
            });
        }
    });
    if let Some(p) = open { ed.open(p); }
}

/// A grid of the catalog's terrain textures of the planet chosen in Objects > Planet (else the map's host): the one
/// clicked, or None. `current` is outlined.
fn texture_grid(ui: &mut egui::Ui, ed: &mut Editor, contexts: &mut EguiContexts, images: &mut Assets<Image>, current: Option<AssetId>, salt: &str) -> Option<AssetId> {
    let level = ed.planet.unwrap_or(ed.def.host_level);
    ui.label(egui::RichText::new(format!("{}'s textures (planet: Objects > Planet)", LEVEL_NAMES[level as usize])).weak().small());
    let cat = ed.catalog.clone()?;
    let textures: Vec<&Entry> = cat.entries.iter().filter(|e| e.id.kind == AssetKind::TfragTexture && !e.thumb.is_empty() && e.ids().any(|i| i.level == level)).collect();
    let mut budget = 24;
    let mut picked = None;
    egui::ScrollArea::vertical().id_salt((salt.to_string(), level)).max_height(220.0).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for e in textures {
                let id = e.ids().find(|i| i.level == level).copied().unwrap_or(e.id);
                let Some(tex) = thumb(ed, contexts, images, &e.thumb, &mut budget) else { continue };
                let on = current.is_some_and(|t| e.ids().any(|i| *i == t));
                let r = ui.add(egui::Button::image(egui::Image::new((tex, egui::vec2(40.0, 40.0)))).selected(on));
                if r.on_hover_text(id.to_string()).clicked() { picked = Some(id); }
            }
        });
    });
    picked
}

/// The Paths mode's panel: how to draw, the settings of the selected path (else of the next new one), the paths.
fn paths_panel(ui: &mut egui::Ui, ed: &mut Editor, contexts: &mut EguiContexts, images: &mut Assets<Image>) {
    ui.label(egui::RichText::new("Click where a path starts, then where it bends; Enter when done. Paths run on the ground or through the air: from island to island they are bridges.").weak());
    ui.label(egui::RichText::new("Drag a point to move it, the mouse wheel over it raises or lowers it, Delete over it takes it out.").weak().small());
    ui.add(egui::Slider::new(&mut ed.walk_lift, 0.0..=20.0).text("points above the ground")).on_hover_text("New points go this high above what you click on");
    if ed.walk_drawing || ed.walk_first.is_some() {
        if ui.button("Done (Enter)").clicked() { ed.walk_drawing = false; ed.walk_first = None; }
    }
    ui.separator();
    walk_settings(ui, ed, contexts, images);
    ui.separator();
    ui.label(egui::RichText::new(format!("Paths on this map ({})", ed.def.walkways.len())).strong());
    let mut remove = None;
    for j in 0..ed.def.walkways.len() {
        let w = &ed.def.walkways[j];
        let label = format!("Path {}: {} points, {:.0} wide", j + 1, w.points.len(), w.width);
        ui.horizontal(|ui| {
            if ui.selectable_label(ed.walk == Some(j), label).clicked() { ed.walk = Some(j); ed.walk_drawing = false; }
            if ui.small_button("x").on_hover_text("Delete this path").clicked() { remove = Some(j); }
        });
    }
    if let Some(j) = remove {
        ed.checkpoint();
        ed.def.walkways.remove(j);
        ed.walk = None;
        ed.walk_drawing = false;
        ed.status = format!("Path {} deleted (Ctrl+Z brings it back)", j + 1);
        ed.changed();
    }
}

/// The selected path's settings (else those the next new one gets): width, thickness, surface, texture, edge.
fn walk_settings(ui: &mut egui::Ui, ed: &mut Editor, contexts: &mut EguiContexts, images: &mut Assets<Image>) {
    let k = ed.walk.filter(|&k| k < ed.def.walkways.len());
    ui.label(egui::RichText::new(match k { Some(k) => format!("Path {}", k + 1), None => "The next new path".into() }).strong());
    let mut w = match k { Some(k) => ed.def.walkways[k].clone(), None => ed.walk_spec.clone() };
    let before = w.clone();
    ui.add(egui::Slider::new(&mut w.width, 0.5..=20.0).text("width"));
    ui.add(egui::Slider::new(&mut w.depth, 0.0..=20.0).text("thickness")).on_hover_text("Sides down from its top, closed underneath (0: a flat sheet)");
    ui.checkbox(&mut w.cut, "Stop where it meets land").on_hover_text("The path is only built where it is off the islands: where the ground comes up to it, it ends (a bridge from coast to coast, no deck or railings poking through the land)");
    ui.horizontal_wrapped(|ui| {
        ui.label("Surface");
        for (b, name, hint) in rc_map::floor::SURFACES { ui.selectable_value(&mut w.surface, b, name).on_hover_text(hint); }
    });
    egui::CollapsingHeader::new("Texture").id_salt("walk_texture").default_open(true).show(ui, |ui| {
        if ui.selectable_label(w.texture.is_none(), "Plain").clicked() { w.texture = None; }
        if let Some(t) = texture_grid(ui, ed, contexts, images, w.texture, "walk_textures") { w.texture = Some(t); }
        ui.add(egui::Slider::new(&mut w.tile, 1.0..=16.0).text("texture size"));
    });
    ui.label(egui::RichText::new("Edge (both sides)").strong());
    edge_picker(ui, ed, contexts, images, &mut w.edge, "walk_edge");
    if k.is_some() {
        let tex = w.texture;
        colour_section(ui, ed, &mut w.tint, tex, "path");
    }
    w.width = (w.width * 64.0).round() / 64.0;
    w.depth = (w.depth * 64.0).round() / 64.0;
    if w != before {
        match k {
            Some(k) if w.check().is_ok() => {
                if !ed.coalescing { ed.checkpoint(); }
                ed.coalescing = true;
                ed.def.walkways[k] = w;
                open_for_path(ed, k);
                crate::map_load::set(ed.def.clone());
                ed.changed();
            }
            Some(_) => {}
            None => ed.walk_spec = w,
        }
    }
    if let Some(k) = k {
        if ui.button("Delete path").clicked() {
            ed.checkpoint();
            ed.def.walkways.remove(k);
            ed.walk = None;
            ed.walk_drawing = false;
            ed.status = format!("Path {} deleted (Ctrl+Z brings it back)", k + 1);
            ed.changed();
        }
    }
}

/// The plants and details of the planet chosen in Objects > Planet (else the host's) as a grid of their pictures: the
/// one clicked.
fn scatter_grid(ui: &mut egui::Ui, ed: &mut Editor, contexts: &mut EguiContexts, images: &mut Assets<Image>) -> Option<AssetId> {
    let level = ed.planet.unwrap_or(ed.def.host_level);
    ui.label(egui::RichText::new(format!("{}'s plants, rocks and details (planet: Objects > Planet)", LEVEL_NAMES[level as usize])).weak().small());
    let cat = ed.catalog.clone()?;
    let shrubs: Vec<&Entry> = cat.entries.iter().filter(|e| e.id.kind == AssetKind::Shrub && !e.thumb.is_empty() && e.ids().any(|i| i.level == level)).collect();
    let mut budget = 24;
    let mut picked = None;
    egui::ScrollArea::vertical().id_salt(("scatter", level)).max_height(220.0).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for e in shrubs {
                let id = e.ids().find(|i| i.level == level).copied().unwrap_or(e.id);
                let Some(tex) = thumb(ed, contexts, images, &e.thumb, &mut budget) else { continue };
                let on = ed.scatter.is_some_and(|t| e.ids().any(|i| *i == t));
                let r = ui.add(egui::Button::image(egui::Image::new((tex, egui::vec2(40.0, 40.0)))).selected(on));
                if r.on_hover_text(e.name.clone()).clicked() { picked = Some(id); }
            }
        });
    });
    picked
}

/// A Build setting carried to the island just built while it is selected (one undo step for a run of changes).
fn adjust_last(ed: &mut Editor, f: impl FnOnce(&mut rc_map::terrain::Patch)) {
    let Some(k) = ed.last_built.filter(|&k| ed.patch == Some(k) && k < ed.def.patches.len()) else { return };
    let mut p = ed.def.patches[k].clone();
    f(&mut p);
    if p == ed.def.patches[k] || p.check().is_err() { return; }
    if !ed.coalescing { ed.checkpoint(); }
    ed.coalescing = true;
    ed.def.patches[k] = p;
    crate::map_load::set(ed.def.clone());
    ed.changed();
}

/// The edge choice: none, an invisible wall, a railing or a wall: the game's own ([`EDGE_PIECES`], shown as their
/// models) or a plain textured wall of any height; then how high it blocks Ratchet. Its openings stay.
fn edge_picker(ui: &mut egui::Ui, ed: &mut Editor, contexts: &mut EguiContexts, images: &mut Assets<Image>, edge: &mut Option<rc_map::terrain::Edge>, salt: &str) {
    use rc_map::terrain::{Edge, EdgeKind};
    let gaps = edge.as_ref().map(|e| e.gaps.clone()).unwrap_or_default();
    // The openings stay; a piece's stretch and inset come from its model.
    let with_gaps = |kind: EdgeKind, height: f32| {
        let mut e = Edge { kind, height, gaps: gaps.clone(), step: 0.0, inset: 0.0 };
        crate::map_load::fit_edge(&mut e);
        Some(e)
    };
    ui.horizontal(|ui| {
        if ui.selectable_label(edge.is_none(), "None").on_hover_text("Nothing: Ratchet can walk off").clicked() { *edge = None; }
        let invisible = edge.as_ref().is_some_and(|e| e.kind == EdgeKind::Invisible);
        if ui.selectable_label(invisible, "Invisible wall").on_hover_text("Nothing drawn, but Ratchet cannot walk off").clicked() {
            *edge = with_gaps(EdgeKind::Invisible, edge.as_ref().map_or(1.5, |e| e.height.min(4.0)));
        }
    });
    ui.horizontal(|ui| {
        ui.selectable_value(&mut ed.edge_walls, false, egui::RichText::new("Railings").strong());
        ui.selectable_value(&mut ed.edge_walls, true, egui::RichText::new("Walls").strong());
    });
    let mut budget = 16;
    ui.push_id(salt, |ui| {
        ui.horizontal_wrapped(|ui| {
            if ed.edge_walls {
                let on = edge.as_ref().is_some_and(|e| matches!(e.kind, EdgeKind::Wall { .. }));
                if ui.add(egui::Button::new("Textured\nwall").min_size(egui::vec2(52.0, 52.0)).selected(on)).on_hover_text("A plain wall of any height with a texture of your choice").clicked() && !on {
                    *edge = with_gaps(EdgeKind::Wall { texture: None, tile: 4.0 }, 4.0);
                    ed.status = "Edge: a textured wall (pick its texture below)".into();
                }
            }
            for (key, name, wall) in EDGE_PIECES {
                if wall != ed.edge_walls { continue; }
                let Ok(id) = AssetId::parse(key) else { continue };
                let rel = ed.entry(&id).map(|e| e.thumb.clone()).unwrap_or_default();
                let on = edge.as_ref().is_some_and(|e| e.piece() == Some(id));
                let tex = (!rel.is_empty()).then(|| thumb(ed, contexts, images, &rel, &mut budget)).flatten();
                let r = match tex {
                    Some(t) => ui.add(egui::Button::image(egui::Image::new((t, egui::vec2(52.0, 52.0)))).selected(on)),
                    None => ui.add(egui::Button::new(name).selected(on)),
                };
                if r.on_hover_text(name).clicked() {
                    // It blocks as high as it stands (a little more for low ones).
                    let height = crate::map_load::piece_model(id).map_or(1.2, |m| m.height().clamp(0.9, 30.0));
                    *edge = with_gaps(EdgeKind::Piece(id), height);
                    ed.status = format!("Edge: {name}");
                }
            }
        });
        let Some(e) = edge.as_mut() else { return };
        let name = match &e.kind {
            EdgeKind::Piece(id) => piece_name(*id),
            EdgeKind::Wall { .. } => "Textured wall".into(),
            EdgeKind::Invisible => "Invisible wall".into(),
        };
        ui.label(egui::RichText::new(name).strong());
        if let EdgeKind::Wall { texture, tile } = &mut e.kind {
            ui.add(egui::Slider::new(&mut e.height, 0.5..=30.0).text("wall height"));
            ui.add(egui::Slider::new(tile, 1.0..=16.0).text("texture size"));
            let current = *texture;
            egui::CollapsingHeader::new("Wall texture").id_salt("wall_texture").default_open(texture.is_none()).show(ui, |ui| {
                if let Some(t) = texture_grid(ui, ed, contexts, images, current, "wall_textures") { *texture = Some(t); }
            });
        } else {
            ui.add(egui::Slider::new(&mut e.height, 0.3..=30.0).logarithmic(true).text("blocks up to")).on_hover_text("How high Ratchet cannot get past it (game units)");
        }
        e.height = (e.height * 64.0).round() / 64.0;
        if !e.gaps.is_empty() {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("{} openings", e.gaps.len())).weak());
                if ui.small_button("Close all").on_hover_text("The edge all the way round again").clicked() { e.gaps.clear(); }
            });
        }
    });
}

/// A path-following object's route (crate::map_load::PATH_CLASSES): its points (the first is where it stands; drag
/// the points in the world), its travel time, ready-made routes.
fn path_settings(ui: &mut egui::Ui, ed: &mut Editor, i: usize) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new("Path").strong());
    let mut o = ed.def.objects[i].clone();
    let before = o.clone();
    let mover = o.asset.number == 726;
    if o.asset.number == 1454 {
        ui.label(egui::RichText::new("It drives from point to point in a random order. Without a path of its own it keeps the one it came with.").weak());
    }
    if o.path.len() < 2 {
        ui.label(egui::RichText::new(if mover { "It moves along a path: give it one." } else if o.asset.number == 1454 { "Give it a path to choose where it drives." } else { "It patrols a path: give it one." }).weak());
        ui.horizontal_wrapped(|ui| {
            let p = o.position;
            if mover && ui.button("Lift: 8 up").clicked() { o.path = vec![p, [p[0], p[1], p[2] + 8.0]]; }
            if ui.button("8 ahead").clicked() { o.path = vec![p, [p[0] + o.yaw.cos() * 8.0, p[1] + o.yaw.sin() * 8.0, p[2]]]; }
            if !mover && ui.button("Square patrol").clicked() {
                o.path = vec![p, [p[0] + 6.0, p[1], p[2]], [p[0] + 6.0, p[1] + 6.0, p[2]], [p[0], p[1] + 6.0, p[2]], p];
            }
        });
    } else {
        ui.label(egui::RichText::new("Green: start, red: end. Drag the points in the world.").weak());
        let mut remove = None;
        egui::Grid::new("path").num_columns(5).show(ui, |ui| {
            let n = o.path.len();
            for (k, q) in o.path.iter_mut().enumerate() {
                ui.label(if k == 0 { "start".to_string() } else if k + 1 == n { "end".to_string() } else { format!("{k}") });
                for v in q.iter_mut() { ui.add(egui::DragValue::new(v).speed(0.1).range(0.0..=1023.0)); }
                if k > 0 && n > 2 && ui.small_button("x").clicked() { remove = Some(k); }
                ui.end_row();
            }
        });
        if let Some(k) = remove { o.path.remove(k); }
        ui.horizontal(|ui| {
            if ui.button("Add a point").clicked() {
                let l = *o.path.last().unwrap_or(&o.position);
                o.path.push([l[0] + 4.0, l[1], l[2]]);
            }
            if ui.button("Remove the path").clicked() { o.path.clear(); }
        });
        if mover {
            let mut t = o.time.unwrap_or(4.0);
            if ui.add(egui::Slider::new(&mut t, 0.5..=30.0).text("seconds end to end")).changed() { o.time = Some(t); }
        }
        // The first point is where it stands.
        if let Some(&q) = o.path.first() { o.position = q; }
    }
    if o != before {
        if !ed.coalescing { ed.checkpoint(); }
        ed.coalescing = true;
        ed.def.objects[i] = o;
        ed.changed();
    }
}

/// The Terrain tab: new patches, the brush, the patches.
fn terrain_panel(ui: &mut egui::Ui, ed: &mut Editor, _cam: Option<(&Camera, &GlobalTransform)>, contexts: &mut EguiContexts, images: &mut Assets<Image>) {
    ui.horizontal(|ui| {
        ui.selectable_value(&mut ed.terrain_mode, TerrainMode::Select, egui::RichText::new("Select").strong()).on_hover_text("Click an island, a path or an object to select it (Esc)");
        ui.selectable_value(&mut ed.terrain_mode, TerrainMode::Build, egui::RichText::new("Build islands").strong()).on_hover_text("Click in the world to put land there");
        ui.selectable_value(&mut ed.terrain_mode, TerrainMode::Shape, egui::RichText::new("Shape").strong()).on_hover_text("Drag an island's coast out or in, move it, lift it");
        ui.selectable_value(&mut ed.terrain_mode, TerrainMode::Openings, egui::RichText::new("Openings").strong()).on_hover_text("Take stretches out of an island's edge (gaps in the fence)");
        ui.selectable_value(&mut ed.terrain_mode, TerrainMode::Sculpt, egui::RichText::new("Sculpt").strong()).on_hover_text("Hills, valleys and paint with brushes");
        ui.selectable_value(&mut ed.terrain_mode, TerrainMode::Paths, egui::RichText::new("Paths").strong()).on_hover_text("Walkways and bridges: click their points");
        ui.selectable_value(&mut ed.terrain_mode, TerrainMode::Rails, egui::RichText::new("Rails").strong()).on_hover_text("Grind rails: click their points");
    });
    ui.separator();
    if ed.terrain_mode == TerrainMode::Build {
        let i = &mut ed.island;
        ui.horizontal(|ui| {
            ui.label("Shape");
            for (sh, label) in [(Shape::Round, "Round"), (Shape::Square, "Square"), (Shape::Blob, "Blob")] { ui.selectable_value(&mut i.shape, sh, label); }
        });
        ui.add(egui::Slider::new(&mut i.size, 6.0..=400.0).logarithmic(true).text("across"));
        ui.add(egui::Slider::new(&mut i.height, 0.0..=80.0).text("height above the ground"));
        let depth = i.depth;
        ui.add(egui::Slider::new(&mut i.depth, 0.0..=80.0).text("cliff depth")).on_hover_text("Walls from its edge down, closed underneath (0: a flat sheet)");
        ui.label(egui::RichText::new(format!("Detail: {} unit grid", i.spacing())).weak().small());
        if ed.island.depth != depth {
            let d = ed.island.depth;
            adjust_last(ed, |p| p.depth = (d * 64.0).round() / 64.0);
        }
        // The island just built (still selected) follows the edge, texture and cliff depth chosen here.
        let last = ed.last_built.filter(|&k| ed.patch == Some(k) && k < ed.def.patches.len());
        if let Some(k) = last {
            ui.label(egui::RichText::new(format!("Changes here also go to Patch {}, the island you just built.", k + 1)).color(egui::Color32::from_rgb(0x8f, 0xe3, 0x88)).small());
        }
        ui.separator();
        ui.label(egui::RichText::new("Edge").strong()).on_hover_text("Along the island's edge, so Ratchet cannot walk off");
        let mut edge = ed.island.edge.clone();
        edge_picker(ui, ed, contexts, images, &mut edge, "island_edge");
        if edge != ed.island.edge {
            let new = edge.clone();
            adjust_last(ed, move |p| {
                let gaps = p.edge.as_ref().map(|e| e.gaps.clone()).unwrap_or_default();
                p.edge = new.map(|mut e| { e.gaps = gaps; e });
            });
        }
        ed.island.edge = edge;
        ui.separator();
        // The texture the next island gets (an island already built: its own, on the right when it is selected).
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("New island's ground").strong());
            let current = ed.island.texture;
            let rel = current.and_then(|t| ed.entry(&t)).map(|e| e.thumb.clone()).unwrap_or_default();
            let mut budget = 2;
            if let Some(tex) = (!rel.is_empty()).then(|| thumb(ed, contexts, images, &rel, &mut budget)).flatten() {
                ui.add(egui::Image::new((tex, egui::vec2(28.0, 28.0))));
            }
        });
        let current = ed.island.texture;
        egui::CollapsingHeader::new("Change the new island's texture").id_salt("island_texture").show(ui, |ui| {
            if let Some(t) = texture_grid(ui, ed, contexts, images, current, "island_textures") {
                ed.island.texture = Some(t);
                ed.status = "The next island you build gets this texture".into();
                adjust_last(ed, |p| p.texture = Some(t));
            }
        });
        ui.label(egui::RichText::new("For the next island you build. To change an island already built, select it (Shape tab or the list below): its texture is on the right.").weak().small());
        ui.separator();
        ui.label(egui::RichText::new("Green outline: the island a click builds (its top, its cliffs, its edge in yellow). Click on the ground or in the air.").weak());
    } else if ed.terrain_mode == TerrainMode::Select {
        ui.label(egui::RichText::new("Click an island, a path or an object to select it: its settings show on the right. Pick a tool above (or in the toolbar over the world) to build. Esc always comes back here.").weak());
    } else if ed.terrain_mode == TerrainMode::Rails {
        rails_panel(ui, ed);
    } else if ed.terrain_mode == TerrainMode::Paths {
        paths_panel(ui, ed, contexts, images);
    } else if ed.terrain_mode == TerrainMode::Openings {
        ui.label(egui::RichText::new("Click an island to select it. Its edge shows in yellow:").weak());
        ui.label("• Click a stretch of the edge to take it out (an opening)");
        ui.label("• Click an opening (red) to put the edge back");
        ui.label("• Hold the button and run along the edge to do many");
        ui.label(egui::RichText::new("Openings stay where you put them when the coast moves. The edge itself (railing, wall) is chosen on the right.").weak().small());
        let k = ed.patch.filter(|&k| k < ed.def.patches.len());
        match k.map(|k| ed.def.patches[k].edge.as_ref().map(|e| e.gaps.len())) {
            None => { ui.label(egui::RichText::new("No island selected.").color(egui::Color32::from_rgb(0xff, 0xb3, 0x47))); }
            Some(None) => { ui.label(egui::RichText::new("This island has no edge: choose one on the right.").color(egui::Color32::from_rgb(0xff, 0xb3, 0x47))); }
            Some(Some(n)) => {
                ui.horizontal(|ui| {
                    ui.label(format!("{n} openings"));
                    if n > 0 && ui.button("Close all").clicked() {
                        let k = k.unwrap();
                        ed.checkpoint();
                        if let Some(e) = ed.def.patches[k].edge.as_mut() { e.gaps.clear(); }
                        crate::map_load::set(ed.def.clone());
                        ed.status = "Edge closed all round".into();
                        ed.changed();
                    }
                });
            }
        }
    } else if ed.terrain_mode == TerrainMode::Shape {
        ui.label(egui::RichText::new("Click an island to select it, then drag its handles:").weak());
        ui.label("• White dots on the coast: pull the coast out or push it in");
        ui.label("• Green diamond in the middle: move the whole island");
        ui.label("• Blue arrow: raise or lower it");
        ui.add(egui::Slider::new(&mut ed.reach, 1.5..=64.0).logarithmic(true).text("reach")).on_hover_text("How much coast follows a dot (the yellow circle). Mouse wheel over a dot changes it.");
        ui.label(egui::RichText::new("Ctrl: snap to the grid. Wheel over a dot: reach. Ctrl+Z undoes a drag.").weak().small());
        ui.add_space(4.0);
        let k = ed.patch.filter(|&k| k < ed.def.patches.len());
        ui.add_enabled_ui(k.is_some(), |ui| {
            ui.horizontal(|ui| {
                for (label, amount, hint) in [("Grow all", 1.0f32, "The whole coast out by a unit"), ("Shrink all", -1.0, "The whole coast in by a unit")] {
                    if ui.button(label).on_hover_text(hint).clicked() {
                        let Some(k) = k else { continue };
                        let mut p = ed.def.patches[k].clone();
                        match p.offset(amount) {
                            Ok(()) if p.check().is_ok() && !p.loops().is_empty() => {
                                ed.checkpoint();
                                ed.def.patches[k] = p;
                                crate::map_load::set(ed.def.clone());
                                ed.status = if amount > 0.0 { "Island grown".into() } else { "Island shrunk".into() };
                                ed.changed();
                            }
                            Ok(()) => ed.status = "It cannot shrink further (delete it in the list below)".into(),
                            Err(e) => ed.status = e,
                        }
                    }
                }
            });
        });
        if k.is_none() { ui.label(egui::RichText::new("No island selected.").color(egui::Color32::from_rgb(0xff, 0xb3, 0x47))); }
    } else {
        ui.label(egui::RichText::new("Brush").strong());
        ui.horizontal_wrapped(|ui| {
            for (b, label, hint) in BRUSHES { ui.selectable_value(&mut ed.brush, b, label).on_hover_text(hint); }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut ed.square, "Square").on_hover_text("A square brush instead of a round one");
            ui.checkbox(&mut ed.invert, "Invert").on_hover_text("The other way, as holding Shift: Raise lowers, Plateau digs, Paint rubs out, Land cuts away");
        });
        ui.add(egui::Slider::new(&mut ed.radius, 1.0..=32.0).text("radius"));
        ui.add(egui::Slider::new(&mut ed.strength, 0.1..=4.0).text("strength"));
        if ed.brush == Brush::Plateau {
            ui.add(egui::Slider::new(&mut ed.plateau, 0.5..=30.0).text("plateau height")).on_hover_text("How far above (inverted: below) the stroke's start the flat top lies");
        }
        if ed.brush == Brush::Paint {
            ui.label(egui::RichText::new("Paint with").strong());
            if let Some(t) = texture_grid(ui, ed, contexts, images, ed.paint, "paint_textures") { ed.paint = Some(t); }
            if ed.paint.is_none() { ui.label(egui::RichText::new("Pick a texture to paint with.").color(egui::Color32::from_rgb(0xff, 0xb3, 0x47))); }
        }
        if ed.brush == Brush::Land {
            ui.label(egui::RichText::new("Land grows within the island's own square; to reach further, pull its coast in Shape.").weak());
        }
        if ed.brush == Brush::Shade {
            ui.horizontal_wrapped(|ui| {
                for (name, c) in SHADES {
                    let swatch = egui::RichText::new("■").color(egui::Color32::from_rgb(c[0], c[1], c[2])).size(18.0);
                    if ui.selectable_label(ed.shade == c, swatch).on_hover_text(name).clicked() { ed.shade = c; }
                }
                ui.color_edit_button_srgb(&mut ed.shade).on_hover_text("Any colour");
            });
        }
        if ed.brush == Brush::Scatter {
            ui.add(egui::Slider::new(&mut ed.scatter_size, 0.3..=3.0).text("size"));
            ui.label(egui::RichText::new("Decor to strew").strong());
            if let Some(a) = scatter_grid(ui, ed, contexts, images) { ed.scatter = Some(a); }
            if ed.scatter.is_none() { ui.label(egui::RichText::new("Pick a plant, rock or detail to strew.").color(egui::Color32::from_rgb(0xff, 0xb3, 0x47))); }
        }
        if ed.brush == Brush::Gaps {
            ui.label(egui::RichText::new("Brush over an island's railing or wall to take it out; Shift (or Invert) puts it back.").weak());
        }
        ui.label(egui::RichText::new("Drag on a patch to use the brush (Shift: the other way). Right mouse: look.").weak());
    }
    ui.separator();
    ui.label(egui::RichText::new(format!("Patches on this map ({})", ed.def.patches.len())).strong());
    let mut remove = None;
    for k in 0..ed.def.patches.len() {
        let p = &ed.def.patches[k];
        let label = format!("Patch {}: {}×{} at {:.0}, {:.0}, height {:.0}", k + 1, p.size[0], p.size[1], p.origin[0], p.origin[1], p.origin[2]);
        ui.horizontal(|ui| {
            if ui.selectable_label(ed.patch == Some(k), label).on_hover_text("Select it (its edge shows in orange)").clicked() { ed.patch = Some(k); }
            if ui.small_button("x").on_hover_text("Delete this patch").clicked() { remove = Some(k); }
        });
    }
    if let Some(k) = remove {
        ed.checkpoint();
        ed.def.patches.remove(k);
        ed.patch = None;
        ed.status = format!("Patch {} deleted (Ctrl+Z brings it back)", k + 1);
        ed.changed();
    }
    if ed.def.patches.is_empty() { ui.label(egui::RichText::new("None yet: build one with Build islands.").weak()); }
}

/// The selected patch: where it is, its cliffs, its texture.
fn patch_inspector(ui: &mut egui::Ui, ed: &mut Editor, k: usize, contexts: &mut EguiContexts, images: &mut Assets<Image>) {
    ui.heading(format!("Patch {}", k + 1));
    let mut p = ed.def.patches[k].clone();
    let before = p.clone();
    let across = [p.size[0] as f32 * p.spacing, p.size[1] as f32 * p.spacing];
    ui.label(egui::RichText::new(format!("{}×{} cells, {} × {} units", p.size[0], p.size[1], across[0], across[1])).weak());
    egui::Grid::new("patch").num_columns(2).show(ui, |ui| {
        for (i, label) in ["X", "Y", "Base height"].iter().enumerate() {
            ui.label(*label);
            ui.add(egui::DragValue::new(&mut p.origin[i]).speed(0.25).range(0.0..=1000.0));
            ui.end_row();
        }
        ui.label("Cliff depth").on_hover_text("Walls from the edges down by this much, closed underneath: a floating island or a mesa (0: an open sheet)");
        ui.add(egui::DragValue::new(&mut p.depth).speed(0.25).range(0.0..=200.0));
        ui.end_row();
        ui.label("Texture size");
        ui.add(egui::DragValue::new(&mut p.tile).speed(0.1).range(1.0..=64.0));
        ui.end_row();
    });
    ui.separator();
    ui.label(egui::RichText::new("Edge").strong()).on_hover_text("Along the island's edge, so Ratchet cannot walk off");
    edge_picker(ui, ed, contexts, images, &mut p.edge, "patch_edge");
    ui.separator();
    ui.label(egui::RichText::new("Surface").strong()).on_hover_text("What its ground does to Ratchet");
    ui.horizontal_wrapped(|ui| {
        for (b, name, hint) in rc_map::floor::SURFACES { ui.selectable_value(&mut p.surface, b, name).on_hover_text(hint); }
    });
    if !p.scatter.is_empty() {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("{} decor pieces", p.scatter.len())).weak());
            if ui.small_button("Clear").on_hover_text("Takes all the decor off this island").clicked() { p.scatter.clear(); }
        });
    }
    ui.separator();
    ui.label(egui::RichText::new("Roof").strong()).on_hover_text("A flat textured plane over the island's land (a ceiling, a canopy). Ratchet can stand on it.");
    let mut roofed = p.roof.is_some();
    if ui.checkbox(&mut roofed, "Roof over this island").changed() {
        p.roof = roofed.then(|| rc_map::terrain::Roof { height: 6.0, texture: p.texture, tile: p.tile });
    }
    if let Some(r) = p.roof.as_mut() {
        ui.add(egui::Slider::new(&mut r.height, 0.5..=60.0).text("roof height")).on_hover_text("Above the island's base height");
        r.height = (r.height * 64.0).round() / 64.0;
        let current = r.texture;
        egui::CollapsingHeader::new("Roof texture").id_salt("roof_texture").show(ui, |ui| {
            if ui.selectable_label(current.is_none(), "Plain").clicked() { r.texture = None; }
            if let Some(t) = texture_grid(ui, ed, contexts, images, current, "roof_textures") { r.texture = Some(t); }
        });
    }
    ui.separator();
    ui.label(egui::RichText::new("Texture").strong());
    if ui.selectable_label(p.texture.is_none(), "Plain").clicked() { p.texture = None; }
    if let Some(id) = texture_grid(ui, ed, contexts, images, p.texture, "patch_textures") { p.texture = Some(id); }
    ui.separator();
    let tex = p.texture;
    colour_section(ui, ed, &mut p.tint, tex, "island");
    ui.add_space(8.0);
    if ui.button("Delete patch").clicked() {
        ed.checkpoint();
        ed.def.patches.remove(k);
        ed.patch = None;
        ed.changed();
        return;
    }
    for v in &mut p.origin { *v = (*v * 64.0).round() / 64.0; }
    p.depth = (p.depth * 64.0).round() / 64.0;
    if p != before && p.check().is_ok() {
        if !ed.coalescing { ed.checkpoint(); }
        ed.coalescing = true;
        ed.def.patches[k] = p;
        crate::map_load::set(ed.def.clone());
        ed.changed();
    }
}

/// The asset browser: the catalog's placeable mobys; a click arms one for placing.
fn asset_browser(ui: &mut egui::Ui, ed: &mut Editor, contexts: &mut EguiContexts, images: &mut Assets<Image>) {
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut ed.filter).hint_text("Search…").desired_width(170.0));
        ui.checkbox(&mut ed.live_only, "Live only");
    });
    let planet = ed.planet.map_or("All planets", |p| LEVEL_NAMES[p as usize]);
    egui::CollapsingHeader::new(format!("Planet: {planet}")).id_salt("planet").show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut ed.planet, None, "All");
            for (i, n) in LEVEL_NAMES.iter().enumerate() { ui.selectable_value(&mut ed.planet, Some(i as u32), *n); }
        });
    });
    let Some(cat) = ed.catalog.clone() else {
        ui.label("No catalog: run rerac --build-catalog");
        return;
    };
    let q = ed.filter.to_lowercase();
    let (live_only, planet) = (ed.live_only, ed.planet);
    // Whether an asset is in a type (and subtype) with the search, planet and Live filters.
    let shows = |e: &Entry, kind: Option<&str>, sub: Option<&str>| {
        (!live_only || e.live == Some(Live::Live))
            && kind.is_none_or(|k| e.kind == k)
            && sub.is_none_or(|s| subtype(&e.kind, &e.name) == Some(s))
            && planet.is_none_or(|p| e.ids().any(|id| id.level == p))
            && (q.is_empty() || e.name.to_lowercase().contains(&q) || e.about.to_lowercase().contains(&q) || e.tags.iter().any(|t| t.contains(&q)) || e.id.to_string().contains(&q))
    };
    // A type's (subtype's) icon: the first asset of its own list.
    let placeable = ed.placeable.clone();
    let first = |kind: Option<&str>, sub: Option<&str>| placeable.iter().copied().find(|&i| shows(&cat.entries[i], kind, sub));
    let mut budget = 24;
    // The types as square tiles (the first asset's picture, a short name; the full name on hover), then the chosen
    // type's subtypes as smaller tiles (one click; no pop-up lists).
    let mut tile = |ui: &mut egui::Ui, ed: &mut Editor, icon: Option<usize>, label: &str, hint: &str, on: bool, size: f32| -> bool {
        let tex = icon.and_then(|i| thumb(ed, contexts, images, &cat.entries[i].thumb, &mut budget));
        let (rect, r) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
        let orange = egui::Color32::from_rgb(0xf0, 0x8a, 0x24);
        let fill = if on { egui::Color32::from_rgb(0x4a, 0x35, 0x18) } else if r.hovered() { egui::Color32::from_rgb(0x3a, 0x4d, 0x75) } else { egui::Color32::from_rgb(0x2a, 0x37, 0x55) };
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 8.0, fill);
        if on { painter.rect_stroke(rect.shrink(1.0), 8.0, egui::Stroke::new(2.0, orange), egui::StrokeKind::Inside); }
        let font = egui::FontId::proportional(if size < 60.0 { 11.0 } else { 12.5 });
        let galley = painter.layout(label.to_string(), font, if on { orange } else { egui::Color32::from_gray(230) }, size - 6.0);
        let img = (size - galley.size().y - 14.0).min(size * 0.62);
        let img_rect = egui::Rect::from_center_size(egui::pos2(rect.center().x, rect.top() + 5.0 + img / 2.0), egui::vec2(img, img));
        match tex {
            Some(t) => egui::Image::new((t, egui::vec2(img, img))).paint_at(ui, img_rect),
            None => {
                // No picture (effects have no model): a spark.
                let (c, r) = (img_rect.center(), img * 0.3);
                for k in 0..4 {
                    let a = k as f32 * std::f32::consts::FRAC_PI_4;
                    let d = egui::vec2(a.cos(), a.sin()) * if k % 2 == 0 { r } else { r * 0.6 };
                    painter.line_segment([c - d, c + d], egui::Stroke::new(2.0, egui::Color32::from_gray(170)));
                }
            }
        }
        painter.galley(egui::pos2(rect.center().x - galley.size().x / 2.0, rect.bottom() - 5.0 - galley.size().y), galley, egui::Color32::WHITE);
        r.on_hover_text(hint).clicked()
    };
    // Tiles in rows that fill the panel's width.
    let rows_of = |ui: &egui::Ui, per_row: usize| { let gap = 6.0; ((ui.available_width() - gap * (per_row as f32 - 1.0)) / per_row as f32).floor().clamp(44.0, 96.0) };
    let icons: Vec<Option<usize>> = std::iter::once(first(None, None)).chain(KINDS.iter().map(|(k, _)| first(Some(k), None))).collect();
    let size = rows_of(ui, 4);
    let mut cells: Vec<(Option<&str>, &str, &str)> = vec![(None, "All", "Every type")];
    cells.extend(KINDS.iter().map(|(key, label)| (Some(*key), label.split(" &").next().unwrap_or(label), *label)));
    for (row, chunk) in cells.chunks(4).enumerate() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for (k, (key, short, full)) in chunk.iter().enumerate() {
                let n = row * 4 + k;
                if tile(ui, ed, icons[n], short, full, ed.kind == *key, size) && ed.kind != *key { ed.kind = *key; ed.sub = None; }
            }
        });
        ui.add_space(2.0);
    }
    if let Some(subs) = ed.kind.and_then(|k| SUBTYPES.iter().find(|(t, _)| *t == k)).map(|(_, s)| *s) {
        let kind = ed.kind;
        let icons: Vec<Option<usize>> = subs.iter().map(|(s, _)| first(kind, Some(s))).collect();
        let subs: Vec<(Option<&str>, Option<usize>)> = std::iter::once((None, first(kind, None))).chain(subs.iter().enumerate().filter(|(n, _)| icons[*n].is_some()).map(|(n, (s, _))| (Some(*s), icons[n]))).collect();
        ui.add_space(2.0);
        let size = rows_of(ui, 5);
        for chunk in subs.chunks(5) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for (sub, icon) in chunk {
                    let label = sub.unwrap_or("All of them");
                    if tile(ui, ed, *icon, label, label, ed.sub == *sub, size) { ed.sub = *sub; }
                }
            });
            ui.add_space(2.0);
        }
    }
    // Always one line, so arming an asset does not move the list under the pointer.
    match ed.armed.map(|e| cat.entries[e].name.clone()) {
        Some(e) => ui.label(egui::RichText::new(format!("Placing: {e} — click in the world (Esc: stop)")).color(egui::Color32::from_rgb(0xff, 0xb3, 0x47))),
        None => ui.label(egui::RichText::new("Click an asset, then click in the world to place it").weak()),
    };
    ui.separator();
    let (kind, sub) = (ed.kind, ed.sub);
    let rows: Vec<usize> = placeable.iter().copied().filter(|&i| shows(&cat.entries[i], kind, sub)).collect();
    ui.label(format!("{} assets", rows.len()));
    egui::ScrollArea::vertical()
        // A new filter starts at the top of its list.
        .id_salt(("rac_maker_assets_list", ed.kind, ed.sub, ed.planet, ed.live_only))
        .auto_shrink([false; 2])
        .scroll_source(egui::scroll_area::ScrollSource::MOUSE_WHEEL | egui::scroll_area::ScrollSource::SCROLL_BAR)
        .show_rows(ui, 56.0, rows.len(), |ui, range| {
            for &i in &rows[range] {
                let e = &cat.entries[i];
                let tex = thumb(ed, contexts, images, &e.thumb, &mut budget);
                let armed = ed.armed == Some(i);
                // The row is drawn, then one click area over all of it (registered last, so it is on top).
                let bg = ui.painter().add(egui::Shape::Noop);
                let row = ui.horizontal(|ui| {
                    ui.set_height(52.0);
                    ui.set_min_width(ui.available_width());
                    match tex {
                        Some(t) => { ui.add(egui::Image::new((t, egui::vec2(48.0, 48.0)))); }
                        None => { ui.add_sized([48.0, 48.0], egui::Label::new("…")); }
                    }
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(if e.name.is_empty() { e.id.to_string() } else { e.name.clone() }).strong());
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(LEVEL_NAMES[e.id.level as usize]).small());
                            badge(ui, e);
                        });
                    });
                }).response.rect;
                let r = ui.interact(row, egui::Id::new(("rac_maker_asset", i)), egui::Sense::click());
                let fill = if armed {
                    egui::Color32::from_rgb(0x4a, 0x35, 0x18)
                } else if r.hovered() {
                    egui::Color32::from_rgb(0x2f, 0x42, 0x68)
                } else {
                    egui::Color32::TRANSPARENT
                };
                ui.painter().set(bg, egui::Shape::rect_filled(row, 4.0, fill));
                if r.clicked() {
                    ed.armed = if armed { None } else { Some(i) };
                    ed.selected = None;
                    ed.status = if armed { "Placing cancelled".into() } else { format!("Click in the world to place {}: on an object's top to stack, its side to line up", e.name) };
                }
                r.on_hover_text(if e.about.is_empty() { e.id.to_string() } else { e.about.clone() });
            }
        });
}

/// Every placed object (the Objects tab's foot): select, delete.
fn placed_list(ui: &mut egui::Ui, ed: &mut Editor) {
    egui::CollapsingHeader::new(format!("Placed objects ({})", ed.def.objects.len())).id_salt("placed").show(ui, |ui| {
        let mut remove = None;
        egui::ScrollArea::vertical().id_salt("placed_list").max_height(200.0).show(ui, |ui| {
            for i in 0..ed.def.objects.len() {
                let o = &ed.def.objects[i];
                let label = format!("{} ({:.0}, {:.0})", ed.name_of(&o.asset), o.position[0], o.position[1]);
                ui.horizontal(|ui| {
                    if ui.selectable_label(ed.selected == Some(i), label).clicked() {
                        ed.selected = Some(i);
                        ed.armed = None;
                    }
                    if ui.small_button("x").on_hover_text("Delete").clicked() { remove = Some(i); }
                });
            }
        });
        if let Some(i) = remove {
            ed.checkpoint();
            let gone = ed.remove_object(i);
            ed.selected = None;
            ed.status = format!("Removed {}", ed.name_of(&gone.asset));
            ed.changed();
        }
    });
}

/// The inspector: the selected object, else the map.
fn inspector(ui: &mut egui::Ui, ed: &mut Editor, contexts: &mut EguiContexts, images: &mut Assets<Image>, cam: Option<&GlobalTransform>) {
    if let Some(i) = ed.selected.filter(|&i| i < ed.def.objects.len()) {
        let id = ed.def.objects[i].asset;
        ui.heading(ed.name_of(&id));
        if let Some(e) = ed.entry(&id).cloned() {
            if let Some(t) = thumb(ed, contexts, images, &e.thumb, &mut 4) { ui.add(egui::Image::new((t, egui::vec2(96.0, 96.0)))); }
            badge(ui, &e);
            if !e.about.is_empty() { ui.label(&e.about); }
        }
        ui.label(egui::RichText::new(id.to_string()).monospace().small());
        ui.separator();
        let mut ob = ed.def.objects[i].clone();
        let mut edited = false;
        egui::Grid::new("object").num_columns(2).show(ui, |ui| {
            for (k, label) in ["X", "Y", "Height"].iter().enumerate() {
                ui.label(*label);
                edited |= ui.add(egui::DragValue::new(&mut ob.position[k]).speed(0.1).range(0.0..=1023.0)).changed();
                ui.end_row();
            }
            ui.label("Facing");
            let mut deg = ob.yaw.to_degrees();
            if ui.add(egui::Slider::new(&mut deg, 0.0..=359.0).suffix("°")).changed() {
                ob.yaw = deg.to_radians();
                edited = true;
            }
            ui.end_row();
            ui.label("Name").on_hover_text("For the Logic tab's rules: \"when the Gate Switch is destroyed, remove the Gate\"");
            let mut name = ob.name.clone().unwrap_or_default();
            if ui.add(egui::TextEdit::singleline(&mut name).hint_text("none").desired_width(150.0)).changed() {
                ob.name = Some(name.trim().to_string()).filter(|n| !n.is_empty());
                edited = true;
            }
            ui.end_row();
        });
        ui.horizontal(|ui| {
            let mut tinted = ob.tint.is_some();
            if ui.checkbox(&mut tinted, "Tint").on_hover_text("Colours this object (its own light colour): a golden statue, a red crate").changed() {
                ob.tint = tinted.then_some([255, 200, 90]);
                edited = true;
            }
            if let Some(c) = ob.tint.as_mut() { edited |= ui.color_edit_button_srgb(c).changed(); }
        });
        if ui.checkbox(&mut ob.hidden, "Hidden until a rule shows it").on_hover_text("Not there, not solid, not moving until a rule in the Logic tab shows it: an ambush, a bridge that appears").changed() {
            if ob.hidden && ob.name.is_none() { ob.name = Some(format!("{} {}", ed.name_of(&id), i + 1)); }
            edited = true;
        }
        if edited {
            if !ed.coalescing { ed.checkpoint(); }
            ed.coalescing = true;
            // Its path moves with it (the first point is where it stands).
            let was = ed.def.objects[i].position;
            let d = [ob.position[0] - was[0], ob.position[1] - was[1], ob.position[2] - was[2]];
            if d != [0.0; 3] { for q in &mut ob.path { for k in 0..3 { q[k] += d[k]; } } }
            ed.def.objects[i] = ob;
            ed.changed();
        }
        // Where the game put it, if it moves itself.
        if let Some(at) = ed.moved_away.get(&ed.ids[i]).copied() {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(format!(
                "Moved by the game: when the level starts, this object jumps to the end of the red line ({:.0}, {:.0}, height {:.0}). Its own code puts it there, not you; so in Play it starts there.",
                at[0], at[1], at[2]
            )).color(egui::Color32::from_rgb(0xef, 0x6b, 0x6b)))
            .on_hover_text("Some enemies start on their patrol route or at a spot from their home planet's data. Most are fixed; if one still jumps, try placing it where the line ends, or pick another enemy.");
        }
        // Solid from its model (many pieces are solid on the disc only through the level geometry around them).
        {
            let mut solid = ed.def.objects[i].solid.unwrap_or(false);
            let hint = "Builds collision from its shape, so Ratchet cannot walk through it. Many pieces have no collision of their own (the level around them was solid). Leave it off for things that move or break: the collision stays where it was put.";
            if ui.checkbox(&mut solid, "Solid (collision from its model)").on_hover_text(hint).changed() {
                ed.checkpoint();
                ed.def.objects[i].solid = Some(solid);
                ed.changed();
            }
        }
        // A mover or a patrolling enemy: its path.
        if crate::map_load::PATH_CLASSES.contains(&id.number) { path_settings(ui, ed, i); }
        // An enemy's sight.
        if ed.entry(&id).is_some_and(|e| e.kind == rc_map::catalog::KIND_ENEMY) {
            ui.add_space(6.0);
            match ed.sight(id) {
                Some((wake, range)) => {
                    ui.label(egui::RichText::new("Sight").strong());
                    let notice = if wake > 0.0 { format!("Notices Ratchet within {wake:.0} units (blue circle)") } else { "Notices Ratchet by its area, not by distance".to_string() };
                    let attack = if range > 0.0 { format!("attacks within {range:.1} (red).") } else { "attacks from wherever it notices him.".to_string() };
                    ui.label(format!("{notice}, {attack}"));
                }
                None => {
                    ui.label(egui::RichText::new("Sight range: not known for this enemy yet.").weak());
                }
            }
        }
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Drag the arrows to move it, the yellow ring to turn it.").weak());
        // Copies next to it: the directions as seen from the camera (snapped to the world's axes).
        ui.label(egui::RichText::new("Add another").strong());
        let fwd = cam.map_or([0.0, 1.0], |c| {
            let f = to_game(c.forward().as_vec3());
            if f[0].abs() > f[1].abs() { [f[0].signum(), 0.0] } else { [0.0, f[1].signum()] }
        });
        let right = [fwd[1], -fwd[0]];
        let mut add = None;
        ui.horizontal(|ui| {
            if ui.button("On top").clicked() { add = Some([0.0, 0.0, 1.0]); }
            if ui.button("Left").clicked() { add = Some([-right[0], -right[1], 0.0]); }
            if ui.button("Right").clicked() { add = Some([right[0], right[1], 0.0]); }
        });
        ui.horizontal(|ui| {
            if ui.button("In front").on_hover_text("Towards the camera").clicked() { add = Some([-fwd[0], -fwd[1], 0.0]); }
            if ui.button("Behind").on_hover_text("Away from the camera").clicked() { add = Some([fwd[0], fwd[1], 0.0]); }
        });
        if let Some(d) = add { ed.add_beside(i, d); }
        ui.add_space(8.0);
        if ui.button("Delete").on_hover_text("Delete key").clicked() {
            ed.checkpoint();
            ed.remove_object(i);
            ed.selected = None;
            ed.changed();
        }
        return;
    }
    ui.heading("Map");
    let before = ed.def.clone();
    let mut m = ed.def.clone();
    egui::Grid::new("map").num_columns(2).show(ui, |ui| {
        ui.label("Name");
        ui.text_edit_singleline(&mut m.name);
        ui.end_row();
        ui.label("Planet (host)");
        ui.label(LEVEL_NAMES[m.host_level as usize]);
        ui.end_row();
        ui.label("Objects");
        ui.label(m.objects.len().to_string());
        ui.end_row();
        ui.label("Death height");
        let mut d = m.death_height.unwrap_or(0.0);
        if ui.add(egui::DragValue::new(&mut d).speed(0.25)).changed() { m.death_height = Some(d); }
        ui.end_row();
        ui.label("Grind rails");
        let mut rails = m.abilities.rails.unwrap_or(false);
        if ui.checkbox(&mut rails, "on").changed() { m.abilities.rails = Some(rails); }
        ui.end_row();
        ui.label("Reset on death");
        ui.checkbox(&mut m.reset_on_death, "");
        ui.end_row();
    });
    ui.separator();
    // Ratchet's ship: the landing at the start, boarding it after the goal finishes the map. His start is then by
    // its hatch (crate::map_load::ship_start).
    let mut has_ship = m.ship.is_some();
    if ui.checkbox(&mut has_ship, egui::RichText::new("Ratchet's ship").strong()).on_hover_text("The level starts with the ship flying in and landing; once the goal is reached Ratchet walks back aboard and flies off to finish, as on the game's planets").changed() {
        let (s, c) = m.start.yaw.sin_cos();
        m.ship = has_ship.then(|| rc_map::Start { position: [m.start.position[0] + c * 6.0, m.start.position[1] + s * 6.0, m.start.position[2] - 0.5], yaw: m.start.yaw });
    }
    let (label, place) = match m.ship.as_mut() {
        Some(sh) => ("Where the ship lands (Ratchet starts by its hatch)", sh),
        None => ("Start", &mut m.start),
    };
    ui.label(egui::RichText::new(label).strong());
    egui::Grid::new("start").num_columns(2).show(ui, |ui| {
        for (k, label) in ["X", "Y", "Height"].iter().enumerate() {
            ui.label(*label);
            ui.add(egui::DragValue::new(&mut place.position[k]).speed(0.1).range(0.0..=1023.0));
            ui.end_row();
        }
        ui.label("Facing");
        let mut deg = place.yaw.to_degrees().rem_euclid(360.0);
        if ui.add(egui::Slider::new(&mut deg, 0.0..=359.0).suffix("°")).changed() { place.yaw = deg.to_radians(); }
        ui.end_row();
    });
    if m.ship.is_some() && m.ship != before.ship {
        let root = crate::level_load::extracted_root();
        if let Some((p, yaw)) = rc_data::level_gameplay(&root, m.host_level).ok().and_then(|g| crate::map_load::ship_start(&m, &g)) {
            m.start = rc_map::Start { position: p.map(|v| (v * 64.0).round() / 64.0), yaw };
        }
    }
    if m.ship.is_some() && !m.zones.iter().any(|z| matches!(z.kind, rc_map::logic::ZoneKind::Goal)) && !m.rules.iter().any(|r| r.then.iter().any(|t| matches!(t, rc_map::logic::Then::Goal))) {
        ui.label(egui::RichText::new("No goal yet (Logic tab): Ratchet can board the ship at any time.").weak().small());
    }
    ui.separator();
    ui.label(egui::RichText::new("Floors").strong());
    let mut remove = None;
    let mut tex_for = None;
    for (k, f) in m.floors.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            if ui.selectable_label(ed.floor_edit == Some(k), format!("{}", k + 1)).on_hover_text("Its texture and texture size").clicked() {
                tex_for = Some(k);
            }
            let (min, max) = (&mut f.min, &mut f.max);
            for v in min.iter_mut().chain(max.iter_mut()) { ui.add(egui::DragValue::new(v).speed(4.0).range(0.0..=1020.0)); }
            ui.label("h");
            ui.add(egui::DragValue::new(&mut f.height).speed(0.25).range(0.0..=1000.0));
            if ui.small_button("x").on_hover_text("Remove this floor").clicked() { remove = Some(k); }
        });
    }
    if let Some(k) = tex_for { ed.floor_edit = if ed.floor_edit == Some(k) { None } else { Some(k) }; }
    if let Some(k) = ed.floor_edit.filter(|&k| k < m.floors.len()) {
        ui.group(|ui| {
            ui.label(egui::RichText::new(format!("Floor {} texture", k + 1)).strong());
            ui.add(egui::Slider::new(&mut m.floors[k].tile, 1.0..=64.0).text("texture size").logarithmic(true))
                .on_hover_text("Units per repeat: smaller is more detailed");
            if ui.selectable_label(m.floors[k].texture.is_none(), "Plain").clicked() { m.floors[k].texture = None; }
            if let Some(id) = texture_grid(ui, ed, contexts, images, m.floors[k].texture, "floor_textures") { m.floors[k].texture = Some(id); }
        });
    }
    if let Some(k) = remove {
        m.floors.remove(k);
        ed.floor_edit = None;
    }
    if ui.button("Add floor").clicked() {
        let s = m.start.position;
        let (x, y) = ((s[0] / 4.0).floor() * 4.0, (s[1] / 4.0).floor() * 4.0);
        m.floors.push(rc_map::Floor { min: [x - 16.0, y - 16.0], max: [x + 16.0, y + 16.0], height: (s[2] - 0.5).floor(), texture: None, tile: 8.0 });
    }
    ui.separator();
    ui.collapsing("Loadout", |ui| {
        for (id, name) in ITEMS {
            let mut on = m.loadout.contains(&id);
            if ui.checkbox(&mut on, name).changed() {
                if on { m.loadout.push(id); } else { m.loadout.retain(|&i| i != id); }
            }
        }
    });
    // Floors are snapped to the collision cells (4 units); a floor that does not check is not taken.
    for f in &mut m.floors {
        for v in f.min.iter_mut().chain(f.max.iter_mut()) { *v = (*v / 4.0).round() * 4.0; }
        if f.max[0] <= f.min[0] { f.max[0] = f.min[0] + 4.0; }
        if f.max[1] <= f.min[1] { f.max[1] = f.min[1] + 4.0; }
        f.height = (f.height * 64.0).round() / 64.0;
    }
    if m != before {
        if !ed.coalescing { ed.checkpoint(); }
        ed.coalescing = true;
        ed.def = m;
        ed.changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every edge piece loads from its level and is railing- or wall-shaped (longer than deep, standing up); laid
    /// along an island it closes the loop. Needs the game data (RC_DATA_DIR); skipped without it.
    #[test]
    fn edge_pieces_load_and_go_round() {
        if std::env::var_os("RC_DATA_DIR").is_none() { return; }
        let island = rc_map::terrain::Patch::island([200.0, 200.0, 60.0], 1.0, 10.0, |x, y| 10.0 - (x * x + y * y).sqrt());
        for (key, name, _) in EDGE_PIECES {
            let m = crate::map_load::piece_model(AssetId::parse(key).unwrap()).unwrap_or_else(|| panic!("{name} ({key}) did not load"));
            let k = m.long_axis();
            let (long, deep, high) = (m.length(), m.hi[1 - k] - m.lo[1 - k], m.height());
            println!("{name:28} {key:20} {long:5.2} long {deep:5.2} deep {high:5.2} high, {} parts", m.parts.len());
            assert!(long > deep && high > 0.2, "{name}: {long} × {deep} × {high}");
            let spans = crate::map_load::piece_spans(&island, m.length());
            assert!(spans.len() >= 3);
            // Each piece ends where the next starts.
            for w in spans.windows(2) { assert_eq!(w[0].1, w[1].0); }
            // An opening over one stretch takes out that stretch only.
            let mut open = island.clone();
            let edge = rc_map::terrain::Edge::new(rc_map::terrain::EdgeKind::Piece(AssetId::parse(key).unwrap()), 1.5);
            open.edge = Some(edge);
            let first = edge_stretches(&open)[0];
            open.edge.as_mut().unwrap().gaps.push([first.mid[0], first.mid[1], first.half * 0.9]);
            assert_eq!(crate::map_load::edge_spans(&open).len(), spans.len() - 1, "{name}");
            assert_eq!(edge_stretches(&open).iter().filter(|s| s.open).count(), 1);
        }
    }

    #[test]
    fn a_turned_box_swaps_its_sides() {
        let b = world_box([-1.0, -0.5, 0.0, 1.0, 0.5, 2.0], [10.0, 20.0, 5.0], std::f32::consts::FRAC_PI_2);
        for (got, want) in b.iter().zip([9.5, 19.0, 5.0, 10.5, 21.0, 7.0]) { assert!((got - want).abs() < 1e-5, "{b:?}"); }
    }

    #[test]
    fn touching_boxes_do_not_overlap() {
        let a = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        assert!(!overlaps(&a, &[1.0, 0.0, 0.0, 2.0, 1.0, 1.0]));
        assert!(!overlaps(&a, &[0.0, 0.0, 1.0, 1.0, 1.0, 2.0]));
        assert!(overlaps(&a, &[0.5, 0.5, 0.5, 1.5, 1.5, 1.5]));
    }

    #[test]
    fn a_ray_from_above_enters_the_top() {
        let hit = ray_box(([0.5, 0.5, 10.0], [0.0, 0.0, -1.0]), &[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]).unwrap();
        assert_eq!((hit.1, hit.2), (2, 1.0));
        assert!((hit.0 - 9.0).abs() < 1e-5);
        let side = ray_box(([-5.0, 0.5, 0.5], [1.0, 0.0, 0.0]), &[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]).unwrap();
        assert_eq!((side.1, side.2), (0, -1.0));
        assert!(ray_box(([-5.0, 5.0, 0.5], [1.0, 0.0, 0.0]), &[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]).is_none());
    }
}
