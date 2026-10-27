//! RAC Maker's look (docs/plan/rac_maker.md, "v0.6: the layout redesign"): the colours, the painted icons and the
//! widgets made of them (tool buttons, icon buttons, icon tiles, cards, key caps). Choices are shown as pictures with a
//! short caption; the help holds the explanations.

use super::*;

pub(super) const ORANGE: egui::Color32 = egui::Color32::from_rgb(0xf0, 0x8a, 0x24);
pub(super) const AMBER: egui::Color32 = egui::Color32::from_rgb(0xff, 0xb3, 0x47);
pub(super) const NAVY: egui::Color32 = egui::Color32::from_rgb(0x1b, 0x27, 0x40);
pub(super) const NAVY2: egui::Color32 = egui::Color32::from_rgb(0x22, 0x31, 0x4f);
pub(super) const DEEP: egui::Color32 = egui::Color32::from_rgb(0x13, 0x1c, 0x2e);
pub(super) const LINE: egui::Color32 = egui::Color32::from_rgb(0x2f, 0x42, 0x68);
pub(super) const CARD: egui::Color32 = egui::Color32::from_rgb(0x20, 0x2e, 0x4a);
pub(super) const TILE: egui::Color32 = egui::Color32::from_rgb(0x2a, 0x37, 0x55);
pub(super) const TILE_HOVER: egui::Color32 = egui::Color32::from_rgb(0x3a, 0x4d, 0x75);
pub(super) const TILE_ON: egui::Color32 = egui::Color32::from_rgb(0x4a, 0x35, 0x18);
pub(super) const INK: egui::Color32 = egui::Color32::from_rgb(0xe8, 0xee, 0xf9);
pub(super) const DIM: egui::Color32 = egui::Color32::from_rgb(0x8f, 0xa4, 0xc8);
pub(super) const GREEN: egui::Color32 = egui::Color32::from_rgb(0x8f, 0xe3, 0x88);
pub(super) const RED: egui::Color32 = egui::Color32::from_rgb(0xef, 0x6b, 0x6b);
pub(super) const DARK_INK: egui::Color32 = egui::Color32::from_rgb(0x1a, 0x1f, 0x2e);

/// The surfaces' colours (rc_map::floor::SURFACES' bytes, as the Collision view draws them).
pub(super) fn surface_colour(b: u8) -> egui::Color32 {
    match b {
        7 => egui::Color32::from_rgb(0x6c, 0xd8, 0xff),
        1 => egui::Color32::from_rgb(0xff, 0x9a, 0x3c),
        8 => egui::Color32::from_rgb(0xef, 0x5b, 0x5b),
        2 => egui::Color32::from_rgb(0xc0, 0x7c, 0xff),
        _ => egui::Color32::from_rgb(0x7e, 0xd6, 0x6e),
    }
}

/// A surface's icon.
pub(super) fn surface_icon(b: u8) -> &'static str {
    match b { 7 => "Slippery", 1 => "Hot", 8 => "Deadly", 2 => "Magnetic", _ => "Ground" }
}

