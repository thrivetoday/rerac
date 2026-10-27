//! RAC Maker maps' gameplay logic while playing (rc_map::logic: zones and when/then rules), on the game's own
//! mechanisms: a checkpoint zone records the game's checkpoint (`classes::checkpoint::record_at`: the death reload
//! comes back there, with its visit records), the goal plays the level's jingle with a banner and ends the run, a
//! death zone runs Ratchet's death (`hero::damage::death_fade`), a camera zone holds the view at its eye looking at
//! Ratchet (crate::play_camera), and rules remove objects (`DeleteMoby`), show objects placed hidden (an ambush, a
//! bridge that appears: hidden, not updated and not solid until then) and show messages.
//!
//! Each tick (crate::gameplay): [`after_tick`] with the game after the tick (Ratchet's feet against the zones, the
//! objects' states for the rules), and [`moby_pass`] inside the next moby loop for what needs the moby world
//! (the checkpoint record, the sounds). [`reset`] at each new play (a load), [`death_reload`] after a death.

use rc_map::logic::{Then, When, ZoneKind};
use rc_map::MapDef;
use std::sync::{Arc, Mutex};

/// Something for the moby loop to do.
enum Pending {
    /// Record the checkpoint here (position, facing).
    Checkpoint([f32; 3], f32),
    /// Play level sound `n` (1: the skill point / goal jingle).
    Sound(i32),
}

#[derive(Default)]
struct Logic {
    def: Option<Arc<MapDef>>,
    /// Per object: its moby (None until the level's statics are known, or when the game did not create it).
    ids: Vec<Option<usize>>,
    ready: bool,
    /// Per zone: Ratchet in it at the last tick.
    inside: Vec<bool>,
    /// Per rule: fired.
    fired: Vec<bool>,
    /// Per zone: the enemies in it when the play began (`When::Cleared`).
    enemies: Vec<Vec<usize>>,
    pending: Vec<Pending>,
    /// The banner text and its number (the HUD shows a new number once).
    banner: (u32, String),
    /// The camera zone Ratchet is in, and the last one he was in (the view blends out from it).
    camera: Option<usize>,
    last_camera: Option<usize>,
    /// The counter when the goal was reached.
    goal: Option<u64>,
    /// How far the camera has blended into the zone's view (0..1).
    blend: f32,
    /// When the goal was reached (the editor ends the run a few seconds later).
    goal_at: Option<std::time::Instant>,
    /// The zone of the checkpoint recorded last (entering it again records nothing: a respawn lands in it).
    checkpoint: Option<String>,
    /// The stick's turn while it stays pushed (camera zones): kept from when it was pushed, so entering or leaving a
    /// zone mid-run or mid-jump does not swing Ratchet round; let go, the zone's turn takes over.
    steer_hold: Option<f32>,
    /// A rule's camera scene running: the camera on the object, the rule's actions half way through, then the cut
    /// back (Ratchet's stick is still meanwhile).
    scene: Option<Scene>,
}

/// A rule shown with the camera (`Rule::cutscene`).
struct Scene {
    /// The object's middle and where the camera stands (towards Ratchet's side of it, up).
    at: [f32; 3],
    eye: [f32; 3],
    /// Ticks since it began.
    t: u32,
    /// The rule's actions, done at [`SCENE_ACT`].
    actions: Vec<Then>,
}

/// A rule scene's timing (ticks): the camera turned to the object, the actions, the cut back.
const SCENE_IN: u32 = 24;
const SCENE_ACT: u32 = 50;
const SCENE_END: u32 = 120;

static LOGIC: Mutex<Option<Logic>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Logic) -> R) -> R {
    let mut g = LOGIC.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Logic::default))
}

/// A new play (a level load): the rules and zones start over.
pub fn reset() { with(|l| *l = Logic::default()); }

/// A death's reload: the objects come back as at the load (the map's `reset_on_death`), so the rules start over and
/// hidden objects are hidden again; the checkpoint stays (the game keeps it).
pub fn death_reload(reset_all: bool) {
    with(|l| {
        if reset_all {
            l.ready = false;
            l.fired.iter_mut().for_each(|f| *f = false);
        }
        l.inside.iter_mut().for_each(|f| *f = false);
        l.goal = None;
        l.goal_at = None;
    });
}

/// A message on the HUD (crate::hud_render shows it once).
fn banner(l: &mut Logic, text: &str) {
    println!("map logic: {text}");
    l.banner = (l.banner.0.wrapping_add(1), text.to_string());
}

