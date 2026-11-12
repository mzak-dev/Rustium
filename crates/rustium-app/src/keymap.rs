//! The keymap: every rebindable keyboard shortcut in one place.
//!
//! Each [`Action`] has a default [`Chord`]; [`Keymap`] stores only what the user changed, so new actions and
//! changed defaults reach existing settings files. Shortcut handling, tooltips, menus and Settings → Input all
//! read the chords from here.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::sync::LazyLock;

use color_eyre::eyre::{Result, WrapErr};
use egui::{Event, InputState, Key, Modifiers};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::editor::Tool;
use crate::hud::GraphTool;
use crate::render_mode::RenderTool;

/// A key together with the modifiers that must be held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Chord {
    pub(crate) key: Key,
    /// Ctrl (Cmd on macOS).
    pub(crate) command: bool,
    pub(crate) shift: bool,
    pub(crate) alt: bool,
}

impl Chord {
    pub(crate) const fn plain(key: Key) -> Self {
        Self {
            key,
            command: false,
            shift: false,
            alt: false,
        }
    }

    pub(crate) const fn command(key: Key) -> Self {
        Self {
            command: true,
            ..Self::plain(key)
        }
    }

    pub(crate) fn modifiers(self) -> Modifiers {
        Modifiers {
            command: self.command,
            ctrl: self.command && !cfg!(target_os = "macos"),
            mac_cmd: self.command && cfg!(target_os = "macos"),
            shift: self.shift,
            alt: self.alt,
        }
    }

    /// How many modifiers the chord needs; chords with more are matched first.
    fn specificity(self) -> u8 {
        u8::from(self.command) + u8::from(self.shift) + u8::from(self.alt)
    }

    /// Whether the chord was pressed this frame, without consuming the key.
    pub(crate) fn pressed(self, input: &InputState) -> bool {
        input.key_pressed(self.key)
            && input.modifiers.command == self.command
            && (self.shift || !input.modifiers.shift)
            && (self.alt || !input.modifiers.alt)
    }

    /// The text stored in files, e.g. `Ctrl+Shift+Z`.
    pub(crate) fn to_text(self) -> String {
        let mut text = String::new();
        if self.command {
            text.push_str("Ctrl+");
        }
        if self.shift {
            text.push_str("Shift+");
        }
        if self.alt {
            text.push_str("Alt+");
        }
        text.push_str(self.key.name());
        text
    }

    /// Parses [`Chord::to_text`] output; modifier names are case-insensitive.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let key = Key::from_name(parts.pop()?)?;
        let mut chord = Self::plain(key);
        for part in parts {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "cmd" | "command" => chord.command = true,
                "shift" => chord.shift = true,
                "alt" | "option" => chord.alt = true,
                _ => return None,
            }
        }
        Some(chord)
    }

    /// The chord a key event would produce, for rebinding.
    fn from_event(key: Key, modifiers: Modifiers) -> Self {
        Self {
            key,
            command: modifiers.command,
            shift: modifiers.shift,
            alt: modifiers.alt,
        }
    }
}

/// Display form as in the mockups: `Ctrl Shift Z`, `Ctrl ,`.
impl fmt::Display for Chord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.command {
            formatter.write_str("Ctrl ")?;
        }
        if self.shift {
            formatter.write_str("Shift ")?;
        }
        if self.alt {
            formatter.write_str("Alt ")?;
        }
        formatter.write_str(self.key.symbol_or_name())
    }
}

/// Which mode a shortcut works in. Shortcuts in different modes may share a chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// Works everywhere.
    Always,
    /// Model mode only.
    Model,
    /// Render mode only.
    Render,
    /// Effects mode (the Material node editor) only.
    Effects,
    /// The 3D view of Model and Render mode, but not the node editor.
    Viewport,
}

impl Scope {
    /// Whether a shortcut with this scope works in `mode` (`Model`, `Render` or `Effects`).
    pub(crate) fn applies_in(self, mode: Self) -> bool {
        match self {
            Self::Always => true,
            Self::Viewport => matches!(mode, Self::Model | Self::Render),
            other => other == mode,
        }
    }

    fn overlaps(self, other: Self) -> bool {
        [Self::Model, Self::Render, Self::Effects]
            .into_iter()
            .any(|mode| self.applies_in(mode) && other.applies_in(mode))
    }
}

