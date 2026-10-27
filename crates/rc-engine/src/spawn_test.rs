//! The RAC Maker spawn test: does each ported moby class work on a custom map? `rerac --data-dir <data> --spawn-test`
//! reads the catalog (crate::catalog), puts the classes the game places and the port runs on test maps (a ring of
//! them around Ratchet on a flat floor, the map hosted by the class's own level), runs each map as a child game for
//! [`TICKS`] ticks, and writes the verdicts into the catalog (`live` / `needs-work` and a note).
//!
//! A child run (`RC_SPAWN_REPORT=<file>`, `RC_SPAWN_TICKS=<n>`) writes, after its moby loop of tick n, how often each
//! class's update ran and the unported calls met while it ran (`rc_game` `FxStats::class_runs` / `class_unported`),
//! then exits ([`report_and_exit`]). A child that crashes or times out has its batch split in two and run again, down
//! to the one class that fails; a map that does not load fails the same way.
//!
//! Classes already found live are skipped unless `RC_SPAWN_ALL=1`.
//!
//! Verdicts: **live** when the update ran and met no unported call; **needs work** when it met one (the note names
//! it), never ran, or failed the run.

use anyhow::{bail, Context, Result};
use rc_map::catalog::{Behaviour, Catalog, Live};
use rc_map::json::Value;
use rc_map::{AssetKind, LEVEL_NAMES};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Ticks a child runs (10 s of game time).
const TICKS: u64 = 600;
/// Objects on one test map.
const BATCH: usize = 10;
/// Children at a time.
const PARALLEL: usize = 3;
/// A child slower than this is stopped and counted as failed.
const TIMEOUT: Duration = Duration::from_secs(150);

// ------------------------------------------------------------------------------------------------
// The child's side.

fn child_settings() -> Option<&'static (PathBuf, u64)> {
    static S: OnceLock<Option<(PathBuf, u64)>> = OnceLock::new();
    S.get_or_init(|| {
        let path = std::env::var_os("RC_SPAWN_REPORT").filter(|v| !v.is_empty())?;
        let ticks = std::env::var("RC_SPAWN_TICKS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(TICKS);
        Some((PathBuf::from(path), ticks))
    })
    .as_ref()
}

/// After the moby loop of tick `counter`: in a spawn-test child, once `RC_SPAWN_TICKS` ticks ran, writes the report
/// to `RC_SPAWN_REPORT` and exits.
pub fn report_and_exit(counter: u64, fx: &rc_game::moby_update::services::FxStats) {
    let Some((path, ticks)) = child_settings() else { return };
    if counter < *ticks { return; }
    let mut classes: Vec<i16> = fx.class_runs.keys().chain(fx.class_unported.keys()).copied().collect();
    classes.sort();
    classes.dedup();
    let rows: Vec<String> = classes
        .iter()
        .map(|c| {
            let unported = fx.class_unported.get(c).map_or_else(Vec::new, |m| m.iter().map(|(w, n)| format!("{:?}", format!("{w} ({n}×)"))).collect());
            format!("{{\"class\": {c}, \"runs\": {}, \"unported\": [{}]}}", fx.class_runs.get(c).copied().unwrap_or(0), unported.join(", "))
        })
        .collect();
    let json = format!("{{\"ticks\": {counter}, \"classes\": [\n{}\n]}}\n", rows.join(",\n"));
    if let Err(e) = std::fs::write(path, json) { eprintln!("spawn test: writing {}: {e}", path.display()); std::process::exit(1); }
    std::process::exit(0);
}

// ------------------------------------------------------------------------------------------------
// The parent's side.

/// One class to test: its catalog entry and the asset it is placed as (on its host level).
#[derive(Clone, Debug)]
struct Case {
    entry: usize,
    host: u32,
    class: i32,
}

/// A run's outcome per class (None: the run failed).
type RunResult = Option<HashMap<i16, (u64, Vec<String>)>>;

/// The test map of `cases` (all on one host level): a flat floor, Ratchet in the middle, the objects on a ring.
fn map_json(host: u32, cases: &[Case]) -> String {
    let (cx, cy, z) = (168.0f32, 144.0f32, 60.0f32);
    let objects: Vec<String> = cases
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let a = i as f32 / cases.len() as f32 * std::f32::consts::TAU;
            let (x, y) = (cx + 8.0 * a.cos(), cy + 8.0 * a.sin());
            // Facing Ratchet.
            format!("{{\"asset\": \"rac1/L{host:02}/moby/{}\", \"position\": [{x:.2}, {y:.2}, {z}], \"yaw\": {:.3}}}", c.class, a + std::f32::consts::PI)
        })
        .collect();
    format!(
        "{{\"maker_format\": 1, \"name\": \"Spawn test\", \"host_level\": {host}, \"start\": {{\"position\": [{cx}, {cy}, {}], \"yaw\": 0}},\n \"death_height\": 40, \"reset_on_death\": true,\n \"terrain\": [{{\"kind\": \"floor\", \"min\": [128, 104], \"max\": [208, 184], \"height\": {z}}}],\n \"objects\": [\n  {}\n ]}}\n",
        z + 0.5,
        objects.join(",\n  ")
    )
}