/// An icon `name`, `s` points across around `c`, drawn in `ink` (drawn on a 20-unit box: -10..10).
pub(super) fn draw_icon(painter: &egui::Painter, c: egui::Pos2, s: f32, name: &str, ink: egui::Color32) {
    let k = s / 20.0;
    let p = |x: f32, y: f32| egui::pos2(c.x + x * k, c.y + y * k);
    let w = (1.6 * k).max(1.2);
    let line = egui::Stroke::new(w, ink);
    let thin = egui::Stroke::new((1.1 * k).max(1.0), ink);
    let poly = |pts: Vec<egui::Pos2>| egui::Shape::line(pts, line);
    let closed = |pts: Vec<egui::Pos2>| egui::Shape::closed_line(pts, line);
    let fill = |pts: Vec<egui::Pos2>| egui::Shape::convex_polygon(pts, ink, egui::Stroke::NONE);
    let arc = |cx: f32, cy: f32, rx: f32, ry: f32, a0: f32, a1: f32| -> Vec<egui::Pos2> {
        (0..=20).map(|j| { let a = a0 + (a1 - a0) * j as f32 / 20.0; p(cx + rx * a.cos(), cy + ry * a.sin()) }).collect()
    };
    let seg = |a: (f32, f32), b: (f32, f32)| painter.line_segment([p(a.0, a.1), p(b.0, b.1)], line);
    let ring = |x: f32, y: f32, r: f32| painter.circle_stroke(p(x, y), r * k, line);
    let dot = |x: f32, y: f32, r: f32| painter.circle_filled(p(x, y), r * k, ink);
    let text = |t: &str, size: f32| painter.text(c, egui::Align2::CENTER_CENTER, t, egui::FontId::proportional(size * k), ink);
    use std::f32::consts::{FRAC_PI_2, PI, TAU};
    match name {
        // The tools.
        "Select" => {
            let pts = vec![p(-4.0, -8.0), p(-4.0, 6.0), p(-0.5, 2.5), p(2.0, 8.0), p(4.0, 7.0), p(1.5, 1.5), p(6.0, 1.5)];
            painter.add(fill(pts[..3].iter().copied().chain([pts[6]]).collect()));
            painter.add(closed(pts));
        }
        "Objects" | "Place" => {
            // A crate: its front, top and side.
            painter.add(closed(vec![p(-8.0, -3.0), p(4.0, -3.0), p(4.0, 9.0), p(-8.0, 9.0)]));
            painter.add(poly(vec![p(-8.0, -3.0), p(-4.0, -7.0), p(8.0, -7.0), p(4.0, -3.0)]));
            painter.add(poly(vec![p(8.0, -7.0), p(8.0, 5.0), p(4.0, 9.0)]));
            seg((-8.0, -3.0), (4.0, 9.0));
            seg((4.0, -3.0), (-8.0, 9.0));
        }
        "Islands" | "Build" => {
            painter.add(closed(arc(0.0, -3.0, 8.5, 3.5, 0.0, TAU)));
            seg((-8.5, -3.0), (-6.0, 5.0));
            seg((8.5, -3.0), (6.0, 5.0));
            painter.add(poly(arc(0.0, 5.0, 6.0, 3.5, 0.0, PI)));
        }
        "Shape" => {
            let pts = vec![p(-8.0, 1.0), p(-5.0, -6.0), p(1.0, -7.0), p(7.0, -3.0), p(6.0, 4.0), p(0.0, 7.0), p(-6.0, 6.0)];
            painter.add(closed(pts.clone()));
            for q in [pts[1], pts[3], pts[5]] { painter.circle_filled(q, 2.2 * k, ink); }
        }
        "Edges" | "Openings" | "Railings" => {
            let gap = name != "Railings";
            let spans: &[(f32, f32)] = if gap { &[(-9.0, -3.0), (3.0, 9.0)] } else { &[(-9.0, 9.0)] };
            for &(x0, x1) in spans {
                seg((x0, -4.0), (x1, -4.0));
                seg((x0, 1.0), (x1, 1.0));
                let n = ((x1 - x0) / 5.0).round().max(1.0) as usize;
                for j in 0..=n { let x = x0 + 0.8 + (x1 - x0 - 1.6) * j as f32 / n as f32; seg((x, -6.0), (x, 6.0)); }
            }
            painter.line_segment([p(-9.0, 7.5), p(9.0, 7.5)], thin);
        }
        "Sculpt" => {
            let hill: Vec<egui::Pos2> = (0..=16).map(|j| { let x = -9.0 + 18.0 * j as f32 / 16.0; p(x, 7.0 - 9.0 * (-(x * x) / 18.0).exp()) }).collect();
            painter.add(poly(hill));
            painter.add(closed(arc(0.0, -6.0, 4.0, 1.6, 0.0, TAU)));
            painter.line_segment([p(0.0, -4.4), p(0.0, -1.0)], thin);
        }
        "Paths" => {
            seg((-8.0, 8.0), (-2.0, -8.0));
            seg((8.0, 8.0), (2.0, -8.0));
            for (y0, y1) in [(5.0, 8.0), (-1.0, 2.0), (-6.0, -4.0)] { painter.line_segment([p(0.0, y0), p(0.0, y1)], thin); }
        }
        "Rails" => {
            let rail = arc(0.0, 12.0, 11.0, 16.0, -2.4, -0.74);
            for j in [2usize, 6, 10, 14, 18] {
                let (a, b) = (rail[j - 1], rail[j + 1]);
                let d = (b - a).normalized();
                let n = egui::vec2(-d.y, d.x) * 3.5 * k;
                painter.line_segment([rail[j] - n, rail[j] + n], thin);
            }
            painter.add(egui::Shape::line(rail, egui::Stroke::new(2.4 * k, ink)));
        }
        "Logic" | "Checkpoint" => {
            seg((-6.0, 9.0), (-6.0, -9.0));
            painter.add(fill(vec![p(-6.0, -9.0), p(8.0, -5.5), p(-6.0, -2.0)]));
        }
        "Goal" => {
            seg((-7.0, 9.0), (-7.0, -9.0));
            // A chequered flag.
            for (j, (x, y)) in [(0, (-7.0, -9.0)), (1, (-2.0, -9.0)), (2, (3.0, -9.0)), (3, (-7.0, -4.5)), (4, (-2.0, -4.5)), (5, (3.0, -4.5))] {
                let r = egui::Rect::from_min_max(p(x, y), p(x + 5.0, y + 4.5));
                if (j + j / 3) % 2 == 0 { painter.rect_filled(r, 0.0, ink); }
            }
            painter.rect_stroke(egui::Rect::from_min_max(p(-7.0, -9.0), p(8.0, 0.0)), 0.0, thin, egui::StrokeKind::Middle);
        }
        "World" => {
            ring(-1.5, 1.5, 7.0);
            painter.add(poly(arc(-1.5, 1.5, 7.0, 2.5, 0.0, PI)));
            painter.add(poly(arc(-1.5, 1.5, 3.0, 7.0, -FRAC_PI_2, FRAC_PI_2)));
            dot(7.0, -7.0, 2.4);
            for j in 0..6 {
                let a = j as f32 * TAU / 6.0;
                painter.line_segment([p(7.0 + 3.4 * a.cos(), -7.0 + 3.4 * a.sin()), p(7.0 + 4.6 * a.cos(), -7.0 + 4.6 * a.sin())], thin);
            }
        }
        "Map" => {
            // A folded map.
            painter.add(closed(vec![p(-9.0, -6.0), p(-3.0, -8.0), p(3.0, -6.0), p(9.0, -8.0), p(9.0, 6.0), p(3.0, 8.0), p(-3.0, 6.0), p(-9.0, 8.0)]));
            seg((-3.0, -8.0), (-3.0, 6.0));
            seg((3.0, -6.0), (3.0, 8.0));
        }
        // The top bar.
        "New" => {
            painter.add(closed(vec![p(-6.0, -9.0), p(3.0, -9.0), p(7.0, -5.0), p(7.0, 9.0), p(-6.0, 9.0)]));
            painter.add(poly(vec![p(3.0, -9.0), p(3.0, -5.0), p(7.0, -5.0)]));
            seg((0.5, -1.0), (0.5, 6.0));
            seg((-3.0, 2.5), (4.0, 2.5));
        }
        "Save" => {
            painter.add(closed(vec![p(-8.0, -8.0), p(5.0, -8.0), p(8.0, -5.0), p(8.0, 8.0), p(-8.0, 8.0)]));
            painter.rect_stroke(egui::Rect::from_min_max(p(-4.5, -8.0), p(3.5, -3.0)), 0.0, thin, egui::StrokeKind::Middle);
            painter.rect_stroke(egui::Rect::from_min_max(p(-5.0, 1.5), p(5.0, 8.0)), 1.0, thin, egui::StrokeKind::Middle);
        }
        "Undo" | "Redo" => {
            let m = if name == "Undo" { 1.0 } else { -1.0 };
            let pts: Vec<egui::Pos2> = (0..=16).map(|j| { let a = PI + PI * j as f32 / 16.0; p(m * (1.0 + 6.0 * a.cos()) * -1.0, 3.0 + 6.0 * a.sin()) }).collect();
            painter.add(poly(pts));
            let tip = p(-5.0 * m, 3.0);
            painter.add(fill(vec![tip + egui::vec2(0.0, 4.0 * k), tip + egui::vec2(-3.5 * k, -1.5 * k), tip + egui::vec2(3.5 * k, -1.5 * k)]));
        }
        "Picture" | "Camera" => {
            painter.rect_stroke(egui::Rect::from_min_max(p(-9.0, -5.0), p(9.0, 8.0)), 2.0 * k, line, egui::StrokeKind::Middle);
            painter.add(closed(vec![p(-4.0, -5.0), p(-2.5, -8.0), p(2.5, -8.0), p(4.0, -5.0)]));
            ring(0.0, 1.5, 4.0);
            dot(6.0, -2.5, 0.9);
        }
        "Collision" => {
            painter.add(closed(vec![p(-9.0, 4.0), p(-2.0, -6.0), p(9.0, -4.0), p(2.0, 7.0)]));
            painter.line_segment([p(-2.0, -6.0), p(2.0, 7.0)], thin);
            painter.line_segment([p(-5.5, -1.0), p(5.5, 1.5)], thin);
            painter.line_segment([p(-9.0, 4.0), p(9.0, -4.0)], thin);
        }
        "View" => {
            painter.add(poly(arc(0.0, 7.0, 10.0, 11.0, -PI * 0.82, -PI * 0.18)));
            painter.add(poly(arc(0.0, -7.0, 10.0, 11.0, PI * 0.18, PI * 0.82)));
            ring(0.0, 0.0, 3.4);
            dot(0.0, 0.0, 1.5);
        }
        "Radio" | "Song" => {
            dot(-5.0, 6.0, 2.8);
            dot(5.0, 4.0, 2.8);
            seg((-2.4, 6.0), (-2.4, -6.0));
            seg((7.6, 4.0), (7.6, -8.0));
            painter.add(fill(vec![p(-2.4, -6.0), p(7.6, -8.0), p(7.6, -5.0), p(-2.4, -3.0)]));
        }
        "Help" => { ring(0.0, 0.0, 9.0); text("?", 14.0); }
        "Bin" => {
            seg((-8.0, -6.0), (8.0, -6.0));
            painter.add(poly(vec![p(-3.0, -6.0), p(-2.0, -9.0), p(2.0, -9.0), p(3.0, -6.0)]));
            painter.add(poly(vec![p(-6.0, -6.0), p(-5.0, 9.0), p(5.0, 9.0), p(6.0, -6.0)]));
            for x in [-2.0, 2.0] { painter.line_segment([p(x, -2.0), p(x, 6.0)], thin); }
        }
        "Warn" => {
            painter.add(closed(vec![p(0.0, -9.0), p(9.5, 8.0), p(-9.5, 8.0)]));
            seg((0.0, -3.0), (0.0, 3.0));
            dot(0.0, 5.5, 1.1);
        }
        "Play" => { painter.add(fill(vec![p(-6.0, -8.0), p(8.0, 0.0), p(-6.0, 8.0)])); }
        "Stop" => { painter.rect_filled(egui::Rect::from_min_max(p(-6.5, -6.5), p(6.5, 6.5)), 1.5 * k, ink); }
        "Refresh" => {
            painter.add(poly(arc(0.0, 0.0, 7.0, 7.0, -PI * 0.35, PI * 1.45)));
            let tip = p(7.0 * (-PI * 0.35f32).cos(), 7.0 * (-PI * 0.35f32).sin());
            painter.add(fill(vec![tip + egui::vec2(-4.5 * k, -1.0 * k), tip + egui::vec2(1.5 * k, -4.5 * k), tip + egui::vec2(1.5 * k, 2.5 * k)]));
        }
        "Loadout" => {
            // A blaster.
            painter.add(closed(vec![p(-9.0, -4.0), p(7.0, -4.0), p(7.0, 0.0), p(-1.0, 0.0), p(-3.0, 7.0), p(-7.0, 7.0), p(-6.0, 0.0), p(-9.0, 0.0)]));
            seg((7.0, -2.0), (9.5, -2.0));
            dot(-4.0, -2.0, 1.0);
        }
        "Plus" => { seg((0.0, -7.0), (0.0, 7.0)); seg((-7.0, 0.0), (7.0, 0.0)); }
        "Close" => { seg((-6.0, -6.0), (6.0, 6.0)); seg((6.0, -6.0), (-6.0, 6.0)); }
        "Back" => { painter.add(poly(vec![p(4.0, -8.0), p(-4.0, 0.0), p(4.0, 8.0)])); }
        "Light" => {
            ring(0.0, -2.5, 6.0);
            painter.add(poly(vec![p(-3.0, 3.0), p(-3.0, 7.0), p(3.0, 7.0), p(3.0, 3.0)]));
            seg((-2.0, 9.0), (2.0, 9.0));
        }
        "Start" => {
            painter.add(closed(arc(0.0, 4.0, 8.0, 3.5, 0.0, TAU)));
            painter.add(fill(vec![p(0.0, -9.0), p(4.5, -3.0), p(-4.5, -3.0)]));
            seg((0.0, -3.0), (0.0, 4.0));
        }
        "Floor" => {
            painter.add(closed(vec![p(-9.0, 4.0), p(-3.0, -4.0), p(9.0, -4.0), p(3.0, 4.0)]));
            painter.add(poly(vec![p(-9.0, 4.0), p(-9.0, 7.0), p(3.0, 7.0), p(9.0, -1.0), p(9.0, -4.0)]));
            seg((3.0, 4.0), (3.0, 7.0));
        }
        "Ship" => {
            painter.add(closed(vec![p(-9.0, 2.0), p(-2.0, -2.0), p(9.0, -1.0), p(2.0, 3.0)]));
            painter.add(closed(vec![p(-4.0, -1.0), p(-1.0, -7.0), p(2.0, -1.0)]));
            seg((-6.0, 3.5), (-8.0, 8.0));
            seg((4.0, 2.5), (6.0, 8.0));
        }
        "Death" | "Deadly" => {
            // A skull.
            painter.add(closed(arc(0.0, -2.0, 7.5, 7.0, PI * 0.8, PI * 2.2)));
            painter.add(poly(vec![p(-6.0, 2.2), p(-4.0, 3.5), p(-4.0, 8.0), p(4.0, 8.0), p(4.0, 3.5), p(6.0, 2.2)]));
            dot(-3.0, -1.5, 2.2);
            dot(3.0, -1.5, 2.2);
            for x in [-1.5, 1.5] { painter.line_segment([p(x, 5.0), p(x, 8.0)], thin); }
        }
        "Trigger" => {
            // A lightning bolt.
            painter.add(fill(vec![p(3.0, -10.0), p(-6.0, 2.0), p(1.0, 2.0)]));
            painter.add(fill(vec![p(-2.0, 10.0), p(6.0, -2.0), p(-1.0, -2.0)]));
        }
        "Wall" | "Walls" => {
            painter.rect_stroke(egui::Rect::from_min_max(p(-9.0, -7.0), p(9.0, 7.0)), 0.0, line, egui::StrokeKind::Middle);
            for y in [-2.3, 2.3] { painter.line_segment([p(-9.0, y), p(9.0, y)], thin); }
            for (y0, y1, xs) in [(-7.0, -2.3, &[-3.0, 3.0][..]), (-2.3, 2.3, &[-6.0, 0.0, 6.0][..]), (2.3, 7.0, &[-3.0, 3.0][..])] {
                for &x in xs { painter.line_segment([p(x, y0), p(x, y1)], thin); }
            }
        }
        "Invisible" => {
            for j in 0..5 {
                let x = -9.0 + j as f32 * 4.0;
                painter.line_segment([p(x, -7.0), p(x + 2.0, -7.0)], thin);
                painter.line_segment([p(x, 7.0), p(x + 2.0, 7.0)], thin);
            }
            for j in 0..4 {
                let y = -7.0 + j as f32 * 4.0;
                painter.line_segment([p(-9.0, y), p(-9.0, y + 2.0)], thin);
                painter.line_segment([p(9.0, y), p(9.0, y + 2.0)], thin);
            }
        }
        "None" => { ring(0.0, 0.0, 7.5); seg((-5.3, 5.3), (5.3, -5.3)); }
        // Island shapes.
        "Round" => { painter.add(closed(arc(0.0, 0.0, 8.5, 6.0, 0.0, TAU))); }
        "Square" => { painter.rect_stroke(egui::Rect::from_min_max(p(-8.0, -6.5), p(8.0, 6.5)), 1.0, line, egui::StrokeKind::Middle); }
        "Blob" => {
            let pts: Vec<egui::Pos2> = (0..24).map(|j| {
                let a = j as f32 * TAU / 24.0;
                let r = 7.0 + 1.6 * (3.0 * a).sin() + 0.9 * (5.0 * a + 1.0).cos();
                p(r * a.cos() * 1.15, r * a.sin() * 0.85)
            }).collect();
            painter.add(closed(pts));
        }
        "Draw" => {
            // A coast traced by hand, and the pencil at its end.
            painter.add(poly(vec![p(2.0, 6.5), p(-4.0, 7.0), p(-8.5, 3.0), p(-7.0, -2.0), p(-8.0, -6.0), p(-3.0, -7.5), p(1.0, -4.0), p(4.0, -6.0)]));
            seg((4.0, -6.0), (8.5, -1.5));
            seg((5.5, -7.5), (10.0, -3.0));
            seg((8.5, -1.5), (10.0, -3.0));
        }
        // Surfaces.
        "Ground" => {
            seg((-9.0, 6.0), (9.0, 6.0));
            for x in [-5.0, 0.0, 5.0] {
                painter.add(poly(vec![p(x - 2.5, 6.0), p(x - 1.0, -1.0)]));
                painter.add(poly(vec![p(x, 6.0), p(x, -4.0)]));
                painter.add(poly(vec![p(x + 2.5, 6.0), p(x + 1.0, -1.0)]));
            }
        }
        "Slippery" => {
            for j in 0..3 {
                let a = j as f32 * PI / 3.0 + FRAC_PI_2;
                let (dx, dy) = (8.0 * a.cos(), 8.0 * a.sin());
                seg((-dx, -dy), (dx, dy));
                for sgn in [-1.0, 1.0] {
                    let (ex, ey) = (sgn * 5.0 * a.cos(), sgn * 5.0 * a.sin());
                    let (nx, ny) = (-a.sin() * 2.2, a.cos() * 2.2);
                    let tip = (sgn * 7.5 * a.cos(), sgn * 7.5 * a.sin());
                    seg((ex + nx, ey + ny), tip);
                    seg((ex - nx, ey - ny), tip);
                }
            }
        }
        "Hot" => {
            let mut pts = arc(0.0, 3.0, 6.0, 6.0, -0.35, PI + 0.35);
            pts.push(p(-3.0, -4.0));
            pts.push(p(-1.0, -2.0));
            pts.push(p(0.5, -9.5));
            pts.push(p(3.5, -3.0));
            pts.push(p(5.5, -0.5));
            painter.add(closed(pts));
            painter.add(closed(arc(0.0, 4.5, 2.5, 3.0, 0.0, TAU)));
        }
        "Magnetic" => {
            painter.add(egui::Shape::line(arc(0.0, -1.0, 6.0, 6.0, 0.0, PI), egui::Stroke::new(3.2 * k, ink)));
            for x in [-6.0, 6.0] {
                painter.line_segment([p(x, -1.0), p(x, -8.0)], egui::Stroke::new(3.2 * k, ink));
                painter.line_segment([p(x - 1.6, -6.0), p(x + 1.6, -6.0)], egui::Stroke::new(1.0 * k, DARK_INK));
            }
        }
        // Brushes.
        "Raise" | "Lower" => {
            let up = name == "Raise";
            let hill: Vec<egui::Pos2> = (0..=16).map(|j| {
                let x = -9.0 + 18.0 * j as f32 / 16.0;
                let h = 6.0 * (-(x * x) / 16.0).exp();
                p(x, if up { 7.0 - h } else { 1.0 + h })
            }).collect();
            painter.add(poly(hill));
            let (y0, y1) = if up { (-2.0, -9.0) } else { (-9.0, -2.0) };
            seg((0.0, y0), (0.0, y1));
            painter.add(poly(vec![p(-3.0, y1 + if up { 3.0 } else { -3.0 }), p(0.0, y1), p(3.0, y1 + if up { 3.0 } else { -3.0 })]));
        }
        "Plateau" => { painter.add(poly(vec![p(-9.0, 7.0), p(-5.0, 7.0), p(-3.0, -3.0), p(3.0, -3.0), p(5.0, 7.0), p(9.0, 7.0)])); }
        "Smooth" => {
            let wave = |amp: f32, y: f32| -> Vec<egui::Pos2> { (0..=20).map(|j| { let x = -9.0 + 18.0 * j as f32 / 20.0; p(x, y + amp * (x * 1.3).sin()) }).collect() };
            painter.add(egui::Shape::line(wave(2.5, -3.0), thin));
            painter.add(poly(wave(0.8, 5.0)));
        }
        "Flatten" => {
            let bumps: Vec<egui::Pos2> = (0..=20).map(|j| { let x = -9.0 + 18.0 * j as f32 / 20.0; p(x, 5.0 + 2.5 * (x * 1.1).sin().abs()) }).collect();
            painter.add(egui::Shape::line(bumps, thin));
            seg((-9.0, 1.0), (9.0, 1.0));
            seg((0.0, -9.0), (0.0, -3.0));
            painter.add(poly(vec![p(-3.0, -6.0), p(0.0, -3.0), p(3.0, -6.0)]));
        }
        "Paint" => {
            painter.add(closed(vec![p(3.0, -9.0), p(7.0, -5.0), p(-1.0, 3.0), p(-5.0, -1.0)]));
            painter.add(fill(vec![p(-5.0, -1.0), p(-1.0, 3.0), p(-5.0, 9.0), p(-9.0, 7.0)]));
        }
        "Land" => {
            painter.add(closed(arc(-2.0, 2.0, 6.0, 4.5, 0.0, TAU)));
            seg((3.0, -3.0), (9.0, -9.0));
            painter.add(poly(vec![p(4.5, -9.0), p(9.0, -9.0), p(9.0, -4.5)]));
        }
        "Shade" => {
            ring(0.0, 0.0, 8.0);
            painter.add(fill(arc(0.0, 0.0, 8.0, 8.0, -FRAC_PI_2, FRAC_PI_2)));
        }
        "Decor" => {
            seg((0.0, 9.0), (0.0, -2.0));
            ring(0.0, -4.0, 4.0);
            ring(-5.0, 0.5, 3.5);
            ring(5.0, 0.5, 3.5);
            seg((-9.0, 9.0), (9.0, 9.0));
        }
        "Gaps" => {
            draw_icon(painter, c + egui::vec2(-2.0 * k, 0.0), s * 0.8, "Railings", ink);
            painter.line_segment([p(3.0, -9.0), p(9.0, -3.0)], egui::Stroke::new(2.4 * k, RED));
            painter.line_segment([p(9.0, -9.0), p(3.0, -3.0)], egui::Stroke::new(2.4 * k, RED));
        }
        _ => { text(name.get(..1).unwrap_or("?"), 14.0); }
    }
}

