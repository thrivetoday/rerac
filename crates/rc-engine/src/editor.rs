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
use rc_map::catalog::{Behaviour, Catalog, Entry, Live, GROUP_GEAR};
use rc_map::{AssetId, AssetKind, MapDef, Object, LEVEL_NAMES};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

mod kit;
mod menu;
mod objects;
use kit::*;
use objects::CATEGORIES;

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
    /// Clicks select (objects, islands, paths, lights; the start is dragged): the left panel lists what is on the map.
    Select,
    Objects,
    Terrain,
    /// Zones and when/then rules (rc_map::logic).
    Logic,
    /// The sky, fog and song (rc_map::World), and the radio.
    World,
}

/// The tool strip (the left edge, docs/plan/rac_maker.md "v0.6"): icon, caption, hint, the tool and terrain mode.
const STRIP: [(&str, &str, &str, Tool, TerrainMode); 10] = [
    ("Select", "Select", "Select and move things (1, Esc)", Tool::Select, TerrainMode::Select),
    ("Objects", "Objects", "Place objects: enemies, crates, platforms (2)", Tool::Objects, TerrainMode::Select),
    ("Islands", "Islands", "Build islands (3)", Tool::Terrain, TerrainMode::Build),
    ("Shape", "Shape", "Shape an island: pull its coast, move it, raise it (4)", Tool::Terrain, TerrainMode::Shape),
    ("Edges", "Edges", "Openings in an island's railing or wall (5)", Tool::Terrain, TerrainMode::Openings),
    ("Sculpt", "Sculpt", "Hills, paint, colour and plants with brushes (6)", Tool::Terrain, TerrainMode::Sculpt),
    ("Paths", "Paths", "Walkways and bridges (7)", Tool::Terrain, TerrainMode::Paths),
    ("Rails", "Rails", "Grind rails (8)", Tool::Terrain, TerrainMode::Rails),
    ("Logic", "Logic", "Checkpoints, goals, cameras and rules (9)", Tool::Logic, TerrainMode::Select),
    ("World", "World", "Sky, light, colours, music (0)", Tool::World, TerrainMode::Select),
];

/// Whether the strip's tool `(tool, mode)` is the current one.
fn tool_is(ed: &Editor, tool: Tool, mode: TerrainMode) -> bool { ed.tool == tool && (tool != Tool::Terrain || ed.terrain_mode == mode) }

/// Picks a tool: whatever was being placed, drawn or brushed stops; the selection stays (its panel is on the right).
fn set_tool(ed: &mut Editor, tool: Tool, mode: TerrainMode) {
    ed.armed = None;
    // A sculpt stroke ends as it would on letting go (the land reloads); the other drags are dropped.
    if ed.stroke.take().is_some() { ed.changed(); }
    (ed.island_drag, ed.shape_drag, ed.gap_paint, ed.walk_drag, ed.rail_drag, ed.rail_axis) = (None, None, None, None, None, None);
    ed.zone_new = None;
    (ed.walk_drawing, ed.walk_first) = (false, None);
    (ed.rail_drawing, ed.rail_first) = (false, None);
    ed.tool = tool;
    if tool == Tool::Terrain { ed.terrain_mode = mode; }
}

/// A drag, stroke or brush is going on (the mouse held in the world): undo, Delete and the tool keys wait for it, else
/// they would pull the island, path or point out from under it.
fn busy(ed: &Editor) -> bool {
    ed.drag.is_some() || ed.stroke.is_some() || ed.island_drag.is_some() || ed.shape_drag.is_some() || ed.gap_paint.is_some()
        || ed.walk_drag.is_some() || ed.rail_drag.is_some() || ed.rail_axis.is_some() || ed.zone_drag.is_some() || ed.light_drag.is_some()
        || ed.start_drag
}

/// Nothing selected: the right panel shows the map's settings.
fn clear_selection(ed: &mut Editor) {
    (ed.selected, ed.patch, ed.walk, ed.zone, ed.rail, ed.rail_point, ed.light_sel) = (None, None, None, None, None, None, None);
    (ed.walk_drawing, ed.walk_first) = (false, None);
    (ed.rail_drawing, ed.rail_first) = (false, None);
}

/// Deletes what is selected (the inspector's bin, the Delete key), one undo step.
fn delete_selected(ed: &mut Editor) {
    let what = if let Some(i) = ed.selected.filter(|&i| i < ed.def.objects.len()) {
        ed.checkpoint();
        let gone = ed.remove_object(i);
        ed.name_of(&gone.asset)
    } else if let Some(z) = ed.zone.filter(|&z| z < ed.def.zones.len()) {
        ed.checkpoint();
        ed.def.zones.remove(z).name
    } else if let Some(k) = ed.patch.filter(|&k| k < ed.def.patches.len()) {
        ed.checkpoint();
        ed.def.patches.remove(k);
        format!("Island {}", k + 1)
    } else if let Some(k) = ed.walk.filter(|&k| k < ed.def.walkways.len()) {
        ed.checkpoint();
        ed.def.walkways.remove(k);
        format!("Path {}", k + 1)
    } else if let Some(k) = ed.rail.filter(|&k| k < ed.def.rails.len()) {
        ed.checkpoint();
        ed.def.rails.remove(k);
        format!("Rail {}", k + 1)
    } else if let Some(k) = ed.light_sel.filter(|&k| k < ed.def.world.lights.len()) {
        ed.checkpoint();
        ed.def.world.lights.remove(k);
        format!("Light {}", k + 1)
    } else {
        return;
    };
    clear_selection(ed);
    crate::map_load::set(ed.def.clone());
    ed.status = format!("{what} deleted (Ctrl+Z brings it back)");
    ed.changed();
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
    /// Its coast traced with the pointer ([`IslandSpec::outline`]).
    Draw,
}

/// The island builder's drag: the ground point pressed (a plain click builds there), the pointer there (window px),
/// the pointer on the level of the island's top (the first where it was pressed; sizing keeps only the latest after it),
/// whether it went past a click's wobble.
struct IslandDrag {
    start: [f32; 3],
    cursor: Vec2,
    points: Vec<[f32; 2]>,
    moved: bool,
}

/// The island builder's settings: shape, across (game units), height above the ground clicked, cliff depth, edge,
/// texture; `seed` varies the blob; `outline` the drawn coast (offsets from the centre, Draw).
#[derive(Clone, Debug)]
struct IslandSpec {
    outline: Vec<[f32; 2]>,
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

/// The terrain tools (the strip's Islands, Shape, Edges, Sculpt, Paths, Rails).
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
            Shape::Round | Shape::Draw => r,
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
            Shape::Draw => outline_land(&self.outline, x, y),
            _ => self.reach(y.atan2(x)) - d,
        }
    }
    /// The grid spacing for its size (at most 128 cells across).
    fn spacing(&self) -> f32 { if self.size <= 60.0 { 1.0 } else if self.size <= 240.0 { 2.0 } else { 4.0 } }
}

/// The distance from a drawn coast (a closed outline), above 0 inside it.
fn outline_land(pts: &[[f32; 2]], x: f32, y: f32) -> f32 {
    let (mut inside, mut best) = (false, f32::MAX);
    for k in 0..pts.len() {
        let (a, b) = (pts[k], pts[(k + 1) % pts.len()]);
        if (a[1] > y) != (b[1] > y) && x < a[0] + (y - a[1]) / (b[1] - a[1]) * (b[0] - a[0]) { inside = !inside; }
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let t = (((x - a[0]) * dx + (y - a[1]) * dy) / (dx * dx + dy * dy).max(1e-6)).clamp(0.0, 1.0);
        best = best.min((x - a[0] - t * dx).hypot(y - a[1] - t * dy));
    }
    if inside { best } else { -best }
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
    /// The island builder's drag going on (sizing the shape, or drawing its coast).
    island_drag: Option<IslandDrag>,
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
    /// What the last save found wrong (the warning sign by Save lists it).
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
    /// The World tool's planet preview: the look it shows (redrawn when the look changes).
    planet_tex: Option<(rc_map::planet::Planet, egui::TextureId, Handle<Image>)>,
    /// The World tab's song list: the planet whose songs show.
    song_planet: Option<u32>,
    /// The thumbnail: its camera to be remembered (Set thumbnail), a retake asked for (a save), and the camera to put
    /// back after a retake (transform, yaw, pitch).
    thumb_record: bool,
    thumb_retake: bool,
    thumb_restore: Option<(Transform, f32, f32)>,
    /// The top bar's menus open: the views, the warnings.
    view_menu: bool,
    warn_menu: bool,
    /// A point light being dragged (World or Select tool), the start being dragged (Select tool).
    light_drag: Option<usize>,
    start_drag: bool,
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

/// The maps folder, the same rule as the launcher's RAC Maker tile (rerac-launcher `maker.rs`): `RERAC_MAPS_DIR` when
/// set; else `RAC Maker` in the Documents folder Windows names (it may be moved, `D:\Documenten`), unless only the
/// older `%USERPROFILE%\Documents\RAC Maker` exists (the maps made before), which is then kept.
fn maps_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("RERAC_MAPS_DIR").filter(|d| !d.is_empty()) { return PathBuf::from(d); }
    let legacy = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Documents").join("RAC Maker");
    let Some(known) = known_folder(&FOLDERID_DOCUMENTS).map(|d| d.join("RAC Maker")) else { return legacy };
    if !known.exists() && legacy.exists() { legacy } else { known }
}

/// A known folder's id (`KNOWNFOLDERID`).
#[repr(C)]
struct Guid(u32, u16, u16, [u8; 8]);
const FOLDERID_DOCUMENTS: Guid = Guid(0xfdd3_9ad0, 0x238f, 0x46af, [0xad, 0xb4, 0x6c, 0x85, 0x48, 0x03, 0x69, 0xc7]);
const FOLDERID_DOWNLOADS: Guid = Guid(0x374d_e290, 0x123f, 0x4565, [0x91, 0x64, 0x39, 0xc4, 0x92, 0x5e, 0x46, 0x7b]);
const FOLDERID_DESKTOP: Guid = Guid(0xb4bf_cc3a, 0xdb2c, 0x424c, [0xb0, 0x29, 0x7f, 0xe9, 0x9a, 0x87, 0xc6, 0x41]);

/// A known folder where Windows says it is (`SHGetKnownFolderPath`: it may be moved, `D:\Documenten`).
#[cfg(windows)]
fn known_folder(id: &Guid) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    #[link(name = "shell32")]
    unsafe extern "system" { fn SHGetKnownFolderPath(id: *const Guid, flags: u32, token: *mut std::ffi::c_void, path: *mut *mut u16) -> i32; }
    #[link(name = "ole32")]
    unsafe extern "system" { fn CoTaskMemFree(p: *mut std::ffi::c_void); }
    let mut p: *mut u16 = std::ptr::null_mut();
    // SAFETY: the call writes a CoTaskMem string (or null) to `p`; it is read up to its NUL and freed once.
    unsafe {
        let ok = SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut p) == 0 && !p.is_null();
        let out = ok.then(|| {
            let n = (0..).take_while(|&k| *p.add(k) != 0).count();
            PathBuf::from(std::ffi::OsString::from_wide(std::slice::from_raw_parts(p, n)))
        });
        if !p.is_null() { CoTaskMemFree(p.cast()); }
        out
    }
}