/// The banner to show: its number and text.
pub fn banner_text() -> (u32, String) { with(|l| l.banner.clone()) }

/// A map with a ship: Ratchet boarded it after the goal (its take-off ended): the map is finished.
pub fn complete(counter: u64) {
    with(|l| {
        if l.goal_at.is_some() { return; }
        l.goal_at = Some(std::time::Instant::now());
        let secs = l.goal.unwrap_or(counter) / 60;
        banner(l, &format!("Level complete!  {}:{:02}", secs / 60, secs % 60));
    });
}

/// How long ago the goal was reached (the editor ends the play run after the banner).
pub fn goal_elapsed() -> Option<std::time::Duration> { with(|l| l.goal_at.map(|t| t.elapsed())) }

/// Inside the next moby loop: the checkpoint records and sounds the last tick's logic asked for.
pub fn moby_pass(w: &mut rc_game::moby_update::World) {
    let todo = with(|l| std::mem::take(&mut l.pending));
    for p in todo {
        match p {
            Pending::Checkpoint(pos, yaw) => rc_game::moby_update::classes::checkpoint::record_at(w, pos, [0.0, 0.0, yaw]),
            Pending::Sound(n) => { w.play_level_sound(n, 0, None); }
        }
    }
}

/// The zone's floor point for a checkpoint: its centre, at its bottom plus a little (Ratchet lands from there).
fn checkpoint_point(z: &rc_map::logic::Zone) -> [f32; 3] { [z.centre[0], z.centre[1], z.centre[2] - z.half[2] + 0.5] }

