//! Bounded pointer history. Preserve corners and pressure peaks under load.
use glam::Vec2;
use std::collections::VecDeque;

/// Avoid paying for a full high-poly dab for subpixel device jitter.
pub fn dab_due(previous: Vec2, next: Vec2, old_pressure: f32, pressure: f32, spacing: f32) -> bool {
    previous.distance_squared(next) >= spacing.max(1.0).powi(2)
        || (old_pressure - pressure).abs() >= 0.1
}

#[derive(Default)]
pub struct Samples(VecDeque<(Vec2, f32)>);

impl Samples {
    pub fn push(&mut self, position: Vec2, pressure: f32) {
        if self
            .0
            .back()
            .is_some_and(|&(p, force)| p == position && force == pressure)
        {
            return;
        }
        self.0.push_back((position, pressure));
        // Keep latency/memory bounded, preferentially removing samples that
        // lie on a straight line with linearly interpolated pressure.
        if self.0.len() > 16 {
            let remove = (1..self.0.len() - 1)
                .min_by(|&a, &b| self.error(a).total_cmp(&self.error(b)))
                .unwrap();
            self.0.remove(remove);
        }
    }

    fn error(&self, index: usize) -> f32 {
        let (a, pa) = self.0[index - 1];
        let (b, pb) = self.0[index];
        let (c, pc) = self.0[index + 1];
        let ac = c - a;
        let t = ((b - a).dot(ac) / ac.length_squared().max(1e-6)).clamp(0.0, 1.0);
        b.distance_squared(a + ac * t) + ((pb - (pa + (pc - pa) * t)) * 64.0).powi(2)
    }

    pub fn pop(&mut self) -> Option<(Vec2, f32)> {
        self.0.pop_front()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn event_rate_does_not_multiply_a_straight_strokes_dabs() {
        let count = |events: usize| {
            let mut previous = Vec2::ZERO;
            let mut dabs = 0;
            for i in 1..=events {
                let point = Vec2::new(i as f32 * 100.0 / events as f32, 0.0);
                if dab_due(previous, point, 0.5, 0.5, 10.0) {
                    dabs += 1;
                    previous = point;
                }
            }
            dabs
        };
        assert_eq!(count(100), 10);
        assert_eq!(count(1000), 10);
        assert!(dab_due(Vec2::ZERO, Vec2::ZERO, 0.2, 0.7, 10.0));
    }
    #[test]
    fn overload_retains_endpoints_corner_and_pressure_peak() {
        let mut samples = Samples::default();
        for i in 0..100 {
            let position = if i < 50 {
                Vec2::new(i as f32, 0.0)
            } else {
                Vec2::new(49.0, (i - 49) as f32)
            };
            samples.push(position, if i == 25 { 1.0 } else { 0.2 });
        }
        assert_eq!(samples.0.len(), 16);
        assert_eq!(samples.0.front().unwrap().0, Vec2::ZERO);
        assert_eq!(samples.0.back().unwrap().0, Vec2::new(49.0, 50.0));
        assert!(samples.0.iter().any(|&(p, _)| p == Vec2::new(49.0, 0.0)));
        assert!(samples.0.iter().any(|&(_, pressure)| pressure == 1.0));
    }
}