/// A painted icon in the line (`s` points).
pub(super) fn icon(ui: &mut egui::Ui, name: &str, s: f32, ink: egui::Color32) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(egui::vec2(s, s), egui::Sense::hover());
    draw_icon(ui.painter(), rect.center(), s * 0.9, name, ink);
    r
}

/// A small icon button (the top bar's): `on` fills it orange.
pub(super) fn icon_button(ui: &mut egui::Ui, name: &str, hint: &str, on: bool) -> egui::Response {
    icon_button_ink(ui, name, hint, on, INK)
}

/// [`icon_button`] with its icon in `ink`.
pub(super) fn icon_button_ink(ui: &mut egui::Ui, name: &str, hint: &str, on: bool, ink: egui::Color32) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(egui::vec2(32.0, 28.0), egui::Sense::click());
    let enabled = ui.is_enabled();
    let fill = if on { ORANGE } else if r.hovered() && enabled { TILE_HOVER } else { egui::Color32::TRANSPARENT };
    ui.painter().rect_filled(rect, 6.0, fill);
    let ink = if !enabled { egui::Color32::from_gray(100) } else if on { DARK_INK } else { ink };
    draw_icon(ui.painter(), rect.center(), 18.0, name, ink);
    r.on_hover_text(hint)
}

/// The Play button: orange, its icon and word.
pub(super) fn play_button(ui: &mut egui::Ui, icon_name: &str, text: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), egui::FontId::proportional(15.0), DARK_INK);
    let (rect, r) = ui.allocate_exact_size(egui::vec2(galley.size().x + 46.0, 30.0), egui::Sense::click());
    let fill = if r.hovered() { egui::Color32::from_rgb(0xff, 0xa0, 0x40) } else { ORANGE };
    ui.painter().rect_filled(rect, 15.0, fill);
    draw_icon(ui.painter(), egui::pos2(rect.left() + 19.0, rect.center().y), 14.0, icon_name, DARK_INK);
    ui.painter().galley(egui::pos2(rect.left() + 32.0, rect.center().y - galley.size().y / 2.0), galley, DARK_INK);
    r
}