/// Something a shortcut can trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Undo,
    Redo,
    NewScene,
    OpenFile,
    SaveFile,
    SaveFileAs,
    OpenSettings,
    FrameModel,
    StepIn,
    StepOut,
    DeleteNodes,
    RenderStart,
    ClearRenderRegion,
    Tool(Tool),
    RenderTool(RenderTool),
    /// Color slot `0..10`; the default keys run 1 – 9 then 0.
    Slot(u8),
    /// Effects mode: pick the Select, Pan or Cut links Tool.
    GraphTool(GraphTool),
    /// Effects mode: open the node search at the pointer.
    AddNode,
    /// Effects mode: wrap the selected nodes in a frame.
    FrameNodes,
    /// Effects mode: zoom so the whole graph is visible.
    FitGraph,
    /// Effects mode: delete the selected nodes or frame.
    DeleteGraphSelection,
    /// Effects mode: copy the selected nodes.
    DuplicateNodes,
    /// Effects mode: select every node.
    SelectAllNodes,
    /// Effects mode: clear the selection.
    ClearNodeSelection,
    /// Effects mode: edit Material `0..10`; the default keys run 1 – 9 then 0.
    MaterialSlot(u8),
}

const SLOT_KEYS: [Key; 10] = [
    Key::Num1,
    Key::Num2,
    Key::Num3,
    Key::Num4,
    Key::Num5,
    Key::Num6,
    Key::Num7,
    Key::Num8,
    Key::Num9,
    Key::Num0,
];

const MATERIAL_IDS: [&str; 10] = [
    "material-1",
    "material-2",
    "material-3",
    "material-4",
    "material-5",
    "material-6",
    "material-7",
    "material-8",
    "material-9",
    "material-10",
];

const SLOT_IDS: [&str; 10] = [
    "slot-1", "slot-2", "slot-3", "slot-4", "slot-5", "slot-6", "slot-7", "slot-8", "slot-9", "slot-10",
];

/// Settings → Input list headings, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Group {
    General,
    Tools,
    ColorSlots,
    Render,
    Effects,
}

impl Group {
    pub(crate) const ALL: [Self; 5] = [
        Self::General,
        Self::Tools,
        Self::ColorSlots,
        Self::Render,
        Self::Effects,
    ];

    pub(crate) fn message_key(self) -> &'static str {
        match self {
            Self::General => "binding-group-general",
            Self::Tools => "binding-group-tools",
            Self::ColorSlots => "binding-color-slots",
            Self::Render => "binding-group-render",
            Self::Effects => "binding-group-effects",
        }
    }
}

