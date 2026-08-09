//! Pointer, pen and touch handling.
//!
//! Touch is not an afterthought here. One finger sculpts, two fingers navigate
//! (drag to orbit, spread to zoom, move together to pan), and a second finger
//! landing mid-stroke cancels the stroke rather than smearing the model while
//! the view swings around.

use glam::Vec2;
use winit::event::{Force, Touch, TouchPhase};

/// A camera move asked for by the pointer or by fingers.
#[derive(Clone, Copy, Debug)]
pub enum Gesture {
    /// Pixels of drag.
    Orbit(Vec2),
    /// Pixels of drag.
    Pan(Vec2),
    /// Multiplicative zoom; above 1 moves closer.
    Zoom(f32),
    /// Wheel notches.
    Wheel(f32),
}

/// What a touch event turned into.
pub enum TouchOutcome {
    StrokeStart { at: Vec2, pressure: f32 },
    StrokeMove { at: Vec2, pressure: f32 },
    StrokeEnd,
    Navigate(Vec<Gesture>),
}

#[derive(Clone, Copy)]
struct Finger {
    id: u64,
    pos: Vec2,
    start: Vec2,
    pressure: f32,
}

/// Below this much movement a two-finger drag counts as a pan rather than an
/// orbit, measured as how parallel the two fingers are moving.
const PAN_PARALLEL: f32 = 0.85;
/// Ignore pinch noise under this many pixels of change.
const PINCH_DEADZONE: f32 = 1.5;

#[derive(Default)]
pub struct Input {
    pub cursor: Vec2,
    pub lmb: bool,
    pub mmb: bool,
    pub rmb: bool,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    fingers: Vec<Finger>,
    /// True once a stroke was handed to the sculptor from a single finger.
    finger_stroke: bool,
    /// Set when a multi-finger gesture starts, so lifting back to one finger
    /// does not resume sculpting mid-air.
    gesture_lock: bool,
}

impl Input {
    /// Records the new cursor position and returns the movement since the last.
    pub fn move_cursor(&mut self, x: f32, y: f32) -> Vec2 {
        let next = Vec2::new(x, y);
        let delta = next - self.cursor;
        self.cursor = next;
        delta
    }

    /// Mouse navigation: middle or right drag orbits, shift makes it pan, and
    /// alt turns the left button into an orbit so a pen can navigate too.
    pub fn mouse_navigation(&self, delta: Vec2) -> Option<Gesture> {
        let navigating = self.mmb || self.rmb || (self.lmb && self.alt);
        if !navigating {
            return None;
        }
        if self.shift {
            Some(Gesture::Pan(delta))
        } else {
            Some(Gesture::Orbit(delta))
        }
    }

    pub fn touch_active(&self) -> bool {
        !self.fingers.is_empty()
    }

    /// Feeds one touch event through the gesture recogniser.
    pub fn on_touch(&mut self, touch: &Touch, over_ui: bool) -> Option<TouchOutcome> {
        let pos = Vec2::new(touch.location.x as f32, touch.location.y as f32);
        let pressure = touch
            .force
            .map(|f| match f {
                Force::Calibrated { force, max_possible_force, .. } => {
                    (force / max_possible_force.max(1e-6)) as f32
                }
                Force::Normalized(v) => v as f32,
            })
            .unwrap_or(1.0)
            .clamp(0.05, 1.0);

        match touch.phase {
            TouchPhase::Started => {
                self.fingers.push(Finger { id: touch.id, pos, start: pos, pressure });
                if self.fingers.len() == 1 {
                    if over_ui {
                        // Let egui own this one.
                        return None;
                    }
                    self.finger_stroke = true;
                    self.gesture_lock = false;
                    return Some(TouchOutcome::StrokeStart { at: pos, pressure });
                }
                // A second finger means the user wants to navigate.
                self.gesture_lock = true;
                if self.finger_stroke {
                    self.finger_stroke = false;
                    return Some(TouchOutcome::StrokeEnd);
                }
                None
            }
            TouchPhase::Moved => {
                let previous = self.update_finger(touch.id, pos, pressure)?;
                match self.fingers.len() {
                    1 if self.finger_stroke => {
                        Some(TouchOutcome::StrokeMove { at: pos, pressure })
                    }
                    2 => Some(TouchOutcome::Navigate(self.two_finger(touch.id, previous))),
                    n if n >= 3 => {
                        let delta = pos - previous;
                        // Three fingers pan, so a palm on the screen never
                        // spins the model.
                        Some(TouchOutcome::Navigate(vec![Gesture::Pan(delta / n as f32)]))
                    }
                    _ => None,
                }
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                self.fingers.retain(|f| f.id != touch.id);
                if self.fingers.is_empty() {
                    let was_stroking = self.finger_stroke;
                    self.finger_stroke = false;
                    self.gesture_lock = false;
                    return was_stroking.then_some(TouchOutcome::StrokeEnd);
                }
                None
            }
        }
    }

    /// Moves one finger and hands back where it was.
    fn update_finger(&mut self, id: u64, pos: Vec2, pressure: f32) -> Option<Vec2> {
        let f = self.fingers.iter_mut().find(|f| f.id == id)?;
        let previous = f.pos;
        f.pos = pos;
        f.pressure = pressure;
        Some(previous)
    }

    /// Turns the current two-finger configuration into orbit, pan and zoom.
    fn two_finger(&self, moved_id: u64, previous: Vec2) -> Vec<Gesture> {
        let mut out = Vec::new();
        let (a, b) = (self.fingers[0], self.fingers[1]);
        let other = if a.id == moved_id { b } else { a };
        let moved = if a.id == moved_id { a } else { b };

        let delta = moved.pos - previous;
        if delta.length_squared() < 1e-6 {
            return out;
        }

        // Pinch: how the distance between the fingers changed.
        let before = (previous - other.pos).length();
        let after = (moved.pos - other.pos).length();
        if (after - before).abs() > PINCH_DEADZONE && before > 1.0 {
            out.push(Gesture::Zoom(after / before.max(1e-3)));
        }

        // Both fingers travelling the same way reads as a pan; otherwise the
        // pair is twisting the model, which reads as an orbit.
        let travel_a = a.pos - a.start;
        let travel_b = b.pos - b.start;
        let parallel = if travel_a.length() > 4.0 && travel_b.length() > 4.0 {
            travel_a.normalize().dot(travel_b.normalize())
        } else {
            0.0
        };
        if parallel > PAN_PARALLEL {
            out.push(Gesture::Pan(delta * 0.5));
        } else {
            out.push(Gesture::Orbit(delta * 0.5));
        }
        out
    }
}
