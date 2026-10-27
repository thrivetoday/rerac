//! A map's planet, as the flight to it shows it (`"world": { "planet": … }`): a picture drawn from a few settings
//! (its pattern, three colours, the air's glow, clouds, a ring), the same in the editor's preview and in the flight
//! (crate rc-engine `flight_render`, where the game shows the destination planet's picture).

/// How its surface is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pattern {
    /// Seas, land and high ground, ice at the poles.
    Continents,
    /// Stripes round it (a gas giant).
    Bands,
    /// A dry ground full of craters.
    Craters,
    /// Colours stirred together (a toxic or molten world).
    Swirls,
}

/// The patterns and their names in the file.
pub const PATTERNS: [(Pattern, &str); 4] = [(Pattern::Continents, "continents"), (Pattern::Bands, "bands"), (Pattern::Craters, "craters"), (Pattern::Swirls, "swirls")];

impl Pattern {
    pub fn name(self) -> &'static str { PATTERNS.iter().find(|p| p.0 == self).map_or("continents", |p| p.1) }
    pub fn from_name(s: &str) -> Option<Pattern> { PATTERNS.iter().find(|p| p.1 == s).map(|p| p.0) }
}

/// The planet's look.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Planet {
    pub pattern: Pattern,
    /// Its three colours: the low ground (seas, the darkest bands), the land, the high ground (peaks, ice).
    pub colours: [[u8; 3]; 3],
    /// The air's glow round its edge.
    pub glow: [u8; 3],
    /// Clouds: 0 none .. 1 nearly covered.
    pub clouds: f32,
    /// Its ring: 0 none .. 1 wide, and its colour.
    pub ring: f32,
    pub ring_colour: [u8; 3],
    /// Where its land and seas fall: every number another world.
    pub seed: u32,
}

impl Default for Planet {
    fn default() -> Planet { PRESETS[0].1 }
}

/// Starting points for the editor (name, look).
pub const PRESETS: [(&str, Planet); 6] = [
    ("Lush", Planet { pattern: Pattern::Continents, colours: [[30, 70, 150], [70, 140, 60], [235, 240, 245]], glow: [120, 180, 255], clouds: 0.45, ring: 0.0, ring_colour: [200, 190, 170], seed: 1 }),
    ("Desert", Planet { pattern: Pattern::Craters, colours: [[150, 90, 50], [215, 165, 95], [245, 220, 170]], glow: [255, 200, 140], clouds: 0.1, ring: 0.0, ring_colour: [200, 180, 150], seed: 2 }),
    ("Ice", Planet { pattern: Pattern::Continents, colours: [[60, 110, 160], [190, 215, 235], [250, 252, 255]], glow: [190, 230, 255], clouds: 0.3, ring: 0.0, ring_colour: [220, 230, 240], seed: 3 }),
    ("Lava", Planet { pattern: Pattern::Swirls, colours: [[40, 20, 20], [120, 40, 20], [255, 150, 40]], glow: [255, 110, 50], clouds: 0.0, ring: 0.0, ring_colour: [160, 100, 80], seed: 4 }),
    ("Gas giant", Planet { pattern: Pattern::Bands, colours: [[150, 100, 70], [220, 180, 130], [245, 235, 210]], glow: [255, 220, 170], clouds: 0.0, ring: 0.7, ring_colour: [210, 195, 165], seed: 5 }),
    ("Toxic", Planet { pattern: Pattern::Swirls, colours: [[40, 60, 30], [110, 170, 40], [210, 240, 90]], glow: [170, 255, 110], clouds: 0.35, ring: 0.3, ring_colour: [150, 200, 120], seed: 6 }),
];