impl Action {
    /// Every action, in the order Settings lists them.
    pub(crate) fn all() -> &'static [Self] {
        static ALL: LazyLock<Vec<Action>> = LazyLock::new(|| {
            let mut all = vec![
                Action::Undo,
                Action::Redo,
                Action::NewScene,
                Action::OpenFile,
                Action::SaveFile,
                Action::SaveFileAs,
                Action::OpenSettings,
                Action::FrameModel,
                Action::StepIn,
                Action::StepOut,
                Action::DeleteNodes,
            ];
            all.extend(Tool::ALL.map(Action::Tool));
            all.extend((0..10).map(Action::Slot));
            all.extend(RenderTool::ALL.map(Action::RenderTool));
            all.extend([Action::RenderStart, Action::ClearRenderRegion]);
            all.extend(GraphTool::ALL.map(Action::GraphTool));
            all.extend([
                Action::AddNode,
                Action::FrameNodes,
                Action::FitGraph,
                Action::DeleteGraphSelection,
                Action::DuplicateNodes,
                Action::SelectAllNodes,
                Action::ClearNodeSelection,
            ]);
            all.extend((0..10).map(Action::MaterialSlot));
            all
        });
        &ALL
    }

    /// Stable name used in the settings and keymap files.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::NewScene => "new-scene",
            Self::OpenFile => "open-file",
            Self::SaveFile => "save-file",
            Self::SaveFileAs => "save-file-as",
            Self::OpenSettings => "open-settings",
            Self::FrameModel => "frame-model",
            Self::StepIn => "step-in",
            Self::StepOut => "step-out",
            Self::DeleteNodes => "delete-nodes",
            Self::RenderStart => "render-start",
            Self::ClearRenderRegion => "clear-render-region",
            Self::Tool(tool) => tool.message_key(),
            Self::RenderTool(tool) => tool.message_key(),
            Self::Slot(slot) => SLOT_IDS.get(usize::from(slot)).copied().unwrap_or("slot-1"),
            Self::GraphTool(tool) => tool.action_id(),
            Self::AddNode => "graph-add-node",
            Self::FrameNodes => "graph-frame-nodes",
            Self::FitGraph => "graph-fit",
            Self::DeleteGraphSelection => "graph-delete",
            Self::DuplicateNodes => "graph-duplicate",
            Self::SelectAllNodes => "graph-select-all",
            Self::ClearNodeSelection => "graph-deselect",
            Self::MaterialSlot(slot) => MATERIAL_IDS.get(usize::from(slot)).copied().unwrap_or("material-1"),
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        Self::all().iter().copied().find(|action| action.id() == id)
    }

    pub(crate) fn scope(self) -> Scope {
        match self {
            Self::Undo
            | Self::Redo
            | Self::NewScene
            | Self::OpenFile
            | Self::SaveFile
            | Self::SaveFileAs
            | Self::OpenSettings => Scope::Always,
            Self::FrameModel => Scope::Viewport,
            Self::GraphTool(_)
            | Self::AddNode
            | Self::FrameNodes
            | Self::FitGraph
            | Self::DeleteGraphSelection
            | Self::DuplicateNodes
            | Self::SelectAllNodes
            | Self::ClearNodeSelection
            | Self::MaterialSlot(_) => Scope::Effects,
            Self::StepIn | Self::StepOut | Self::DeleteNodes | Self::Tool(_) | Self::Slot(_) => Scope::Model,
            Self::RenderStart | Self::ClearRenderRegion | Self::RenderTool(_) => Scope::Render,
        }
    }

    pub(crate) fn group(self) -> Group {
        match self {
            Self::Undo
            | Self::Redo
            | Self::NewScene
            | Self::OpenFile
            | Self::SaveFile
            | Self::SaveFileAs
            | Self::OpenSettings
            | Self::FrameModel
            | Self::StepIn
            | Self::StepOut
            | Self::DeleteNodes => Group::General,
            Self::Tool(_) => Group::Tools,
            Self::Slot(_) | Self::MaterialSlot(_) => Group::ColorSlots,
            Self::RenderStart | Self::ClearRenderRegion | Self::RenderTool(_) => Group::Render,
            Self::GraphTool(_)
            | Self::AddNode
            | Self::FrameNodes
            | Self::FitGraph
            | Self::DeleteGraphSelection
            | Self::DuplicateNodes
            | Self::SelectAllNodes
            | Self::ClearNodeSelection => Group::Effects,
        }
    }

    /// Message key of the action's name; slots use `binding-slot` with a `slot` argument.
    pub(crate) fn message_key(self) -> &'static str {
        match self {
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::NewScene => "menu-new",
            Self::OpenFile => "menu-open",
            Self::SaveFile => "menu-save",
            Self::SaveFileAs => "menu-save-as",
            Self::OpenSettings => "menu-settings",
            Self::FrameModel => "action-frame-model",
            Self::StepIn => "action-step-in",
            Self::StepOut => "action-step-out",
            Self::DeleteNodes => "action-delete-nodes",
            Self::RenderStart => "action-render-start",
            Self::ClearRenderRegion => "action-clear-region",
            Self::Tool(tool) => tool.message_key(),
            Self::RenderTool(tool) => tool.message_key(),
            Self::Slot(_) => "binding-slot",
            Self::GraphTool(tool) => tool.message_key(),
            Self::AddNode => "gt-add",
            Self::FrameNodes => "gt-frame",
            Self::FitGraph => "gt-fit",
            Self::DeleteGraphSelection => "action-graph-delete",
            Self::DuplicateNodes => "action-graph-duplicate",
            Self::SelectAllNodes => "action-graph-select-all",
            Self::ClearNodeSelection => "action-graph-deselect",
            Self::MaterialSlot(_) => "binding-material",
        }
    }

    /// The slot number shown to the user (1 – 10), for [`Action::Slot`].
    pub(crate) fn slot_number(self) -> Option<u8> {
        match self {
            Self::Slot(slot) | Self::MaterialSlot(slot) => Some(slot + 1),
            _ => None,
        }
    }

    pub(crate) fn default_chord(self) -> Option<Chord> {
        Some(match self {
            Self::Undo => Chord::command(Key::Z),
            Self::Redo => Chord::command(Key::Y),
            Self::NewScene => Chord::command(Key::N),
            Self::OpenFile => Chord::command(Key::O),
            Self::SaveFile => Chord::command(Key::S),
            Self::SaveFileAs => Chord {
                shift: true,
                ..Chord::command(Key::S)
            },
            Self::OpenSettings => Chord::command(Key::Comma),
            Self::FrameModel | Self::FitGraph => Chord::plain(Key::Home),
            Self::StepIn => Chord::plain(Key::Enter),
            Self::StepOut | Self::ClearRenderRegion | Self::ClearNodeSelection => Chord::plain(Key::Escape),
            Self::DeleteNodes | Self::DeleteGraphSelection => Chord::plain(Key::Delete),
            Self::RenderStart => Chord::plain(Key::F5),
            Self::Tool(tool) => Chord::plain(tool.key()),
            Self::RenderTool(tool) => Chord::plain(tool.key()),
            Self::Slot(slot) | Self::MaterialSlot(slot) => Chord::plain(*SLOT_KEYS.get(usize::from(slot))?),
            Self::GraphTool(tool) => Chord::plain(tool.key()),
            Self::AddNode => Chord::plain(Key::Tab),
            Self::FrameNodes => Chord::plain(Key::N),
            Self::DuplicateNodes => Chord::command(Key::D),
            Self::SelectAllNodes => Chord::command(Key::A),
        })
    }
}