/// Runs one test map as a child game; None when it crashed, timed out or wrote no report.
fn run(root: &Path, dir: &Path, n: usize, host: u32, cases: &[Case]) -> Result<RunResult> {
    let map = dir.join(format!("map_{n:04}.json"));
    let report = dir.join(format!("report_{n:04}.json"));
    let log = dir.join(format!("log_{n:04}.txt"));
    std::fs::write(&map, map_json(host, cases))?;
    let _ = std::fs::remove_file(&report);
    let mut child = Command::new(std::env::current_exe()?)
        .arg("--data-dir").arg(root).arg("--map").arg(&map)
        .env("RC_SPAWN_REPORT", &report).env("RC_SPAWN_TICKS", TICKS.to_string())
        .env("RC_FULLSCREEN", "0").env("RC_RESOLUTION", "416").env("RC_FRAME_CAP", "0").env("RC_AUDIO", "0").env("RC_SAVE_DIR", "0")
        .stdout(Stdio::null()).stderr(std::fs::File::create(&log)?)
        .spawn()
        .context("starting a child run")?;
    let t0 = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? { break Some(s); }
        if t0.elapsed() > TIMEOUT { let _ = child.kill(); let _ = child.wait(); break None; }
        std::thread::sleep(Duration::from_millis(200));
    };
    if !status.is_some_and(|s| s.success()) { return Ok(None); }
    let Ok(text) = std::fs::read_to_string(&report) else { return Ok(None) };
    let v = rc_map::json::parse(&text).map_err(anyhow::Error::msg)?;
    let mut out = HashMap::new();
    for c in v.get("classes").and_then(Value::as_arr).unwrap_or(&[]) {
        let class = c.get("class").and_then(Value::as_i64).unwrap_or(-1) as i16;
        let runs = c.get("runs").and_then(Value::as_i64).unwrap_or(0) as u64;
        let unported = c.get("unported").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(|u| u.as_str().map(str::to_string)).collect();
        out.insert(class, (runs, unported));
    }
    Ok(Some(out))
}

/// The classes to test: ported mobys the game places, with a model, on a level that places Ratchet; not Ratchet,
/// his other bodies or the player's ship.
fn cases(root: &Path, c: &Catalog) -> Vec<Case> {
    use rc_game::hero::bodies::{CLANK_CLASS, DISGUISE_CLASS, GIANT_CLASS};
    let skip = |o: i32| o == 0 || [CLANK_CLASS, GIANT_CLASS, DISGUISE_CLASS].contains(&(o as i16)) || rc_game::travel::ship::CLASSES.contains(&(o as i16));
    // Per level: the classes its gameplay file places (the template instance is copied from there).
    let mut placed: HashMap<u32, std::collections::HashSet<i32>> = HashMap::new();
    let mut places = |l: u32, o: i32| placed.entry(l).or_insert_with(|| {
        rc_data::level_gameplay(root, l).ok().and_then(|g| rc_formats::gameplay::parse_moby_instances(&g).ok()).map_or_else(Default::default, |v| v.iter().map(|m| m.o_class).collect())
    }).contains(&o);
    let mut out = Vec::new();
    for (i, e) in c.entries.iter().enumerate() {
        if e.id.kind != AssetKind::Moby || e.behaviour != Some(Behaviour::Ported) || e.placed == 0 || e.thumb.is_empty() || skip(e.id.number) { continue; }
        // A class already found live is not run again unless RC_SPAWN_ALL=1 (a new ReRAC version: rebuild the catalog
        // and run them all).
        if e.live == Some(Live::Live) && !std::env::var("RC_SPAWN_ALL").is_ok_and(|v| v == "1") { continue; }
        // Its first level that places both Ratchet and it hosts it (the same content may have another number there).
        let Some(id) = e.ids().find(|i| places(i.level, 0) && places(i.level, i.number)) else { continue };
        out.push(Case { entry: i, host: id.level, class: id.number });
    }
    out
}

/// `--spawn-test`: tests every case, writes the verdicts into the catalog, returns the exit code.
pub fn run_all(root: &Path) -> i32 {
    match run_all_inner(root) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: spawn test: {e:#}");
            1
        }
    }
}

