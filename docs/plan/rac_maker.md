# RAC Maker: custom maps

Status: **Phase 0 (proof of concept) working, 2026-10-22.** RAC Maker is a level editor that builds new planets from
the game's own parts; the full design (editor, catalog, terrain, colour, rails, sequel-ready foundations, launcher
tile) is the design doc: <https://claude.ai/artifact/JhSj2E6YaTtDP3SgyLUM66>. This page records what the engine does
today.

## Running a map

```sh
cargo dev -- --data-dir <data> --map crates/rc-map/maps/phase0.json
```

`--map` (crate::disc_source) plays a map on its host level instead of the front end. `--version-json` reports
`"maker_format":1`: a runtime without it cannot play maps (0.3.4-era builds ignore `--map` silently).

The example map (`crates/rc-map/maps/phase0.json`) is a flat 80 × 80 floor on Novalis with Novalis's stone texture,
one grind rail, a Novalis bolt crate (class 500) and **Gaspar's pack biter** (class 193, `rac1/L09/moby/193`).

## The map file (`crates/rc-map`)

JSON, `maker_format` 1. Asset ids are `game/Lnn/kind/number` (`rac1/L09/moby/193`, `rac1/L01/tfrag-tex/13`), so the
id scheme already names the game and the source level (GC / UYA / DL are reserved names). Unknown keys are ignored,
so newer maps still read on older runtimes. Fields: `name`, `host_level`, `start`, `death_height`, `loadout` (item
ids), `abilities` (`rails`, `cables`), `reset_on_death`, `terrain` (`floor` boxes), `rails`, `objects` (`asset`, `position`, `yaw`,
optional `template`: the source level's instance whose record and pvars are copied).

## What the engine does with it (crate::map_load)

1. **Gameplay file** (`rc_map::patch`): the host's decompressed `gameplay_ntsc` with new sections appended and their
   header pointers moved. Moby instances: Ratchet (the host's record, at the map's start), then the objects. Pvars are
   copied from each template with its moby links and shared-data words cut and its pointer fixups kept; an object
   with death bits gets a fresh spawn id and flag 8, so its bolts drop (flags 0 skips `set_death_bits`). Groups,
   level cameras, sound instances, shared data, link fixups and occlusion mappings are absent (the host's cameras and
   sound instances index the host's pvar table: left in, they stopped the whole audio load); grind paths are the
   map's rails; level settings get the death height and no loader ship. Every other section stays the host's.
2. **Collision** (`rc_map::floor`): the floors as cell quads, plus a 0.3-wide strip under every rail. The strip is
   needed: on the disc rails are ties whose collision is in the level mesh, and the grind code drops Ratchet off a
   rail when he is more than 0.87 above the ground for 30 ticks (rc_game::hero::boots).
3. **Imports**: each object from another level brings its moby class (model, animation and textures, with the
   textures appended to the host's moby texture table under new indices), its update (`LevelPorts::import`: what
   the source level's class table runs for it), its reaction tables, its collision blob, and its joint lists and gait
   records (`gameplay::class_blobs` reads a class's blob from the level that holds it). A class number the host
   already uses is refused.
4. **Ratchet's animations** are completed from the other levels (`map_load::complete_ratchet_sequences`): a level
   carries only the sequences its moves need (Novalis has none of the grind sequences 47..55, so he grinds in the
   bind pose); 88 on Novalis, 134 with the others.
5. **Hero**: `rc_game::hero::Abilities` replaces the level-number rail / cable rules (`RAIL_LEVELS`, `CABLE_LEVELS`;
   Novalis has no rails) with the map's; the loadout is granted like `RC_GIVE_ITEMS`.
6. **Not drawn**: the host's tfrags, ties and shrubs. The floors and rails are drawn by `MapRenderPlugin` (unlit
   textured meshes). The host's sky stays.
7. **Saves**: the memory card is off unless `RC_SAVE_DIR` is set, so a map never writes the player's saves.
8. **Death**: with `"reset_on_death"` (default true) a death reload drops this visit's death, kill and collected bits,
   so every object comes back as at the first load (the checkpoint record stays); false keeps the game's rule (what
   was broken this visit stays gone).

Fixes that came with it (also right without maps): `load_mobys` read `gs_ram.bin` of the *current* level instead of
the level being loaded (the chrome / glass maps of a background level switch came from the old level); the spawn
pass and the fog state re-read the gameplay file and re-parsed the collision from disk instead of using the loaded
level's.

## Verified (2026-10-22, scripted runs with `RC_PLAY_SCRIPT` / `RC_PLAY_TRACE`, and played with a DualSense)

- Ratchet ground-snaps onto the map floor and walks on it.
- He jumps onto the drawn rail, grinds it end to end (state 0x28, sparks), flies off its open end and lands.
- The imported pack biter runs its port (`Unit(42)`): it notices him, charges and bites (HP 4 → 1 over ~4 s), and
  dies to the wrench combo and the Bomb Glove (its death pieces, then deleted).
- The crate breaks and drops bolts (8 collected); falling off the floor kills Ratchet and respawns him; audio plays.

## Phase 1: the asset catalog

```sh
cargo dev -- --data-dir <data> --build-catalog   # catalog.json, thumbnails, index.html (about 20 s)
cargo dev -- --data-dir <data> --spawn-test      # every ported, placed moby class on test maps (about 15 min)
```

**The index** (`rc_map::catalog`, built by crate::catalog, 8 levels at a time) covers every moby, tie and shrub
class, the sky and every tfrag texture of the 19 levels. Assets with the same content (class blob plus the pixels of
the textures it uses) are one entry, the other levels in `also`. First run (2026-10-22): 5,498 entries from 7,659
finds: 1,855 mobys, 1,727 ties, 460 shrubs, 19 skies, 1,437 tfrag textures. It is built by the engine, not
`rerac-extract`, because the behaviour status needs the class ports.

**Behaviour.** Mobys carry `ported` (the level's class table names an update the port runs, by the same `LevelPorts`
matching the game load uses), `static` (no update) or `unported` (1,254 / 218 / 383; only 4 unported classes are
placed: the rest are made by other classes' code). The port's name is a unit port's module and its `units` index
description (read from `units/mod.rs` at compile time, keyed by module: census ids repeat across runs).

**Names and tags.** Classes nothing names get a description from their model: size (tiny..huge by the bounding box),
shape (column, platform, beam, block), the mean colour of their textures, and the planet: "Aridia large beam",
"Blarg Station medium plant" (shrubs: plant when green, else detail). The same words are the entry's tags.

**Thumbnails** (`rc_extract::thumbs`): a CPU rasteriser over the class-space triangles the glTF exports decode (mobys
in the bind pose, high LOD; ties LOD 0; shrubs), three-quarter view, textured, 2× supersampled, 96 px PNG; skies show
their largest texture, tfrag textures themselves. Named by content hash (`thumbs/<kind>/<hash>.png`), so each is
drawn once and a rebuild keeps the files. 4,823 thumbnails, 49 MB; the mobys without one have no model (logic
classes, buried bolts …).

**The catalog page** (`<data>/catalog/index.html`, from `catalog_viewer.html` with the catalog embedded): tabs per
kind, planet, status and text filters, cards with thumbnails and badges, a detail panel. Serve the folder or open the
file.

**The spawn test** (crate::spawn_test): the classes the game places and the port runs, with a model, go on test maps
(10 in a ring around Ratchet on a flat floor), each map hosted by a level that places both Ratchet and the class
(that level's instance is the template), and run as a child game (`RC_SPAWN_REPORT`, `RC_SPAWN_TICKS`, windowed,
uncapped, silent) for 600 ticks. The child reports per class the updates run and the unported calls met while its
update ran (`FxStats::class_runs` / `class_unported`, kept by `scheduler::run_moby` and `Services::unported`). A
failed or timed-out map is split in two until the failing class is alone. Verdicts go into the catalog (`live` /
`needs-work` with a note) and survive `--build-catalog`.

Classes already live are skipped on a rerun unless `RC_SPAWN_ALL=1`.

First run (2026-10-22): 581 classes in 68 maps (21 min, 3 at a time): **578 live**, 3 need work: Gaspar's chain links
1181 and tethered platforms 1182 (they look for their neighbour pieces: the debug prints the game has for a lone
piece), and Blarg Station's glass 1083 (its map times out without a crash: the tick stops, likely a scene it starts).

A child takes about 24 s in a dev build, 10 s of it matching level overlays (`level_ports` and `level_reactions`):
a disk cache of each level's port bindings (they only change with the ReRAC version) would cut map loads to well
under a second of that.

## Phase 2: the editor (`--edit`)

`cargo dev -- --data-dir <data> --edit <map.json>` opens the map in the editor (crate::editor); a missing file starts
as `MapDef::new_default` (an 80×80 floor on Novalis). The editor is the game with the tick frozen
(`gameplay::tick.run_if(editor::playing)`), on the fly camera, with `bevy_egui` 0.42 panels on the window (the egui
context sits on the present camera, crate::display) in the launcher's navy and orange:

- **Assets** (left): the catalog's placeable mobys (thumbnails, search, type chips, planet chips, Live only). The
  types are the catalog's `kind` (`catalog::kind_of`: crates and pickups by their group and "breakable"; enemies and
  creatures by a creature reaction table, `gameplay::own_level_reactions`, or an enemy's name; effects, machines and
  scenery by the words of the port and name, never the description, which lists the classes a port also runs).
  egui's click-to-open popups (ComboBox) do not open in this setup, so filters are chips. Click one, then click in
  the world: a green box shows where it would go. On an object's top it is stacked, on its side it goes next to it,
  else on the floor under the pointer; anything it would overlap pushes it out. It is copied from the first level that
  places the class (the host when it has it).
- **Inspector** (right): the selected object (position, facing; "Add another" on top / left / right / in front /
  behind, the sides as seen from the camera, past copies already there and down onto what is under them; delete),
  else the map (name, death height, rails, reset on death, start, floors, loadout).
- **Viewport**: right mouse look, WASD fly (Q / Space down / up; the cursor shows while editing, and the wheel and
  keys are the panels' while the pointer is over them or a field is typed in); click selects, drag moves on the
  object's height (out of other objects), R / Shift+R turn 15°, Delete removes, Esc stops placing. Ctrl+Z / Ctrl+Y
  undo / redo (whole-map snapshots; an inspector drag is one step), Ctrl+S saves.
- **Play (F5)**: the tick runs on the loaded level and the play camera follows Ratchet; F5 again reloads the map and
  goes back to editing, at the view from before Play. While editing the level is never ticked, so Play needs no
  reload; Play asked for during a reload waits for it (the panels are locked meanwhile; F5 again cancels).

Every change hands the map to the loader (`map_load::set`) and reloads the host in the background
(`LevelChange::start_load` + `swap`): **~0.6 s** in a dev build after the first load (the level load itself ~200 ms;
`map: level load` prints its stages). The first load of a planet takes ~8 s (particles and the water data's overlay
relocations); the water data's overlay part is kept per level for the process (`LevelWaterData::from_overlay` +
`read_gameplay`, crate::water_render), which took reloads from 5.6 s to 0.6 s. A load that fails puts the last good
map back and says why (`LevelChange::last_error`).

**At once, before the reload.** Every object has an editor id; the loaded level's static instance `k + 1` is the
object the reload loaded as `loaded.ids[k]`. A moved or turned object's instance is redrawn there and a removed one
hidden (`gameplay::editor_show` → `MobyOcclusion::drive`, only the drawing), and a new object shows as its box and
thumbnail until the reload draws it.

**Crate stacks.** Crates fall onto the crate below only within one moby group (rc_game `crate_` links a group's
stack at its init; a break drops the crates linked above). Objects carry `group` in the map file
(`GameplayPatch::groups` writes the 0x48 section and each record's +0x48); the editor puts every set of touching crates
(classes 500 / 501 / 502 / 505 / 511, stacked or side by side) in one group before each load and save
(`Editor::group_crates`, also when a map is opened). Verified with `RC_PLAY_SCRIPT` wrench hits on a 3-crate stack:
each crate drops to the floor when the one under it breaks.

**Sizes.** `catalog.json` entries carry `bounds` (the model's box × its first placement's scale, game axes, z up;
the bolt crate is 1×1×1). Boxes are drawn for every object (orange: selected; pale orange: not loaded yet).

Verified (2026-10-22, scripted input on the real window): place, stack by clicking a crate's top, "On top", "Right"
×2 (the copies land on the floor), drag (the model follows), Play during a reload (waits, then plays), Play / Stop
(view restored), save; the cursor shows while editing (GetCursorInfo); 60 fps; no render errors.

Fixed on the way: a level swap after the level had ticked could draw an effect item (an Rgba8Unorm effect pipeline)
in the main pass and abort on a wgpu validation error; `display_blend::split_effects` now also takes items by their
pipeline's target format.

Editor gaps: objects whose class starts hidden until its update runs (the amoeboid) show only their box while
editing; a crate left in the air by a drag falls in the game (the editor keeps the dragged height); rails and floors are edited as numbers (no drawing yet); the egui panels cover the frame's edges (the
present camera's viewport is not shrunk to the free area).

## Phase 3: terrain (first slice: sculpted patches)

**Patches** (`rc_map::terrain::Patch`, `"kind": "patch"` in the map's `"terrain"`): a grid of height points over a
rectangle (`origin` corner and base height, `spacing` 1 / 2 / 4 units, `size` up to 128 × 128 cells, `heights` per
point above the base at 1/64), a `texture` (any planet's tfrag texture) and `tile`, and a `depth`: walls from every
edge point down by that much, closed underneath (a raised patch with walls is a floating island or a mesa; 0 is an
open sheet). Its top is two triangles per cell (split along the (0, 0)–(1, 1) diagonal, wound like the floors' quads).

**Collision** (`rc_map::floor::collision`, now one builder for floors, patches and rails): every face goes into each
4-unit cell its box overlaps (a flat face on a cell boundary into both layers, as the disc stores floors), cells share
vertices, quads first, then triangles; walls are cut into pieces at most 8 units tall so every corner stays within the
packed vertex range (±32 units of the cell centre); a face that would not fit (255 vertices, range) is left out and
counted on stderr. Verified in game: Ratchet stands and walks on a sculpted hill and the patch top.

**Drawing** (crate::map_load): a patch's mesh has smooth normals and the PS2 look of baked light in vertex colours
(0.5 ambient + 0.55 × the sun on the normal, the walls a little darker), world-planar UVs; the renderer redraws at once
when the map's terrain or rails change (`redraw_changed` compares `map_load::active()`), and keeps decoded textures.

**Editor** (Terrain tab): New patch (16 / 32 / 64 / 128 cells, detail 1 / 2 / 4, a unit above the ground in front of
the camera, with 6 units of cliff); brushes Raise / Lower / Smooth / Flatten with radius and strength (Shift: the other
way; a soft falloff); dragging on a patch sculpts it, redrawn every frame, and the reload with the new collision runs
when the stroke ends (one undo step per stroke). The patch inspector: position, base height, cliff depth, texture size,
a texture grid (the catalog's tfrag textures of the planet chosen in Objects → Planet, else the host's), delete.
Objects are placed on the terrain under the pointer and settle onto the ground under them (not a cliff top above).

Islands (2026-10-24): a patch may carry a land mask (a value per grid point in eighths, above 0 is land, the border
row -1) cut by marching squares into land polygons: the top is their triangles, the cliff walls and the cap under them
follow the outline, and an optional edge (Railing, Wall, Invisible; 0.3–4 high) stands along it, in the collision as
upright quads whatever it looks like. An unmasked patch is all land (its rim is the outline). The Terrain tab has two
modes: Build islands (shape Round/Square/Blob, across, height above the ground clicked, cliff depth, edge, texture; a
wireframe of the island follows the pointer; a click builds it, on the ground or in the air) and Sculpt (the brushes,
plus Land: grows the mask, Shift cuts it; a patch without a mask gets one on the first Land stroke). The brush outline
is laid on the ground. A new map's ground is a 40×40 patch (spacing 2) of rac1/L01 tfrag texture 8, not a floor.

Shape and edges (2026-10-24, round 3, after a look at how Tiny Glade, The Sims, Planet Zoo, Cities: Skylines,
WorldEdit, Axiom, Roblox, Unreal's splines, Blender's proportional editing and LittleBigPlanet's corner editor do it):
the Terrain tab's Shape mode shows the selected island's coastline anchors (about a 32nd of the coast apart), a move
diamond and a height arrow. Dragging an anchor pulls the coast (`Patch::warped`: each grid point takes the land, height
and paint from where the pull came from, the pull fading over the reach, so land grows out or a bay cuts in), the grid
growing to take it (`grow_to`, up to 128 cells); the wheel over a handle sets the reach; Ctrl snaps; each frame starts
from the island before the drag (one undo step, the collision on release). Grow all / Shrink all offset the mask.
Mask values are never exactly 0 (the outline would pass through a point and break into crumbs). Edges are game pieces
(`Edge::piece`, a tie or moby of any level; none: an invisible wall): the map renderer reads the class once
(`map_load::piece_model`), lays a whole number of copies along each outline loop (`piece_spans`: stretched a little to
close it, sheared to follow the ground, back to the coast), alpha cut out; the collision stays the edge's upright quads
`height` high. The editor offers 14 pieces each checked by its model to be a railing, fence or wall (two Umbris
handrails were left out: wall-mounted bars 0.4 high).

Edges, openings, roofs (2026-10-24, round 4): `Edge { kind: Invisible | Piece(id) | Wall { texture, tile }, height,
gaps }`; a gap `[x, y, radius]` leaves out the edge's stretches (pieces) and collision segments whose middle lies in it,
in world space so it survives reshaping (the move handle carries them along). The editor's Openings mode toggles the
stretch under the pointer (a click, or a drag along the edge). The edge picker has Railings and Walls tabs; Walls adds a
textured wall (0.5–30 high, both sides drawn). The collision fence is cut into ≤ 8 unit pieces like the cliffs. A
patch's `roof { height, texture, tile }` draws its land's shape at base + height (both sides) and adds walkable top
triangles to the collision. The Build settings' edge, texture and cliff depth also change the island just built while
it stays selected (`last_built`, cleared by undo and map switches): the first "I cannot change the edge" report was
the builder's edge picker only setting the next island.

Phase 3 finished (2026-10-24, round 5):

- **Walls**: the edge picker's Walls are tall walls found by a survey of every tie and moby class of the 19 levels
  (long, thin, 3.5–30 high; 210 after the catalog's dedupe, each looked at): Novalis's wall 729, rock faces, brick,
  metal, hedge and building walls (16). The low walls moved to Railings; the Pokitaru "railing" (a 2-deep window
  wall) went. Pieces bend along the coast (each vertex at its own distance along it, `map_load::Coast`), so long pieces
  no longer cut corners; walls block their full height.
- **Edge collision both ways**: the fence quads are stored twice, one per side (a face stops Ratchet only coming at
  its front: in game he walked out through a railing from the land side).
- **Surfaces**: a patch's and a walkway's top carry a surface byte (`floor::SURFACES`: ground 0x1f, slippery 7, hot 1,
  deadly 8, magnetic 2). A custom map plays by the full surface rules (`rc_game::hero::surface::set_full_rules`, set at
  each level load): Novalis's own build ignores 7 and 1. Verified in game: 7 gives the slide 0x2f, 8 the pit death 0x79.
- **Paths** (`rc_map::walkway::Walkway`, `"kind": "walkway"`): a Catmull-Rom centre line through its points, a strip
  `width` wide, `depth` thick, its own texture, surface and an edge both sides (pieces along open coasts, textured
  walls, openings). Editor: Terrain > Paths (click points, Enter; drag points, the wheel raises one, Delete removes).
- **Shade** (`Patch::shade`, 0xRRGGBB per point, multiplied into the baked light) and **Scatter** (`Patch::scatter`:
  shrub/tie/moby models at x, y standing on the land wherever it is sculpted; drawn only) brushes.
- **Collision view** (top bar) and **save warnings** (`map_warnings`: faces left out, a start over nothing, ground at
  or below the death height, objects over nothing), listed in the Maps tab.
- **Gate**: `maps/three_islands.json` (written by the `three_islands_gate` test, which checks no face is left out and
  walks both bridges a quarter unit at a time). Played: Ratchet walks island 1 → bridge → island 2 → bridge → island
  3 (60 → 64 → 68), the railings stop him at the far coasts.
- Test runs: `RC_WINDOW_AT=x,y` opens the window there; the scratch launcher runs the test copy hidden.

Round 6 (2026-10-24, playtest feedback): an edge's openings and collision now follow the drawn pieces exactly: the edge
keeps its stretch length `step` and `inset` (a piece's length and depth, filled from its model where a map lacks them:
`map_load::fit_edges`), `rc_map::coast::Coast` cuts loops and walkway sides into the same stretches for the pieces and
for `Edge::fence_along`, and the barrier stands `inset` in (Ratchet stops on the land side of a railing instead of in
it). Played: through an opening he walks off; at a railing he stops 0.6 short of its inner face. Paths open the railings
they cross (`Patch::open_where`); the Sculpt brush Edge gaps opens or closes under it (`open_near`). Collision view in
play too (F6). A toolbar over the world (Select, Place, the terrain tools), Select mode (clicks select objects, then
paths, then islands; the right panel follows), Esc to Select. Scatter is called Decor. The asset browser's types show
their first asset as an icon and split into subtypes by name words (`SUBTYPES`). Not done: doors in openings; a path
whose middle runs into an island's cliff is drawn into it (place its points on the islands).

Round 7: an opening now cuts exactly the coast inside its circle (`Edge::spans`, sampled every eighth of a unit), and
the standing stretches between openings are refitted (a whole number of pieces each, squeezed or stretched) so the
pieces and the barrier end where the opening begins. A path opens the railing only inside its own railings
(`Walkway::inside` with the railing's inset as margin, sides only), so the island's railing meets the path's: sealed.
A path's old openings (centred on it) are cut again on every change and once when a map opens. Gate test: one opening
per landing, every railing end beside a bridge within 0.35 of the bridge's railing; played: down the middle Ratchet
crosses, beside the bridge or 4 units off he ends in the corner, on the island.

## Editor round 2 (2026-10-22, the first hands-on feedback)

- **Loadout** (`gameplay::apply_loadout`): the game state outlives the editor's reloads, so the map's loadout is set
  exactly (the editable items not in it taken away, the rest granted as the boot grants them) before every reload;
  the host's gold-bolt bytes are cleared too (a collected bolt would otherwise never come back).
- **A game menu left open by Stop** (`menu_render::request_leave`): the menu's pad input stops with the play camera
  and `menu_frame` runs with the frozen tick, and `MenuMode` outlives the level reload, so the pause menu's snapshot
  stayed over the view. Stop now puts the modes back to gameplay.
- **Gold bolt** (1134): its pickup camera's two cuboids (pvar +0x04 / +0x08) named the host's cuboids (the camera flew
  into the sky): set to −1, the orbit about Ratchet (`map_load::source_free`).
- **Ammo crates** are 1×1×1 like every crate (every crate instance on the disc is placed at scale 1.0): unchanged.
- **Camera speed**: shown in the top bar with a reset; the wheel scales by how far it turned (a touchpad's pixel
  steps collapsed the speed), 2..400.
- **Help** (top bar): the topics as tabs like the game's help pages, open on the first run; "Open the full guide"
  writes `docs/guide/rac_maker_guide.html` (compiled in) to `%LOCALAPPDATA%\rerac\maker` and opens it.
- **Files**: New (left of Save) makes `Map N.json` in `Documents\RAC Maker`; the Maps tab lists recent maps (in
  `%LOCALAPPDATA%\rerac\maker\settings.txt`), the folder's maps and this map's backups (`backups\<map>\<time>.json`:
  the version every save replaces, and unsaved work every 5 minutes; 30 kept), each restorable.
- **Lists**: placed objects (Objects tab) and patches (Terrain tab) with select and delete.
- **Floors**: the default floor is Novalis's grass (`rac1/L01/tfrag-tex/8`); a floor's texture and texture size are
  set in the map inspector (click its number).
- **Placing**: R / Shift+R turn the object before it is placed; its real model shows where it will go
  (`Previews`: the catalog thumbnails' triangles, textured, each planet's models read once on a worker thread), and so
  do placed objects until the reload draws them. Boxes only on the selected object; the others get a small circle.
- **Gizmos** on the selected object: arrows move it along x / y / up (never into the ground), the yellow ring turns it
  in 15° steps; dragging the object itself follows the ground under the pointer.
- **Paths** (`Object::path`, `time`; `GameplayPatch::splines` appends them after the host's splines): the path
  platform 726 (spline +0xb4, starts at the first point, travel time +0xb0) and the robot trooper 459 (patrol spline
  + 1 at +0x160) get the map's path (`map_load::path_pvars`); a placed platform starts with an 8-unit lift; points are
  dragged in the world or typed on the right.
- **Sight**: the selected robot trooper shows its wake (blue) and attack (red) ranges from its template (+0x1a8 /
  +0x1a0); other enemies' ranges are not mapped yet.
- **Terrain brushes**: Plateau (a flat top at the stroke's start + a height; inverted a flat-bottomed pit), Paint (a
  texture painted in soft layers: `Patch::layers`, each drawn as a blended overlay a hair above the top), Square,
  Invert.

## Solid objects and objects that move themselves (2026-10-22)

- **A click no longer moves an object**: the drag starts once the pointer leaves the press point by 5 pixels, and its
  grab offset is taken on the surface the drag follows. (A click used to re-snap the object, the editor then redrew it
  at its placed spot, and the reload put it back where its class's own code places it: it seemed to jump on select.)
- **Moved away** (`Editor::moved_away`, `gameplay::editor_instance_at`): after each load the editor compares where the
  game put each object with where it was placed (more than 1 unit aside, 1 up or 3 down); such an object (its class
  positions itself from its home level's data: the hinged bridge 746 went 7 units aside) gets a red line to where it
  went and a warning in the inspector.
- **Solid** (`Object::solid`, `map_load::solid_triangles`): the object's model (its low LOD when it has one), at its
  record's scale, facing and position, snapped to a quarter unit, baked into the map's collision; each face wound for
  the collision by its side of the model's middle, both ways when that cannot be told. Measured with `RC_PLAY_SCRIPT`
  (Ratchet walking into it for 4 s): the shells' wall 709, the big wall 729, the hinged bridge 746 and the item display
  1461 let him through on their own (their class collision is not the hero's: on the disc the level geometry around
  them is solid), a bolt crate stops him; with Solid the wall 709 stops him (123 triangles). The editor turns Solid on
  for scenery, platforms and other pieces that stay put (not the path classes, not names that move: hinge, lift, door,
  …), off for enemies, pickups and effects; the inspector's checkbox changes it. The catalog records `collision` (the
  class has a collision blob) and files walls as scenery.

## Phase 4: rails and gameplay (2026-10-25)

**Grind rails.** `rails[]` carry `"smooth": true` (Catmull-Rom through the points, about one unit a step:
`walkway::curve`, `Rail::line`), used for the patch, the collision check and the drawing. Terrain tab > Rails draws them
like paths; the first rail turns `abilities.rails` on.

**Zones and rules** (`rc_map::logic`, `"zones"` / `"rules"` in the map file; engine side `crate::map_logic`). A zone is
a turned box (`centre`, `size`, `yaw`) of kind trigger, checkpoint, goal, death or camera (`eye`). After every tick
Ratchet's feet are tested against the zones: a checkpoint records the game's own checkpoint (class 805's
`checkpoint::record_at`, run inside the next moby loop), so the death reload comes back there; the goal plays level
sound 1 with a "Goal! m:ss" banner (the editor ends the run 4 s later); a death zone runs `hero::damage::death_fade`; a
camera zone holds the view at its eye looking at Ratchet (`play_camera::apply`, smoothstep blend over half a second).
Rules fire once per play (again after a death reload when `reset_on_death`): when `enters` a zone / `cleared` (every
catalog enemy standing in the zone at load is dead) / `destroyed` (a named object's state 0xfd / 0xfe), then `remove`
(`MobyTable::delete` + off the moby grid), `show` (a `hidden` object: HIDDEN | NO_UPDATE | NO_ANIM, collision off and
off the moby grid until then), `message` (the HUD banner), `checkpoint`, `goal`. Objects carry `name` and `hidden`.
Logic tab: zone kinds to place, zones list, rules from chips; zone inspector (size keeps the bottom, camera "use the
current view"). Save warnings: rules naming what is not there, duplicate names, hidden objects never shown, ammo
pickups without an ammo weapon in the loadout, objects that may vanish.

**Enemies away from home.** Templates name their home level's splines, areas and cuboids by index; on another host
those indices hit the wrong data (teleports, culls, deletes). `map_load::OWN_SPLINES` copies the class's home splines
(moved and turned with the object) and repoints the pvars; `OWN_WORDS` clears or sets words (cull cuboids, split
waits). Fixed and seen standing: 1023, 1382, 749, 1454, 238, 865, 866. Still wandering or vanishing:
`map_load::MAY_VANISH` (the asset browser badges them). Not placeable at all: `NOT_PLACEABLE`.
The robot trooper 459's init reads its patrol spline at **+0x164** and arrival spline at +0x134 (and writes the "+1"
pointers +0x160 / +0x130 itself): `path_pvars` sets +0x164, +0x134 = −1 and +0x1b4 (no 20-unit lift: it stands on its
path); a trooper without a path gets a two-point guard path where it stands. The Veldin tank 1454 takes a map path at
+0x100 (drives to a random next point). Sight rings: `editor::SIGHT` (wake / attack per class: a template pvar or the
code's constant), from a read of the ported classes; 459's wake +0x1a8, attack +0x1a0.

**Main menu** (`editor/menu.rs`): cards of every map in the maps folder with `<map>.png` thumbnails ("Set thumbnail"
saves the game view without the panels, 640 × 360), Load / Play / Duplicate / Rename / Delete (to `Deleted/`), named
versions (`versions/<stem>/<stamp> <name>.json`, never pruned) and the backups. `RC_EDITOR_TAB=objects|terrain|rails|
logic|maps` skips the menu (tests and screenshots).

Verified with hidden runs on `Sample course` (four islands, two bridges, a rail, troopers, a gate, an ambush):
the bridge trigger's message and the checkpoint; debug hits on both guards → "cleared" → the gate deleted (gone from
the moby dump); entering island C shows the ambush trooper; the camera zone takes the view; the goal at 0:19.
Not verified by a scripted run: grinding the rail end to end, the tank driving its whole path.

## Feedback round 8 (2026-10-25): enemies everywhere, world, ship, editor polish

**Enemies away from home** (research in full: every live catalog enemy audited per pvar): `map_load::AWAY` is one
table per class of `LevelRef`s (`Spline`, `Cuboid`, `Word`, `SetYaw`, `Yaw`) and the classes its code makes. Home
splines keep their w words (area walls, graph masks, node marks: `GameplayPatch::splines` is `[f32; 4]` now; the w = 0
the patch wrote erased every copied area's walls). Home cuboids are copied, moved and turned (centre, matrix rows,
inverse rows, Euler z) into a new cuboid section after the host's (`GameplayPatch::cuboids`). A spline or cuboid named
twice is copied once. `all_imports` brings the children (guns, shots, treads, riders, pieces) the host lacks;
`import_classes` shares a source level's textures between classes and skips a child that does not fit (the u8 moby
texture table: about 18 free slots on Novalis), and `services::SKIP_MISSING_CLASSES` makes `create_moby` of a class
the level lacks make nothing on maps (it used to leave a hidden moby in its slot for ever: shooters leaked slots per
shot). Set pieces and companion-bound enemies moved to `NOT_PLACEABLE` / `MAY_VANISH`.

**The class tables of imports** (`gameplay::level_ports`) are built outside their lock (the main thread asked every
frame and waited for the loader thread's build: the editor froze for seconds), every overlay is parsed once
(`OVERLAYS`). The editor shows a loading card (`level_load::LOAD_STEP` / `LOAD_STAGES`).

**World** (`MapDef::world`, `"world"`): `sky` (another level's sky block; its settings words 0x00..0x24, the
background colour and fog, written over the host's), `fog`, `music` (level, start track: music slots 0 / 1 replaced in
`audio_out::load`), `silent`. Maps drop the host's fog zones (0x80). The radio: `AudioSystem::play_song`, and the audio
runs its own EE frames while the editor freezes the tick.

**Ship** (`MapDef::ship`): settings +0x2c..+0x38 written; Ratchet starts at the host's start moved from the host's
ship to the map's (`map_load::ship_start`). A play run lands (`ShipMode::map_landing`, any level); the ship is locked
(`ShipGlobals::locked`) until the goal; the take-off's end (`SpaceEvent::PlanetPage`) finishes the map
(`map_logic::complete`), no planet page. `walk_back` releases Ratchet on maps when the start has no ground behind it.
Verified hidden: landing, walk to the hatch, △, take-off, "Level complete".

**Rails**: on the disc grind rails are tfrag geometry. `map_load::RAIL_LOOKS` cuts a piece from a level's tfrags along
the straightest stretch of one of its grind paths (in the rail's frame) and lays it along a map rail
(`rail_piece_tris`).

**Editor**: tools as painted icons in the top bar; square type tiles; zone preview at the pointer; help search
(selecting something closes the help); main menu cards dragged into an order (`settings.txt` `order=`); view buttons
(Top, Front, ...); paths `cut` (built only off the land: `Walkway::pieces`); rules `cutscene` (camera on the object,
the actions half way, a cut back; Ratchet's stick still); the stick follows a camera zone's view
(`map_logic::steer`, not while grinding); objects' load-made pieces follow a moved object in the live preview
(`gameplay::editor_show`); `RC_EDITOR_TAB` opens a tab without the menu.

## Camera (all RAC1 play, not only maps)

RAC1's camera always turns inverted, left / right and up / down (user request 2026-10-22), whatever the Options
menu's camera rows say: `gameplay::camera_inverted` on the options the tick reads. Deterministic runs
(`RC_DETERMINISTIC`, screenshots, frame dumps) keep the game's own rule for their recorded pads.

## Known gaps

- The host's moby texture table is a `u8` slot space: Novalis uses 237 of 255, so only ~18 imported textures fit.
  Imports need wider slots (the Weapon Evolution mod's `imported_textures` design).
- Imported classes have no shadow blocks, attachments or class sounds yet (crate::shadow_render, crate::moby_attach
  and the sound bank still read the host). Not checked: whether the biter is silent.
- Floors and rails are plain unlit meshes: no level fog, lighting or vertex colours.
- The HUD, help text, music and level name are the host's.
- Imported classes' sounds index their own level's sound bank; on the host they may play the wrong sound or none.
- The Grindboots work when owned, not only when equipped: that is the game's own rule (`rail_contact_probe`).