#[cfg(not(windows))]
fn known_folder(id: &Guid) -> Option<PathBuf> {
    let name = if std::ptr::eq(id, &FOLDERID_DOWNLOADS) { "Downloads" } else if std::ptr::eq(id, &FOLDERID_DESKTOP) { "Desktop" } else { "Documents" };
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(name))
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
const HELP: [(&str, &[&str]); 13] = [
    ("Start here", &[
        "The editor is the game itself, paused: what you see is what you will play.",
        "Left edge: the tools (keys 1 to 0). Select, Objects, Islands, Shape, Edges, Sculpt, Paths, Rails, Logic, World.",
        "Left panel: the tool's own things under its name: the objects to place, the brushes, the zone kinds, the sky.",
        "Right panel: whatever you selected, in any tool. With nothing selected it shows the map: start, death height, ship, loadout, floors. The Map chip (or Esc) goes back to it.",
        "Top bar: New, Save, Undo, Redo, Play, the map's picture, the collision view; on the right the radio, the views, the fly speed and this help (F1).",
        "Bottom bar: what the mouse and keys do in the tool you hold.",
        "Hover anything for its name and key. Every change shows at once and the level reloads in the background.",
        "The main menu (RAC MAKER, top left) lists your maps with their pictures, versions and backups.",
    ]),
    ("Camera", &[
        "Hold the right mouse button and move the mouse to look around.",
        "W A S D fly, Space goes up, Q goes down. Hold Shift to fly five times faster.",
        "The mouse wheel over the world changes the fly speed; the number in the top bar shows it, a click on it puts it back to 16.",
        "The eye in the top bar: look at the selection from the top, the front, the back, a side or at an angle.",
        "Double click an object in Select's list to look at it.",
    ]),
    ("Select", &[
        "Select (1 or Esc): click anything to change it on the right: an object, an island, a path, a lamp.",
        "On this map (left) lists everything by kind: objects, islands, paths, rails, zones, lights. Click one to select it.",
        "Drag the blue ring (the start) to move where Ratchet starts. With nothing selected, the red grid shows the death height.",
        "Delete (or the bin on the right) deletes what is selected; Ctrl+Z brings it back.",
    ]),
    ("Objects", &[
        "Categories: Enemies, Bosses, Creatures, Characters, Machines, Crates, Pickups, Gadget points, Platforms & lifts, Doors & gates, Switches & triggers, Hazards, Breakables, Vehicles, Scenery, Effects. Search finds names, categories and moby ids.",
        "An object's panel shows its category and its file (rac1/L01/moby/729). Amber notes say what it needs on a map: its partner (a switch, a crank), its planet's route, a group, or that it is a story object.",
        "Breakables (a wall that the Bomb Glove breaks…): Hides puts a gold bolt or a crate behind it, hidden until it breaks.",
        "Objects that move, break or have their own collision are never baked solid: what breaks is gone, collision and all.",
        "Objects (2): pick a type, then an object, then click in the world. The green box shows where it lands: on the ground, on top of what you point at, or next to it.",
        "Planet: show one planet's objects (its globe). Live only hides objects that do not work away from their home planet yet.",
        "The dot on a picture: green works, yellow needs work, red does not work on maps yet, grey is scenery.",
        "R / Shift+R turns it before you place it. Esc stops placing.",
        "Selected object (right): Place (position, facing), Colour (tint), Rules (a name for the rules, hidden until a rule shows it), Body (solid), Copy beside it (rows and stacks).",
        "Drag a selected object to slide it along the ground; drag its arrows to move it on one axis, the yellow ring to turn it.",
        "Crates that touch are linked: break the bottom one and the ones above fall.",
        "\"Starts at the red ball\": the game moves that object when the level loads. Place it at the end of the red line, or pick another.",
    ]),
    ("Islands & terrain", &[
        "Islands (3): pick a shape, its size and height, its ground and its edge, then click: the green outline shows the island a click builds. Drag instead of clicking to size it from where you press.",
        "Draw (Islands): hold the left button and trace the coast in the world, let go: the island has the shape you drew.",
        "Shape (4): select an island, drag the white dots on its coast (the wheel over a dot: how much follows), the green diamond to move it, the blue arrow to raise it. Ctrl snaps.",
        "Edges (5): click a stretch of an island's railing or wall to take it out, a red one to put it back.",
        "Sculpt (6): brushes: Raise, Lower, Plateau, Smooth, Flatten, Paint, Land, Shade, Decor, Gaps. Shift (or Other way): the opposite.",
        "Selected island (right): Ground (texture), Colour, Edge (none, invisible, railings, walls), Surface (ground, slippery, hot, deadly, magnetic), Size and place, Roof.",
        "Texture and plant grids show one planet at a time: step through the planets with the arrows by its globe.",
        "Colour: tint one island, or change its texture's colours everywhere it is used.",
        "Islands pushed into each other lose their railing where they meet; your own openings are kept.",
    ]),
    ("Paths & rails", &[
        "Paths (7): click where a walkway starts and where it bends, Enter when done. Railings it crosses open by themselves.",
        "Drag a point to move it; the wheel over it raises or lowers it; Delete over it takes it out.",
        "The left panel holds what the next path gets; a selected path's settings are on the right (size, ground, surface, edge, colour).",
        "Stop where it meets land: a bridge only over the air, nothing poking through the islands.",
        "Rails (8): grind rails, drawn like paths. Look: a planet's rail or plain. Smooth curves it; One way: only from its first point.",
        "Pick a rail point (Points, on the right) to move it with its arrows. Grind rails must be on for the map (Play, map settings).",
    ]),
    ("Logic", &[
        "Logic (9): pick a zone kind (Checkpoint, Goal, Death, Camera, Trigger), then click in the world. Click a zone to select it, drag it to move it.",
        "Checkpoint: after a death Ratchet comes back to the last one he walked into. Goal: walking in finishes the map with your time.",
        "Death: walking in costs a life. Camera: inside it the camera watches Ratchet from its purple ball (Use this view sets it).",
        "A zone's kind, name, size and turn are on the right.",
        "Rules: When (Ratchet enters a zone / all enemies in a zone are beaten / an object is destroyed) then (remove or show an object, a message, a checkpoint, the goal). Each rule runs once.",
        "Give objects a name (Rules, on the right) to use them in rules. Hidden until a rule shows it: an ambush, a bridge that appears.",
        "Show it with the camera: the camera turns to the object, it happens, then the view cuts back.",
        "In a camera zone the stick follows the zone's camera: push it towards where you want Ratchet to go on screen.",
    ]),
    ("Enemies", &[
        "Select an enemy: the blue circle is where it notices Ratchet, the red one where it attacks (Sight, on the right).",
        "Robot troopers and the Veldin tank take a path (Path, on the right): Square patrol or 8 ahead, then drag the points.",
        "\"May vanish on maps\": that enemy still uses its home planet's data and can disappear or wander off. Try it with Play first.",
        "Ammo crates refill the weapons in the loadout (map settings): tick a weapon, or the crate gives nothing.",
    ]),
    ("World", &[
        "World (0): Themes (a whole look in one click), Planet, Sky, Fog, Sunlight, Colours, Lamps, Song.",
        "Planet: the planet Ratchet's ship flies to when you press Play, with your map's name under it. Start from a look, then its pattern, colours, glow, clouds and ring; New world gives another layout. Fly here on Play off: play starts at once.",
        "Sky: another planet's sky; raise and turn move it; Stars and shooting stars and Clouds drift switch its motion; Sky layers hide or tint its parts.",
        "Fog: a colour the distance fades into. Sunlight: the sun's direction, height and colour, the light in the shade, darker hollows.",
        "Colours: the whole map's hue, saturation, brightness and tint (Objects too: also the enemies and pickups). One texture can be changed everywhere it is used.",
        "Lamps: + Lamp where the camera looks; click a lamp's ball in the world to select it and drag it; its colour, reach and strength are on the right.",
        "Changes show at once; the sky and the objects' colours change when you let go of the slider.",
        "Song: Use picks what the map plays; ▶ listens (the radio, also in the top bar), ■ stops.",
    ]),
    ("Playing", &[
        "Play (F5) starts at the blue ring with the loadout ticked in the map settings. F5 again stops: everything comes back.",
        "Play first flies Ratchet's ship through space to your planet (World > Planet), then lands; any button skips the flight.",
        "Start (map settings): on foot, or Ship: it lands and Ratchet steps out; after the goal he walks back to it and presses △ to fly off and finish.",
        "With a ship, Ratchet starts by its hatch: make sure there is ground there and a way to the ship.",
        "Reaching a Goal shows your time; a few seconds later you are back in the editor.",
        "Collision (F6, also while playing): what Ratchet stands on and bumps into.",
    ]),
    ("Files & backups", &[
        "Ctrl+S saves. A dot after the map's name means unsaved changes.",
        "The warning sign by Save: things the last save found to check (holes, a start over nothing, a too-high death height).",
        "Every save keeps a backup; unsaved work is backed up every 5 minutes. Open them from the main menu (click a map).",
        "Main menu: Save version keeps a named copy that is never pruned. Drag a card to order your maps.",
        "Share (main menu, a map picked): one .racmap file with its picture, to send to someone. Import… finds the ones in Downloads and on the Desktop, or Browse… for any file. The ReRAC Launcher's RAC Maker tile imports them too.",
        "The camera button: the map's picture (what the camera sees). The circle arrow next to it: every save takes it again from that spot.",
    ]),
    ("Undo", &[
        "Ctrl+Z undoes, Ctrl+Y redoes (also the arrows in the top bar): 200 steps.",
        "A drag or a slider is one step. Deleting is never final until you save over it, and even then the backup keeps it.",
    ]),
    ("All keys", &[
        "1 to 0: the tools. Esc: stop placing, let go of the selection, back to Select.",
        "Right mouse + move: look around. W A S D, Space, Q: fly. Shift: faster. Wheel: fly speed.",
        "Left click: select, place, build, brush (the tool's). R / Shift+R: turn. Delete: delete.",
        "Enter: finish a path or rail. Backspace: its last point off.",
        "Ctrl+Z / Ctrl+Y: undo / redo. Ctrl+S: save. F5: play / stop. F6: collision. F1: help.",
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
            c.entries.iter().enumerate().filter(|(_, e)| e.id.kind == AssetKind::Moby && !e.thumb.is_empty() && e.group != GROUP_GEAR && e.behaviour.is_some() && !crate::map_load::NOT_PLACEABLE.contains(&e.id.number) && !objects::info(e.id.number).is_some_and(|i| i.has(objects::HIDE))).map(|(i, _)| i).collect()
        });
        if catalog.is_none() { eprintln!("editor: no catalog at {}: run --build-catalog for the asset browser", root.join("catalog").display()); }
        let saved = path.exists();
        let mut settings = Settings::load();
        settings.opened(&path);
        let ids: Vec<u64> = (0..def.objects.len() as u64).collect();
        let loaded = Snap { def: (*def).clone(), ids: ids.clone() };
        // RC_EDITOR_TAB=select|objects|islands|shape|edges|sculpt|paths|rails|logic|world (tests and screenshots): straight into that tab, the menu closed.
        let start_tab = std::env::var("RC_EDITOR_TAB").ok();
        // A strip tool by its caption ("islands", "sculpt", "rails"…; "terrain" is Islands).
        let tab = start_tab.as_deref().map(|t| if t == "terrain" { "islands".to_string() } else { t.to_lowercase() });
        let (tool, terrain_mode) = STRIP.iter().find(|t| Some(t.1.to_lowercase()) == tab).map_or((Tool::Select, TerrainMode::Select), |t| (t.3, t.4));
        // RC_EDITOR_PICK=object:N|island:N|path:N|rail:N|zone:N|light:N (tests and screenshots): selected at the start.
        let pick = std::env::var("RC_EDITOR_PICK").ok().and_then(|p| p.split_once(':').and_then(|(k, n)| n.parse::<usize>().ok().map(|n| (k.to_string(), n))));
        let picked = |kind: &str| pick.as_ref().filter(|p| p.0 == kind).map(|p| p.1);
        let mut main_menu = menu::MainMenu::opened();
        main_menu.open = start_tab.is_none();
        app.add_plugins(EguiPlugin::default())
            .insert_resource(EguiGlobalSettings { auto_create_primary_context: false, ..default() })
            .insert_resource(EditorMode::Editing)
            .insert_resource(UiFocus { editing: true, ..default() })
            .insert_resource(Editor {
                def: (*def).clone(), next_id: ids.len() as u64, ids, path, loaded, loading: None, undo: Vec::new(), redo: Vec::new(), saved,
                selected: picked("object"), armed: None, drag: None, drag_start: None, reload: Reload::Idle, reload_again: false, play_pending: false,
                coalescing: false, status: String::new(),
                catalog, placeable, filter: String::new(), planet: None, kind: None, live_only: true, thumbs: HashMap::new(),
                placed_by_level: HashMap::new(), camera_set: false, fps: 0.0,
                tool, brush: Brush::Raise, radius: 4.0, strength: 1.0, patch: picked("island"), stroke: None, brush_at: None,
                place_yaw: 0.0, terrain_mode, island_at: None, island_drag: None, reach: 8.0, shape_hover: None, shape_drag: None, gap_hover: None, gap_paint: None, last_built: None, edge_walls: false,
                shade: SHADES[0].1, scatter: None, scatter_size: 1.0, seed: 0x2545_f491,
                walk: picked("path"), walk_drawing: false, walk_first: None, walk_spec: rc_map::walkway::Walkway { cut: true, ..rc_map::walkway::Walkway::new(vec![[0.0; 3], [1.0, 0.0, 0.0]], 4.0) }, walk_lift: 0.0,
                walk_hover: None, walk_drag: None, walk_at: None,
                rail: picked("rail"), rail_drawing: false, rail_first: None, rail_lift: 1.0, rail_smooth: true, rail_hover: None, rail_drag: None, rail_point: None, rail_axis: None, rail_at: None, show_collision: false, collision_view: None, warnings: Vec::new(),
                menu: main_menu, thumb_shot: None, zone: picked("zone"), zone_new: None, zone_drag: None, zone_ghost: None, help_query: String::new(), view_snap: None, radio: None, radio_menu: false, radio_want: None, rail_look: Some("kerwan".into()), globes: HashMap::new(), planet_tex: None, song_planet: None, light_add: false, light_sel: picked("light"), sky_shells: HashMap::new(), thumb_record: false, thumb_retake: false, thumb_restore: None, view_menu: false, warn_menu: false, light_drag: None, start_drag: false,
                island: IslandSpec { outline: Vec::new(), shape: Shape::Round, size: 24.0, height: 8.0, depth: 8.0, edge: None, texture: Some(AssetId { game: rc_map::Game::Rac1, level: 1, kind: AssetKind::TfragTexture, number: 8 }), seed: 1 }, square: false, invert: false, plateau: 3.0, paint: None, help: (!settings.help_seen || std::env::var_os("RC_EDITOR_HELP").is_some()).then_some(0), settings,
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

/// The window closing with unsaved changes: they are kept as a backup (the main menu opens it).
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

/// A planet picture as an egui texture.
fn planet_image(contexts: &mut EguiContexts, images: &mut Assets<Image>, p: &rc_map::planet::Planet, size: usize) -> egui::TextureId {
    let img = Image::new(
        bevy::render::render_resource::Extent3d { width: size as u32, height: size as u32, depth_or_array_layers: 1 },
        bevy::render::render_resource::TextureDimension::D2,
        p.render(size),
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    contexts.add_image(EguiTextureHandle::Strong(images.add(img)))
}

/// The World tool's Planet card: the planet the flight to the map shows (its preview, a starting look, its pattern,
/// colours, clouds and ring), and whether Play starts with that flight.
fn planet_card(ui: &mut egui::Ui, w: &mut rc_map::World, ed: &mut Editor, contexts: &mut EguiContexts, images: &mut Assets<Image>) {
    use rc_map::planet::{Pattern, Planet, PRESETS};
    let mut flight = !w.no_flight;
    if ui.checkbox(&mut flight, "Fly here on Play").on_hover_text("Play starts with Ratchet's ship flying through space to this planet, then landing (any button skips the flight)").changed() {
        w.no_flight = !flight;
    }
    let mut p = w.planet.unwrap_or_default();
    // The preview, redrawn when the look changes.
    if ed.planet_tex.as_ref().is_none_or(|t| t.0 != p) {
        // The old picture let go (a slider dragged makes one a frame).
        if let Some((_, _, old)) = ed.planet_tex.take() {
            contexts.remove_image(&old);
            images.remove(&old);
        }
        let img = Image::new(
            bevy::render::render_resource::Extent3d { width: 160, height: 160, depth_or_array_layers: 1 },
            bevy::render::render_resource::TextureDimension::D2,
            p.render(160),
            bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::RENDER_WORLD,
        );
        let handle = images.add(img);
        let id = contexts.add_image(EguiTextureHandle::Strong(handle.clone()));
        ed.planet_tex = Some((p, id, handle));
    }
    if let Some((_, id, _)) = ed.planet_tex {
        ui.vertical_centered(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(150.0, 150.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 10.0, egui::Color32::from_rgb(8, 10, 28));
            ui.painter().image(id, rect.shrink(4.0), egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
            ui.label(egui::RichText::new(&ed.def.name).color(DIM).small());
        });
    }
    // Starting looks.
    let texs: Vec<Option<egui::TextureId>> = (0..PRESETS.len()).map(|k| {
        let key = 300 + k as u32;
        if let Some(t) = ed.globes.get(&key) { return *t; }
        let id = Some(planet_image(contexts, images, &PRESETS[k].1, 64));
        ed.globes.insert(key, id);
        id
    }).collect();
    tile_rows(ui, PRESETS.len(), 3, |ui, k, size| {
        if picture_tile(ui, texs[k], PRESETS[k].0, &format!("Start from a {} planet", PRESETS[k].0.to_lowercase()), false, size) {
            p = Planet { seed: p.seed, ..PRESETS[k].1 };
        }
    });
    // Its pattern.
    ui.horizontal_wrapped(|ui| {
        for (pat, name, hint) in [(Pattern::Continents, "Land & sea", "Seas, land and high ground, ice at the poles"), (Pattern::Bands, "Bands", "Stripes round it, like a gas giant"), (Pattern::Craters, "Craters", "Dry ground full of craters"), (Pattern::Swirls, "Swirls", "Colours stirred together: toxic or molten")] {
            if ui.selectable_label(p.pattern == pat, name).on_hover_text(hint).clicked() { p.pattern = pat; }
        }
    });
    ui.horizontal(|ui| {
        for (k, hint) in ["Low ground: seas, the darkest bands", "Land", "High ground: peaks, ice"].into_iter().enumerate() {
            ui.color_edit_button_srgb(&mut p.colours[k]).on_hover_text(hint);
        }
        ui.separator();
        ui.color_edit_button_srgb(&mut p.glow).on_hover_text("The air's glow round its edge");
        ui.label(egui::RichText::new("glow").small().color(DIM));
    });
    ui.add(egui::Slider::new(&mut p.clouds, 0.0..=1.0).text("clouds"));
    ui.horizontal(|ui| {
        ui.add(egui::Slider::new(&mut p.ring, 0.0..=1.0).text("ring"));
        if p.ring > 0.0 { ui.color_edit_button_srgb(&mut p.ring_colour).on_hover_text("The ring's colour"); }
    });
    if ui.button("New world").on_hover_text("Another layout of land and sea, the same colours").clicked() {
        p.seed = p.seed.wrapping_mul(1_103_515_245).wrapping_add(12_345) % 100_000;
    }
    if p != w.planet.unwrap_or_default() { w.planet = Some(p); }
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
    {
        ui.horizontal(|ui| {
            let mut on = tint.is_some();
            if ui.checkbox(&mut on, format!("Tint this {what}")).changed() {
                *tint = on.then_some([255, 220, 180]);
            }
            if let Some(c) = tint.as_mut() { ui.color_edit_button_srgb(c); }
        });
        let Some(id) = texture else { return };
        let k = ed.def.world.recolour.iter().position(|r| r.0 == id);
        let mut on = k.is_some();
        if ui.checkbox(&mut on, "Its texture, everywhere").on_hover_text("This texture's colours on every island and path").changed() {
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
    }
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
    let orange = ORANGE;
    let mut w = ed.def.world.clone();
    egui::ScrollArea::vertical().id_salt("world_tab").show(ui, |ui| {
        card(ui, "world_themes", "World", "Themes", true, |ui| {
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
        card(ui, "world_planet", "World", "Planet", true, |ui| planet_card(ui, &mut w, ed, contexts, images));
        ui.add_space(4.0);
        card(ui, "world_sky", "View", "Sky", true, |ui| {
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
        card(ui, "world_fog", "Invisible", "Fog", false, |ui| {
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
        card(ui, "world_light", "Light", "Sunlight", false, |ui| {
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
        card(ui, "world_colours", "Shade", "Colours", false, |ui| {
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
                label_small(ui, "One texture, everywhere it is used");
                for id in used {
                    let k = w.recolour.iter().position(|r| r.0 == id);
                    let tex = ed.entry(&id).map(|e| e.thumb.clone()).and_then(|t| thumb(ed, contexts, images, &t, &mut 4));
                    ui.horizontal(|ui| {
                        let mut on = k.is_some();
                        if ui.checkbox(&mut on, "").on_hover_text(id.to_string()).changed() {
                            if on { w.recolour.push((id, rc_map::Palette::default())); } else if let Some(k) = k { w.recolour.remove(k); }
                        }
                        if let Some(t) = tex { ui.add(egui::Image::new((t, egui::vec2(36.0, 36.0))).corner_radius(4)); }
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
        card(ui, "world_lights", "Light", "Lamps", false, |ui| {
            ui.label(egui::RichText::new("Point lights: they light your ground and the objects near them (a lamp, lava glow, a crystal).").weak().small());
            if ui.button("+ Lamp where the camera looks").clicked() { ed.light_add = true; }
            for (k, l) in w.lights.iter().enumerate() {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 6.0, egui::Color32::from_rgb(l.colour[0], l.colour[1], l.colour[2]));
                    if ui.selectable_label(ed.light_sel == Some(k), format!("Light {}", k + 1)).clicked() { ed.light_sel = Some(k); }
                });
            }
        });
        ui.add_space(4.0);
        card(ui, "world_song", "Song", "Song", true, |ui| {
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
        // The planet and the flight only matter on Play: no reload for them.
        let flight_only = rc_map::World { planet: w.planet, no_flight: w.no_flight, ..ed.def.world.clone() } == w;
        ed.def.world = w;
        // Live: the map's light, colours, lights and fog redrawn now; the sky and the objects' colours come with the
        // reload when the slider is let go.
        crate::map_load::set(ed.def.clone());
        if flight_only { ed.saved = false; } else { ed.changed(); }
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
            ed.status = "Lamp placed where the camera looks: its settings are on the right".into();
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
    /// After an undo or redo: a selection whose list changed length (a thing added or taken out) may point at another
    /// one now, so it goes; the rest stay selected.
    fn keep_selection(&mut self, before: [usize; 5]) {
        let d = &self.def;
        if d.patches.len() != before[0] { (self.patch, self.shape_drag) = (None, None); }
        if d.walkways.len() != before[1] { (self.walk, self.walk_drawing, self.walk_first) = (None, false, None); }
        if d.rails.len() != before[2] { (self.rail, self.rail_point, self.rail_drawing, self.rail_first) = (None, None, false, None); }
        if d.zones.len() != before[3] { self.zone = None; }
        if d.world.lights.len() != before[4] { self.light_sel = None; }
    }
    fn counts(&self) -> [usize; 5] { [self.def.patches.len(), self.def.walkways.len(), self.def.rails.len(), self.def.zones.len(), self.def.world.lights.len()] }
    fn undo(&mut self) {
        if let Some(s) = self.undo.pop() {
            let before = self.counts();
            let now = self.restore(s);
            self.redo.push(now);
            self.keep_selection(before);
            self.selected = None;
            self.changed();
            self.status = "Undone".into();
        }
    }
    fn redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            let before = self.counts();
            let now = self.restore(s);
            self.undo.push(now);
            self.keep_selection(before);
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
        (self.armed, self.floor_edit) = (None, None);
        clear_selection(self);
        // The other map's warnings and anything half dragged go with it.
        self.warnings.clear();
        self.warn_menu = false;
        (self.drag, self.gizmo, self.island_drag, self.shape_drag, self.gap_paint, self.walk_drag, self.rail_drag, self.rail_axis, self.zone_drag, self.light_drag, self.start_drag) =
            (None, None, None, None, None, None, None, None, None, None, false);
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
                format!("Saved, with {} things to check (the sign by Save)", self.warnings.len())
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
        if let Some(i) = objects::info(id.number).filter(|_| id.kind == AssetKind::Moby) { return i.name.to_string(); }
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
        // Paint with nothing picked would wear every paint away; it waits for a texture (Shift still rubs out).
        if brush == Brush::Paint && paint.is_none() && !invert {
            self.status = "Pick a texture to paint with (the Paint with card)".into();
            return;
        }
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
    /// The island builder's drag let go: a click builds the island as set; a drag sizes it from its centre, or (Draw)
    /// the coast traced is the island's.
    fn finish_island(&mut self, d: IslandDrag) {
        let mut spec = self.island.clone();
        if !d.moved {
            if spec.shape == Shape::Draw {
                self.status = "Draw: hold the left button and trace the island's coast, let go to build it".into();
                return;
            }
            return self.build_island(d.start, spec);
        }
        if spec.shape != Shape::Draw {
            let (c, end) = (d.points[0], *d.points.last().unwrap_or(&d.points[0]));
            spec.size = (2.0 * (end[0] - c[0]).hypot(end[1] - c[1])).clamp(6.0, 400.0);
            self.island.size = spec.size;
            return self.build_island([c[0], c[1], d.start[2]], spec);
        }
        // The coast: its centre the middle of what was drawn.
        let (lo, hi) = d.points.iter().fold(([f32::MAX; 2], [f32::MIN; 2]), |(lo, hi), p| ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])]));
        let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
        spec.outline = d.points.iter().map(|p| [p[0] - c[0], p[1] - c[1]]).collect();
        let area = spec.outline.iter().zip(spec.outline.iter().cycle().skip(1)).map(|(a, b)| a[0] * b[1] - b[0] * a[1]).sum::<f32>().abs() / 2.0;
        let reach = spec.outline.iter().map(|p| p[0].hypot(p[1])).fold(0.0, f32::max);
        if spec.outline.len() < 3 || area < 9.0 {
            self.status = "Too small to be an island: trace a bigger coast".into();
            return;
        }
        spec.size = (2.0 * reach + 2.0).max(6.0);
        if spec.size > 500.0 {
            self.status = format!("Too big: {:.0} units across, an island is at most 500", spec.size);
            return;
        }
        self.build_island([c[0], c[1], d.start[2]], spec);
    }
    /// Builds island `spec` centred on ground point `at`: its top `height` above it, selected.
    fn build_island(&mut self, at: [f32; 3], spec: IslandSpec) {
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
            // Only still things without collision of their own (a class with its own keeps it as it moves or breaks).
            let solid = !crate::map_load::PATH_CLASSES.contains(&id.number)
                && !objects::info(id.number).is_some_and(|i| i.has(objects::NEVER_SOLID) || i.has(objects::MOVES_ITSELF))
                && self.entry(&id).is_some_and(|e| {
                    let name = e.name.to_lowercase();
                    (!e.collision || crate::map_load::BAKE_ANYWAY.contains(&id.number))
                        && [rc_map::catalog::KIND_SCENERY, rc_map::catalog::KIND_PLATFORM, rc_map::catalog::KIND_OTHER].contains(&e.kind.as_str())
                        && !MOVES.iter().any(|w| name.contains(w))
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
        // A name is one object's (the rules find it by name).
        copy.name = None;
        let from = copy.position;
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
        // A mover's route goes along with it.
        let off = [copy.position[0] - from[0], copy.position[1] - from[1], copy.position[2] - from[2]];
        for q in &mut copy.path { for k in 0..3 { q[k] += off[k]; } }
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

/// The lamp whose ball the ray passes through (the nearest).
fn pick_light(def: &MapDef, (o, d): ([f32; 3], [f32; 3])) -> Option<usize> {
    def.world.lights.iter().enumerate().filter_map(|(k, l)| {
        let v = [l.position[0] - o[0], l.position[1] - o[1], l.position[2] - o[2]];
        let t = v[0] * d[0] + v[1] * d[1] + v[2] * d[2];
        let c = [o[0] + d[0] * t - l.position[0], o[1] + d[1] * t - l.position[1], o[2] + d[2] * t - l.position[2]];
        // The ball, a little wider far away (as the gizmos: about the same size on screen).
        let r = (t * 0.025).max(0.8);
        (t > 0.0 && c[0] * c[0] + c[1] * c[1] + c[2] * c[2] < r * r).then_some((k, t))
    }).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(k, _)| k)
}

/// Whether the ray passes over the start's ring.
fn near_start(def: &MapDef, ray: ([f32; 3], [f32; 3])) -> bool {
    let s = def.start.position;
    on_plane(ray, s[2] - 0.45).is_some_and(|h| (h[0] - s[0]).powi(2) + (h[1] - s[1]).powi(2) < 1.3 * 1.3)
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
        // The island builder: where the pointer meets the ground (else the start's height): a click builds it there, a
        // drag sizes it (or draws its coast), on the level of its top over the ground pressed.
        let at = terrain_hit(&ed.def, ray).map(|h| h.1).or_else(|| on_plane(ray, ed.def.start.position[2] - 0.5));
        ed.brush_at = None;
        let cursor = window.cursor_position();
        if let Some(mut d) = ed.island_drag.take() {
            if cursor.is_some_and(|c| c.distance(d.cursor) > 6.0) { d.moved = true; }
            // On the island's top, where the outline is drawn: it stays under the pointer.
            if let Some(h) = on_plane(ray, d.start[2] + ed.island.height.max(0.25)) {
                let last = *d.points.last().unwrap_or(&[d.start[0], d.start[1]]);
                let step = if ed.island.shape == Shape::Draw { 0.75 } else { 0.0 };
                if (h[0] - last[0]).hypot(h[1] - last[1]) > step {
                    // Sizing keeps the centre and the latest point; drawing all of them.
                    if ed.island.shape != Shape::Draw { d.points.truncate(1); }
                    d.points.push([h[0], h[1]]);
                }
            }
            ed.island_at = None;
            if buttons.pressed(MouseButton::Left) { ed.island_drag = Some(d); } else { ed.finish_island(d); }
            return;
        }
        ed.island_at = at.filter(|_| !over_ui);
        if buttons.just_pressed(MouseButton::Left) && !over_ui {
            if let Some(at) = at {
                let top = on_plane(ray, at[2] + ed.island.height.max(0.25)).unwrap_or(at);
                ed.island_drag = Some(IslandDrag { start: at, cursor: cursor.unwrap_or_default(), points: vec![[top[0], top[1]]], moved: false });
            }
        }
        return;
    }
    ed.island_at = None;
    ed.island_drag = None;
    if ed.tool == Tool::Terrain && ed.terrain_mode == TerrainMode::Sculpt {
        let hit = terrain_hit(&ed.def, ray);
        ed.brush_at = hit.filter(|h| h.2.is_some()).map(|h| h.1);
        if buttons.just_pressed(MouseButton::Left) && !over_ui {
            // The stroke leaves the selection alone (else Delete would take the island brushed).
            if let Some((_, at, Some(_))) = hit {
                ed.checkpoint();
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
                n if crate::map_load::MOVERS.contains(&n) => vec![p, [p[0], p[1], p[2] + 8.0]],
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
        } else if let Some(k) = matches!(ed.tool, Tool::World | Tool::Select).then(|| pick_light(&ed.def, ray)).flatten().filter(|&k| {
            // Only a lamp in front of the object under the pointer (one behind it is not what was clicked).
            let l = ed.def.world.lights[k].position;
            let (o, d) = ray;
            let t = (l[0] - o[0]) * d[0] + (l[1] - o[1]) * d[1] + (l[2] - o[2]) * d[2];
            ed.tool == Tool::World || pick(&ed, ray).and_then(|j| ray_box(ray, &ed.box_of(j))).is_none_or(|h| t < h.0)
        }) {
            // A lamp: picked, and dragged across at its height.
            clear_selection(&mut ed);
            ed.light_sel = Some(k);
            ed.light_drag = Some(k);
            ed.drag_start = Some(ed.snap());
        } else if ed.tool == Tool::World {
            // World picks lamps only: a miss lets the selection go (it never grabs an object or an island).
            clear_selection(&mut ed);
        } else if ed.tool == Tool::Select && pick(&ed, ray).is_none() && near_start(&ed.def, ray) {
            // The start (or the ship's landing): dragged over the ground.
            clear_selection(&mut ed);
            ed.start_drag = true;
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
                        ed.status = format!("Island {} selected", k + 1);
                    }
                    Some((_, h, None)) => {
                        ed.walk = ed.def.walkways.iter().position(|w| w.height_at(h[0], h[1]).is_some_and(|z| (z - h[2]).abs() < 0.25));
                        ed.patch = None;
                        if let Some(k) = ed.walk { ed.status = format!("Path {} selected", k + 1); }
                    }
                    None => clear_selection(&mut ed),
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
        if let Some(k) = ed.light_drag.filter(|&k| k < ed.def.world.lights.len()) {
            let z = ed.def.world.lights[k].position[2];
            if let Some(h) = on_plane(ray, z) { ed.def.world.lights[k].position = [(h[0] * 4.0).round() / 4.0, (h[1] * 4.0).round() / 4.0, z]; }
        }
        if ed.start_drag {
            if let Some(h) = terrain_hit(&ed.def, ray).map(|h| h.1) {
                let s0 = ed.def.start.position;
                let to = [(h[0] * 8.0).round() / 8.0, (h[1] * 8.0).round() / 8.0, (h[2] * 8.0).round() / 8.0 + 0.5];
                let d = [to[0] - s0[0], to[1] - s0[1], to[2] - s0[2]];
                ed.def.start.position = to;
                // The ship lands as far from Ratchet as it did.
                if let Some(sh) = ed.def.ship.as_mut() { for k in 0..3 { sh.position[k] += d[k]; } }
            }
        }
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
    if buttons.just_released(MouseButton::Left) && (ed.light_drag.is_some() || ed.start_drag) {
        let lamp = ed.light_drag.take().is_some();
        ed.start_drag = false;
        if let Some(start) = ed.drag_start.take() {
            if start.def != ed.def {
                ed.undo.push(start);
                ed.redo.clear();
                if lamp { crate::map_load::set(ed.def.clone()); }
                ed.status = if lamp { "Lamp moved".into() } else { "Start moved".into() };
                ed.changed();
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
        ed.status = format!("Opened {opened} stretches of railing or wall where the path crosses (Edges tool: click one to close it)");
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
                        ed.status = "Rail made: click for more bends, Enter when done (grinding is on for this map)".into();
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

/// The Rails tool's panel: how high new points go, Done, the next new rail's look (a rail picked in the world shows
/// its own settings on the right).
fn rails_panel(ui: &mut egui::Ui, ed: &mut Editor, contexts: &mut EguiContexts, images: &mut Assets<Image>) {
    rails_off(ui, ed);
    ui.add(egui::Slider::new(&mut ed.rail_lift, 0.0..=20.0).text("points above the ground"));
    if (ed.rail_drawing || ed.rail_first.is_some()) && ui.button("Done (Enter)").clicked() { ed.rail_drawing = false; ed.rail_first = None; }
    ui.add_space(6.0);
    label_small(ui, "The next new rail");
    let mut look = ed.rail_look.clone();
    card(ui, "new_rail_look", "Rails", "Look", true, |ui| rail_looks(ui, ed, contexts, images, &mut look));
    ed.rail_look = look;
    ui.checkbox(&mut ed.rail_smooth, "Smooth curve");
}

/// Grind rails off for the map: a warning and the switch.
fn rails_off(ui: &mut egui::Ui, ed: &mut Editor) {
    if ed.def.abilities.rails == Some(true) { return; }
    ui.horizontal(|ui| {
        icon(ui, "Warn", 18.0, AMBER);
        ui.label(egui::RichText::new("Grind rails are off").color(AMBER));
        if ui.button("Turn on").clicked() { ed.checkpoint(); ed.def.abilities.rails = Some(true); ed.changed(); }
    });
}

/// The rail looks as tiles (the planet each comes from), and Plain.
fn rail_looks(ui: &mut egui::Ui, ed: &mut Editor, contexts: &mut EguiContexts, images: &mut Assets<Image>, look: &mut Option<String>) {
    let texs: Vec<Option<egui::TextureId>> = crate::map_load::RAIL_LOOKS.iter().map(|l| globe_tex(ed, contexts, images, l.2)).collect();
    tile_rows(ui, crate::map_load::RAIL_LOOKS.len() + 1, 3, |ui, k, size| {
        if k == 0 {
            if picture_tile(ui, None, "Plain", "A plain orange beam", look.is_none(), size) { *look = None; }
            return;
        }
        let (key, name, ..) = crate::map_load::RAIL_LOOKS[k - 1];
        if picture_tile(ui, texs[k - 1], name, "That planet's grind rail, repeated along this one", look.as_deref() == Some(key), size) { *look = Some(key.to_string()); }
    });
}

/// The selected rail (the right panel): its look, how it rides, its points.
fn rail_inspector(ui: &mut egui::Ui, ed: &mut Editor, k: usize, contexts: &mut EguiContexts, images: &mut Assets<Image>) {
    if inspector_head(ui, ed, "Rails", ORANGE, None, &format!("Rail {}", k + 1)) {
        delete_selected(ed);
        return;
    }
    rails_off(ui, ed);
    let mut r = ed.def.rails[k].clone();
    card(ui, "rail_look", "Rails", "Look", true, |ui| rail_looks(ui, ed, contexts, images, &mut r.look));
    card(ui, "rail_ride", "Paths", "Ride", true, |ui| {
        ui.checkbox(&mut r.smooth, "Smooth curve");
        ui.checkbox(&mut r.one_way, "One way").on_hover_text("Only from its first point to its last");
    });
    card(ui, "rail_points", "Shape", &format!("Points  ·  {}", r.points.len()), false, |ui| {
        let mut drop = None;
        egui::Grid::new("rail_points").num_columns(5).striped(true).show(ui, |ui| {
            let n = r.points.len();
            for (i, q) in r.points.iter_mut().enumerate() {
                if ui.selectable_label(ed.rail_point == Some(i), format!("{}", i + 1)).on_hover_text("Its arrows show in the world").clicked() {
                    ed.rail_point = if ed.rail_point == Some(i) { None } else { Some(i) };
                }
                for (a, v) in q.iter_mut().enumerate() {
                    ui.add(egui::DragValue::new(v).speed(0.1).range(if a == 2 { 1.0..=1000.0 } else { 0.0..=1023.0 })).on_hover_text(["x", "y", "height"][a]);
                }
                if ui.add_enabled(n > 2, egui::Button::new("x").small()).on_hover_text("Take this point out").clicked() { drop = Some(i); }
                ui.end_row();
            }
        });
        if let Some(i) = drop {
            r.points.remove(i);
            ed.rail_point = None;
        }
    });
    for q in &mut r.points { for v in q.iter_mut() { *v = (*v * 64.0).round() / 64.0; } }
    if r != ed.def.rails[k] {
        if !ed.coalescing { ed.checkpoint(); }
        ed.coalescing = true;
        ed.def.rails[k] = r;
        ed.changed();
    }
}

/// The selected point light (the right panel): its colour, reach, strength and place.
fn light_inspector(ui: &mut egui::Ui, ed: &mut Editor, k: usize) {
    let mut l = ed.def.world.lights[k];
    let c = egui::Color32::from_rgb(l.colour[0], l.colour[1], l.colour[2]);
    if inspector_head(ui, ed, "Light", c, None, &format!("Light {}", k + 1)) {
        delete_selected(ed);
        return;
    }
    card(ui, "light_look", "Light", "Light", true, |ui| {
        ui.horizontal(|ui| { ui.label("Colour"); ui.color_edit_button_srgb(&mut l.colour); });
        ui.add(egui::Slider::new(&mut l.radius, 2.0..=60.0).text("reach"));
        ui.add(egui::Slider::new(&mut l.intensity, 0.1..=3.0).text("strength"));
    });
    card(ui, "light_place", "Start", "Place", true, |ui| {
        egui::Grid::new("light_place").num_columns(2).show(ui, |ui| {
            for (a, label) in ["X", "Y", "Height"].iter().enumerate() { num_row(ui, label, &mut l.position[a], 0.1, 0.0..=1023.0); }
        });
        label_small(ui, "Drag the ball in the world (World tool)");
    });
    if k >= 4 { label_small(ui, "Only the first 4 lights light the objects; all light the ground."); }
    l.radius = l.radius.round();
    l.intensity = (l.intensity * 10.0).round() / 10.0;
    for v in l.position.iter_mut() { *v = (*v * 4.0).round() / 4.0; }
    if l != ed.def.world.lights[k] {
        if !ed.coalescing { ed.checkpoint(); }
        ed.coalescing = true;
        ed.def.world.lights[k] = l;
        crate::map_load::set(ed.def.clone());
        ed.changed();
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
            clear_selection(ed);
            ed.zone = Some(i);
            if let Some(g) = ground { ed.zone_drag = Some(([ed.def.zones[i].centre[0] - g[0], ed.def.zones[i].centre[1] - g[1]], ed.snap())); }
        } else {
            clear_selection(ed);
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

/// The status bar's hints: the current tool's mouse buttons and keys.
fn hints(ui: &mut egui::Ui, ed: &Editor) {
    enum H { M(u8, &'static str), K(&'static str, &'static str) }
    use H::{K, M};
    let list: Vec<H> = match (ed.tool, ed.terrain_mode) {
        (Tool::Objects, _) if ed.armed.is_some() => vec![M(0, "place"), K("R", "turn"), K("Esc", "stop"), M(1, "look")],
        (Tool::Terrain, TerrainMode::Build) if ed.island.shape == Shape::Draw => vec![M(0, "drag: trace the coast"), M(1, "look"), K("WASD", "fly")],
        (Tool::Terrain, TerrainMode::Build) => vec![M(0, "build, drag: size"), M(1, "look"), K("WASD", "fly")],
        (Tool::Terrain, TerrainMode::Shape) => vec![M(0, "drag the dots"), M(2, "reach"), K("Ctrl", "snap"), M(1, "look")],
        (Tool::Terrain, TerrainMode::Openings) => vec![M(0, "open or close"), M(1, "look"), K("WASD", "fly")],
        (Tool::Terrain, TerrainMode::Sculpt) => vec![M(0, "brush"), K("Shift", "the other way"), M(1, "look")],
        (Tool::Terrain, TerrainMode::Paths | TerrainMode::Rails) => vec![M(0, "add a point"), K("Enter", "done"), M(2, "height"), K("Del", "point out"), M(1, "look")],
        (Tool::Logic, _) if ed.zone_new.is_some() => vec![M(0, "place the zone"), K("Esc", "stop"), M(1, "look")],
        (Tool::Logic, _) => vec![M(0, "select, drag a zone"), M(1, "look"), K("WASD", "fly")],
        (Tool::World, _) => vec![M(0, "select, drag a light"), M(1, "look"), K("WASD", "fly")],
        _ => vec![M(0, "select, drag"), M(1, "look"), K("WASD", "fly"), K("R", "turn"), K("Del", "delete")],
    };
    ui.spacing_mut().item_spacing.x = 4.0;
    for h in list {
        match h {
            M(b, t) => { mouse_cap(ui, b); ui.label(egui::RichText::new(t).small()); }
            K(k, t) => { key_cap(ui, k); ui.label(egui::RichText::new(t).small()); }
        }
        ui.add_space(6.0);
    }
}

/// The Select tool's panel: the controls, and everything on the map (a click selects it; its settings are on the right).
fn select_panel(ui: &mut egui::Ui, ed: &mut Editor, contexts: &mut EguiContexts, images: &mut Assets<Image>) {
    egui::ScrollArea::vertical().id_salt("select_panel").show(ui, |ui| {
        card(ui, "controls", "View", "Controls", true, |ui| {
            egui::Grid::new("controls").num_columns(2).spacing([12.0, 7.0]).show(ui, |ui| {
                let rows: [(&dyn Fn(&mut egui::Ui), &str); 9] = [
                    (&|ui| mouse_cap(ui, 0), "Select, drag"),
                    (&|ui| mouse_cap(ui, 1), "Hold: look around"),
                    (&|ui| key_cap(ui, "W A S D"), "Fly"),
                    (&|ui| { key_cap(ui, "Space"); key_cap(ui, "Q"); }, "Up, down"),
                    (&|ui| mouse_cap(ui, 2), "Fly speed"),
                    (&|ui| key_cap(ui, "R"), "Turn"),
                    (&|ui| key_cap(ui, "Del"), "Delete"),
                    (&|ui| { key_cap(ui, "Ctrl"); key_cap(ui, "Z"); }, "Undo"),
                    (&|ui| key_cap(ui, "F5"), "Play"),
                ];
                for (cap, what) in rows {
                    ui.horizontal(|ui| { ui.spacing_mut().item_spacing.x = 3.0; cap(ui); });
                    ui.label(what);
                    ui.end_row();
                }
            });
        });
        card(ui, "on_map", "Map", "On this map", true, |ui| outliner(ui, ed, contexts, images));
    });
}

/// Everything on the map by kind, a row each: a click selects it (double click: the camera looks at an object).
fn outliner(ui: &mut egui::Ui, ed: &mut Editor, contexts: &mut EguiContexts, images: &mut Assets<Image>) {
    let d = &ed.def;
    let total = d.objects.len() + d.patches.len() + d.walkways.len() + d.rails.len() + d.zones.len() + d.world.lights.len();
    if total == 0 {
        empty_hint(ui, "Islands", "Nothing yet: start with Islands");
        return;
    }
    let head = |ui: &mut egui::Ui, icon_name: &str, title: &str, n: usize| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            icon(ui, icon_name, 16.0, ORANGE);
            ui.label(egui::RichText::new(title).strong());
            ui.label(egui::RichText::new(n.to_string()).small().color(DIM));
        });
    };
    if !ed.def.objects.is_empty() {
        head(ui, "Objects", "Objects", ed.def.objects.len());
        let mut budget = 8;
        egui::ScrollArea::vertical().id_salt("outline_objects").max_height(240.0).show_rows(ui, 24.0, ed.def.objects.len(), |ui, range| {
            for i in range {
                let o = &ed.def.objects[i];
                let name = o.name.clone().unwrap_or_else(|| ed.name_of(&o.asset));
                let rel = ed.entry(&o.asset).map(|e| e.thumb.clone()).unwrap_or_default();
                ui.horizontal(|ui| {
                    match (!rel.is_empty()).then(|| thumb(ed, contexts, images, &rel, &mut budget)).flatten() {
                        Some(t) => { ui.add(egui::Image::new((t, egui::vec2(20.0, 20.0)))); }
                        None => { ui.add_space(28.0); }
                    }
                    let r = ui.selectable_label(ed.selected == Some(i), name);
                    if r.clicked() { ed.selected = Some(i); ed.armed = None; }
                    if r.double_clicked() { ed.view_snap = Some("3/4"); }
                });
            }
        });
    }
    let mut pick = None;
    let mut group = |ui: &mut egui::Ui, icon_name: &str, title: &str, rows: Vec<(String, bool, Option<egui::Color32>)>, kind: usize| {
        if rows.is_empty() { return; }
        head(ui, icon_name, title, rows.len());
        for (k, (label, on, dot)) in rows.into_iter().enumerate() {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
                if let Some(c) = dot { ui.painter().circle_filled(rect.center(), 5.0, c); }
                if ui.selectable_label(on, label).clicked() { pick = Some((kind, k)); }
            });
        }
    };
    let d = ed.def.clone();
    group(ui, "Islands", "Islands", d.patches.iter().enumerate().map(|(k, p)| (format!("Island {}  ·  {:.0}×{:.0}", k + 1, p.size[0] as f32 * p.spacing, p.size[1] as f32 * p.spacing), ed.patch == Some(k), None)).collect(), 1);
    group(ui, "Paths", "Paths", d.walkways.iter().enumerate().map(|(k, w)| (format!("Path {}  ·  {} points", k + 1, w.points.len()), ed.walk == Some(k), None)).collect(), 2);
    group(ui, "Rails", "Rails", d.rails.iter().enumerate().map(|(k, r)| (format!("Rail {}  ·  {} points", k + 1, r.points.len()), ed.rail == Some(k), None)).collect(), 4);
    group(ui, "Logic", "Zones", d.zones.iter().enumerate().map(|(k, z)| (z.name.clone(), ed.zone == Some(k), Some(zone_ecolour(&z.kind)))).collect(), 3);
    group(ui, "Light", "Lights", d.world.lights.iter().enumerate().map(|(k, l)| (format!("Light {}", k + 1), ed.light_sel == Some(k), Some(egui::Color32::from_rgb(l.colour[0], l.colour[1], l.colour[2])))).collect(), 5);
    match pick {
        Some((1, k)) => ed.patch = Some(k),
        Some((2, k)) => ed.walk = Some(k),
        Some((3, k)) => ed.zone = Some(k),
        Some((4, k)) => (ed.rail, ed.rail_point, ed.rail_drawing, ed.rail_first) = (Some(k), None, false, None),
        Some((5, k)) => ed.light_sel = Some(k),
        _ => {}
    }
}

/// A zone kind's colour for the panels (as [`zone_colour`] draws it in the world).
fn zone_ecolour(k: &rc_map::logic::ZoneKind) -> egui::Color32 {
    let c = zone_colour(k).to_srgba();
    egui::Color32::from_rgb((c.red * 255.0) as u8, (c.green * 255.0) as u8, (c.blue * 255.0) as u8)
}

/// The inspector's head: back to the map and a bin on top, then the thing's picture (or icon) and name. True: the
/// bin was pressed (the caller deletes it).
fn inspector_head(ui: &mut egui::Ui, ed: &mut Editor, icon_name: &str, accent: egui::Color32, pic: Option<egui::TextureId>, title: &str) -> bool {
    let mut delete = false;
    ui.horizontal(|ui| {
        if back_chip(ui).clicked() { clear_selection(ed); }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icon_button_ink(ui, "Bin", "Delete (Del)", false, RED).clicked() { delete = true; }
        });
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(52.0, 52.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 10.0, TILE);
        match pic {
            Some(t) => egui::Image::new((t, egui::vec2(48.0, 48.0))).corner_radius(8).paint_at(ui, rect.shrink(2.0)),
            None => draw_icon(ui.painter(), rect.center(), 30.0, icon_name, accent),
        }
        ui.add(egui::Label::new(egui::RichText::new(title).size(18.0).strong()).wrap());
    });
    ui.add_space(6.0);
    delete
}

/// A number row of an inspector grid: its label, then a drag value.
fn num_row(ui: &mut egui::Ui, label: &str, v: &mut f32, speed: f64, range: std::ops::RangeInclusive<f32>) -> bool {
    ui.label(label);
    let r = ui.add(egui::DragValue::new(v).speed(speed).range(range)).changed();
    ui.end_row();
    r
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

/// The Logic tool: the zone kinds to place, the rules.
fn logic_panel(ui: &mut egui::Ui, ed: &mut Editor) {
    // The zone kinds: pick one, then click in the world (again: put away).
    card(ui, "logic_zones", "Logic", "Zones", true, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            for (kind, label, hint) in ZONE_KINDS {
                let k = zone_kind_of(kind, [0.0; 3]);
                if icon_tile(ui, zone_icon(&k), label, hint, ed.zone_new == Some(kind), Some(zone_ecolour(&k)), 52.0).clicked() {
                    ed.zone_new = if ed.zone_new == Some(kind) { None } else { Some(kind) };
                    if ed.zone_new.is_some() { ed.status = format!("Click in the world to place a {label}"); }
                }
            }
        });
    });
    let zones: Vec<String> = ed.def.zones.iter().map(|z| z.name.clone()).collect();
    let objects: Vec<String> = ed.def.objects.iter().filter_map(|o| o.name.clone()).collect();
    let mut rules = ed.def.rules.clone();
    let mut drop_rule = None;
    ui.horizontal(|ui| {
        icon(ui, "Trigger", 20.0, ORANGE);
        ui.label(egui::RichText::new("Rules").size(16.0).strong());
    });
    egui::ScrollArea::vertical().id_salt("rules").show(ui, |ui| {
    for (r, rule) in rules.iter_mut().enumerate() {
        egui::Frame::new().fill(CARD).corner_radius(8).inner_margin(8).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("When").strong());
                use rc_map::logic::When;
                let kind = match &rule.when { When::Enters(_) => 0, When::Cleared(_) => 1, When::Destroyed(_) => 2 };
                for (k, label) in [(0, "Ratchet enters"), (1, "all enemies beaten in"), (2, "is destroyed:")] {
                    if ui.selectable_label(kind == k, label).clicked() && kind != k {
                        rule.when = match k { 0 => When::Enters(zones.first().cloned().unwrap_or_default()), 1 => When::Cleared(zones.first().cloned().unwrap_or_default()), _ => When::Destroyed(objects.first().cloned().unwrap_or_default()) };
                    }
                }
                if icon_button_ink(ui, "Bin", "Delete this rule", false, RED).clicked() { drop_rule = Some(r); }
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
            if !missing.is_empty() { ui.label(egui::RichText::new(format!("Not on the map: {}", missing.join(", "))).color(AMBER).small()); }
        });
        ui.add_space(4.0);
    }
    if ui.button("+ New rule").clicked() {
        rules.push(rc_map::logic::Rule { when: rc_map::logic::When::Enters(zones.first().cloned().unwrap_or_default()), then: vec![rc_map::logic::Then::Message("Hello!".into())], cutscene: false });
    }
    });
    if let Some(r) = drop_rule { rules.remove(r); }
    if rules != ed.def.rules {
        if !ed.coalescing { ed.checkpoint(); }
        ed.coalescing = true;
        ed.def.rules = rules;
        ed.changed();
    }
}

/// The selected zone: its kind, name, size, facing; a camera zone's eye.
fn zone_inspector(ui: &mut egui::Ui, ed: &mut Editor, i: usize, cam: Option<&GlobalTransform>) {
    use rc_map::logic::ZoneKind;
    let mut z = ed.def.zones[i].clone();
    let icon_name = zone_icon(&z.kind);
    if inspector_head(ui, ed, icon_name, zone_ecolour(&z.kind), None, &z.name.clone()) {
        delete_selected(ed);
        return;
    }
    card(ui, "zone_kind", icon_name, "Kind", true, |ui| {
        let mut kind = zone_key(&z.kind);
        if choice_tiles(ui, &mut kind, &ZONE_KINDS.map(|(k, label, hint)| (k, zone_icon(&zone_kind_of(k, z.centre)), label, hint)), |k| Some(zone_ecolour(&zone_kind_of(k, z.centre))), 50.0) {
            z.kind = zone_kind_of(kind, z.centre);
        }
    });
    card(ui, "zone_shape", "Shape", "Size and place", true, |ui| {
        egui::Grid::new("zone").num_columns(2).show(ui, |ui| {
            ui.label("Name");
            ui.add(egui::TextEdit::singleline(&mut z.name).desired_width(150.0));
            ui.end_row();
            // The box stands on its bottom: a taller box grows upwards.
            let (was_bottom, was_half) = (z.centre[2] - z.half[2], z.half[2]);
            let mut bottom = was_bottom;
            for (k, label) in ["Width", "Length", "Height"].iter().enumerate() {
                let mut size = z.half[k] * 2.0;
                if num_row(ui, label, &mut size, 0.1, 0.25..=300.0) { z.half[k] = (size / 2.0 * 8.0).round() / 8.0; }
            }
            ui.label("Turn");
            let mut deg = z.yaw.to_degrees();
            if ui.add(egui::Slider::new(&mut deg, 0.0..=359.0).suffix("°")).changed() { z.yaw = deg.to_radians(); }
            ui.end_row();
            num_row(ui, "Bottom", &mut bottom, 0.1, -100.0..=1000.0);
            if bottom != was_bottom || z.half[2] != was_half { z.centre[2] = bottom + z.half[2]; }
        });
    });
    if let ZoneKind::Camera { eye } = &mut z.kind {
        card(ui, "zone_camera", "Camera", "Camera", true, |ui| {
            if ui.button("Use this view").on_hover_text("Fly where its camera should stand, then press this").clicked() {
                if let Some(t) = cam { *eye = to_game(t.translation()).map(|v| (v * 8.0).round() / 8.0); }
            }
            label_small(ui, &format!("at {:.0}, {:.0}, {:.0} (the purple ball)", eye[0], eye[1], eye[2]));
        });
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
}

/// The zone kinds: key, name, what it does.
const ZONE_KINDS: [(&str, &str, &str); 5] = [
    ("checkpoint", "Checkpoint", "Ratchet comes back here after a death"),
    ("goal", "Goal", "Walking in finishes the map"),
    ("death", "Death", "Walking in costs a life (a pit, lava)"),
    ("camera", "Camera", "Inside it, the camera stands at a spot you choose"),
    ("trigger", "Trigger", "Rules watch it: \"when Ratchet enters…\""),
];

fn zone_key(k: &rc_map::logic::ZoneKind) -> &'static str {
    use rc_map::logic::ZoneKind;
    match k { ZoneKind::Checkpoint => "checkpoint", ZoneKind::Goal => "goal", ZoneKind::Death => "death", ZoneKind::Camera { .. } => "camera", ZoneKind::Trigger => "trigger" }
}

fn zone_icon(k: &rc_map::logic::ZoneKind) -> &'static str {
    use rc_map::logic::ZoneKind;
    match k { ZoneKind::Checkpoint => "Checkpoint", ZoneKind::Goal => "Goal", ZoneKind::Death => "Death", ZoneKind::Camera { .. } => "Camera", ZoneKind::Trigger => "Trigger" }
}

/// A zone kind by its key (a camera's eye up and back from the zone's centre).
fn zone_kind_of(key: &str, centre: [f32; 3]) -> rc_map::logic::ZoneKind {
    use rc_map::logic::ZoneKind;
    match key {
        "checkpoint" => ZoneKind::Checkpoint,
        "goal" => ZoneKind::Goal,
        "death" => ZoneKind::Death,
        "camera" => ZoneKind::Camera { eye: [centre[0] - 10.0, centre[1] - 10.0, centre[2] + 8.0] },
        _ => ZoneKind::Trigger,
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
        if let Some((k, z)) = low { out.push(format!("Island {} has ground at height {z:.1}, at or below the death height {d:.1}: Ratchet dies standing there.", k + 1)); }
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
                    ed.status = if ed.def.patches[j].edge.is_some() { format!("Island {} selected: click its edge", j + 1) } else { format!("Island {} has no edge: pick one in the Edge card", j + 1) };
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
                    ed.status = format!("Island {} selected: drag its handles", k + 1);
                }
            }
        }
    }
    // The island dragged is gone (undone, deleted): the drag with it.
    if ed.shape_drag.as_ref().is_some_and(|d| d.patch >= ed.def.patches.len()) { ed.shape_drag = None; }
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
    // The main menu over the map: its keys are the menu's (Delete there must not take something off the map).
    if ed.menu.open && *mode == EditorMode::Editing { return; }
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
    // Nothing that changes the map's lists while the mouse holds something in the world.
    let held = busy(&ed);
    if ctrl && keys.just_pressed(KeyCode::KeyZ) && !held { ed.undo(); }
    if ctrl && keys.just_pressed(KeyCode::KeyY) && !held { ed.redo(); }
    let del = keys.just_pressed(KeyCode::Delete) && !held;
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
        let take = if del { ed.rail_hover } else if keys.just_pressed(KeyCode::Backspace) && ed.rail_drawing { k.map(|k| ed.def.rails[k].points.len() - 1) } else { None };
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
        let take = if del { ed.walk_hover } else if keys.just_pressed(KeyCode::Backspace) && ed.walk_drawing { k.map(|k| ed.def.walkways[k].points.len() - 1) } else { None };
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
    // Esc: the object or zone being placed put away, else the selection let go (the map's settings), else back to
    // the Select tool.
    if esc {
        let picked = ed.selected.is_some() || ed.patch.is_some() || ed.walk.is_some() || ed.zone.is_some() || ed.rail.is_some() || ed.light_sel.is_some();
        if ed.armed.is_some() || ed.zone_new.is_some() {
            (ed.armed, ed.zone_new) = (None, None);
            ed.status = "Placing cancelled".into();
        } else if picked {
            clear_selection(&mut ed);
        } else if ed.tool != Tool::Select {
            set_tool(&mut ed, Tool::Select, TerrainMode::Select);
        }
    }
    // Delete: what is selected (in Paths and Rails, Delete takes out the point under the pointer).
    let points = ed.tool == Tool::Terrain && matches!(ed.terrain_mode, TerrainMode::Paths | TerrainMode::Rails);
    if del && ed.selected.is_none() && !points {
        delete_selected(&mut ed);
    }
    // While placing, R / Shift+R turn the object to place.
    if ed.armed.is_some() && keys.just_pressed(KeyCode::KeyR) && !ctrl {
        let step = if shift { -15f32 } else { 15.0 }.to_radians();
        ed.place_yaw = (ed.place_yaw + step).rem_euclid(std::f32::consts::TAU);
        ed.status = format!("Placing at {:.0}°", ed.place_yaw.to_degrees());
        return;
    }
    if let Some(i) = ed.selected {
        if del {
            delete_selected(&mut ed);
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
    let start_c = if ed.start_drag { Color::WHITE } else { Color::srgb(0.3, 0.9, 1.0) };
    gizmos.circle(Isometry3d::new(game_to_bevy([s[0], s[1], s[2] - 0.45]), flat), 0.8, start_c);
    let yaw = ed.def.start.yaw;
    gizmos.arrow(game_to_bevy([s[0], s[1], s[2] - 0.4]), game_to_bevy([s[0] + yaw.cos() * 1.6, s[1] + yaw.sin() * 1.6, s[2] - 0.4]), start_c);
    // The map's settings showing (nothing selected): the death height as a red grid under the start.
    let nothing = ed.selected.is_none() && ed.patch.is_none() && ed.walk.is_none() && ed.zone.is_none() && ed.rail.is_none() && ed.light_sel.is_none();
    if let (true, Tool::Select, Some(d)) = (nothing, ed.tool, ed.def.death_height) {
        let red = Color::srgba(1.0, 0.3, 0.3, 0.55);
        let (cx, cy, r) = ((s[0] / 8.0).round() * 8.0, (s[1] / 8.0).round() * 8.0, 48.0);
        for k in -6..=6 {
            let o = k as f32 * 8.0;
            gizmos.line(game_to_bevy([cx + o, cy - r, d]), game_to_bevy([cx + o, cy + r, d]), red);
            gizmos.line(game_to_bevy([cx - r, cy + o, d]), game_to_bevy([cx + r, cy + o, d]), red);
        }
    }
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
    let terrain = ed.tool == Tool::Terrain;
    {
        if let Some(at) = ed.brush_at.filter(|_| terrain) {
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
        if let Some(p) = ed.patch.and_then(|k| ed.def.patches.get(k)).filter(|_| !(terrain && matches!(ed.terrain_mode, TerrainMode::Rails | TerrainMode::Paths))) {
            // Its coast.
            for lp in p.loops() {
                let pts: Vec<Vec3> = lp.iter().map(|q| game_to_bevy([q[0], q[1], q[2] + 0.06])).collect();
                gizmos.linestrip(pts.iter().copied().chain(pts.first().copied()), Color::srgb(1.0, 0.6, 0.1));
            }
            if terrain && ed.terrain_mode == TerrainMode::Openings {
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
            if terrain && ed.terrain_mode == TerrainMode::Shape {
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
    // The lamps (World and Select tools, or one selected): a ball in their colour and their reach on the ground, the
    // picked one brighter.
    if matches!(ed.tool, Tool::World | Tool::Select) || ed.light_sel.is_some() {
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
        // The selected zone white in any tool (it may have been picked from "On this map").
        let on = ed.zone == Some(i);
        let c = if on { Color::WHITE } else if logic { c } else { c.with_alpha(0.25) };
        let k = z.corners().map(game_to_bevy);
        for (a, b) in [(0, 1), (1, 3), (3, 2), (2, 0), (4, 5), (5, 7), (7, 6), (6, 4), (0, 4), (1, 5), (2, 6), (3, 7)] { gizmos.line(k[a], k[b], c); }
        if let (true, rc_map::logic::ZoneKind::Camera { eye }) = (logic || on, &z.kind) {
            gizmos.sphere(Isometry3d::from_translation(game_to_bevy(*eye)), 0.5, c);
            gizmos.line(game_to_bevy(*eye), game_to_bevy(z.centre), c.with_alpha(0.5));
        }
    }
    // A rail selected outside the Rails tool (from "On this map"): its line, so it can be found.
    if !(ed.tool == Tool::Terrain && ed.terrain_mode == TerrainMode::Rails) {
        if let Some(r) = ed.rail.and_then(|k| ed.def.rails.get(k)) {
            gizmos.linestrip(r.line().iter().map(|q| game_to_bevy([q[0], q[1], q[2] + 0.05])), Color::srgb(1.0, 0.85, 0.2));
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
    // The island builder's drag: the coast traced (closing back to where it began), or the shape at the size dragged.
    if let (Tool::Terrain, TerrainMode::Build, Some(d)) = (ed.tool, ed.terrain_mode, ed.island_drag.as_ref()) {
        let top = d.start[2] + ed.island.height.max(0.25);
        let c = Color::srgb(0.4, 1.0, 0.5);
        if ed.island.shape == Shape::Draw {
            gizmos.linestrip(d.points.iter().map(|p| game_to_bevy([p[0], p[1], top])), c);
            if let (Some(a), Some(b)) = (d.points.first(), d.points.last()) {
                gizmos.line(game_to_bevy([b[0], b[1], top]), game_to_bevy([a[0], a[1], top]), Color::srgba(0.4, 1.0, 0.5, 0.4));
            }
        } else if let (Some(o), Some(e)) = (d.points.first(), d.points.last()) {
            let mut i = ed.island.clone();
            i.size = (2.0 * (e[0] - o[0]).hypot(e[1] - o[1])).clamp(6.0, 400.0);
            let ring = (0..=72).map(|k| { let a = k as f32 / 72.0 * std::f32::consts::TAU; let r = i.reach(a); game_to_bevy([o[0] + a.cos() * r, o[1] + a.sin() * r, top]) });
            gizmos.linestrip(ring, c);
            gizmos.line(game_to_bevy([o[0], o[1], top]), game_to_bevy([e[0], e[1], top]), Color::srgba(1.0, 1.0, 1.0, 0.6));
        }
    }
    // The island the builder would make: its outline at the top, its cliffs down to the bottom, its edge (Draw: only
    // where it begins).
    if let (Tool::Terrain, TerrainMode::Build, Some(at)) = (ed.tool, ed.terrain_mode, ed.island_at.filter(|_| ed.island.shape != Shape::Draw)) {
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

fn theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = NAVY;
    v.window_fill = NAVY;
    v.extreme_bg_color = DEEP;
    v.faint_bg_color = NAVY2;
    v.selection.bg_fill = ORANGE;
    v.selection.stroke = egui::Stroke::new(1.0, DARK_INK);
    v.hyperlink_color = AMBER;
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, LINE);
    v.widgets.inactive.weak_bg_fill = TILE;
    v.widgets.inactive.bg_fill = TILE;
    v.widgets.hovered.weak_bg_fill = TILE_HOVER;
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, ORANGE);
    v.widgets.active.weak_bg_fill = ORANGE;
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = egui::CornerRadius::same(6);
    }
    v.window_corner_radius = egui::CornerRadius::same(10);
    v.override_text_color = Some(INK);
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        // Rows are clicked as a whole: labels must not take the click for text selection.
        s.interaction.selectable_labels = false;
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(8.0, 4.0);
        s.spacing.slider_width = 140.0;
        s.spacing.interact_size.y = 22.0;
    });
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
    mut last_pick: Local<[Option<usize>; 6]>,
    mut loading_since: Local<Option<std::time::Instant>>,
) -> Result {
    let ctx = contexts.ctx_mut()?.clone();
    if !*themed {
        theme(&ctx);
        *themed = true;
    }
    let ed = &mut *ed;
    // One thing selected at a time: the newest pick lets the others go (the right panel shows it).
    let picked = [ed.selected, ed.patch, ed.walk, ed.zone, ed.rail, ed.light_sel];
    if picked != *last_pick {
        if let Some(new) = (0..6).find(|&k| picked[k].is_some() && picked[k] != last_pick[k]) {
            if new != 0 { ed.selected = None; }
            if new != 1 { ed.patch = None; }
            if new != 2 { ed.walk = None; }
            if new != 3 { ed.zone = None; }
            if new != 4 { (ed.rail, ed.rail_point) = (None, None); }
            if new != 5 { ed.light_sel = None; }
        }
        *last_pick = [ed.selected, ed.patch, ed.walk, ed.zone, ed.rail, ed.light_sel];
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
    let no_text = !ctx.egui_wants_keyboard_input();
    // F1: the help; 1..9, 0: the strip's tools.
    // F1 (and Esc, when it is open): the help. Closing it counts as seen: it no longer opens by itself.
    let close_help = ed.help.is_some() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) && no_text;
    if (ctx.input(|i| i.key_pressed(egui::Key::F1)) || close_help) && *mode == EditorMode::Editing {
        if ed.help.take().is_some() {
            ed.settings.help_seen = true;
            ed.settings.store();
        } else {
            ed.help = Some(0);
        }
    }
    if no_text && can_edit && !ctx.input(|i| i.modifiers.ctrl) && !busy(ed) {
        const DIGITS: [egui::Key; 10] = [egui::Key::Num1, egui::Key::Num2, egui::Key::Num3, egui::Key::Num4, egui::Key::Num5, egui::Key::Num6, egui::Key::Num7, egui::Key::Num8, egui::Key::Num9, egui::Key::Num0];
        for (k, key) in DIGITS.iter().enumerate() {
            if ctx.input(|i| i.key_pressed(*key)) { set_tool(ed, STRIP[k].3, STRIP[k].4); }
        }
    }
    // The top bar (docs/plan/rac_maker.md, "v0.6"): the menu and the map, its file, undo, Play, its picture; on the
    // right the radio, the views, the camera speed and the help.
    let mut popup: Option<(u8, egui::Rect)> = None;
    let top = egui::Panel::top("rac_maker_top").frame(egui::Frame::new().fill(DEEP).inner_margin(egui::Margin::symmetric(8, 5))).show(&mut root, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if ui.add(egui::Button::new(egui::RichText::new("RAC MAKER").strong().size(16.0).color(ORANGE)).frame(false)).on_hover_text("Main menu: your maps, versions, backups").clicked() {
                ed.menu.reopen();
            }
            ui.add_space(6.0);
            ui.label(egui::RichText::new(format!("{}{}", ed.def.name, if ed.saved { "" } else { " •" })).strong().size(15.0)).on_hover_text(if ed.saved { "Saved" } else { "Not saved yet" });
            ui.add_space(10.0);
            if *mode == EditorMode::Editing {
                ui.add_enabled_ui(can_edit, |ui| {
                    if icon_button(ui, "New", "New map", false).clicked() { ed.new_map(); }
                    if icon_button(ui, "Save", "Save (Ctrl+S)", false).clicked() { ed.save(); }
                    if !ed.warnings.is_empty() {
                        let r = icon_button_ink(ui, "Warn", &format!("{} things to check", ed.warnings.len()), ed.warn_menu, AMBER);
                        if r.clicked() { ed.warn_menu = !ed.warn_menu; }
                        if ed.warn_menu { popup = Some((2, r.rect)); }
                    }
                    ui.add_space(6.0);
                    let can_undo = !ed.undo.is_empty();
                    if ui.add_enabled_ui(can_undo, |ui| icon_button(ui, "Undo", "Undo (Ctrl+Z)", false)).inner.clicked() { ed.undo(); }
                    let can_redo = !ed.redo.is_empty();
                    if ui.add_enabled_ui(can_redo, |ui| icon_button(ui, "Redo", "Redo (Ctrl+Y)", false)).inner.clicked() { ed.redo(); }
                });
                ui.add_space(10.0);
            }
            let (glyph, text) = match (*mode, ed.play_pending) {
                (EditorMode::Playing, _) => ("Stop", "Stop"),
                (_, true) => ("Play", "Starting…"),
                _ => ("Play", "Play"),
            };
            if play_button(ui, glyph, text).on_hover_text("F5").clicked() {
                toggle_play(ed, &mut mode, source.as_deref_mut());
            }
            ui.add_space(10.0);
            if icon_button(ui, "Picture", "Take the map's picture: what the camera sees now", false).clicked() {
                ed.thumb_shot = Some((menu::thumb_path(&ed.path), 2));
                ed.thumb_record = true;
            }
            if *mode == EditorMode::Editing {
                let auto = !ed.settings.no_thumb_on_save;
                let hint = if auto { "Each save takes the picture again from its spot (on)" } else { "Each save takes the picture again from its spot (off)" };
                if icon_button(ui, "Refresh", hint, auto).clicked() {
                    ed.settings.no_thumb_on_save = auto;
                    ed.settings.store();
                }
            }
            if icon_button(ui, "Collision", "What Ratchet stands on and bumps into (F6)", ed.show_collision).clicked() {
                ed.show_collision = !ed.show_collision;
                ed.collision_view = None;
            }
            if !matches!(ed.reload, Reload::Idle) {
                ui.add_space(6.0);
                ui.add(egui::Spinner::new().color(ORANGE));
            }
            let room = ui.available_width();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if room > 420.0 { ui.label(egui::RichText::new(format!("{:.0} fps", ed.fps)).small().color(DIM)); }
                if *mode == EditorMode::Editing && icon_button(ui, "Help", "Help (F1)", ed.help.is_some()).clicked() {
                    if ed.help.is_some() {
                        ed.help = None;
                        ed.settings.help_seen = true;
                        ed.settings.store();
                    } else {
                        ed.help = Some(0);
                    }
                }
                if *mode == EditorMode::Editing {
                    let r = icon_button(ui, "View", "Look from above, the front, the side…", ed.view_menu);
                    if r.clicked() { ed.view_menu = !ed.view_menu; }
                    if ed.view_menu { popup = Some((1, r.rect)); }
                }
                if let Some(mut fly) = fly.single_mut().ok().filter(|_| room > 300.0) {
                    if ui.add(egui::Button::new(egui::RichText::new(format!("{:.0}", fly.speed)).small()).corner_radius(10)).on_hover_text("Fly speed: the mouse wheel over the world (click: back to 16)").clicked() { fly.speed = 16.0; }
                }
                if room > 520.0 && *mode == EditorMode::Editing {
                    let root = crate::level_load::extracted_root();
                    let songs = crate::map_load::songs(&root);
                    let playing = ed.radio.and_then(|k| songs.get(k)).map(|s| s.2.split(": ").nth(1).unwrap_or(&s.2).to_string());
                    let r = icon_button(ui, "Radio", "Radio: listen to any song while you build", ed.radio.is_some() || ed.radio_menu);
                    if r.clicked() { ed.radio_menu = !ed.radio_menu; }
                    if ed.radio_menu { popup = Some((0, r.rect)); }
                    if let Some(name) = playing {
                        ui.label(egui::RichText::new(name).small().color(ORANGE));
                    }
                }
            });
        });
    });
    rects.push(top.response.rect);
    // The top bar's menus: the radio's songs, the views, the warnings. A click elsewhere closes them.
    if let Some((which, at)) = popup {
        let root = crate::level_load::extracted_root();
        let songs = crate::map_load::songs(&root);
        let (width, x) = match which { 2 => (300.0, at.left()), 1 => (210.0, at.right() - 210.0), _ => (260.0, at.right() - 260.0) };
        let list = egui::Area::new(egui::Id::new(("rac_maker_menu", which))).fixed_pos(egui::pos2(x, at.bottom() + 6.0)).order(egui::Order::Foreground).show(&ctx, |ui| {
            egui::Frame::popup(ui.style()).corner_radius(10).inner_margin(10).show(ui, |ui| {
                ui.set_width(width);
                match which {
                    0 => {
                        if ed.radio.is_some() && ui.button("■  Stop the radio").clicked() { ed.radio_want = Some(None); ed.radio_menu = false; }
                        egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                            let mut planet = u32::MAX;
                            for (k, (l, _, name)) in songs.iter().enumerate() {
                                if *l != planet {
                                    planet = *l;
                                    ui.add_space(2.0);
                                    ui.label(egui::RichText::new(LEVEL_NAMES[*l as usize]).strong().color(ORANGE));
                                }
                                let short = name.split(": ").nth(1).unwrap_or(name);
                                if ui.selectable_label(ed.radio == Some(k), format!("   {short}")).clicked() {
                                    ed.radio_want = Some(Some(k));
                                    ed.radio_menu = false;
                                }
                            }
                        });
                    }
                    1 => {
                        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                        for row in [["Top", "3/4"], ["Front", "Back"], ["Left", "Right"]] {
                            ui.horizontal(|ui| {
                                for name in row {
                                    if ui.add_sized([96.0, 26.0], egui::Button::new(name)).clicked() {
                                        ed.view_snap = Some(name);
                                        ed.view_menu = false;
                                    }
                                }
                            });
                        }
                    }
                    _ => {
                        ui.horizontal(|ui| {
                            icon(ui, "Warn", 18.0, AMBER);
                            ui.label(egui::RichText::new("To check").strong().color(AMBER));
                        });
                        for w in &ed.warnings { ui.label(format!("• {w}")); }
                    }
                }
            });
        });
        rects.push(list.response.rect);
        if ctx.input(|i| i.pointer.any_pressed()) && ctx.pointer_latest_pos().is_some_and(|p| !list.response.rect.contains(p) && !at.contains(p)) {
            (ed.radio_menu, ed.view_menu, ed.warn_menu) = (false, false, false);
        }
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
            egui::Frame::popup(ui.style()).corner_radius(10).inner_margin(12).show(ui, |ui| {
                ui.set_width(320.0);
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(22.0).color(ORANGE));
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("Loading the level").strong());
                        let what = if step < n { crate::level_load::LOAD_STAGES[step] } else { "putting it in place" };
                        ui.label(egui::RichText::new(format!("{what} · {:.0} s", since.elapsed().as_secs_f32())).small().color(DIM));
                    });
                });
                ui.add_space(4.0);
                ui.add(egui::ProgressBar::new(((step as f32 + 0.5) / (n as f32 + 1.0)).min(1.0)).desired_height(8.0).fill(ORANGE).animate(true));
            });
        });
        ctx.request_repaint();
    } else if since.is_none() {
        *loading_since = None;
    }
    // The status bar: the current tool's mouse and keys, then the last message.
    rects.push(egui::Panel::bottom("rac_maker_status").frame(egui::Frame::new().fill(DEEP).inner_margin(egui::Margin::symmetric(10, 4))).show(&mut root, |ui| {
        ui.horizontal(|ui| {
            if *mode == EditorMode::Editing { hints(ui, ed); ui.add_space(12.0); }
            ui.label(egui::RichText::new(&ed.status).color(DIM));
        });
    }).response.rect);
    if *mode == EditorMode::Editing {
        // The tool strip.
        let strip = egui::Panel::left("rac_maker_tools").exact_size(72.0).resizable(false).frame(egui::Frame::new().fill(DEEP).inner_margin(egui::Margin::symmetric(5, 8))).show(&mut root, |ui| {
            // A short window scrolls the strip (the wheel over it) instead of cutting Logic and World off.
            egui::ScrollArea::vertical().id_salt("strip_scroll").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| ui.add_enabled_ui(can_edit, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                for (k, &(icon_name, caption, hint, tool, tmode)) in STRIP.iter().enumerate() {
                    if [1, 2, 6, 8].contains(&k) {
                        ui.add_space(4.0);
                        let r = ui.available_rect_before_wrap();
                        ui.painter().line_segment([egui::pos2(r.left() + 12.0, r.top()), egui::pos2(r.right() - 12.0, r.top())], egui::Stroke::new(1.0, LINE));
                        ui.add_space(5.0);
                    }
                    if tool_button(ui, icon_name, caption, hint, tool_is(ed, tool, tmode)).clicked() { set_tool(ed, tool, tmode); }
                }
            }));
        });
        rects.push(strip.response.rect);
        // The left panel: the tool's own things under its name.
        let left = egui::Panel::left("rac_maker_assets").default_size(310.0).show(&mut root, |ui| {
            ui.add_enabled_ui(can_edit, |ui| {
                ui.add_space(4.0);
                let k = STRIP.iter().position(|t| tool_is(ed, t.3, t.4)).unwrap_or(0);
                let title = match k { 2 => "Build islands", 4 => "Edge openings", 7 => "Grind rails", _ => STRIP[k].1 };
                ui.horizontal(|ui| {
                    icon(ui, STRIP[k].0, 26.0, ORANGE);
                    ui.label(egui::RichText::new(title).size(20.0).strong());
                });
                ui.add_space(6.0);
                match ed.tool {
                    Tool::Select => select_panel(ui, ed, &mut contexts, &mut images),
                    Tool::Objects => asset_browser(ui, ed, &mut contexts, &mut images),
                    Tool::Terrain => terrain_panel(ui, ed, &mut contexts, &mut images),
                    Tool::Logic => logic_panel(ui, ed),
                    Tool::World => world_panel(ui, ed, &mut contexts, &mut images),
                }
            });
        });
        // The right panel: what is selected (in any tool), else the map.
        let right = egui::Panel::right("rac_maker_inspector").default_size(300.0).show(&mut root, |ui| {
            ui.add_enabled_ui(can_edit, |ui| {
                egui::ScrollArea::vertical().id_salt("inspector").auto_shrink([false, false]).show(ui, |ui| {
                    ui.add_space(4.0);
                    let cam = cams.single().ok().map(|c| c.1);
                    if let Some(i) = ed.selected.filter(|&i| i < ed.def.objects.len()) {
                        object_inspector(ui, ed, i, &mut contexts, &mut images, cam);
                    } else if let Some(z) = ed.zone.filter(|&z| z < ed.def.zones.len()) {
                        zone_inspector(ui, ed, z, cam);
                    } else if let Some(k) = ed.patch.filter(|&k| k < ed.def.patches.len()) {
                        patch_inspector(ui, ed, k, &mut contexts, &mut images);
                    } else if let Some(k) = ed.walk.filter(|&k| k < ed.def.walkways.len()) {
                        path_inspector(ui, ed, k, &mut contexts, &mut images);
                    } else if let Some(k) = ed.rail.filter(|&k| k < ed.def.rails.len()) {
                        rail_inspector(ui, ed, k, &mut contexts, &mut images);
                    } else if let Some(k) = ed.light_sel.filter(|&k| k < ed.def.world.lights.len()) {
                        light_inspector(ui, ed, k);
                    } else {
                        map_inspector(ui, ed, &mut contexts, &mut images);
                    }
                });
            });
        });
        rects.extend([left.response.rect, right.response.rect]);
        // The help: a window by the right panel (the settings stay in view).
        if let Some(t) = ed.help {
            // In the world view between the panels, never over the left panel (its controls stay usable); a narrow
            // view takes a narrower window, at worst over the inspector's edge.
            let r = right.response.rect;
            let room = r.left() - left.response.rect.right() - 24.0;
            let w = room.clamp(280.0, 560.0);
            let x = (r.left() - w - 12.0).max(left.response.rect.right() + 12.0);
            let at = egui::pos2(x, top.response.rect.bottom() + 12.0);
            let h = (r.bottom() - at.y - 12.0).max(220.0);
            let area = egui::Area::new(egui::Id::new("rac_maker_help")).fixed_pos(at).order(egui::Order::Foreground).show(&ctx, |ui| {
                egui::Frame::popup(ui.style()).fill(NAVY).corner_radius(12).inner_margin(14).show(ui, |ui| {
                    ui.set_width(w - 28.0);
                    ui.set_height(h - 28.0);
                    help_panel(ui, ed, t);
                });
            });
            rects.push(area.response.rect);
        }
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
    // A narrow window gives the topics less room; a short one scrolls them.
    let topics_w = (ui.available_width() * 0.3).clamp(100.0, 150.0);
    ui.horizontal_top(|ui| {
        egui::ScrollArea::vertical().id_salt("help_topics").max_width(topics_w).show(ui, |ui| ui.vertical(|ui| {
            ui.set_width(topics_w);
            for (k, (title, lines)) in HELP.iter().enumerate() {
                let found = !words.is_empty() && (hits(title) || lines.iter().any(|l| hits(l)));
                let text = egui::RichText::new(*title).strong();
                let text = if !words.is_empty() && !found { text.weak() } else { text };
                if ui.selectable_label(k == topic && words.is_empty(), text).clicked() {
                    ed.help = Some(k);
                    ed.help_query.clear();
                }
            }
        }));
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

/// Which planet's pictures a grid shows (textures, plants; shared with the Objects browser): its globe and name
/// between arrows that step through the planets. The planet shown.
fn planet_switch(ui: &mut egui::Ui, ed: &mut Editor, contexts: &mut EguiContexts, images: &mut Assets<Image>, salt: &str) -> u32 {
    let level = ed.planet.unwrap_or(ed.def.host_level);
    ui.push_id((salt, "planet"), |ui| {
        ui.horizontal(|ui| {
            if ui.small_button("<").on_hover_text("The planet before").clicked() { ed.planet = Some((level + 18) % 19); }
            if let Some(t) = globe_tex(ed, contexts, images, level) { ui.add(egui::Image::new((t, egui::vec2(22.0, 22.0)))); }
            ui.label(egui::RichText::new(LEVEL_NAMES[level as usize]).strong());
            if ui.small_button(">").on_hover_text("The next planet").clicked() { ed.planet = Some((level + 1) % 19); }
        });
    });
    ed.planet.unwrap_or(ed.def.host_level)
}

/// A grid of the catalog's terrain textures of the planet chosen in Objects > Planet (else the map's host): the one
/// clicked, or None. `current` is outlined.
fn texture_grid(ui: &mut egui::Ui, ed: &mut Editor, contexts: &mut EguiContexts, images: &mut Assets<Image>, current: Option<AssetId>, salt: &str) -> Option<AssetId> {
    let level = planet_switch(ui, ed, contexts, images, salt);
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

/// The Paths tool's panel: how high new points go, Done, and what the next new path gets (a path picked in the world
/// shows its own settings on the right).
fn paths_panel(ui: &mut egui::Ui, ed: &mut Editor, contexts: &mut EguiContexts, images: &mut Assets<Image>) {
    ui.add(egui::Slider::new(&mut ed.walk_lift, 0.0..=20.0).text("points above the ground"));
    if (ed.walk_drawing || ed.walk_first.is_some()) && ui.button("Done (Enter)").clicked() { ed.walk_drawing = false; ed.walk_first = None; }
    ui.add_space(6.0);
    egui::ScrollArea::vertical().id_salt("paths_panel").show(ui, |ui| {
        label_small(ui, "The next new path");
        walk_settings(ui, ed, None, contexts, images);
    });
}

/// The selected path (the right panel).
fn path_inspector(ui: &mut egui::Ui, ed: &mut Editor, k: usize, contexts: &mut EguiContexts, images: &mut Assets<Image>) {
    let pic = ed.def.walkways[k].texture.and_then(|t| ed.entry(&t)).map(|e| e.thumb.clone()).and_then(|rel| thumb(ed, contexts, images, &rel, &mut 2));
    if inspector_head(ui, ed, "Paths", INK, pic, &format!("Path {}", k + 1)) {
        delete_selected(ed);
        return;
    }
    walk_settings(ui, ed, Some(k), contexts, images);
}

/// A path's settings (`k`), else those the next new one gets: size, ground, surface, edge, colour.
fn walk_settings(ui: &mut egui::Ui, ed: &mut Editor, k: Option<usize>, contexts: &mut EguiContexts, images: &mut Assets<Image>) {
    let k = k.filter(|&k| k < ed.def.walkways.len());
    let mut w = match k { Some(k) => ed.def.walkways[k].clone(), None => ed.walk_spec.clone() };
    let before = w.clone();
    let salt = if k.is_some() { "path" } else { "new_path" };
    card(ui, (salt, "size"), "Paths", "Size", true, |ui| {
        ui.add(egui::Slider::new(&mut w.width, 0.5..=20.0).text("width"));
        ui.add(egui::Slider::new(&mut w.depth, 0.0..=20.0).text("thickness"));
        ui.checkbox(&mut w.cut, "Stop where it meets land").on_hover_text("A bridge from coast to coast: nothing poking through the land");
    });
    card(ui, (salt, "ground"), "Paint", "Ground", true, |ui| {
        if ui.selectable_label(w.texture.is_none(), "Plain").clicked() { w.texture = None; }
        if let Some(t) = texture_grid(ui, ed, contexts, images, w.texture, "walk_textures") { w.texture = Some(t); }
        ui.add(egui::Slider::new(&mut w.tile, 1.0..=16.0).text("texture size"));
    });
    card(ui, (salt, "surface"), "Ground", "Surface", false, |ui| surface_tiles(ui, &mut w.surface));
    card(ui, (salt, "edge"), "Railings", "Edge", true, |ui| edge_picker(ui, ed, contexts, images, &mut w.edge, "walk_edge"));
    if k.is_some() {
        card(ui, (salt, "colour"), "Shade", "Colour", false, |ui| {
            let tex = w.texture;
            colour_section(ui, ed, &mut w.tint, tex, "path");
        });
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
}

/// The plants and details of the planet chosen in Objects > Planet (else the host's) as a grid of their pictures: the
/// one clicked.
fn scatter_grid(ui: &mut egui::Ui, ed: &mut Editor, contexts: &mut EguiContexts, images: &mut Assets<Image>) -> Option<AssetId> {
    let level = planet_switch(ui, ed, contexts, images, "scatter");
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
    // The kind as tiles: none, an invisible wall, the railings, the walls (their pieces under them).
    let current = match edge.as_ref().map(|e| &e.kind) {
        None => 0,
        Some(EdgeKind::Invisible) => 1,
        Some(EdgeKind::Wall { .. }) => 3,
        Some(EdgeKind::Piece(id)) => if EDGE_PIECES.iter().any(|p| p.2 && AssetId::parse(p.0).ok() == Some(*id)) { 3 } else { 2 },
    };
    if current >= 2 { ed.edge_walls = current == 3; }
    let mut pick = if current >= 2 { if ed.edge_walls { 3 } else { 2 } } else { current };
    let was = pick;
    ui.push_id((salt, "kind"), |ui| {
        choice_tiles(ui, &mut pick, &[(0, "None", "None", "Ratchet can walk off"), (1, "Invisible", "Invisible", "Nothing drawn, but Ratchet cannot walk off"), (2, "Railings", "Railings", ""), (3, "Walls", "Walls", "")], |_| None, 54.0);
    });
    if pick != was {
        match pick {
            0 => *edge = None,
            1 => *edge = with_gaps(EdgeKind::Invisible, edge.as_ref().map_or(1.5, |e| e.height.min(4.0))),
            k => {
                // Railings or walls: the first of them at once (the edge shows them; the others are under the tiles).
                ed.edge_walls = k == 3;
                if let Some((id, name)) = EDGE_PIECES.iter().filter(|p| p.2 == ed.edge_walls).find_map(|p| Some((AssetId::parse(p.0).ok()?, p.1))) {
                    let height = crate::map_load::piece_model(id).map_or(1.2, |m| m.height().clamp(0.9, 30.0));
                    *edge = with_gaps(EdgeKind::Piece(id), height);
                    ed.status = format!("Edge: {name} (pick another below)");
                }
            }
        }
    }
    let show_pieces = pick >= 2;
    let mut budget = 16;
    ui.push_id(salt, |ui| {
        ui.horizontal_wrapped(|ui| {
            if !show_pieces { return; }
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
        label_small(ui, &name);
        if let EdgeKind::Wall { texture, tile } = &mut e.kind {
            ui.add(egui::Slider::new(&mut e.height, 0.5..=30.0).text("wall height"));
            ui.add(egui::Slider::new(tile, 1.0..=16.0).text("texture size"));
            let current = *texture;
            egui::CollapsingHeader::new("Wall texture").id_salt("wall_texture").default_open(texture.is_none()).show(ui, |ui| {
                if let Some(t) = texture_grid(ui, ed, contexts, images, current, "wall_textures") { *texture = Some(t); }
            });
        } else {
            ui.add(egui::Slider::new(&mut e.height, 0.3..=30.0).logarithmic(true).text("blocks up to")).on_hover_text("How high Ratchet cannot get past it");
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
    let mover = crate::map_load::MOVERS.contains(&o.asset.number);
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

/// The terrain tools' panels: Islands (what the next island gets), Shape, Edges, Sculpt (the brushes), Paths, Rails.
fn terrain_panel(ui: &mut egui::Ui, ed: &mut Editor, contexts: &mut EguiContexts, images: &mut Assets<Image>) {
    let picked = ed.patch.filter(|&k| k < ed.def.patches.len());
    match ed.terrain_mode {
        TerrainMode::Rails => return rails_panel(ui, ed, contexts, images),
        TerrainMode::Paths => return paths_panel(ui, ed, contexts, images),
        TerrainMode::Shape | TerrainMode::Openings if picked.is_none() => return empty_hint(ui, "Select", "Click an island"),
        _ => {}
    }
    egui::ScrollArea::vertical().id_salt("terrain_panel").show(ui, |ui| match ed.terrain_mode {
        TerrainMode::Build => {
            // The island just built (still selected) follows the shape's depth, the edge and the ground chosen here.
            let last = ed.last_built.filter(|&k| ed.patch == Some(k) && k < ed.def.patches.len());
            card(ui, "build_shape", "Islands", "Shape", true, |ui| {
                let i = &mut ed.island;
                choice_tiles(ui, &mut i.shape, &[(Shape::Round, "Round", "Round", ""), (Shape::Square, "Square", "Square", ""), (Shape::Blob, "Blob", "Blob", ""), (Shape::Draw, "Draw", "Draw", "Hold the left button and trace the coast; let go to build it")], |_| None, 58.0);
                ui.label(egui::RichText::new(if i.shape == Shape::Draw { "Drag in the world to trace the coast" } else { "Click to build, or drag out its size" }).small().weak());
                ui.add(egui::Slider::new(&mut i.size, 6.0..=400.0).logarithmic(true).text("across"));
                ui.add(egui::Slider::new(&mut i.height, 0.0..=80.0).text("above the ground"));
                let depth = i.depth;
                ui.add(egui::Slider::new(&mut i.depth, 0.0..=80.0).text("cliff depth"));
                if ed.island.depth != depth {
                    let d = ed.island.depth;
                    adjust_last(ed, |p| p.depth = (d * 64.0).round() / 64.0);
                }
            });
            card(ui, "build_ground", "Paint", "Ground", true, |ui| {
                let current = ed.island.texture;
                if let Some(t) = texture_grid(ui, ed, contexts, images, current, "island_textures") {
                    ed.island.texture = Some(t);
                    adjust_last(ed, |p| p.texture = Some(t));
                }
            });
            card(ui, "build_edge", "Railings", "Edge", true, |ui| {
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
            });
            if let Some(k) = last {
                ui.horizontal(|ui| {
                    icon(ui, "Islands", 16.0, GREEN);
                    ui.label(egui::RichText::new(format!("Island {} follows these too", k + 1)).small().color(GREEN));
                });
            }
        }
        TerrainMode::Shape => {
            let k = picked.unwrap();
            ui.add(egui::Slider::new(&mut ed.reach, 1.5..=64.0).logarithmic(true).text("reach")).on_hover_text("How much coast follows a dot (the yellow circle; the wheel over a dot)");
            ui.horizontal(|ui| {
                for (label, amount) in [("Grow", 1.0f32), ("Shrink", -1.0)] {
                    if ui.button(label).on_hover_text("The whole coast by a unit").clicked() {
                        let mut p = ed.def.patches[k].clone();
                        match p.offset(amount) {
                            Ok(()) if p.check().is_ok() && !p.loops().is_empty() => {
                                ed.checkpoint();
                                ed.def.patches[k] = p;
                                crate::map_load::set(ed.def.clone());
                                ed.status = if amount > 0.0 { "Island grown".into() } else { "Island shrunk".into() };
                                ed.changed();
                            }
                            Ok(()) => ed.status = "It cannot shrink further".into(),
                            Err(e) => ed.status = e,
                        }
                    }
                }
            });
        }
        TerrainMode::Openings => {
            let k = picked.unwrap();
            match ed.def.patches[k].edge.as_ref().map(|e| e.gaps.len()) {
                None => {
                    ui.horizontal(|ui| { icon(ui, "Warn", 18.0, AMBER); ui.label(egui::RichText::new("No edge yet: pick one").color(AMBER)); });
                    let mut edge = None;
                    edge_picker(ui, ed, contexts, images, &mut edge, "openings_edge");
                    if edge.is_some() {
                        ed.checkpoint();
                        ed.def.patches[k].edge = edge;
                        crate::map_load::set(ed.def.clone());
                        ed.changed();
                    }
                }
                Some(n) => {
                    ui.horizontal(|ui| {
                        icon(ui, "Edges", 20.0, ORANGE);
                        ui.label(format!("{n} openings"));
                        if n > 0 && ui.button("Close all").clicked() {
                            ed.checkpoint();
                            if let Some(e) = ed.def.patches[k].edge.as_mut() { e.gaps.clear(); }
                            crate::map_load::set(ed.def.clone());
                            ed.status = "Edge closed all round".into();
                            ed.changed();
                        }
                    });
                }
            }
        }
        _ => {
            card(ui, "brushes", "Sculpt", "Brush", true, |ui| {
                let items: Vec<(Brush, &str, &str, &str)> = BRUSHES.iter().map(|&(b, name, hint)| (b, brush_icon(b), name, hint)).collect();
                choice_tiles(ui, &mut ed.brush, &items, |_| None, 50.0);
                ui.horizontal(|ui| {
                    ui.checkbox(&mut ed.square, "Square");
                    ui.checkbox(&mut ed.invert, "Other way").on_hover_text("As holding Shift: Raise lowers, Paint rubs out, Land cuts away");
                });
                ui.add(egui::Slider::new(&mut ed.radius, 1.0..=32.0).text("size"));
                ui.add(egui::Slider::new(&mut ed.strength, 0.1..=4.0).text("strength"));
                if ed.brush == Brush::Plateau { ui.add(egui::Slider::new(&mut ed.plateau, 0.5..=30.0).text("plateau height")); }
            });
            match ed.brush {
                Brush::Paint => {
                    card(ui, "brush_paint", "Paint", "Paint with", true, |ui| {
                        if ed.paint.is_none() { ui.label(egui::RichText::new("Pick a texture to paint with").color(AMBER)); }
                        if let Some(t) = texture_grid(ui, ed, contexts, images, ed.paint, "paint_textures") { ed.paint = Some(t); }
                    });
                }
                Brush::Shade => {
                    card(ui, "brush_shade", "Shade", "Colour", true, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for (name, c) in SHADES {
                                let (rect, r) = ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::click());
                                ui.painter().rect_filled(rect, 8.0, egui::Color32::from_rgb(c[0], c[1], c[2]));
                                if ed.shade == c { ui.painter().rect_stroke(rect, 8.0, egui::Stroke::new(2.5, ORANGE), egui::StrokeKind::Inside); }
                                if r.on_hover_text(name).clicked() { ed.shade = c; }
                            }
                            ui.color_edit_button_srgb(&mut ed.shade).on_hover_text("Any colour");
                        });
                    });
                }
                Brush::Scatter => {
                    card(ui, "brush_decor", "Decor", "Plants and details", true, |ui| {
                        if ed.scatter.is_none() { ui.label(egui::RichText::new("Pick a plant, rock or detail to strew").color(AMBER)); }
                        ui.add(egui::Slider::new(&mut ed.scatter_size, 0.3..=3.0).text("size"));
                        if let Some(a) = scatter_grid(ui, ed, contexts, images) { ed.scatter = Some(a); }
                    });
                }
                _ => {}
            }
        }
    });
}

/// A brush's icon.
fn brush_icon(b: Brush) -> &'static str {
    match b {
        Brush::Raise => "Raise",
        Brush::Lower => "Lower",
        Brush::Plateau => "Plateau",
        Brush::Smooth => "Smooth",
        Brush::Flatten => "Flatten",
        Brush::Paint => "Paint",
        Brush::Land => "Land",
        Brush::Shade => "Shade",
        Brush::Scatter => "Decor",
        Brush::Gaps => "Gaps",
    }
}

/// The selected island: its ground, colour, edge, surface, size and place, roof.
fn patch_inspector(ui: &mut egui::Ui, ed: &mut Editor, k: usize, contexts: &mut EguiContexts, images: &mut Assets<Image>) {
    let mut p = ed.def.patches[k].clone();
    let before = p.clone();
    let pic = p.texture.and_then(|t| ed.entry(&t)).map(|e| e.thumb.clone()).and_then(|rel| thumb(ed, contexts, images, &rel, &mut 2));
    if inspector_head(ui, ed, "Islands", INK, pic, &format!("Island {}", k + 1)) {
        delete_selected(ed);
        return;
    }
    card(ui, "isl_ground", "Paint", "Ground", true, |ui| {
        if ui.selectable_label(p.texture.is_none(), "Plain").clicked() { p.texture = None; }
        if let Some(id) = texture_grid(ui, ed, contexts, images, p.texture, "patch_textures") { p.texture = Some(id); }
        ui.add(egui::Slider::new(&mut p.tile, 1.0..=64.0).logarithmic(true).text("texture size"));
        if !p.scatter.is_empty() {
            ui.horizontal(|ui| {
                icon(ui, "Decor", 16.0, DIM);
                ui.label(egui::RichText::new(format!("{} plants and details", p.scatter.len())).small());
                if ui.small_button("Clear").clicked() { p.scatter.clear(); }
            });
        }
    });
    card(ui, "isl_colour", "Shade", "Colour", false, |ui| {
        let tex = p.texture;
        colour_section(ui, ed, &mut p.tint, tex, "island");
    });
    card(ui, "isl_edge", "Railings", "Edge", true, |ui| edge_picker(ui, ed, contexts, images, &mut p.edge, "patch_edge"));
    card(ui, "isl_surface", "Ground", "Surface", false, |ui| surface_tiles(ui, &mut p.surface));
    card(ui, "isl_place", "Shape", "Size and place", false, |ui| {
        label_small(ui, &format!("{:.0} × {:.0} units", p.size[0] as f32 * p.spacing, p.size[1] as f32 * p.spacing));
        egui::Grid::new("patch").num_columns(2).show(ui, |ui| {
            for (i, label) in ["X", "Y", "Height"].iter().enumerate() { num_row(ui, label, &mut p.origin[i], 0.25, 0.0..=1000.0); }
            num_row(ui, "Cliff depth", &mut p.depth, 0.25, 0.0..=200.0);
        });
    });
    card(ui, "isl_roof", "Floor", "Roof", false, |ui| {
        let mut roofed = p.roof.is_some();
        if ui.checkbox(&mut roofed, "A roof over it").changed() {
            p.roof = roofed.then(|| rc_map::terrain::Roof { height: 6.0, texture: p.texture, tile: p.tile });
        }
        if let Some(r) = p.roof.as_mut() {
            ui.add(egui::Slider::new(&mut r.height, 0.5..=60.0).text("height"));
            r.height = (r.height * 64.0).round() / 64.0;
            let current = r.texture;
            if ui.selectable_label(current.is_none(), "Plain").clicked() { r.texture = None; }
            if let Some(t) = texture_grid(ui, ed, contexts, images, current, "roof_textures") { r.texture = Some(t); }
        }
    });
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

/// The surfaces as tiles in their Collision-view colours.
fn surface_tiles(ui: &mut egui::Ui, surface: &mut u8) {
    let items: Vec<(u8, &str, &str, &str)> = rc_map::floor::SURFACES.iter().map(|&(b, name, hint)| (b, surface_icon(b), name, hint)).collect();
    choice_tiles(ui, surface, &items, |b| Some(surface_colour(b)), 54.0);
}

/// An object's category in the browser (the research's table; else its catalog kind's).
fn category_of(e: &Entry) -> &'static str {
    if let Some(i) = objects::info(e.id.number) { return i.category_name(); }
    match e.kind.as_str() {
        rc_map::catalog::KIND_ENEMY => "Enemies",
        rc_map::catalog::KIND_PICKUP => "Pickups",
        rc_map::catalog::KIND_PLATFORM => "Platforms & lifts",
        rc_map::catalog::KIND_EFFECT => "Effects",
        _ => "Scenery",
    }
}

/// The asset browser: the catalog's placeable mobys; a click arms one for placing.
fn asset_browser(ui: &mut egui::Ui, ed: &mut Editor, contexts: &mut EguiContexts, images: &mut Assets<Image>) {
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut ed.filter).hint_text("Search…").desired_width(170.0));
        ui.checkbox(&mut ed.live_only, "Live only").on_hover_text("Only objects that fully work on maps");
    });
    // The planets as their globes (All first).
    let planet = ed.planet.map_or("All planets", |p| LEVEL_NAMES[p as usize]);
    card(ui, "objects_planet", "World", &format!("Planet  ·  {planet}"), false, |ui| {
        let texs: Vec<Option<egui::TextureId>> = (0..19u32).map(|l| globe_tex(ed, contexts, images, l)).collect();
        tile_rows(ui, 20, 5, |ui, k, size| {
            let (tex, name, level) = if k == 0 { (None, "All", None) } else { (texs[k - 1], LEVEL_NAMES[k - 1], Some(k as u32 - 1)) };
            if picture_tile(ui, tex, name, name, ed.planet == level, size) { ed.planet = level; }
        });
    });
    let Some(cat) = ed.catalog.clone() else {
        ui.label("No catalog: run rerac --build-catalog");
        return;
    };
    let q = ed.filter.to_lowercase();
    let (live_only, planet) = (ed.live_only, ed.planet);
    // Whether an asset is in a category, with the search (its plain name, its port name, its tags, its id), planet
    // and Live filters.
    let shows = |e: &Entry, category: Option<&str>| {
        let info = objects::info(e.id.number);
        (!live_only || e.live == Some(Live::Live))
            && category.is_none_or(|c| category_of(e) == c)
            && planet.is_none_or(|p| e.ids().any(|id| id.level == p))
            && (q.is_empty()
                || info.is_some_and(|i| i.name.to_lowercase().contains(&q) || i.category_name().to_lowercase().contains(&q))
                || e.name.to_lowercase().contains(&q)
                || e.about.to_lowercase().contains(&q)
                || e.tags.iter().any(|t| t.contains(&q))
                || e.id.to_string().contains(&q))
    };
    let placeable = ed.placeable.clone();
    let mut budget = 24;
    // The categories as picture tiles (a typical object of each), those with something to place.
    let cats: Vec<usize> = (0..CATEGORIES.len()).filter(|&c| placeable.iter().any(|&i| shows(&cat.entries[i], Some(CATEGORIES[c].0)))).collect();
    let pics: Vec<Option<egui::TextureId>> = std::iter::once(None)
        .chain(cats.iter().map(|&c| Some(CATEGORIES[c].2)))
        .map(|n| n.and_then(|n| cat.entries.iter().find(|e| e.id.kind == AssetKind::Moby && e.id.number == n && !e.thumb.is_empty()).map(|e| e.thumb.clone())))
        .collect::<Vec<_>>()
        .into_iter()
        .map(|rel| rel.and_then(|rel| thumb(ed, contexts, images, &rel, &mut budget)))
        .collect();
    tile_rows(ui, cats.len() + 1, 4, |ui, k, size| {
        let key = (k > 0).then(|| CATEGORIES[cats[k - 1]].0);
        if picture_tile(ui, pics[k], key.unwrap_or("All"), key.unwrap_or("Everything"), ed.kind == key, size) { ed.kind = key; }
    });
    if ed.kind.is_some_and(|k| !cats.iter().any(|&c| CATEGORIES[c].0 == k)) { ed.kind = None; }
    // Always one line, so arming an asset does not move the list under the pointer.
    match ed.armed.map(|e| cat.entries[e].name.clone()) {
        Some(e) => ui.label(egui::RichText::new(format!("Placing {e}: click in the world")).color(AMBER)),
        None => ui.label(egui::RichText::new("Pick one, then click in the world").color(DIM)),
    };
    let kind = ed.kind;
    let rows: Vec<usize> = placeable.iter().copied().filter(|&i| shows(&cat.entries[i], kind)).collect();
    // The objects as picture tiles, three a row (the status dot: green works, yellow needs work, red unported).
    let per_row = 3;
    let gap = 6.0;
    let size = ((ui.available_width() - 14.0 - gap * (per_row as f32 - 1.0)) / per_row as f32).floor().clamp(60.0, 120.0);
    egui::ScrollArea::vertical()
        // A new filter starts at the top of its list.
        .id_salt(("rac_maker_assets_list", ed.kind, ed.planet, ed.live_only))
        .auto_shrink([false; 2])
        .scroll_source(egui::scroll_area::ScrollSource::MOUSE_WHEEL | egui::scroll_area::ScrollSource::SCROLL_BAR)
        .show_rows(ui, size + gap, rows.len().div_ceil(per_row), |ui, range| {
            for row in range {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for &i in rows.iter().skip(row * per_row).take(per_row) {
                        let e = &cat.entries[i];
                        let tex = thumb(ed, contexts, images, &e.thumb, &mut budget);
                        let armed = ed.armed == Some(i);
                        let name = objects::info(e.id.number).map_or_else(|| if e.name.is_empty() { e.id.to_string() } else { e.name.clone() }, |i| i.name.to_string());
                        let hint = format!("{name}\n{}", if e.about.is_empty() { e.id.to_string() } else { e.about.clone() });
                        let clicked = picture_tile(ui, tex, &name, &hint, armed, size);
                        let dot = match (e.live, e.behaviour) {
                            (Some(Live::Live), _) | (_, Some(Behaviour::Ported)) => GREEN,
                            (Some(Live::NeedsWork), _) => AMBER,
                            (_, Some(Behaviour::Static)) => DIM,
                            _ => RED,
                        };
                        let r = ui.min_rect();
                        let corner = egui::pos2(r.right() - 9.0, r.top() + 9.0);
                        ui.painter().circle_filled(corner, 4.0, dot);
                        if clicked {
                            ed.armed = if armed { None } else { Some(i) };
                            ed.selected = None;
                            ed.status = if armed { "Placing cancelled".into() } else { format!("Click in the world to place {name}") };
                        }
                    }
                });
                ui.add_space(gap);
            }
        });
}

