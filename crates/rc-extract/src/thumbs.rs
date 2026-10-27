//! Asset thumbnails for the RAC Maker catalog: a small CPU rasteriser over a class's class-space triangles (the same
//! vertex and texture decode the glTF exports use), so thumbnails need no GPU, no window and come out the same on every
//! machine. A three-quarter view from the front right and above, orthographic, fitted to the model; textured (nearest,
//! wrapped) with a soft light from the viewer; the background transparent. Rendered at twice the size and averaged
//! down (2×2) for smooth edges.
//!
//! Mobys are drawn in their bind pose (high LOD), ties at LOD 0, shrubs as their mesh.

use rc_formats::moby::LevelMobyClass;
use rc_formats::shrub::{self, LevelShrubClass};
use rc_formats::texture::{Texture, TextureTable};
use rc_formats::tie::{self, LevelTieClass};

/// One triangle: class-space positions (game axes, z up), texture coordinates, and its texture (None: untextured).
#[derive(Clone, Copy, Debug)]
pub struct Tri {
    pub p: [[f32; 3]; 3],
    pub uv: [[f32; 2]; 3],
    pub tex: Option<(TextureTable, usize)>,
}

/// A moby class's high-LOD triangles in its bind pose (metal and texture −1 parts untextured); the low LOD for a class
/// whose high LOD is empty.
pub fn moby_tris(m: &LevelMobyClass) -> Vec<Tri> {
    let c = &m.class;
    let high = c.high_lod.iter().any(|sm| !sm.triangles.is_empty());
    let lists: Vec<&rc_formats::moby::MobySubmesh> = if high { c.high_lod.iter().chain(&c.metal).collect() } else { c.low_lod.iter().collect() };
    let mut out = Vec::new();
    for sm in lists {
        for t in &sm.triangles {
            let v = [t.a, t.b, t.c].map(|i| &sm.vertices[i as usize]);
            let tex = if sm.is_metal { None } else { m.texture_table_index(t.texture).map(|i| (TextureTable::Moby, i)) };
            out.push(Tri { p: v.map(|x| c.position(x)), uv: v.map(|x| x.st.map(|s| s as f32 / 4096.0)), tex });
        }
    }
    out
}

/// A tie class's LOD-0 triangles.
pub fn tie_tris(t: &LevelTieClass) -> Vec<Tri> {
    let scale = t.class.header.scale;
    let mut out = Vec::new();
    for p in &t.class.lods[0] {
        for tri in tie::tie_triangles(p) {
            let v = [tri.a, tri.b, tri.c].map(|i| &p.vertices[i as usize]);
            out.push(Tri { p: v.map(|x| x.class_position(scale)), uv: v.map(|x| x.uv()), tex: t.texture_table_index(tri.ad_gif).map(|i| (TextureTable::Tie, i)) });
        }
    }
    out
}

/// A shrub class's triangles.
pub fn shrub_tris(s: &LevelShrubClass) -> Vec<Tri> {
    let scale = s.class.header.scale;
    let mut out = Vec::new();
    for p in &s.class.packets {
        for tri in shrub::shrub_triangles(p) {
            let v = [tri.a, tri.b, tri.c].map(|i| &p.vertices[i as usize]);
            out.push(Tri { p: v.map(|x| x.class_position(scale)), uv: v.map(|x| x.uv()), tex: s.texture_table_index(tri.texture).map(|i| (TextureTable::Shrub, i)) });
        }
    }
    out
}

/// The texture's alpha scale: PS2 textures store 0..0x80 (0x80 opaque); a texture with alpha above that is 0..0xff.
fn alpha_scale(t: &Texture) -> u32 { if t.rgba.chunks(4).all(|p| p[3] <= 0x80) { 2 } else { 1 } }