/// The inspector's way back to the map's settings: a small rounded chip.
pub(super) fn back_chip(ui: &mut egui::Ui) -> egui::Response {
    let galley = ui.painter().layout_no_wrap("Map".to_string(), egui::FontId::proportional(12.5), INK);
    let (rect, r) = ui.allocate_exact_size(egui::vec2(galley.size().x + 30.0, 22.0), egui::Sense::click());
    ui.painter().rect_filled(rect, 11.0, if r.hovered() { TILE_HOVER } else { TILE });
    draw_icon(ui.painter(), egui::pos2(rect.left() + 12.0, rect.center().y), 10.0, "Back", INK);
    ui.painter().galley(egui::pos2(rect.left() + 21.0, rect.center().y - galley.size().y / 2.0), galley, INK);
    r.on_hover_text("Back to the map's settings (Esc)")
}

/// A greyed hint where a panel waits for something (a big icon over a few words).
pub(super) fn empty_hint(ui: &mut egui::Ui, icon_name: &str, text: &str) {
    egui::Frame::new().fill(CARD).corner_radius(8).inner_margin(12).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.vertical_centered(|ui| {
            icon(ui, icon_name, 34.0, DIM);
            ui.label(egui::RichText::new(text).color(DIM));
        });
    });
    ui.add_space(6.0);
}