/// The selected object: where it stands, its look, its part in the rules, its body, its path, its sight; copies.
fn object_inspector(ui: &mut egui::Ui, ed: &mut Editor, i: usize, contexts: &mut EguiContexts, images: &mut Assets<Image>, cam: Option<&GlobalTransform>) {
    let id = ed.def.objects[i].asset;
    let entry = ed.entry(&id).cloned();
    let pic = entry.as_ref().and_then(|e| thumb(ed, contexts, images, &e.thumb, &mut 4));
    let title = ed.def.objects[i].name.clone().unwrap_or_else(|| ed.name_of(&id));
    if inspector_head(ui, ed, "Objects", INK, pic, &title) {
        delete_selected(ed);
        return;
    }
    // Its category, status and file (the moby id, small: "rac1/L00/moby/1185").
    let info = objects::info(id.number);
    ui.horizontal_wrapped(|ui| {
        if let Some(i) = info {
            icon(ui, CATEGORIES[i.category as usize].1, 14.0, ORANGE);
            ui.label(egui::RichText::new(i.category_name()).small().color(ORANGE));
        }
        if let Some(e) = &entry { badge(ui, e); }
        ui.label(egui::RichText::new(id.to_string()).monospace().small().color(DIM));
    });
    // What it needs to work on a map (research: reports/RAC1 objects for RAC Maker.md).
    if let Some(i) = info {
        let pathed = crate::map_load::PATH_CLASSES.contains(&id.number);
        let notes: [(bool, &str, &str, &str); 4] = [
            (i.has(objects::NEEDS_LINK), "Needs its partner", "Link", "It works with another object (a switch, a crank, a door, a lock) that maps cannot link yet: alone it may not move or may stay open"),
            (i.has(objects::NEEDS_PATH) && !pathed, "Uses its planet's route", "Paths", "It follows a route from its home planet; on a map it may wander or stand still"),
            (i.has(objects::NEEDS_GROUP), "Works in a group", "Objects", "It needs other objects of its group (its set of switches, its swarm)"),
            (i.has(objects::STORY_SIDE_EFFECTS), "Story object", "Warn", "In the game it gives items, unlocks planets or saves: on a map it may do that too"),
        ];
        for (on, text, icon_name, hint) in notes {
            if !on { continue; }
            ui.horizontal(|ui| {
                icon(ui, if icon_name == "Link" { "Shape" } else { icon_name }, 14.0, AMBER);
                ui.label(egui::RichText::new(text).small().color(AMBER)).on_hover_text(hint);
            });
        }
        // How it breaks (what breaks it, or what alone does in the game: a boss, a story shot).
        if !i.breaks.is_empty() {
            let by_ratchet = !i.breaks.starts_with("Not by Ratchet");
            ui.horizontal_wrapped(|ui| {
                icon(ui, "Gaps", 14.0, if by_ratchet { GREEN } else { AMBER });
                ui.label(egui::RichText::new(i.breaks).small().color(if by_ratchet { GREEN } else { AMBER }));
            });
        }
    }
    // What it is (the research's, else the catalog's).
    let about = info.map(|i| i.about).filter(|a| !a.is_empty()).or(entry.as_ref().map(|e| e.about.as_str()).filter(|a| !a.is_empty()));
    if let Some(a) = about { ui.label(egui::RichText::new(a).small().color(DIM)); }
    // Where the game put it, if it moves itself.
    if let Some(at) = ed.moved_away.get(&ed.ids[i]).copied() {
        ui.horizontal(|ui| {
            icon(ui, "Warn", 16.0, RED);
            ui.label(egui::RichText::new("Starts at the red ball").color(RED)).on_hover_text(format!(
                "The game moves it there when the level starts ({:.0}, {:.0}, height {:.0}): its own code, not you. Place it where the line ends, or pick another enemy.",
                at[0], at[1], at[2]
            ));
        });
    }
    ui.add_space(4.0);
    let mut ob = ed.def.objects[i].clone();
    let mut edited = false;
    card(ui, "obj_place", "Start", "Place", true, |ui| {
        egui::Grid::new("object").num_columns(2).show(ui, |ui| {
            for (k, label) in ["X", "Y", "Height"].iter().enumerate() {
                edited |= num_row(ui, label, &mut ob.position[k], 0.1, 0.0..=1023.0);
            }
            ui.label("Facing");
            let mut deg = ob.yaw.to_degrees();
            if ui.add(egui::Slider::new(&mut deg, 0.0..=359.0).suffix("°")).changed() {
                ob.yaw = deg.to_radians();
                edited = true;
            }
            ui.end_row();
        });
    });
    card(ui, "obj_look", "Shade", "Colour", false, |ui| {
        ui.horizontal(|ui| {
            let mut tinted = ob.tint.is_some();
            if ui.checkbox(&mut tinted, "Tint").changed() {
                ob.tint = tinted.then_some([255, 200, 90]);
                edited = true;
            }
            if let Some(c) = ob.tint.as_mut() { edited |= ui.color_edit_button_srgb(c).changed(); }
        });
    });
    card(ui, "obj_logic", "Logic", "Rules", false, |ui| {
        ui.horizontal(|ui| {
            ui.label("Name");
            let mut name = ob.name.clone().unwrap_or_default();
            let r = ui.add(egui::TextEdit::singleline(&mut name).hint_text("for the rules").desired_width(150.0));
            if r.changed() {
                // Spaces kept while typing ("Gate switch"); the ends trimmed when the field is left.
                ob.name = (!name.trim().is_empty()).then_some(name);
                edited = true;
            } else if r.lost_focus() && ob.name.as_deref().is_some_and(|n| n != n.trim()) {
                ob.name = ob.name.as_deref().map(|n| n.trim().to_string());
                edited = true;
            }
        });
        if ui.checkbox(&mut ob.hidden, "Hidden until a rule shows it").changed() {
            if ob.hidden && ob.name.is_none() { ob.name = Some(format!("{} {}", ed.name_of(&id), i + 1)); }
            edited = true;
        }
    });
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
    // Solid from its model (many pieces are solid on the disc only through the level geometry around them).
    let own = entry.as_ref().is_some_and(|e| e.collision) && !crate::map_load::BAKE_ANYWAY.contains(&id.number);
    card(ui, "obj_body", "Collision", "Body", false, |ui| {
        if own {
            ui.horizontal(|ui| {
                icon(ui, "Collision", 16.0, GREEN);
                ui.label("Solid by itself").on_hover_text("Its own collision: it moves, breaks and disappears with it");
            });
        }
        let mut solid = ed.def.objects[i].solid.unwrap_or(false);
        let (label, hint) = if own {
            ("Extra solid from its model", "Also builds collision from its whole shape, where it stands now. That copy stays put: if the object moves, breaks or hides, the extra collision is still there")
        } else {
            ("Solid", "Ratchet cannot walk through it (leave it off for things that move or break)")
        };
        if ui.checkbox(&mut solid, label).on_hover_text(hint).changed() {
            ed.checkpoint();
            ed.def.objects[i].solid = Some(solid);
            ed.changed();
        }
    });
    // A mover or a patrolling enemy: its path.
    if crate::map_load::PATH_CLASSES.contains(&id.number) {
        card(ui, "obj_path", "Paths", "Path", true, |ui| path_settings(ui, ed, i));
    }
    // An enemy's sight.
    if entry.as_ref().is_some_and(|e| e.kind == rc_map::catalog::KIND_ENEMY) {
        if let Some((wake, range)) = ed.sight(id) {
            card(ui, "obj_sight", "View", "Sight", true, |ui| {
                let dot = |ui: &mut egui::Ui, c: egui::Color32| { let (r, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover()); ui.painter().circle_filled(r.center(), 5.0, c); };
                ui.horizontal(|ui| {
                    dot(ui, egui::Color32::from_rgb(0x66, 0xb3, 0xff));
                    ui.label(if wake > 0.0 { format!("Notices {wake:.0}") } else { "Notices by area".into() });
                    ui.add_space(8.0);
                    dot(ui, RED);
                    ui.label(if range > 0.0 { format!("Attacks {range:.1}") } else { "Attacks on sight".into() });
                });
            });
        }
    }
    if info.is_some_and(|i| i.category_name() == "Breakables") {
        card(ui, "obj_hides", "Gaps", "Hides", true, |ui| hides_section(ui, ed, i, contexts, images));
    }
    // Copies next to it: the directions as seen from the camera (snapped to the world's axes).
    card(ui, "obj_copy", "Plus", "Copy beside it", false, |ui| {
        let fwd = cam.map_or([0.0, 1.0], |c| {
            let f = to_game(c.forward().as_vec3());
            if f[0].abs() > f[1].abs() { [f[0].signum(), 0.0] } else { [0.0, f[1].signum()] }
        });
        let right = [fwd[1], -fwd[0]];
        let mut add = None;
        ui.horizontal_wrapped(|ui| {
            if ui.button("On top").clicked() { add = Some([0.0, 0.0, 1.0]); }
            if ui.button("Left").clicked() { add = Some([-right[0], -right[1], 0.0]); }
            if ui.button("Right").clicked() { add = Some([right[0], right[1], 0.0]); }
            if ui.button("In front").clicked() { add = Some([-fwd[0], -fwd[1], 0.0]); }
            if ui.button("Behind").clicked() { add = Some([fwd[0], fwd[1], 0.0]); }
        });
        if let Some(d) = add { ed.add_beside(i, d); }
    });
}