/// The user's shortcuts: defaults plus the changes stored in the settings file.
///
/// Serialized as a table of `action-id = "Ctrl+Z"`, where an empty string means unbound.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(from = "BTreeMap<String, String>", into = "BTreeMap<String, String>")]
pub(crate) struct Keymap {
    overrides: BTreeMap<&'static str, Option<Chord>>,
}

impl From<BTreeMap<String, String>> for Keymap {
    fn from(table: BTreeMap<String, String>) -> Self {
        Self::from_table(&table).0
    }
}

impl From<Keymap> for BTreeMap<String, String> {
    fn from(keymap: Keymap) -> Self {
        keymap
            .overrides
            .into_iter()
            .map(|(id, chord)| (id.to_owned(), chord.map(Chord::to_text).unwrap_or_default()))
            .collect()
    }
}

/// Shortcut chord a key press would rebind to, or a cancellation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Capture {
    Chord(Chord),
    Cancel,
}

impl Keymap {
    /// The chord bound to `action`, if any.
    pub(crate) fn chord(&self, action: Action) -> Option<Chord> {
        match self.overrides.get(action.id()) {
            Some(chord) => *chord,
            None => action.default_chord(),
        }
    }

    /// Display text of the binding, e.g. `Ctrl Z`; an em dash when unbound.
    pub(crate) fn label(&self, action: Action) -> String {
        self.chord(action)
            .map_or_else(|| "—".to_owned(), |chord| chord.to_string())
    }

    /// Display text for tooltips: ` (Ctrl Z)`, or nothing when unbound.
    pub(crate) fn hint(&self, action: Action) -> String {
        self.chord(action)
            .map_or_else(String::new, |chord| format!(" ({chord})"))
    }

    /// Binds `action` to `chord` (or unbinds it) without looking at conflicts.
    pub(crate) fn set(&mut self, action: Action, chord: Option<Chord>) {
        if chord == action.default_chord() {
            self.overrides.remove(action.id());
        } else {
            self.overrides.insert(action.id(), chord);
        }
    }

    /// Whether nothing differs from the defaults.
    pub(crate) fn is_default(&self) -> bool {
        self.overrides.is_empty()
    }

    /// Goes back to the default keys.
    pub(crate) fn reset(&mut self) {
        self.overrides.clear();
    }

    /// Other actions that already use `chord` in an overlapping mode.
    pub(crate) fn conflicts(&self, action: Action, chord: Chord) -> Vec<Action> {
        Action::all()
            .iter()
            .copied()
            .filter(|other| {
                *other != action && other.scope().overlaps(action.scope()) && self.chord(*other) == Some(chord)
            })
            .collect()
    }

    /// Whether `action` shares its chord with another action.
    pub(crate) fn has_conflict(&self, action: Action) -> bool {
        self.chord(action)
            .is_some_and(|chord| !self.conflicts(action, chord).is_empty())
    }

    /// Binds `action` to `chord` and gives each conflicting action the key `action` had before.
    pub(crate) fn swap(&mut self, action: Action, chord: Chord) {
        let previous = self.chord(action);
        for other in self.conflicts(action, chord) {
            self.set(other, previous);
        }
        self.set(action, Some(chord));
    }

    /// Binds `action` to `chord` and unbinds every conflicting action.
    pub(crate) fn take_over(&mut self, action: Action, chord: Chord) {
        for other in self.conflicts(action, chord) {
            self.set(other, None);
        }
        self.set(action, Some(chord));
    }