/// A tool of the tool strip: its icon over a short caption.
pub(super) fn tool_button(ui: &mut egui::Ui, name: &str, caption: &str, hint: &str, on: bool) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(egui::vec2(60.0, 50.0), egui::Sense::click());
    let enabled = ui.is_enabled();
    let fill = if on { ORANGE } else if r.hovered() && enabled { TILE_HOVER } else { egui::Color32::TRANSPARENT };
    ui.painter().rect_filled(rect.shrink(2.0), 8.0, fill);
    let ink = if !enabled { egui::Color32::from_gray(100) } else if on { DARK_INK } else { INK };
    draw_icon(ui.painter(), rect.center() - egui::vec2(0.0, 7.0), 22.0, name, ink);
    ui.painter().text(rect.center_bottom() - egui::vec2(0.0, 8.0), egui::Align2::CENTER_CENTER, caption, egui::FontId::proportional(11.0), ink);
    r.on_hover_text(hint)
}

/// A square choice tile: an icon (in `accent`, else the text colour) over a caption; outlined orange when `on`.
pub(super) fn icon_tile(ui: &mut egui::Ui, name: &str, caption: &str, hint: &str, on: bool, accent: Option<egui::Color32>, size: f32) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    let fill = if on { TILE_ON } else if r.hovered() { TILE_HOVER } else { TILE };
    ui.painter().rect_filled(rect, 8.0, fill);
    if on { ui.painter().rect_stroke(rect, 8.0, egui::Stroke::new(2.0, ORANGE), egui::StrokeKind::Inside); }
    let ink = accent.unwrap_or(INK);
    let has_caption = !caption.is_empty();
    let icon_at = if has_caption { rect.center() - egui::vec2(0.0, size * 0.12) } else { rect.center() };
    draw_icon(ui.painter(), icon_at, size * if has_caption { 0.42 } else { 0.55 }, name, ink);
    if has_caption {
        ui.painter().text(rect.center_bottom() - egui::vec2(0.0, size * 0.15), egui::Align2::CENTER_CENTER, caption, egui::FontId::proportional((size * 0.19).clamp(10.0, 12.5)), if on { ORANGE } else { INK });
    }
    if hint.is_empty() { r } else { r.on_hover_text(hint) }
}

