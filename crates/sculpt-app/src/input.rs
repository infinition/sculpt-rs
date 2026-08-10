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

/// What a device does when it is dragged.
///
/// Everyone holds a tablet differently and every application trains a different
/// reflex, so rather than pick one, each device gets a job you can change.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// Sculpt on the model, spin the view off it. Decided where the press
    /// landed, once, and held for the rest of that press.
    Auto,
    Sculpt,
    Orbit,
    Pan,
    Zoom,
    Nothing,
}

impl Role {
    pub const ALL: [Role; 6] = [
        Role::Auto,
        Role::Sculpt,
        Role::Orbit,
        Role::Pan,
        Role::Zoom,
        Role::Nothing,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Role::Auto => "Auto",
            Role::Sculpt => "Sculpt",
            Role::Orbit => "Orbit",
            Role::Pan => "Pan",
            Role::Zoom => "Zoom",
            Role::Nothing => "Nothing",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Role::Auto => "Press on the model to sculpt it, press off it to spin the view.",
            Role::Sculpt => "Always draws with the current tool.",
            Role::Orbit => "Always spins the view.",
            Role::Pan => "Always slides the view.",
            Role::Zoom => "Drag up and down to move closer and further.",
            Role::Nothing => "Ignored.",
        }
    }

    /// What this role means for a press that landed where it did.
    ///
    /// Only `Auto` cares where. It is the binding most people want and few
    /// would think to ask for: the model is the canvas and the space around it
    /// is the turntable, which is how you would handle the real thing.
    fn resolve(self, target: PressTarget) -> Role {
        match (self, target) {
            // A press the interface has taken belongs to the interface,
            // whatever the device is otherwise for. Without this, dragging a
            // slider would also spin the model behind it.
            (_, PressTarget::Ui) => Role::Nothing,
            (Role::Auto, PressTarget::Model) => Role::Sculpt,
            (Role::Auto, PressTarget::Space) => Role::Orbit,
            (other, _) => other,
        }
    }

    /// Turns a drag into a camera move, or `None` when this role sculpts.
    fn gesture(self, delta: Vec2) -> Option<Gesture> {
        match self {
            Role::Orbit => Some(Gesture::Orbit(delta)),
            Role::Pan => Some(Gesture::Pan(delta)),
            // Vertical travel reads as zoom, as it does everywhere else here.
            Role::Zoom => Some(Gesture::Wheel(-delta.y * 0.04)),
            // An unresolved `Auto` never reaches here: it is settled at the
            // moment of contact, when there is something to ask about.
            Role::Auto | Role::Sculpt | Role::Nothing => None,
        }
    }
}

/// Where a press landed, which is all the automatic binding needs to know.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PressTarget {
    /// On the model, or close enough to it for the brush to reach.
    Model,
    /// On empty space in the viewport.
    Space,
    /// On the interface, which has already claimed it.
    Ui,
}

/// What each device does. Pen and touch are separate because a stylus usually
/// wants to draw while a finger usually wants to move the model.
#[derive(Clone, Copy, Debug)]
pub struct Bindings {
    pub left: Role,
    pub middle: Role,
    pub right: Role,
    /// A single finger.
    pub touch: Role,
    /// A stylus. Reported by the same events as touch, told apart by the
    /// pressure the digitiser sends, which a finger does not have.
    pub pen: Role,
}

impl Default for Bindings {
    fn default() -> Self {
        // The three that touch the model decide on contact. A press that misses
        // the model was never going to leave a mark anyway, so spending it on
        // the view costs nothing and saves reaching for a modifier.
        Self {
            left: Role::Auto,
            middle: Role::Orbit,
            right: Role::Orbit,
            touch: Role::Auto,
            pen: Role::Auto,
        }
    }
}

/// What a touch event turned into.
pub enum TouchOutcome {
    StrokeStart { at: Vec2, pressure: f32 },
    StrokeMove { at: Vec2, pressure: f32 },
    StrokeEnd,
    /// A second finger arrived on the heels of the first: the stroke it began
    /// was not meant, and should be rolled back rather than merely stopped.
    CancelStroke,
    Navigate(Vec<Gesture>),
    Undo,
    Redo,
}