    /// Consumes the shortcuts pressed this frame that work in `mode` and returns their actions.
    ///
    /// Chords with more modifiers are tried first because egui ignores extra Shift and Alt when matching.
    pub(crate) fn take_pressed(&self, input: &mut InputState, mode: Scope) -> Vec<Action> {
        let mut bindings: Vec<(Chord, Action)> = Action::all()
            .iter()
            .copied()
            .filter(|action| action.scope().applies_in(mode))
            .filter_map(|action| self.chord(action).map(|chord| (chord, action)))
            .collect();
        // Ctrl+Shift+Z has always redone; keep it unless the user gave it to something else.
        let alias = Chord {
            shift: true,
            ..Chord::command(Key::Z)
        };
        if self.conflicts(Action::Redo, alias).is_empty() && self.chord(Action::Redo) != Some(alias) {
            bindings.push((alias, Action::Redo));
        }
        bindings.sort_by_key(|(chord, _)| std::cmp::Reverse(chord.specificity()));
        let mut pressed = Vec::new();
        for (chord, action) in bindings {
            if input.consume_key(chord.modifiers(), chord.key) {
                pressed.push(action);
            }
        }
        pressed
    }

    /// The first key pressed this frame as a chord; Escape cancels.
    pub(crate) fn capture(input: &InputState) -> Option<Capture> {
        input.events.iter().find_map(|event| match event {
            Event::Key {
                key,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } => Some(if *key == Key::Escape {
                Capture::Cancel
            } else {
                Capture::Chord(Chord::from_event(*key, *modifiers))
            }),
            _ => None,
        })
    }

    /// The keymap as a file anyone can edit: every action with its current key.
    pub(crate) fn to_file_text(&self) -> String {
        let mut text = String::from(
            "# Rustium keymap. One line per action; an empty string leaves the action unbound.\n\
             # Keys: letters, digits, F1-F12, Enter, Escape, Home, Delete, Comma, ... with optional Ctrl+, Shift+, Alt+ prefixes.\n\
             [keymap]\n",
        );
        for action in Action::all() {
            let chord = self.chord(*action).map(Chord::to_text).unwrap_or_default();
            let _ = writeln!(text, "{} = \"{chord}\"", action.id());
        }
        text
    }

    /// Reads a keymap file. Actions missing from it keep their default key; unknown actions and unreadable
    /// chords are skipped and counted.
    ///
    /// # Errors
    ///
    /// Fails when `text` is not TOML or has no `[keymap]` table of strings.
    pub(crate) fn from_file_text(text: &str) -> Result<(Self, usize)> {
        #[derive(Deserialize)]
        struct File {
            keymap: BTreeMap<String, String>,
        }
        let file: File = toml::from_str(text).wrap_err("not a keymap file")?;
        Ok(Self::from_table(&file.keymap))
    }

