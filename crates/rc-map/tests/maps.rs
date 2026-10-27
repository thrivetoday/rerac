//! The shipped example maps parse, and the Phase 0 map patches into Novalis with Gaspar's pack biter (skipped
//! without the development data tree, `RC_EXTRACTED`: rc_formats::test_data).

use rc_formats::gameplay;
use rc_map::patch::{patch_gameplay, GameplayPatch, TemplateRecord};
use rc_map::MapDef;

fn phase0() -> MapDef { MapDef::parse(include_str!("../maps/phase0.json")).expect("maps/phase0.json") }

#[test]
fn example_map_parses() {
    let m = phase0();
    assert_eq!((m.host_level, m.imports()), (1, vec![(9, 193)]));
    assert!(m.floors.iter().all(|f| rc_map::floor::check(f).is_ok()));
}

#[test]
fn phase0_patches_novalis() {
    let (Some(host), Some(gaspar)) = (rc_formats::test_data::gameplay(1), rc_formats::test_data::gameplay(9)) else {
        eprintln!("skipped: no development data (RC_EXTRACTED)");
        return;
    };
    let m = phase0();
    let mut records = vec![TemplateRecord::first_of_class(&host, 0).unwrap().placed(m.start.position, m.start.yaw)];
    for o in &m.objects {
        let source = if o.asset.level == 1 { &host } else { &gaspar };
        let t = TemplateRecord::from_level(source, o.template.unwrap()).unwrap();
        assert_eq!(t.instance.o_class, o.asset.number, "template of {}", o.asset);
        records.push(t.placed(o.position, o.yaw));
    }
    let patch = GameplayPatch { records, rails: m.rails.iter().map(|r| r.points.clone()).collect(), groups: Vec::new(), splines: Vec::new(), cuboids: Vec::new(), settings: Vec::new(), ship: None, death_z: m.death_height };
    let g = patch_gameplay(&host, &patch).unwrap();

    let inst = gameplay::parse_moby_instances(&g).unwrap();
    assert_eq!(inst.iter().map(|i| i.o_class).collect::<Vec<_>>(), vec![0, 500, 193]);
    assert_eq!(inst[0].position, m.start.position);
    let pvars = gameplay::parse_pvars(&g).unwrap();
    // The biter's block is Gaspar instance 52's, 0x270 bytes (units::pack_biter), its leash moby link cut.
    let biter = inst[2].pvar(&pvars).unwrap();
    assert_eq!(biter.len(), 0x270);
    assert_eq!(rc_formats::volumes::parse_grind_paths(&g).unwrap().len(), 1);
    assert_eq!(gameplay::ship_placement(&g).unwrap(), None);
    // The sections the map leaves alone still read.
    assert!(rc_formats::volumes::parse_volumes(&g).is_ok());
    assert!(gameplay::parse_fog_zones(&g).is_ok());
    assert!(gameplay::parse_static_lights(&g).is_ok());
    assert!(rc_formats::cameras::parse_level_cameras(&g).unwrap().is_empty());
}

/// The Phase 3 gate: three islands joined by two bridges, railings round the islands with openings where the
/// bridges land. Its collision fits whole (no face left out), and walking the bridges from island to island finds
/// ground under every step, never more than a step up or down from the last. `RC_WRITE_MAPS=1` writes it to
/// `maps/three_islands.json`; the shipped file must equal it.
fn three_islands() -> MapDef {
    use rc_map::terrain::{Edge, EdgeKind, Patch};
    use rc_map::walkway::Walkway;
    let railing = EdgeKind::Piece(rc_map::AssetId::parse("rac1/L02/tie/495").unwrap());
    let grass = rc_map::AssetId::parse("rac1/L01/tfrag-tex/8").unwrap();
    let mut m = MapDef::new_default("Three islands", 1);
    m.patches.clear();
    let islands: [([f32; 3], f32); 3] = [([150.0, 150.0, 60.0], 12.0), ([196.0, 150.0, 64.0], 10.0), ([196.0, 196.0, 68.0], 11.0)];
    for (c, r) in &islands {
        let mut p = Patch::island(*c, 1.0, *r, |x, y| r - (x * x + y * y).sqrt());
        p.texture = Some(grass);
        p.depth = 10.0;
        // The truss railing: 4.3 long, 1 deep (its model).
        p.edge = Some(Edge { step: 4.3, inset: 1.0625, ..Edge::new(railing.clone(), 2.5) });
        m.patches.push(p);
    }
    let bridge = |a: [f32; 3], b: [f32; 3]| {
        let mut w = Walkway::new(vec![a, [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0, (a[2] + b[2]) / 2.0 + 0.5], b], 5.0);
        w.depth = 0.5;
        w.edge = Some(Edge { step: 4.3, inset: 1.0625, ..Edge::new(railing.clone(), 2.5) });
        w
    };
    // Each bridge starts a little inside one island and ends a little inside the next, at their heights.
    m.walkways.push(bridge([160.0, 150.0, 60.0], [188.0, 150.0, 64.0]));
    m.walkways.push(bridge([196.0, 158.0, 64.0], [196.0, 187.0, 68.0]));
    // The railings open where the bridges come in (as the editor does when a path is drawn).
    for p in &mut m.patches {
        let ws = m.walkways.clone();
        // Inside the bridges' own railings: the island's railing comes right up to theirs.
        p.open_where(|x, y| ws.iter().any(|w| w.inside(x, y, w.edge.as_ref().map_or(0.0, |e| e.inset))));
    }
    m.start = rc_map::Start { position: [146.0, 150.0, 60.5], yaw: 0.0 };
    m.death_height = Some(40.0);
    m
}