/// What a breakable hides (its "Hides" card): an object behind it, hidden until it breaks (a rule: when it is
/// destroyed, show it). The pick: a gold bolt, a crate or any other object.
fn hides_section(ui: &mut egui::Ui, ed: &mut Editor, i: usize, contexts: &mut EguiContexts, images: &mut Assets<Image>) {
    use rc_map::logic::{Rule, Then, When};
    const HIDEABLE: [(i32, &str); 5] = [(1134, "Gold Bolt"), (500, "Bolt crate"), (511, "Ammo crate"), (501, "Nanotech crate"), (505, "TNT crate")];
    let name = ed.def.objects[i].name.clone();
    // What it hides now: the objects a "when destroyed, show" rule of it shows.
    let hidden: Vec<String> = name.as_ref().map(|n| ed.def.rules.iter().filter(|r| r.when == When::Destroyed(n.clone())).flat_map(|r| r.then.iter().filter_map(|t| if let Then::Show(o) = t { Some(o.clone()) } else { None })).collect()).unwrap_or_default();
    for h in &hidden {
        ui.horizontal(|ui| {
            icon(ui, "Plus", 14.0, GREEN);
            ui.label(h);
            if icon_button_ink(ui, "Bin", "Hide nothing there any more (the object stays, shown)", false, RED).clicked() {
                ed.checkpoint();
                let n = name.clone().unwrap_or_default();
                ed.def.rules.retain(|r| !(r.when == When::Destroyed(n.clone()) && r.then == vec![Then::Show(h.clone())]));
                if let Some(o) = ed.def.objects.iter_mut().find(|o| o.name.as_deref() == Some(h.as_str())) { o.hidden = false; }
                ed.changed();
            }
        });
    }
    label_small(ui, "Put behind it, shown when it breaks:");
    let cat = ed.catalog.clone();
    let mut budget = 6;
    let texs: Vec<Option<egui::TextureId>> = HIDEABLE.iter().map(|(n, _)| {
        cat.as_ref().and_then(|c| c.entries.iter().find(|e| e.id.kind == AssetKind::Moby && e.id.number == *n).map(|e| e.thumb.clone())).and_then(|rel| thumb(ed, contexts, images, &rel, &mut budget))
    }).collect();
    let mut pick = None;
    tile_rows(ui, HIDEABLE.len(), 3, |ui, k, size| {
        if picture_tile(ui, texs[k], HIDEABLE[k].1, HIDEABLE[k].1, false, size) { pick = Some(k); }
    });
    let Some(k) = pick else { return };
    let Some(asset) = cat.as_ref().and_then(|c| c.entries.iter().find(|e| e.id.kind == AssetKind::Moby && e.id.number == HIDEABLE[k].0).map(|e| e.id)) else { return };
    ed.checkpoint();
    // The breakable named (for the rule), the thing put just behind it (away from the camera side it faces: its back).
    let wall = name.unwrap_or_else(|| {
        let n = (1..).map(|k| format!("{} {k}", ed.name_of(&ed.def.objects[i].asset))).find(|n| ed.def.objects.iter().all(|o| o.name.as_deref() != Some(n))).unwrap();
        ed.def.objects[i].name = Some(n.clone());
        n
    });
    let o = ed.def.objects[i].clone();
    let back = [o.position[0] - o.yaw.cos() * 1.5, o.position[1] - o.yaw.sin() * 1.5, o.position[2]];
    let base = format!("{} behind {wall}", HIDEABLE[k].1);
    let thing = (1..).map(|n| if n == 1 { base.clone() } else { format!("{base} {n}") }).find(|n| ed.def.objects.iter().all(|o| o.name.as_deref() != Some(n))).unwrap();
    ed.add_object(Object { asset, position: back, yaw: o.yaw, template: None, group: None, path: Vec::new(), time: None, solid: None, name: Some(thing.clone()), hidden: true, tint: None });
    ed.def.rules.push(Rule { when: When::Destroyed(wall), then: vec![Then::Show(thing.clone())], cutscene: false });
    ed.status = format!("{thing}: hidden until it breaks (see Logic)");
    ed.changed();
}