impl Planet {
    /// The picture, `size` × `size` RGBA (alpha 0..255; clear round the planet): lit from the upper left, its glow
    /// round the edge, the ring's far half behind it and its near half in front.
    pub fn render(&self, size: usize) -> Vec<u8> {
        let s = size as f32;
        let c = (s - 1.0) * 0.5;
        // Room for the ring (it reaches 1.25 + 0.9·ring radii) or the glow.
        let reach = if self.ring > 0.0 { 1.3 + 0.9 * self.ring.clamp(0.0, 1.0) } else { 1.12 };
        let r = c / reach;
        let sun = norm([-0.55, -0.5, 0.67]);
        let seed = self.seed as f32 * 7.31;
        let col = |k: usize| self.colours[k].map(|x| x as f32 / 255.0);
        let glow = self.glow.map(|x| x as f32 / 255.0);
        let ring_c = self.ring_colour.map(|x| x as f32 / 255.0);
        let mut out = vec![0u8; size * size * 4];
        for y in 0..size {
            for x in 0..size {
                let dx = (x as f32 - c) / r;
                let dy = (y as f32 - c) / r;
                let d2 = dx * dx + dy * dy;
                let mut px = [0.0f32; 4];
                // The ring: a flat band seen from a little above (its far half has dy < 0, behind the planet).
                let ring = (self.ring > 0.0).then(|| {
                    let (u, v) = (dx, dy / 0.28);
                    let rr = (u * u + v * v).sqrt();
                    let (inner, outer) = (1.25, 1.25 + 0.9 * self.ring.clamp(0.0, 1.0));
                    if rr < inner || rr > outer { return None; }
                    let t = (rr - inner) / (outer - inner);
                    let stripes = 0.65 + 0.35 * (t * 23.0 + seed).sin() * (t * 7.0 + seed * 0.3).cos();
                    let edge = (t * 8.0).min((1.0 - t) * 8.0).min(1.0);
                    let lit = if v < 0.0 { 0.75 } else { 1.0 };
                    Some(([ring_c[0] * stripes * lit, ring_c[1] * stripes * lit, ring_c[2] * stripes * lit], 0.85 * edge, v < 0.0))
                }).flatten();
                if let Some((rc, a, true)) = ring { over(&mut px, rc, a); }
                if d2 <= 1.0 {
                    let z = (1.0 - d2).sqrt();
                    let n = [dx, dy, z];
                    // The surface turned a little so the poles are not at the picture's edge.
                    let p = [dx * 0.95 + z * 0.3, dy, z * 0.95 - dx * 0.3];
                    let surf = self.surface(p, seed, &col);
                    let light = (dot(n, sun).max(0.0) * 0.9 + 0.12).min(1.0);
                    let mut rgb = surf.map(|v| v * light);
                    if self.clouds > 0.0 {
                        let cl = fbm([p[0] * 3.0 + seed + 17.0, p[1] * 5.0, p[2] * 3.0], 5);
                        let k = self.clouds.clamp(0.0, 1.0);
                        let a = smooth(0.62 - k * 0.3, 0.78 - k * 0.3, cl) * (0.5 + 0.5 * k);
                        let w = 0.95 * light + 0.05;
                        rgb = mix3(rgb, [w, w, w], a);
                    }
                    // The air: the glow brightest at the rim.
                    let rim = (1.0 - z).powi(3);
                    rgb = mix3(rgb, glow, (rim * 0.85).min(1.0));
                    // A soft edge over the glow's start (no dark seam between them).
                    let a = ((1.0 - d2.sqrt()) * r).clamp(0.0, 1.0);
                    if a < 1.0 { over(&mut px, glow, 0.7); }
                    over(&mut px, rgb, a);
                } else {
                    // The glow outside the edge, fading over a tenth of the radius.
                    let d = d2.sqrt();
                    let a = (1.0 - (d - 1.0) / 0.1).clamp(0.0, 1.0).powi(2) * 0.7;
                    if a > 0.0 { over(&mut px, glow, a); }
                }
                if let Some((rc, a, false)) = ring { over(&mut px, rc, a); }
                let o = (y * size + x) * 4;
                for k in 0..4 { out[o + k] = (px[k].clamp(0.0, 1.0) * 255.0).round() as u8; }
            }
        }
        out
    }

    /// The surface colour at the point `p` of the unit sphere.
    fn surface(&self, p: [f32; 3], seed: f32, col: &dyn Fn(usize) -> [f32; 3]) -> [f32; 3] {
        let at = |f: f32, o: f32| [p[0] * f + seed + o, p[1] * f + o * 0.5, p[2] * f - seed];
        match self.pattern {
            Pattern::Continents => {
                let h = fbm(at(2.2, 0.0), 5);
                let ice = smooth(0.78, 0.9, p[1].abs() + (h - 0.5) * 0.3);
                let ground = if h < 0.5 {
                    mix3(col(0).map(|v| v * 0.7), col(0), smooth(0.3, 0.5, h))
                } else {
                    mix3(col(1), col(2), smooth(0.66, 0.74, h))
                };
                mix3(ground, col(2), ice)
            }
            Pattern::Bands => {
                let w = fbm(at(1.6, 3.0), 4);
                let b = (p[1] * 9.0 + w * 4.0).sin() * 0.5 + 0.5;
                let b2 = (p[1] * 23.0 + w * 6.0).sin() * 0.5 + 0.5;
                let t = b * 0.75 + b2 * 0.25;
                if t < 0.5 { mix3(col(0), col(1), t * 2.0) } else { mix3(col(1), col(2), (t - 0.5) * 2.0) }
            }
            Pattern::Craters => {
                let h = fbm(at(3.0, 5.0), 5);
                let mut g = if h < 0.5 { mix3(col(0), col(1), smooth(0.3, 0.5, h)) } else { mix3(col(1), col(2), smooth(0.55, 0.75, h)) };
                // Craters: the nearest of a few cells, a dark floor and a bright rim.
                let (d, _) = cells(at(4.0, 11.0));
                let floor = 1.0 - smooth(0.18, 0.28, d);
                let rim = smooth(0.2, 0.27, d) * (1.0 - smooth(0.27, 0.34, d));
                g = mix3(g, col(0).map(|v| v * 0.8), floor * 0.6);
                mix3(g, col(2), rim * 0.5)
            }
            Pattern::Swirls => {
                let q = fbm(at(2.0, 1.0), 4);
                let t = fbm([p[0] * 2.5 + q * 2.5 + seed, p[1] * 2.5 + q * 1.5, p[2] * 2.5 - q * 2.0], 5);
                if t < 0.5 { mix3(col(0), col(1), smooth(0.3, 0.5, t)) } else { mix3(col(1), col(2), smooth(0.52, 0.7, t)) }
            }
        }
    }
}

fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = dot(v, v).sqrt();
    v.map(|x| x / l)
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] { [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t] }

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `rgb` with opacity `a` over the premultiplied-free pixel `px` (straight alpha).
fn over(px: &mut [f32; 4], rgb: [f32; 3], a: f32) {
    let out_a = a + px[3] * (1.0 - a);
    if out_a <= 0.0 { return; }
    for k in 0..3 { px[k] = (rgb[k] * a + px[k] * px[3] * (1.0 - a)) / out_a; }
    px[3] = out_a;
}

fn hash(x: i32, y: i32, z: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ (z as u32).wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 65535.0
}

/// Value noise in 0..1.
fn noise(p: [f32; 3]) -> f32 {
    let i = p.map(|v| v.floor());
    let f = [p[0] - i[0], p[1] - i[1], p[2] - i[2]].map(|t| t * t * (3.0 - 2.0 * t));
    let (x, y, z) = (i[0] as i32, i[1] as i32, i[2] as i32);
    let l = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let c = |dx: i32, dy: i32, dz: i32| hash(x + dx, y + dy, z + dz);
    l(
        l(l(c(0, 0, 0), c(1, 0, 0), f[0]), l(c(0, 1, 0), c(1, 1, 0), f[0]), f[1]),
        l(l(c(0, 0, 1), c(1, 0, 1), f[0]), l(c(0, 1, 1), c(1, 1, 1), f[0]), f[1]),
        f[2],
    )
}

/// Fractal noise in about 0..1.
fn fbm(p: [f32; 3], octaves: u32) -> f32 {
    let (mut sum, mut amp, mut norm_, mut q) = (0.0, 0.5, 0.0, p);
    for _ in 0..octaves {
        sum += noise(q) * amp;
        norm_ += amp;
        amp *= 0.5;
        q = q.map(|v| v * 2.03);
    }
    sum / norm_
}

/// The distance to the nearest cell point (one jittered point per unit cell) and that point's hash.
fn cells(p: [f32; 3]) -> (f32, f32) {
    let i = p.map(|v| v.floor() as i32);
    let mut best = (f32::MAX, 0.0);
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (x, y, z) = (i[0] + dx, i[1] + dy, i[2] + dz);
                let q = [x as f32 + hash(x, y, z), y as f32 + hash(y, z, x), z as f32 + hash(z, x, y)];
                let d = ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2) + (q[2] - p[2]).powi(2)).sqrt();
                if d < best.0 { best = (d, hash(x, z, y)); }
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_a_round_planet() {
        for (_, p) in PRESETS {
            let img = p.render(64);
            assert_eq!(img.len(), 64 * 64 * 4);
            // Opaque in the middle, clear in the corners.
            assert_eq!(img[(32 * 64 + 32) * 4 + 3], 255);
            assert_eq!(img[3], 0);
        }
    }

    /// `RC_PLANET_DUMP=<folder> cargo test -p rc-map dump_presets -- --ignored`: each preset as raw 256² RGBA.
    #[test]
    #[ignore]
    fn dump_presets() {
        let dir = std::env::var("RC_PLANET_DUMP").expect("RC_PLANET_DUMP");
        for (name, p) in PRESETS { std::fs::write(format!("{dir}/{name}.rgba"), p.render(256)).unwrap(); }
    }

    #[test]
    fn pattern_names_round_trip() {
        for (p, n) in PATTERNS { assert_eq!(Pattern::from_name(n), Some(p)); assert_eq!(p.name(), n); }
    }
}