#[test]
fn three_islands_gate() {
    let m = three_islands();
    let text = m.to_json();
    if std::env::var_os("RC_WRITE_MAPS").is_some() { std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/maps/three_islands.json"), &text).unwrap(); }
    assert_eq!(MapDef::parse(include_str!("../maps/three_islands.json")).expect("maps/three_islands.json"), m, "the shipped map is out of date (RC_WRITE_MAPS=1)");
    let rails: Vec<Vec<[f32; 3]>> = m.rails.iter().map(|r| r.points.clone()).collect();
    let (_, dropped) = rc_map::floor::collision_counted(&m.floors, &m.patches, &m.walkways, &rails, &[], rc_map::floor::GROUND);
    assert_eq!(dropped, 0, "collision faces left out");
    // The ground under (x, y) closest below `z` + a step.
    let ground = |x: f32, y: f32, z: f32| {
        m.patches.iter().filter_map(|p| p.height_at(x, y)).chain(m.walkways.iter().filter_map(|w| w.height_at(x, y))).filter(|&h| h <= z + 0.75).reduce(f32::max)
    };
    // From each island's middle across its bridge to the next one's middle, a quarter unit at a time.
    for (from, to) in [([150.0, 150.0, 60.0], [196.0, 150.0, 64.0]), ([196.0, 150.0, 64.0], [196.0, 196.0, 68.0])] {
        let n = 400;
        let mut z: f32 = from[2];
        for i in 0..=n {
            let t = i as f32 / n as f32;
            let (x, y) = (from[0] + (to[0] - from[0]) * t, from[1] + (to[1] - from[1]) * t);
            let h = ground(x, y, z).unwrap_or_else(|| panic!("a hole at {x:.2}, {y:.2} (from {from:?})"));
            assert!((h - z).abs() < 0.75, "a step of {:.2} at {x:.2}, {y:.2}", h - z);
            z = h;
        }
        assert!((z - to[2]).abs() < 0.1, "arrived at {z}, not {}", to[2]);
    }
    // Every island has an opening for each bridge that lands on it (1, 2, 1).
    assert_eq!(m.patches.iter().map(|p| p.edge.as_ref().map_or(0, |e| e.gaps.len())).collect::<Vec<_>>(), [1, 2, 1]);
    // Sealed: where an island's railing stops for a bridge, it meets the bridge's railing (no gap beside the bridge
    // to walk off through). Each end of an island's barrier near a bridge lies on the bridge's barrier line.
    let seg_dist = |p: [f32; 2], a: [f32; 3], b: [f32; 3]| {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / (dx * dx + dy * dy).max(1e-9)).clamp(0.0, 1.0);
        ((p[0] - a[0] - dx * t).powi(2) + (p[1] - a[1] - dy * t).powi(2)).sqrt()
    };
    for w in &m.walkways {
        let walls = w.fence(8.0);
        for p in &m.patches {
            let fence = p.fence(8.0);
            // The island's barrier corners within 3 units of the bridge's top.
            let near: Vec<[f32; 2]> = fence.iter().flat_map(|q| [q[0], q[3]]).map(|v| [v[0], v[1]]).filter(|v| {
                (-3..=3).any(|i| (-3..=3).any(|j| w.height_at(v[0] + i as f32, v[1] + j as f32).is_some()))
            }).collect();
            if near.is_empty() { continue; }
            // The ends: the corners nearest the bridge's middle line on each side.
            let gap = near.iter().map(|v| walls.iter().map(|q| seg_dist(*v, q[0], q[3])).fold(f32::MAX, f32::min)).fold(f32::MAX, f32::min);
            assert!(gap < 0.35, "an island railing stops {gap:.2} short of a bridge's railing");
        }
    }
    // The railings stop at the openings: no barrier across the bridges' ends.
    for (k, p) in m.patches.iter().enumerate() {
        for q in p.fence(8.0) {
            let mid = [(q[0][0] + q[3][0]) / 2.0, (q[0][1] + q[3][1]) / 2.0];
            for w in &m.walkways {
                // Not across the walkable middle (inside the bridge's own railings, a little in).
                let inner = w.edge.as_ref().map_or(0.0, |e| e.inset) + 0.1;
                assert!(!w.inside(mid[0], mid[1], inner) || w.height_at(mid[0], mid[1]).is_some_and(|h| (h - q[0][2]).abs() > 3.0), "island {k}'s railing stands across a bridge at {mid:?}");
            }
        }
    }
}
