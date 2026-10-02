//! Physically based spring animation.
//!
//! Springs are parameterised the same way SwiftUI does it: a `response`
//! (roughly the period of the oscillation, in seconds) and a damping ratio
//! (1.0 = critically damped, < 1.0 = bouncy).  Integration uses fixed
//! sub-steps so the motion is identical at 60, 120 or 240 Hz.

const SUBSTEP: f32 = 1.0 / 480.0;

#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub value: f32,
    pub velocity: f32,
    pub target: f32,
    stiffness: f32,
    damping: f32,
    eps: f32,
    /// A target that will be applied once `delay` has elapsed.
    pending: Option<(f32, f32)>,
}

impl Spring {
    pub fn new(value: f32, response: f32, damping_ratio: f32) -> Self {
        let mut s =
            Self { value, velocity: 0.0, target: value, stiffness: 0.0, damping: 0.0, eps: 0.01, pending: None };
        s.set_params(response, damping_ratio);
        s
    }

    /// Spring for 0..1 values (opacity, progress) — tighter settle threshold.
    pub fn unit(value: f32, response: f32, damping_ratio: f32) -> Self {
        let mut s = Self::new(value, response, damping_ratio);
        s.eps = 0.0015;
        s
    }

    pub fn set_params(&mut self, response: f32, damping_ratio: f32) {
        let response = response.max(0.02);
        let w = std::f32::consts::TAU / response;
        self.stiffness = w * w;
        self.damping = 2.0 * damping_ratio * w;
    }

    pub fn set_target(&mut self, t: f32) {
        self.pending = None;
        self.target = t;
    }

    /// Change the target after `delay` seconds (cancelled by `set_target`).
    pub fn set_target_delayed(&mut self, t: f32, delay: f32) {
        if delay <= 0.0 {
            self.set_target(t);
        } else if self.pending.map(|p| p.0) != Some(t) && self.target != t {
            self.pending = Some((t, delay));
        } else if self.target == t {
            self.pending = None;
        }
    }

    pub fn snap(&mut self, v: f32) {
        self.value = v;
        self.target = v;
        self.velocity = 0.0;
        self.pending = None;
    }

    /// Advance the simulation. Returns true while still moving.
    pub fn step(&mut self, dt: f32) -> bool {
        let mut dt = dt.clamp(0.0, 0.1);
        if let Some((t, d)) = self.pending {
            let d = d - dt;
            if d <= 0.0 {
                self.target = t;
                self.pending = None;
            } else {
                self.pending = Some((t, d));
            }
        }
        if self.velocity == 0.0 && self.value == self.target {
            return self.pending.is_some();
        }
        while dt > 0.0 {
            let h = dt.min(SUBSTEP);
            let force = -self.stiffness * (self.value - self.target) - self.damping * self.velocity;
            self.velocity += force * h;
            self.value += self.velocity * h;
            dt -= h;
        }
        if (self.value - self.target).abs() < self.eps && self.velocity.abs() < self.eps * 10.0 {
            self.value = self.target;
            self.velocity = 0.0;
        }
        true
    }
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Quint ease-out, close to CSS `cubic-bezier(0.23, 1, 0.32, 1)`.
pub fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_settles() {
        let mut s = Spring::new(0.0, 0.4, 0.8);
        s.set_target(100.0);
        let mut frames = 0;
        while s.step(1.0 / 60.0) {
            frames += 1;
            assert!(frames < 600, "spring never settled");
        }
        assert_eq!(s.value, 100.0);
    }

    #[test]
    fn delayed_target() {
        let mut s = Spring::unit(0.0, 0.2, 1.0);
        s.set_target_delayed(1.0, 0.1);
        s.step(0.05);
        assert_eq!(s.target, 0.0);
        s.step(0.06);
        assert_eq!(s.target, 1.0);
    }
}