/// Renders `tris` into a `size`×`size` RGBA image (None: nothing to draw). `tex` finds a triangle's texture.
pub fn render<'a>(tris: &[Tri], tex: &dyn Fn(TextureTable, usize) -> Option<&'a Texture>, size: u32) -> Option<Vec<u8>> {
    if tris.is_empty() { return None; }
    // View: yaw −35° about z, then looking 25° down (camera right / up / forward in game space).
    let (yaw, pitch) = (-35f32.to_radians(), 25f32.to_radians());
    let (cy, sy, cp, sp) = (yaw.cos(), yaw.sin(), pitch.cos(), pitch.sin());
    let view = |p: [f32; 3]| -> [f32; 3] {
        let (x, y) = (p[0] * cy - p[1] * sy, p[0] * sy + p[1] * cy);
        // Screen x, screen y (up), depth (far = larger).
        [x, y * sp + p[2] * cp, y * cp - p[2] * sp]
    };
    let v: Vec<[[f32; 3]; 3]> = tris.iter().map(|t| t.p.map(view)).collect();
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for p in v.iter().flatten() {
        if !p.iter().all(|c| c.is_finite()) { continue; }
        for k in 0..2 { lo[k] = lo[k].min(p[k]); hi[k] = hi[k].max(p[k]); }
    }
    let extent = (hi[0] - lo[0]).max(hi[1] - lo[1]);
    if !(extent > 1e-6) { return None; }
    let n = (size * 2) as usize;
    let k = n as f32 * 0.9 / extent;
    let (ox, oy) = (n as f32 / 2.0 - (lo[0] + hi[0]) / 2.0 * k, n as f32 / 2.0 + (lo[1] + hi[1]) / 2.0 * k);
    let to_px = |p: [f32; 3]| [ox + p[0] * k, oy - p[1] * k, p[2]];
    let mut color = vec![[0f32; 4]; n * n];
    let mut depth = vec![f32::MAX; n * n];
    let mut scales: std::collections::HashMap<(TextureTable, usize), u32> = std::collections::HashMap::new();
    for (t, vv) in tris.iter().zip(&v) {
        let [a, b, c] = vv.map(to_px);
        let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        if area.abs() < 1e-6 || !area.is_finite() { continue; }
        // Two-sided light from the viewer and above: the face normal in view space.
        let (e1, e2) = ([vv[1][0] - vv[0][0], vv[1][1] - vv[0][1], vv[1][2] - vv[0][2]], [vv[2][0] - vv[0][0], vv[2][1] - vv[0][1], vv[2][2] - vv[0][2]]);
        let nrm = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
        let len = (nrm[0] * nrm[0] + nrm[1] * nrm[1] + nrm[2] * nrm[2]).sqrt().max(1e-12);
        let light = [0.3f32, 0.5, -0.81];
        let shade = 0.55 + 0.45 * ((nrm[0] * light[0] + nrm[1] * light[1] + nrm[2] * light[2]) / len).abs();
        let texture = t.tex.and_then(|(tb, i)| tex(tb, i).map(|x| (x, *scales.entry((tb, i)).or_insert_with(|| alpha_scale(x)))));
        let x0 = a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize;
        let x1 = (a[0].max(b[0]).max(c[0]).ceil() as usize).min(n - 1);
        let y0 = a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize;
        let y1 = (a[1].max(b[1]).max(c[1]).ceil() as usize).min(n - 1);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let w0 = ((b[0] - px) * (c[1] - py) - (b[1] - py) * (c[0] - px)) / area;
                let w1 = ((c[0] - px) * (a[1] - py) - (c[1] - py) * (a[0] - px)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 { continue; }
                let z = w0 * a[2] + w1 * b[2] + w2 * c[2];
                let i = y * n + x;
                if z >= depth[i] { continue; }
                let mut rgba = [0.5f32, 0.5, 0.5, 1.0];
                if let Some((tx, s)) = texture {
                    let u = w0 * t.uv[0][0] + w1 * t.uv[1][0] + w2 * t.uv[2][0];
                    let vv = w0 * t.uv[0][1] + w1 * t.uv[1][1] + w2 * t.uv[2][1];
                    let (tw, th) = (tx.width.max(1) as f32, tx.height.max(1) as f32);
                    let sx = ((u.rem_euclid(1.0) * tw) as usize).min(tx.width as usize - 1);
                    let sy = ((vv.rem_euclid(1.0) * th) as usize).min(tx.height as usize - 1);
                    let o = (sy * tx.width as usize + sx) * 4;
                    let Some(p) = tx.rgba.get(o..o + 4) else { continue };
                    let alpha = (p[3] as u32 * s).min(255);
                    // Alpha-tested cut-outs (leaves, fences).
                    if alpha < 64 { continue; }
                    rgba = [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0, 1.0];
                }
                depth[i] = z;
                color[i] = [rgba[0] * shade, rgba[1] * shade, rgba[2] * shade, 1.0];
            }
        }
    }
    Some(downsample(&color, n))
}

