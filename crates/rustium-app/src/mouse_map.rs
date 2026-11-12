//! Rebindable mouse gestures of the Model view: which button, with which key held, orbits, pans or flies.

use std::fmt;

use egui::{InputState, Key, PointerButton};
use serde::{Deserialize, Serialize};

/// A camera gesture driven by dragging with a mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gesture {
    Orbit,
    Pan,
    Fly,
}

impl Gesture {
    pub(crate) const ALL: [Self; 3] = [Self::Orbit, Self::Pan, Self::Fly];

    pub(crate) fn message_key(self) -> &'static str {
        match self {
            Self::Orbit => "hint-orbit",
            Self::Pan => "hint-pan",
            Self::Fly => "hint-fly",
        }
    }
}

/// The mouse button of a [`MouseChord`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Button {
    Left,
    Middle,
    Right,
}

impl Button {
    fn pointer(self) -> PointerButton {
        match self {
            Self::Left => PointerButton::Primary,
            Self::Middle => PointerButton::Middle,
            Self::Right => PointerButton::Secondary,
        }
    }
}

/// The key held together with the button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) enum Hold {
    #[default]
    Nothing,
    Shift,
    Ctrl,
    Alt,
    Space,
}

impl Hold {
    /// The key held right now, the most specific first.
    fn now(input: &InputState) -> Self {
        if input.key_down(Key::Space) {
            Self::Space
        } else if input.modifiers.command {
            Self::Ctrl
        } else if input.modifiers.alt {
            Self::Alt
        } else if input.modifiers.shift {
            Self::Shift
        } else {
            Self::Nothing
        }
    }
}

/// A mouse button plus the key held with it, e.g. `Space LMB`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MouseChord {
    pub(crate) button: Button,
    #[serde(default)]
    pub(crate) hold: Hold,
}

impl MouseChord {
    const fn new(button: Button, hold: Hold) -> Self {
        Self { button, hold }
    }

    /// Whether this chord would take every plain left click away from the Tools.
    pub(crate) fn blocks_tools(self) -> bool {
        self == Self::new(Button::Left, Hold::Nothing)
    }
}

impl fmt::Display for MouseChord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hold = match self.hold {
            Hold::Nothing => "",
            Hold::Shift => "Shift ",
            Hold::Ctrl => "Ctrl ",
            Hold::Alt => "Alt ",
            Hold::Space => "Space ",
        };
        let button = match self.button {
            Button::Left => "LMB",
            Button::Middle => "MMB",
            Button::Right => "RMB",
        };
        write!(formatter, "{hold}{button}")
    }
}

/// Which chord starts each [`Gesture`]; saved with the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct MouseMap {
    pub(crate) orbit: MouseChord,
    pub(crate) pan: MouseChord,
    pub(crate) fly: MouseChord,
}

impl Default for MouseMap {
    fn default() -> Self {
        Self {
            orbit: MouseChord::new(Button::Middle, Hold::Nothing),
            pan: MouseChord::new(Button::Middle, Hold::Shift),
            fly: MouseChord::new(Button::Right, Hold::Nothing),
        }
    }
}

impl MouseMap {
    pub(crate) fn chord(self, gesture: Gesture) -> MouseChord {
        match gesture {
            Gesture::Orbit => self.orbit,
            Gesture::Pan => self.pan,
            Gesture::Fly => self.fly,
        }
    }

    /// Binds `gesture` to `chord`; a gesture that had `chord` gets the old chord of `gesture`, so none is lost.
    pub(crate) fn set(&mut self, gesture: Gesture, chord: MouseChord) {
        let previous = self.chord(gesture);
        for other in Gesture::ALL {
            if other != gesture && self.chord(other) == chord {
                *self.slot(other) = previous;
            }
        }
        *self.slot(gesture) = chord;
    }

    fn slot(&mut self, gesture: Gesture) -> &mut MouseChord {
        match gesture {
            Gesture::Orbit => &mut self.orbit,
            Gesture::Pan => &mut self.pan,
            Gesture::Fly => &mut self.fly,
        }
    }

    /// The gesture a button pressed this frame starts: the exact chord first, then a chord of that button with
    /// nothing held (so Shift-clicking with the orbit button still orbits when Shift means nothing else).
    pub(crate) fn pressed(self, input: &InputState) -> Option<Gesture> {
        let hold = Hold::now(input);
        let pressed = |gesture: Gesture| input.pointer.button_pressed(self.chord(gesture).button.pointer());
        Gesture::ALL
            .into_iter()
            .find(|gesture| pressed(*gesture) && self.chord(*gesture).hold == hold)
            .or_else(|| {
                Gesture::ALL.into_iter().find(|gesture| {
                    let chord = self.chord(*gesture);
                    pressed(*gesture) && chord.hold == Hold::Nothing && chord.button != Button::Left
                })
            })
    }

    /// Whether the button of `gesture` is still held.
    pub(crate) fn held(self, input: &InputState, gesture: Gesture) -> bool {
        input.pointer.button_down(self.chord(gesture).button.pointer())
    }

    /// The chord of a mouse button pressed this frame, for rebinding.
    pub(crate) fn capture(input: &InputState) -> Option<MouseChord> {
        let button = [Button::Left, Button::Middle, Button::Right]
            .into_iter()
            .find(|button| input.pointer.button_pressed(button.pointer()))?;
        Some(MouseChord::new(button, Hold::now(input)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebinding_onto_a_used_chord_swaps_the_two() {
        let mut map = MouseMap::default();
        let space_left = MouseChord::new(Button::Left, Hold::Space);
        map.set(Gesture::Pan, space_left);
        assert_eq!(map.pan, space_left);
        map.set(Gesture::Orbit, space_left);
        assert_eq!(map.orbit, space_left);
        assert_eq!(
            map.pan,
            MouseChord::new(Button::Middle, Hold::Nothing),
            "pan took orbit's old chord"
        );
    }

    #[test]
    fn chords_read_like_the_hints() {
        assert_eq!(MouseChord::new(Button::Left, Hold::Space).to_string(), "Space LMB");
        assert_eq!(MouseMap::default().pan.to_string(), "Shift MMB");
    }

    #[test]
    fn settings_text_round_trips() {
        let mut map = MouseMap::default();
        map.set(Gesture::Pan, MouseChord::new(Button::Left, Hold::Space));
        let text = toml::to_string(&map).unwrap();
        assert_eq!(toml::from_str::<MouseMap>(&text).unwrap(), map);
    }
}