/// Nothing selected: the map itself: its name, where Ratchet starts (or the ship lands), its rules, the loadout, the
/// floors.
fn map_inspector(ui: &mut egui::Ui, ed: &mut Editor, contexts: &mut EguiContexts, images: &mut Assets<Image>) {
    let before = ed.def.clone();
    let mut m = ed.def.clone();
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(52.0, 52.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 10.0, TILE);
        draw_icon(ui.painter(), rect.center(), 30.0, "Map", ORANGE);
        ui.vertical(|ui| {
            ui.add(egui::TextEdit::singleline(&mut m.name).font(egui::TextStyle::Heading).desired_width(200.0)).on_hover_text("The map's name");
            ui.label(egui::RichText::new(format!("on {}", LEVEL_NAMES[m.host_level as usize])).small().color(DIM));
        });
    });
    ui.add_space(8.0);
    card(ui, "map_start", "Start", "Start", true, |ui| {
        // On foot, or Ratchet's ship landing (and taking him away after the goal).
        let mut ship = m.ship.is_some();
        let was = ship;
        choice_tiles(ui, &mut ship, &[(false, "Start", "On foot", "Ratchet starts here"), (true, "Ship", "Ship", "The ship lands and Ratchet steps out; after the goal he flies off in it")], |_| None, 62.0);
        if ship != was {
            let (s, c) = m.start.yaw.sin_cos();
            m.ship = ship.then(|| rc_map::Start { position: [m.start.position[0] + c * 6.0, m.start.position[1] + s * 6.0, m.start.position[2] - 0.5], yaw: m.start.yaw });
        }
        let place = match m.ship.as_mut() { Some(sh) => sh, None => &mut m.start };
        egui::Grid::new("start").num_columns(2).show(ui, |ui| {
            for (k, label) in ["X", "Y", "Height"].iter().enumerate() { num_row(ui, label, &mut place.position[k], 0.1, 0.0..=1023.0); }
            ui.label("Facing");
            let mut deg = place.yaw.to_degrees().rem_euclid(360.0);
            if ui.add(egui::Slider::new(&mut deg, 0.0..=359.0).suffix("°")).changed() { place.yaw = deg.to_radians(); }
            ui.end_row();
        });
        label_small(ui, "Drag the blue ring in the world (Select tool)");
        if m.ship.is_some() && !m.zones.iter().any(|z| matches!(z.kind, rc_map::logic::ZoneKind::Goal)) && !m.rules.iter().any(|r| r.then.iter().any(|t| matches!(t, rc_map::logic::Then::Goal))) {
            ui.horizontal(|ui| { icon(ui, "Warn", 16.0, AMBER); ui.label(egui::RichText::new("No goal yet (Logic)").color(AMBER)); });
        }
    });
    if m.ship.is_some() && m.ship != before.ship {
        let root = crate::level_load::extracted_root();
        if let Some((p, yaw)) = rc_data::level_gameplay(&root, m.host_level).ok().and_then(|g| crate::map_load::ship_start(&m, &g)) {
            m.start = rc_map::Start { position: p.map(|v| (v * 64.0).round() / 64.0), yaw };
        }
    }
    card(ui, "map_rules", "Logic", "Play", true, |ui| {
        ui.horizontal(|ui| {
            icon(ui, "Death", 18.0, RED);
            ui.label("Falls to death below");
            let mut d = m.death_height.unwrap_or(0.0);
            if ui.add(egui::DragValue::new(&mut d).speed(0.25)).on_hover_text("The red plane under the map").changed() { m.death_height = Some(d); }
        });
        ui.horizontal(|ui| {
            icon(ui, "Rails", 18.0, INK);
            let mut rails = m.abilities.rails.unwrap_or(false);
            if ui.checkbox(&mut rails, "Grind rails").changed() { m.abilities.rails = Some(rails); }
        });
        ui.horizontal(|ui| {
            icon(ui, "Refresh", 18.0, INK);
            ui.checkbox(&mut m.reset_on_death, "Reset the map after a death");
        });
    });
    card(ui, "map_loadout", "Loadout", &format!("Loadout  ·  {}", m.loadout.len()), false, |ui| {
        egui::Grid::new("loadout").num_columns(2).spacing([6.0, 2.0]).show(ui, |ui| {
            for (n, (id, name)) in ITEMS.iter().enumerate() {
                let mut on = m.loadout.contains(id);
                if ui.checkbox(&mut on, *name).changed() {
                    if on { m.loadout.push(*id); } else { m.loadout.retain(|i| i != id); }
                }
                if n % 2 == 1 { ui.end_row(); }
            }
        });
    });
    card(ui, "map_floors", "Floor", &format!("Floors  ·  {}", m.floors.len()), false, |ui| {
        let mut remove = None;
        let mut tex_for = None;
        egui::Grid::new("floors").num_columns(5).spacing([4.0, 4.0]).show(ui, |ui| {
            for t in ["", "from x, y", "to x, y", "height", ""] { label_small(ui, t); }
            ui.end_row();
            for (k, f) in m.floors.iter_mut().enumerate() {
                if ui.selectable_label(ed.floor_edit == Some(k), format!("{}", k + 1)).on_hover_text("Its texture").clicked() { tex_for = Some(k); }
                ui.horizontal(|ui| { for v in f.min.iter_mut() { ui.add(egui::DragValue::new(v).speed(4.0).range(0.0..=1020.0)); } });
                ui.horizontal(|ui| { for v in f.max.iter_mut() { ui.add(egui::DragValue::new(v).speed(4.0).range(0.0..=1020.0)); } });
                ui.add(egui::DragValue::new(&mut f.height).speed(0.25).range(0.0..=1000.0));
                if icon_button_ink(ui, "Bin", "Remove this floor", false, RED).clicked() { remove = Some(k); }
                ui.end_row();
            }
        });
        if let Some(k) = tex_for { ed.floor_edit = if ed.floor_edit == Some(k) { None } else { Some(k) }; }
        if let Some(k) = ed.floor_edit.filter(|&k| k < m.floors.len()) {
            ui.add(egui::Slider::new(&mut m.floors[k].tile, 1.0..=64.0).text("texture size").logarithmic(true));
            if ui.selectable_label(m.floors[k].texture.is_none(), "Plain").clicked() { m.floors[k].texture = None; }
            if let Some(id) = texture_grid(ui, ed, contexts, images, m.floors[k].texture, "floor_textures") { m.floors[k].texture = Some(id); }
        }
        if let Some(k) = remove {
            m.floors.remove(k);
            ed.floor_edit = None;
        }
        if ui.button("+ Floor").on_hover_text("A flat floor under the start").clicked() {
            let s = m.start.position;
            let (x, y) = ((s[0] / 4.0).floor() * 4.0, (s[1] / 4.0).floor() * 4.0);
            m.floors.push(rc_map::Floor { min: [x - 16.0, y - 16.0], max: [x + 16.0, y + 16.0], height: (s[2] - 0.5).floor(), texture: None, tile: 8.0 });
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