/// 2×2 box filter of an `n`×`n` linear image into RGBA8 (alpha-weighted).
fn downsample(color: &[[f32; 4]], n: usize) -> Vec<u8> {
    let m = n / 2;
    let mut out = vec![0u8; m * m * 4];
    for y in 0..m {
        for x in 0..m {
            let mut acc = [0f32; 4];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let c = color[(y * 2 + dy) * n + x * 2 + dx];
                for k in 0..3 { acc[k] += c[k] * c[3]; }
                acc[3] += c[3];
            }
            let o = (y * m + x) * 4;
            if acc[3] > 0.0 {
                for k in 0..3 { out[o + k] = (acc[k] / acc[3] * 255.0).round().clamp(0.0, 255.0) as u8; }
                out[o + 3] = (acc[3] / 4.0 * 255.0).round() as u8;
            }
        }
    }
    out
}

/// A texture fitted into a `size`×`size` square (nearest; transparent margins keep its aspect), opaque.
pub fn texture_thumb(t: &Texture, size: u32) -> Option<Vec<u8>> {
    if t.width == 0 || t.height == 0 { return None; }
    let s = size as usize;
    let k = (size as f32 / t.width.max(t.height) as f32).min(f32::MAX);
    let (w, h) = (((t.width as f32 * k) as usize).clamp(1, s), ((t.height as f32 * k) as usize).clamp(1, s));
    let (x0, y0) = ((s - w) / 2, (s - h) / 2);
    let mut out = vec![0u8; s * s * 4];
    for y in 0..h {
        for x in 0..w {
            let sx = (x * t.width as usize / w).min(t.width as usize - 1);
            let sy = (y * t.height as usize / h).min(t.height as usize - 1);
            let i = (sy * t.width as usize + sx) * 4;
            let o = ((y0 + y) * s + x0 + x) * 4;
            out[o..o + 3].copy_from_slice(&t.rgba[i..i + 3]);
            out[o + 3] = 255;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_textured_quad_and_skips_nothing_to_draw() {
        let tex = Texture { width: 2, height: 2, rgba: vec![255, 0, 0, 0x80, 0, 255, 0, 0x80, 0, 0, 255, 0x80, 255, 255, 255, 0x80] };
        let quad = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
        let uv = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let t = |a: usize, b: usize, c: usize| Tri { p: [quad[a], quad[b], quad[c]], uv: [uv[a], uv[b], uv[c]], tex: Some((TextureTable::Tie, 0)) };
        let img = render(&[t(0, 1, 2), t(0, 2, 3)], &|_, _| Some(&tex), 32).unwrap();
        assert_eq!(img.len(), 32 * 32 * 4);
        // Opaque in the middle, transparent in a corner.
        let px = |x: usize, y: usize| &img[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4];
        assert_eq!(px(16, 16)[3], 255);
        assert_eq!(px(0, 0)[3], 0);
        assert!(render(&[], &|_, _| None, 32).is_none());
        assert_eq!(texture_thumb(&tex, 8).unwrap().len(), 8 * 8 * 4);
    }
}