/// After a tick: Ratchet's feet against the zones, the rules, the actions on the game's mobys and Ratchet.
pub fn after_tick(p: &mut crate::gameplay::Play) {
    let Some(def) = crate::map_load::on(p.level_index()) else { return };
    let mut kill = false;
    with(|l| {
        if !l.def.as_ref().is_some_and(|d| Arc::ptr_eq(d, &def)) {
            *l = Logic { def: Some(def.clone()), banner: l.banner.clone(), ..Default::default() };
        }
        let (nz, nr) = (def.zones.len(), def.rules.len());
        l.inside.resize(nz, false);
        l.fired.resize(nr, false);
        // Object k is static instance k + 1 (Ratchet is instance 0).
        let ids = (!l.ready).then(|| (0..def.objects.len()).map(|k| p.driven_moby(k + 1)).collect::<Vec<_>>());
        let table = &mut p.game.mobys;
        if let Some(ids) = ids {
            l.ids = ids;
            if l.ids.iter().all(|i| i.is_none()) && !def.objects.is_empty() { return; }
            l.ready = true;
            for (k, o) in def.objects.iter().enumerate() {
                let Some(id) = l.ids[k] else { continue };
                if let Some(n) = &o.name { println!("map logic: object {k} \"{n}\" is moby {id}"); }
                let m = &mut table.mobys[id];
                if o.hidden {
                    m.mode |= rc_game::moby_runtime::mode::HIDDEN | rc_game::moby_runtime::mode::NO_UPDATE | rc_game::moby_runtime::mode::NO_ANIM;
                    m.coll_disable = 0xffff_ffff;
                    // Off the moby grid: nothing bumps into it (Ratchet, the camera, shots) until it is shown.
                    std::sync::Arc::make_mut(&mut p.svc.grid).remove(m);
                }
            }
            // The enemies each zone starts with (the catalog does not travel here: an object that updates and can be
            // hit, by its class's collision; crates and scenery count only when named in a Destroyed rule).
            l.enemies = def.zones.iter().map(|z| {
                def.objects.iter().enumerate().filter_map(|(k, o)| {
                    let id = l.ids[k]?;
                    (z.contains(o.position) && crate::map_load::is_enemy(o.asset)).then_some(id)
                }).collect()
            }).collect();
        }
        let alive = |table: &rc_game::moby_runtime::MobyTable, id: usize| { let s = table.mobys[id].state; s != 0xfe && s != 0xfd };
        let h = &p.game.hero;
        let feet = [h.pos[0].to_f32(), h.pos[1].to_f32(), h.pos[2].to_f32() + 0.2];
        let yaw = h.yaw().to_f32();
        let mut actions: Vec<Then> = Vec::new();
        l.camera = None;
        for (i, z) in def.zones.iter().enumerate() {
            let now = z.contains(feet);
            let entered = now && !l.inside[i];
            l.inside[i] = now;
            if now { if let ZoneKind::Camera { .. } = z.kind { l.camera = Some(i); l.last_camera = Some(i); } }
            if entered { println!("map logic: Ratchet entered zone \"{}\" ({})", z.name, z.kind.name()); }
            if !entered { continue; }
            match z.kind {
                ZoneKind::Checkpoint => actions.push(Then::Checkpoint(z.name.clone())),
                ZoneKind::Goal => actions.push(Then::Goal),
                ZoneKind::Death => kill = true,
                ZoneKind::Trigger | ZoneKind::Camera { .. } => {}
            }
        }
        // The rules whose condition holds now (each once).
        for (r, rule) in def.rules.iter().enumerate() {
            if l.fired[r] { continue; }
            let zone = |n: &str| def.zones.iter().position(|z| z.name == n);
            let object = |n: &str| def.objects.iter().position(|o| o.name.as_deref() == Some(n));
            let holds = match &rule.when {
                When::Enters(n) => zone(n).is_some_and(|i| l.inside[i]),
                When::Cleared(n) => zone(n).is_some_and(|i| l.enemies.get(i).is_some_and(|e| e.iter().all(|&id| !alive(table, id)))),
                When::Destroyed(n) => object(n).and_then(|k| l.ids[k]).is_some_and(|id| !alive(table, id)),
            };
            if holds {
                l.fired[r] = true;
                // Shown with the camera: on the first object it removes or shows, the actions half way through.
                let shown = rule.then.iter().find_map(|t| match t { Then::Remove(n) | Then::Show(n) => Some(n.as_str()), _ => None });
                let at = shown.filter(|_| rule.cutscene && l.scene.is_none()).and_then(|n| def.objects.iter().position(|o| o.name.as_deref() == Some(n))).and_then(|k| l.ids[k]).map(|id| {
                    let m = &table.mobys[id];
                    [m.position[0], m.position[1], m.position[2] + 1.0]
                });
                match at {
                    Some(at) => {
                        let (dx, dy) = (feet[0] - at[0], feet[1] - at[1]);
                        let len = (dx * dx + dy * dy).sqrt().max(0.01);
                        let eye = [at[0] + dx / len * 9.0, at[1] + dy / len * 9.0, at[2] + 4.5];
                        l.scene = Some(Scene { at, eye, t: 0, actions: rule.then.clone() });
                    }
                    None => actions.extend(rule.then.iter().cloned()),
                }
            }
        }
        if let Some(sc) = l.scene.as_mut() {
            sc.t += 1;
            if sc.t == SCENE_ACT { actions.extend(std::mem::take(&mut sc.actions)); }
            if sc.t >= SCENE_END { l.scene = None; }
        }
        for a in actions {
            let object = |n: &str| def.objects.iter().position(|o| o.name.as_deref() == Some(n)).and_then(|k| l.ids[k]);
            match a {
                Then::Remove(n) => {
                    if let Some(id) = object(&n).filter(|&id| alive(table, id)) {
                        table.delete(id, p.game.counter);
                        std::sync::Arc::make_mut(&mut p.svc.grid).remove(&mut table.mobys[id]);
                    }
                }
                Then::Show(n) => {
                    if let Some(id) = object(&n) {
                        let m = &mut table.mobys[id];
                        m.mode &= !(rc_game::moby_runtime::mode::HIDDEN | rc_game::moby_runtime::mode::NO_UPDATE | rc_game::moby_runtime::mode::NO_ANIM);
                        m.coll_disable = 0;
                        std::sync::Arc::make_mut(&mut p.svc.grid).register(m);
                    }
                }
                Then::Message(t) => banner(l, &t),
                Then::Checkpoint(n) => {
                    if l.checkpoint.as_deref() == Some(n.as_str()) { continue; }
                    if let Some(z) = def.zones.iter().find(|z| z.name == n) {
                        l.checkpoint = Some(n.clone());
                        l.pending.push(Pending::Checkpoint(checkpoint_point(z), yaw));
                        banner(l, "Checkpoint");
                    }
                }
                Then::Goal => {
                    if l.goal.is_none() {
                        l.goal = Some(p.game.counter);
                        l.pending.push(Pending::Sound(1));
                        let secs = p.game.counter / 60;
                        if def.ship.is_some() {
                            // With a ship the map ends aboard it (`complete`).
                            banner(l, &format!("Goal!  {}:{:02}  Back to your ship!", secs / 60, secs % 60));
                        } else {
                            l.goal_at = Some(std::time::Instant::now());
                            banner(l, &format!("Goal!  {}:{:02}", secs / 60, secs % 60));
                        }
                    }
                }
            }
        }
        // A ship takes Ratchet aboard once the goal is reached.
        let has_goal = def.zones.iter().any(|z| matches!(z.kind, ZoneKind::Goal)) || def.rules.iter().any(|r| r.then.iter().any(|t| matches!(t, Then::Goal)));
        if def.ship.is_some() { p.svc.travel.locked = (has_goal && l.goal.is_none()) || l.goal_at.is_some(); }
        // The camera blends into a zone's view and back out over about half a second.
        l.blend = if l.camera.is_some() { (l.blend + 1.0 / 30.0).min(1.0) } else { (l.blend - 1.0 / 30.0).max(0.0) };
    });
    if kill && p.game.hero.fell_out == 0 {
        rc_game::hero::damage::death_fade(&mut p.game.hero);
        println!("map logic: Ratchet entered a death zone");
    }
}