    fn from_table(table: &BTreeMap<String, String>) -> (Self, usize) {
        let mut keymap = Self::default();
        let mut skipped = 0;
        for (id, text) in table {
            let Some(action) = Action::from_id(id) else {
                warn!(id, "unknown keymap action ignored");
                skipped += 1;
                continue;
            };
            if text.trim().is_empty() {
                keymap.set(action, None);
            } else if let Some(chord) = Chord::parse(text) {
                keymap.set(action, Some(chord));
            } else {
                warn!(id, text, "unreadable keymap chord ignored");
                skipped += 1;
            }
        }
        (keymap, skipped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_have_no_conflicts() {
        let keymap = Keymap::default();
        for action in Action::all() {
            assert!(!keymap.has_conflict(*action), "{} conflicts", action.id());
            assert!(keymap.chord(*action).is_some(), "{} has no default", action.id());
        }
    }

    #[test]
    fn action_ids_are_unique() {
        let mut ids: Vec<_> = Action::all().iter().map(|action| action.id()).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count);
    }

    #[test]
    fn chord_text_round_trips_for_every_key() {
        for key in Key::ALL {
            for (command, shift, alt) in [(false, false, false), (true, false, false), (true, true, true)] {
                let chord = Chord {
                    key: *key,
                    command,
                    shift,
                    alt,
                };
                assert_eq!(Chord::parse(&chord.to_text()), Some(chord), "{chord}");
            }
        }
    }

    #[test]
    fn chord_parse_accepts_loose_spelling_and_rejects_nonsense() {
        assert_eq!(
            Chord::parse("ctrl + shift + z"),
            Some(Chord {
                shift: true,
                ..Chord::command(Key::Z)
            })
        );
        assert_eq!(Chord::parse("Esc"), Some(Chord::plain(Key::Escape)));
        assert_eq!(Chord::parse("Hyper+Z"), None);
        assert_eq!(Chord::parse("Ctrl+"), None);
        assert_eq!(Chord::parse(""), None);
    }

    #[test]
    fn display_matches_the_mockups() {
        assert_eq!(Chord::command(Key::K).to_string(), "Ctrl K");
        assert_eq!(Chord::command(Key::Comma).to_string(), "Ctrl ,");
        assert_eq!(Chord::plain(Key::B).to_string(), "B");
    }

    #[test]
    fn same_chord_conflicts_within_a_mode_but_not_across_modes() {
        let keymap = Keymap::default();
        // Fill (Model) and Pick focus (Render) both use F by default.
        let fill = Action::Tool(Tool::Fill);
        let focus = Action::RenderTool(RenderTool::PickFocus);
        assert_eq!(keymap.chord(fill), keymap.chord(focus));
        assert!(!keymap.has_conflict(fill));

        let b = Chord::plain(Key::B);
        assert_eq!(
            keymap.conflicts(Action::Tool(Tool::Erase), b),
            vec![Action::Tool(Tool::Attach)]
        );
        // Undo works everywhere, so it clashes with anything in either mode.
        let ctrl_z = Chord::command(Key::Z);
        assert_eq!(keymap.conflicts(Action::RenderStart, ctrl_z), vec![Action::Undo]);
    }

    #[test]
    fn swap_exchanges_keys_and_take_over_unbinds() {
        let mut keymap = Keymap::default();
        let attach = Action::Tool(Tool::Attach);
        let erase = Action::Tool(Tool::Erase);
        let b = Chord::plain(Key::B);
        let e = Chord::plain(Key::E);

        keymap.swap(erase, b);
        assert_eq!(keymap.chord(erase), Some(b));
        assert_eq!(keymap.chord(attach), Some(e));
        assert!(!keymap.has_conflict(erase));

        keymap.reset();
        keymap.take_over(erase, b);
        assert_eq!(keymap.chord(erase), Some(b));
        assert_eq!(keymap.chord(attach), None);
    }

    #[test]
    fn setting_the_default_removes_the_override() {
        let mut keymap = Keymap::default();
        let attach = Action::Tool(Tool::Attach);
        keymap.set(attach, Some(Chord::plain(Key::Q)));
        assert!(!keymap.is_default());
        keymap.set(attach, attach.default_chord());
        assert!(keymap.is_default());
    }

    #[test]
    fn settings_table_keeps_only_changes_and_round_trips() {
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        struct Holder {
            keymap: Keymap,
        }
        let mut keymap = Keymap::default();
        keymap.set(Action::Tool(Tool::Attach), Some(Chord::plain(Key::Q)));
        keymap.set(Action::Undo, None);
        let holder = Holder { keymap };
        let text = toml::to_string_pretty(&holder).unwrap();
        assert!(text.contains("tool-attach = \"Q\""), "{text}");
        assert!(!text.contains("redo"), "{text}");
        assert_eq!(toml::from_str::<Holder>(&text).unwrap(), holder);
    }

    #[test]
    fn exported_file_imports_to_the_same_keymap() {
        let mut keymap = Keymap::default();
        keymap.set(Action::Tool(Tool::Fill), Some(Chord::plain(Key::Num5)));
        keymap.set(
            Action::Redo,
            Some(Chord {
                shift: true,
                ..Chord::command(Key::Z)
            }),
        );
        keymap.set(Action::DeleteNodes, None);
        let (imported, skipped) = Keymap::from_file_text(&keymap.to_file_text()).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(imported, keymap);
    }

    #[test]
    fn import_skips_unknown_actions_and_bad_chords_and_keeps_defaults() {
        let text = "[keymap]\ntool-erase = \"X\"\nfly = \"F\"\nundo = \"Hyper+Z\"\n";
        let (keymap, skipped) = Keymap::from_file_text(text).unwrap();
        assert_eq!(skipped, 2);
        assert_eq!(keymap.chord(Action::Tool(Tool::Erase)), Some(Chord::plain(Key::X)));
        assert_eq!(keymap.chord(Action::Undo), Action::Undo.default_chord());
    }

    #[test]
    fn import_rejects_files_without_a_keymap_table() {
        assert!(Keymap::from_file_text("not toml [").is_err());
        assert!(Keymap::from_file_text("title = \"x\"").is_err());
    }
}