/// A row of icon tiles that pick one of `items` (value, icon, caption, hint) into `value`: true when it changed.
pub(super) fn choice_tiles<T: PartialEq + Copy>(ui: &mut egui::Ui, value: &mut T, items: &[(T, &str, &str, &str)], accent: impl Fn(T) -> Option<egui::Color32>, size: f32) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
        for &(v, icon_name, caption, hint) in items {
            if icon_tile(ui, icon_name, caption, hint, *value == v, accent(v), size).clicked() && *value != v {
                *value = v;
                changed = true;
            }
        }
    });
    changed
}

/// A card: a rounded box with a header (icon, title, an arrow) that folds it; its state is kept per `id`.
pub(super) fn card<R>(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, icon_name: &str, title: &str, default_open: bool, add: impl FnOnce(&mut egui::Ui) -> R) -> Option<R> {
    let id = egui::Id::new(("rac_maker_card", id));
    let mut open = ui.data_mut(|d| *d.get_persisted_mut_or(id, default_open));
    let out = egui::Frame::new().fill(CARD).corner_radius(8).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let (rect, r) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.0), egui::Sense::click());
        if r.hovered() { ui.painter().rect_filled(rect.expand(2.0), 5.0, NAVY2); }
        if !icon_name.is_empty() { draw_icon(ui.painter(), egui::pos2(rect.left() + 11.0, rect.center().y), 17.0, icon_name, ORANGE); }
        ui.painter().text(egui::pos2(rect.left() + if icon_name.is_empty() { 2.0 } else { 26.0 }, rect.center().y), egui::Align2::LEFT_CENTER, title, egui::FontId::proportional(15.0), INK);
        let a = egui::pos2(rect.right() - 9.0, rect.center().y);
        let pts = if open { vec![a + egui::vec2(-4.0, -2.0), a + egui::vec2(0.0, 2.5), a + egui::vec2(4.0, -2.0)] } else { vec![a + egui::vec2(-2.0, -4.0), a + egui::vec2(2.5, 0.0), a + egui::vec2(-2.0, 4.0)] };
        ui.painter().add(egui::Shape::line(pts, egui::Stroke::new(1.6, DIM)));
        if r.clicked() {
            open = !open;
            ui.data_mut(|d| d.insert_persisted(id, open));
        }
        open.then(|| { ui.add_space(4.0); add(ui) })
    }).inner;
    ui.add_space(6.0);
    out
}