/// A tap is short, still, and ends with every finger off the glass.
const TAP_MAX_SECONDS: f64 = 0.3;
const TAP_MAX_TRAVEL: f32 = 26.0;
/// How long the second tap of a double tap may take to arrive.
const DOUBLE_TAP_SECONDS: f64 = 0.45;
/// A second finger landing this soon means the first one was never meant to
/// draw.
const STROKE_GRACE_SECONDS: f64 = 0.25;

/// Recognises taps across a whole touch sequence.
#[derive(Default)]
struct TapTracker {
    /// When the first finger of the current sequence landed.
    began: Option<std::time::Instant>,
    /// Most fingers down at once during it.
    fingers: usize,
    /// Furthest any finger strayed from where it landed.
    travel: f32,
    /// Finger count and time of the last completed tap.
    last: Option<(usize, std::time::Instant)>,
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
    /// What the mouse button now down settled on, for the roles that are
    /// decided by where the press landed rather than in the settings.
    ///
    /// Held for the whole press: a drag that starts on the model must keep
    /// sculpting once it wanders off the silhouette, and a drag that starts in
    /// space must not start carving the moment it crosses the model.
    held: Option<Role>,
    fingers: Vec<Finger>,
    /// The same, for the finger or pen currently down.
    held_touch: Option<Role>,
    /// True once a stroke was handed to the sculptor from a single finger.
    finger_stroke: bool,
    /// Set when a multi-finger gesture starts, so lifting back to one finger
    /// does not resume sculpting mid-air.
    gesture_lock: bool,
    taps: TapTracker,
}

impl Input {
    /// Records the new cursor position and returns the movement since the last.
    pub fn move_cursor(&mut self, x: f32, y: f32) -> Vec2 {
        let next = Vec2::new(x, y);
        let delta = next - self.cursor;
        self.cursor = next;
        delta
    }

    /// Turns the held buttons into a camera move, if any of them asks for one.
    ///
    /// Alt always navigates whatever the left button is bound to, which is the
    /// escape hatch when every button has been given to sculpting. Shift turns
    /// an orbit into a pan, the way it does in most viewers.
    pub fn mouse_navigation(&self, delta: Vec2, b: &Bindings) -> Option<Gesture> {
        let role = if self.lmb && self.alt {
            Role::Orbit
        } else if self.lmb {
            b.left
        } else if self.mmb {
            b.middle
        } else if self.rmb {
            b.right
        } else {
            return None;
        };
        // Whatever the press decided stands until it is let go.
        let role = match role {
            Role::Auto => self.held.unwrap_or(Role::Orbit),
            other => other,
        };
        let role = match (role, self.shift) {
            (Role::Orbit, true) => Role::Pan,
            (r, _) => r,
        };
        role.gesture(delta)
    }

    /// Settles what a press of this button does and reports whether it draws.
    ///
    /// `target` says what the press landed on, which is the only thing the
    /// automatic binding needs to make up its mind. Call it on the release too,
    /// with whatever target: the release only reads back what the press decided.
    pub fn sculpts(
        &mut self,
        button: winit::event::MouseButton,
        b: &Bindings,
        down: bool,
        target: PressTarget,
    ) -> bool {
        let role = match button {
            winit::event::MouseButton::Left => b.left,
            winit::event::MouseButton::Middle => b.middle,
            winit::event::MouseButton::Right => b.right,
            _ => Role::Nothing,
        };
        let role = if down {
            let settled = role.resolve(target);
            self.held = Some(settled);
            settled
        } else {
            let settled = self.held.take().unwrap_or(role);
            // A button going up while another is still down leaves that other
            // one without its decision, so put the raw role back for it.
            if self.lmb || self.mmb || self.rmb {
                self.held = Some(settled);
            }
            settled
        };
        role == Role::Sculpt && !self.alt
    }