fn run_all_inner(root: &Path) -> Result<()> {
    let t0 = Instant::now();
    let cat_dir = root.join("catalog");
    let text = std::fs::read_to_string(cat_dir.join("catalog.json")).context("no catalog: run --build-catalog first")?;
    let mut catalog = Catalog::parse(&text)?;
    let dir = cat_dir.join("spawn-test");
    std::fs::create_dir_all(&dir)?;
    let cases = cases(root, &catalog);
    if cases.is_empty() { bail!("no ported, placed moby classes in the catalog"); }
    // Batches per host, in catalog order.
    let mut by_host: BTreeMap<u32, Vec<Case>> = BTreeMap::new();
    for c in &cases { by_host.entry(c.host).or_default().push(c.clone()); }
    let queue: Mutex<VecDeque<(u32, Vec<Case>)>> = Mutex::new(by_host.into_iter().flat_map(|(h, v)| v.chunks(BATCH).map(|b| (h, b.to_vec())).collect::<Vec<_>>()).collect());
    println!("spawn test: {} classes in {} maps, {PARALLEL} at a time, {TICKS} ticks each", cases.len(), queue.lock().unwrap().len());
    let verdicts: Mutex<HashMap<usize, (Live, String)>> = Mutex::new(HashMap::new());
    let counter = Mutex::new(0usize);
    let busy = Mutex::new(0usize);
    std::thread::scope(|s| {
        for _ in 0..PARALLEL {
            s.spawn(|| loop {
                let job = {
                    let mut q = queue.lock().unwrap();
                    match q.pop_front() {
                        Some(j) => { *busy.lock().unwrap() += 1; j }
                        // Another worker may still split a failed batch into the queue.
                        None if *busy.lock().unwrap() > 0 => { drop(q); std::thread::sleep(Duration::from_millis(300)); continue; }
                        None => break,
                    }
                };
                let (host, batch) = job;
                let n = { let mut c = counter.lock().unwrap(); *c += 1; *c };
                let result = run(root, &dir, n, host, &batch).unwrap_or_else(|e| { eprintln!("spawn test: map {n}: {e:#}"); None });
                let mut v = verdicts.lock().unwrap();
                match result {
                    Some(r) => {
                        for c in &batch {
                            let (runs, unported) = r.get(&(c.class as i16)).cloned().unwrap_or_default();
                            let verdict = if runs == 0 {
                                (Live::NeedsWork, format!("its update did not run in {TICKS} ticks on a test map"))
                            } else if !unported.is_empty() {
                                (Live::NeedsWork, format!("reached unported code: {}", unported.join("; ")))
                            } else {
                                (Live::Live, format!("ran {runs} updates on a test map ({} host) with no unported call", LEVEL_NAMES[host as usize]))
                            };
                            v.insert(c.entry, verdict);
                        }
                    }
                    None if batch.len() == 1 => {
                        v.insert(batch[0].entry, (Live::NeedsWork, format!("the test map crashed or did not load (log: spawn-test/log_{n:04}.txt)")));
                    }
                    None => {
                        let (a, b) = batch.split_at(batch.len() / 2);
                        let mut q = queue.lock().unwrap();
                        q.push_back((host, a.to_vec()));
                        q.push_back((host, b.to_vec()));
                    }
                }
                let done = v.len();
                drop(v);
                *busy.lock().unwrap() -= 1;
                println!("spawn test: map {n} ({} on {}, {} classes): {} of {} classes done, {:.0} s", if result_ok(&dir, n) { "ok" } else { "failed" }, LEVEL_NAMES[host as usize], batch.len(), done, cases.len(), t0.elapsed().as_secs_f64());
            });
        }
    });
    let verdicts = verdicts.into_inner().unwrap();
    for (i, (live, note)) in &verdicts {
        let e = &mut catalog.entries[*i];
        e.live = Some(*live);
        e.live_note = note.clone();
    }
    crate::catalog::write(&cat_dir, &catalog)?;
    let live = verdicts.values().filter(|v| v.0 == Live::Live).count();
    println!("spawn test: {live} live, {} need work, of {} tested, in {:.0} s -> {}", verdicts.len() - live, verdicts.len(), t0.elapsed().as_secs_f64(), cat_dir.join("catalog.json").display());
    Ok(())
}

fn result_ok(dir: &Path, n: usize) -> bool { dir.join(format!("report_{n:04}.json")).exists() }

#[cfg(test)]
mod tests {
    use super::*;

    /// Every case's host places both Ratchet and the class (its instance is the template). Needs the game data
    /// (`RC_DATA_DIR`) and its catalog; skipped without them.
    #[test]
    fn hosts_place_their_classes() {
        let Some(root) = std::env::var_os("RC_DATA_DIR").map(PathBuf::from) else { eprintln!("skipped: no RC_DATA_DIR"); return };
        let Ok(text) = std::fs::read_to_string(root.join("catalog/catalog.json")) else { eprintln!("skipped: no catalog"); return };
        let c = Catalog::parse(&text).unwrap();
        for case in cases(&root, &c) {
            let g = rc_data::level_gameplay(&root, case.host).unwrap();
            let classes: Vec<i32> = rc_formats::gameplay::parse_moby_instances(&g).unwrap().iter().map(|m| m.o_class).collect();
            assert!(classes.contains(&0) && classes.contains(&case.class), "class {} on host {:02}: {:?}", case.class, case.host, c.entries[case.entry].id);
        }
    }

    #[test]
    fn test_maps_parse() {
        let cases: Vec<Case> = (0..3).map(|i| Case { entry: i, host: 9, class: 193 + i as i32 }).collect();
        let m = rc_map::MapDef::parse(&map_json(9, &cases)).unwrap();
        assert_eq!((m.host_level, m.objects.len(), m.imports()), (9, 3, vec![]));
        assert!(m.reset_on_death);
    }
}