/// A key cap ("WASD", "F5", "Esc") for the status bar's hints.
pub(super) fn key_cap(ui: &mut egui::Ui, key: &str) {
    let galley = ui.painter().layout_no_wrap(key.to_string(), egui::FontId::monospace(11.0), INK);
    let (rect, _) = ui.allocate_exact_size(galley.size() + egui::vec2(10.0, 4.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 4.0, TILE);
    ui.painter().rect_stroke(rect, 4.0, egui::Stroke::new(1.0, LINE), egui::StrokeKind::Inside);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, INK);
}

/// A mouse with its left (0), right (1) or middle (2: the wheel) button lit, for the status bar's hints.
pub(super) fn mouse_cap(ui: &mut egui::Ui, button: u8) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 18.0), egui::Sense::hover());
    let body = rect.shrink2(egui::vec2(1.0, 0.5));
    let painter = ui.painter();
    painter.rect_stroke(body, 6.0, egui::Stroke::new(1.2, INK), egui::StrokeKind::Inside);
    let mid = body.center().x;
    let split = body.top() + body.height() * 0.42;
    match button {
        0 => { painter.rect_filled(egui::Rect::from_min_max(body.min + egui::vec2(1.5, 1.5), egui::pos2(mid, split)), egui::CornerRadius { nw: 5, ..Default::default() }, ORANGE); }
        1 => { painter.rect_filled(egui::Rect::from_min_max(egui::pos2(mid, body.top() + 1.5), egui::pos2(body.right() - 1.5, split)), egui::CornerRadius { ne: 5, ..Default::default() }, ORANGE); }
        _ => { painter.rect_filled(egui::Rect::from_center_size(egui::pos2(mid, body.top() + 5.0), egui::vec2(2.5, 5.0)), 1.0, ORANGE); }
    }
    painter.line_segment([egui::pos2(body.left(), split), egui::pos2(body.right(), split)], egui::Stroke::new(1.0, INK));
    if button != 2 { painter.line_segment([egui::pos2(mid, body.top()), egui::pos2(mid, split)], egui::Stroke::new(1.0, INK)); }
}

/// A section's small heading inside a card or panel.
pub(super) fn label_small(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).small().color(DIM));
}