    /// Whether the pointer needs a pick before the next press can be decided.
    ///
    /// Picking is not free at five million triangles, so it happens only for
    /// the bindings that actually ask a question.
    pub fn needs_pick(b: &Bindings) -> bool {
        [b.left, b.middle, b.right, b.touch, b.pen].contains(&Role::Auto)
    }

    pub fn touch_active(&self) -> bool {
        !self.fingers.is_empty()
    }

    /// Feeds one touch event through the gesture recogniser.
    ///
    /// `on_model` is whether the model lies under this contact, and only the
    /// automatic binding reads it.
    pub fn on_touch(
        &mut self,
        touch: &Touch,
        over_ui: bool,
        b: &Bindings,
        on_model: bool,
    ) -> Option<TouchOutcome> {
        let target = if over_ui {
            PressTarget::Ui
        } else if on_model {
            PressTarget::Model
        } else {
            PressTarget::Space
        };
        let pos = Vec2::new(touch.location.x as f32, touch.location.y as f32);
        // A digitiser reports pressure; a fingertip does not. That is the only
        // signal we get to tell a stylus from a finger, and it is good enough
        // to let the two have different jobs.
        let is_pen = touch.force.is_some();
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
        let raw = if is_pen { b.pen } else { b.touch };
        // A finger that is already down keeps the job it landed with; a new one
        // decides now.
        let role = match touch.phase {
            TouchPhase::Started => {
                let settled = raw.resolve(target);
                self.held_touch = Some(settled);
                settled
            }
            _ => self.held_touch.unwrap_or(raw),
        };

        match touch.phase {
            TouchPhase::Started => {
                let now = std::time::Instant::now();
                if self.fingers.is_empty() {
                    self.taps = TapTracker { last: self.taps.last, began: Some(now), ..Default::default() };
                }
                self.fingers.push(Finger { id: touch.id, pos, start: pos, pressure });
                self.taps.fingers = self.taps.fingers.max(self.fingers.len());

                if self.fingers.len() == 1 {
                    if over_ui {
                        // Let egui own this one.
                        return None;
                    }
                    if role != Role::Sculpt {
                        // This device navigates; nothing to do until it moves.
                        self.finger_stroke = false;
                        self.gesture_lock = true;
                        return None;
                    }
                    self.finger_stroke = true;
                    self.gesture_lock = false;
                    return Some(TouchOutcome::StrokeStart { at: pos, pressure });
                }
                // A second finger means the user wants to navigate, or to tap.
                self.gesture_lock = true;
                if self.finger_stroke {
                    self.finger_stroke = false;
                    // Landing this fast means the first finger was part of the
                    // same gesture, not a stroke someone wanted to keep.
                    let quick = self
                        .taps
                        .began
                        .is_some_and(|t| now.duration_since(t).as_secs_f64() < STROKE_GRACE_SECONDS);
                    return Some(if quick {
                        TouchOutcome::CancelStroke
                    } else {
                        TouchOutcome::StrokeEnd
                    });
                }
                None
            }
            TouchPhase::Moved => {
                let previous = self.update_finger(touch.id, pos, pressure)?;
                if let Some(f) = self.fingers.iter().find(|f| f.id == touch.id) {
                    self.taps.travel = self.taps.travel.max((pos - f.start).length());
                }
                match self.fingers.len() {
                    1 if self.finger_stroke => {
                        Some(TouchOutcome::StrokeMove { at: pos, pressure })
                    }
                    // A single device bound to navigation drives the camera.
                    1 => role
                        .gesture(pos - previous)
                        .map(|g| TouchOutcome::Navigate(vec![g])),
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
                if !self.fingers.is_empty() {
                    return None;
                }
                let was_stroking = self.finger_stroke;
                self.finger_stroke = false;
                self.gesture_lock = false;
                self.held_touch = None;

                // Every finger is off: the sequence is over, so decide whether
                // it was a tap, and whether it was the second of a pair.
                if let Some(action) = self.finish_tap() {
                    return Some(action);
                }
                was_stroking.then_some(TouchOutcome::StrokeEnd)
            }
        }
    }

    /// Closes off a touch sequence and reports the double tap it completed.
    ///
    /// Two fingers undo, three redo. A double tap rather than a single one
    /// because two fingers resting briefly on the way to an orbit is far too
    /// easy to do by accident, and undoing work nobody asked to undo is the
    /// worst thing an input gesture can do.
    fn finish_tap(&mut self) -> Option<TouchOutcome> {
        let began = self.taps.began.take()?;
        let now = std::time::Instant::now();
        let quick = now.duration_since(began).as_secs_f64() < TAP_MAX_SECONDS;
        let still = self.taps.travel < TAP_MAX_TRAVEL;
        let fingers = self.taps.fingers;
        self.taps.travel = 0.0;
        self.taps.fingers = 0;

        if !quick || !still || !(2..=3).contains(&fingers) {
            self.taps.last = None;
            return None;
        }

        let paired = self
            .taps
            .last
            .is_some_and(|(n, at)| n == fingers && now.duration_since(at).as_secs_f64() < DOUBLE_TAP_SECONDS);
        if !paired {
            self.taps.last = Some((fingers, now));
            return None;
        }
        self.taps.last = None;
        Some(if fingers == 2 { TouchOutcome::Undo } else { TouchOutcome::Redo })
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

#[cfg(test)]
mod tests {
    use super::*;
    use winit::event::MouseButton;

    #[test]
    fn auto_sculpts_on_the_model_and_orbits_off_it() {
        assert_eq!(Role::Auto.resolve(PressTarget::Model), Role::Sculpt);
        assert_eq!(Role::Auto.resolve(PressTarget::Space), Role::Orbit);
    }

    /// A fixed binding must not start meaning something else because of where
    /// the press landed. Only `Auto` asks the question.
    #[test]
    fn a_fixed_binding_ignores_where_the_press_landed() {
        for role in [Role::Sculpt, Role::Orbit, Role::Pan, Role::Zoom, Role::Nothing] {
            assert_eq!(role.resolve(PressTarget::Model), role);
            assert_eq!(role.resolve(PressTarget::Space), role);
        }
    }

    /// Dragging a slider must never also move the camera behind it.
    #[test]
    fn a_press_the_interface_took_does_nothing_else() {
        for role in Role::ALL {
            assert_eq!(role.resolve(PressTarget::Ui), Role::Nothing);
        }
    }

    /// The decision is made once, on contact. A stroke that starts on the model
    /// and wanders off it has to keep sculpting, or every stroke over an edge
    /// would end by spinning the view.
    #[test]
    fn the_press_decides_once_and_the_drag_keeps_it() {
        let mut input = Input::default();
        let b = Bindings::default();
        input.lmb = true;
        assert!(input.sculpts(MouseButton::Left, &b, true, PressTarget::Model));
        // Off the silhouette now, but still the same press.
        assert!(input.mouse_navigation(Vec2::new(10.0, 0.0), &b).is_none());

        input.lmb = false;
        input.sculpts(MouseButton::Left, &b, false, PressTarget::Space);
        input.lmb = true;
        assert!(!input.sculpts(MouseButton::Left, &b, true, PressTarget::Space));
        assert!(matches!(
            input.mouse_navigation(Vec2::new(10.0, 0.0), &b),
            Some(Gesture::Orbit(_))
        ));
    }

    /// Picking costs real time on a dense model, so it must not happen for
    /// bindings that were never going to look at the answer.
    #[test]
    fn only_the_automatic_binding_asks_for_a_pick() {
        let fixed = Bindings {
            left: Role::Sculpt,
            middle: Role::Orbit,
            right: Role::Orbit,
            touch: Role::Sculpt,
            pen: Role::Sculpt,
        };
        assert!(!Input::needs_pick(&fixed));
        assert!(Input::needs_pick(&Bindings::default()));
    }
}