/// A stick byte → its value in [−1, 1] (the inverse of `PadInput::axis_byte`; within ±48 of the centre: 0).
fn axis_value(b: u8) -> f32 {
    let d = b as f32 - 127.0;
    if d.abs() <= 48.0 { 0.0 } else { (d.signum() * (d.abs() - 48.0) / 76.0).clamp(-1.0, 1.0) }
}

/// The left stick turned for a camera zone: Ratchet walks where the stick points as seen through the zone's camera,
/// not the follow camera behind him (which the game steers by). Grinding (hero group 0xf) keeps the game's own rule.
pub fn steer(mut pad: rc_game::pad::PadInput, hero: [f32; 3], group: i32, follow: &rc_game::follow_camera::CameraView) -> rc_game::pad::PadInput {
    if group == 0xf { return pad; }
    // A rule's scene: Ratchet waits.
    if with(|l| l.scene.is_some()) {
        pad.lx = 0x7f;
        pad.ly = 0x7f;
        return pad;
    }
    let (sx, sy) = (axis_value(pad.lx), -axis_value(pad.ly));
    if sx == 0.0 && sy == 0.0 {
        with(|l| l.steer_hold = None);
        return pad;
    }
    // The turn the zone's camera asks for (none outside a zone), growing with its blend (the shorter way round).
    let want = camera(hero).map_or(0.0, |(eye, at, k)| {
        let [f, _, _] = follow.rows_f32();
        let g = f[1].atan2(f[0]);
        let z = (at[1] - eye[1]).atan2(at[0] - eye[0]);
        let mut d = z - g;
        while d > std::f32::consts::PI { d -= std::f32::consts::TAU; }
        while d < -std::f32::consts::PI { d += std::f32::consts::TAU; }
        d * k * k * (3.0 - 2.0 * k)
    });
    let d = with(|l| *l.steer_hold.get_or_insert(want));
    if d == 0.0 { return pad; }
    // The stick in the follow camera's frame (x right, y forward: right is forward turned −90°, so a turn by d
    // anticlockwise in the world is the same turn here). Zone looking east, follow camera north: forward → right.
    let (s, c) = d.sin_cos();
    let (nx, ny) = (sx * c - sy * s, sx * s + sy * c);
    pad.lx = rc_game::pad::PadInput::axis_byte(nx);
    pad.ly = rc_game::pad::PadInput::axis_byte(-ny);
    pad
}

/// The camera a zone holds (its eye, the point it looks at: Ratchet's chest) and how far it has blended in.
pub fn camera(hero: [f32; 3]) -> Option<([f32; 3], [f32; 3], f32)> {
    with(|l| {
        // A rule's scene first: blended in, held, then a cut back (no blend out).
        if let Some(sc) = &l.scene {
            return Some((sc.eye, sc.at, (sc.t as f32 / SCENE_IN as f32).min(1.0)));
        }
        if l.blend <= 0.0 { return None; }
        let def = l.def.as_ref()?;
        // The zone Ratchet is in, else (blending out) the last one with a camera he was in.
        let z = l.camera.or(l.last_camera).and_then(|i| def.zones.get(i))?;
        let ZoneKind::Camera { eye } = z.kind else { return None };
        Some((eye, [hero[0], hero[1], hero[2] + 1.0], l.blend))
    })
}
