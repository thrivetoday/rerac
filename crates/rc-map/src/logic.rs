//! A map's gameplay logic (docs/plan/rac_maker.md, Phase 4): boxes in the world ([`Zone`]) that do something when
//! Ratchet is in them (a checkpoint, the goal, a death zone, a fixed camera) or that rules watch, and when/then
//! rules ([`Rule`]) between zones and named objects (crate::Object::name): when Ratchet enters a zone, when every
//! enemy in a zone is beaten, when an object is destroyed, then remove or show objects, show a message, set a
//! checkpoint, reach the goal. The engine runs them (crate `rc-engine` `map_logic`).

/// A box in the world: `centre`, half its size along its own x, y, z (`half`), turned by `yaw` about z.
#[derive(Clone, Debug, PartialEq)]
pub struct Zone {
    /// How rules and the editor name it (unique on a map).
    pub name: String,
    pub kind: ZoneKind,
    pub centre: [f32; 3],
    pub half: [f32; 3],
    pub yaw: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ZoneKind {
    /// Does nothing by itself: rules watch it.
    Trigger,
    /// Ratchet entering it makes it where he comes back after a death.
    Checkpoint,
    /// Ratchet entering it finishes the map.
    Goal,
    /// Ratchet entering it loses a life.
    Death,
    /// While Ratchet is in it the camera stands at `eye` and looks at him.
    Camera { eye: [f32; 3] },
}

impl ZoneKind {
    pub const NAMES: [&'static str; 5] = ["trigger", "checkpoint", "goal", "death", "camera"];
    pub fn name(&self) -> &'static str {
        match self {
            ZoneKind::Trigger => "trigger",
            ZoneKind::Checkpoint => "checkpoint",
            ZoneKind::Goal => "goal",
            ZoneKind::Death => "death",
            ZoneKind::Camera { .. } => "camera",
        }
    }
}

impl Zone {
    /// `p` lies in it.
    pub fn contains(&self, p: [f32; 3]) -> bool {
        let (s, c) = self.yaw.sin_cos();
        let d = [p[0] - self.centre[0], p[1] - self.centre[1], p[2] - self.centre[2]];
        // Into the box's own axes (turned back by yaw).
        let (x, y) = (d[0] * c + d[1] * s, -d[0] * s + d[1] * c);
        x.abs() <= self.half[0] && y.abs() <= self.half[1] && d[2].abs() <= self.half[2]
    }

    /// Its eight corners (bottom four, then top four), for drawing.
    pub fn corners(&self) -> [[f32; 3]; 8] {
        let (s, c) = self.yaw.sin_cos();
        std::array::from_fn(|k| {
            let (sx, sy, sz) = (if k & 1 == 0 { -1.0 } else { 1.0 }, if k & 2 == 0 { -1.0 } else { 1.0 }, if k < 4 { -1.0 } else { 1.0 });
            let (x, y) = (sx * self.half[0], sy * self.half[1]);
            [self.centre[0] + x * c - y * s, self.centre[1] + x * s + y * c, self.centre[2] + sz * self.half[2]]
        })
    }

    pub fn check(&self) -> Result<(), String> {
        if self.name.trim().is_empty() { return Err("a zone needs a name".into()); }
        if !self.half.iter().all(|&h| h.is_finite() && (0.1..=200.0).contains(&h)) { return Err(format!("zone {}: size must be 0.2..400", self.name)); }
        if !self.centre.iter().all(|v| v.is_finite() && (0.0..crate::WORLD_SIZE).contains(v)) { return Err(format!("zone {}: outside the world", self.name)); }
        Ok(())
    }
}

/// When a rule fires.
#[derive(Clone, Debug, PartialEq)]
pub enum When {
    /// Ratchet enters the zone named.
    Enters(String),
    /// Every enemy that started in the zone named is beaten (gone).
    Cleared(String),
    /// The object named is destroyed (a crate broken, an enemy beaten).
    Destroyed(String),
}

/// What a rule does.
#[derive(Clone, Debug, PartialEq)]
pub enum Then {
    /// The object named goes (a gate opens, a wall falls).
    Remove(String),
    /// The object named (hidden until now) appears.
    Show(String),
    /// A message on screen.
    Message(String),
    /// The zone named becomes the checkpoint.
    Checkpoint(String),
    /// The map is finished.
    Goal,
}

/// When `when`, then each of `then` (once).
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub when: When,
    pub then: Vec<Then>,
    /// The camera shows it: it turns to the first object the rule removes or shows, that happens, and the view cuts
    /// back to Ratchet (`"cutscene": true`).
    pub cutscene: bool,
}

impl Rule {
    /// The names it mentions that `zones` / `objects` lack.
    pub fn missing<'a>(&'a self, zones: &[Zone], objects: &[crate::Object]) -> Vec<&'a str> {
        let zone = |n: &str| zones.iter().any(|z| z.name == n);
        let object = |n: &str| objects.iter().any(|o| o.name.as_deref() == Some(n));
        let mut out = Vec::new();
        match &self.when {
            When::Enters(z) | When::Cleared(z) => { if !zone(z) { out.push(z.as_str()); } }
            When::Destroyed(o) => { if !object(o) { out.push(o.as_str()); } }
        }
        for t in &self.then {
            match t {
                Then::Remove(o) | Then::Show(o) => { if !object(o) { out.push(o.as_str()); } }
                Then::Checkpoint(z) => { if !zone(z) { out.push(z.as_str()); } }
                Then::Message(_) | Then::Goal => {}
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turned_box_holds_what_is_in_it() {
        let z = Zone { name: "a".into(), kind: ZoneKind::Goal, centre: [100.0, 100.0, 50.0], half: [4.0, 1.0, 2.0], yaw: std::f32::consts::FRAC_PI_2 };
        // Turned a quarter: its long side runs along y.
        assert!(z.contains([100.5, 103.5, 51.0]));
        assert!(!z.contains([103.5, 100.0, 50.0]));
        assert!(!z.contains([100.0, 100.0, 52.5]));
        let c = z.corners();
        assert!(c.iter().all(|p| (p[0] - 100.0).abs() <= 1.0 + 1e-4 && (p[1] - 100.0).abs() <= 4.0 + 1e-4));
    }
}
