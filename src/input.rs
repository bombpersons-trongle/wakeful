//! The input manager: a PlayStation-flavored action vocabulary, bound
//! to keyboard/mouse/gamepad by `assets/input.ron`.
//!
//! Gameplay never reads raw devices. Systems and scripts ask about
//! [`PadButton`]s (`pressed`, `just_pressed`) and [`PadAxis`] values.
//! [`InputManager::aggregate`] runs at the head of `FixedUpdate` and
//! rebuilds the shared [`InputState`] from Bevy's raw input resources;
//! edges are diffed against our own previous snapshot, so each fixed
//! tick sees each press exactly once — friendlier to scripts than
//! Bevy's frame-scoped flags. The PS-letter vocabulary maps onto Bevy's
//! geometric gamepad enums internally (Cross→South, Circle→East, ...).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};

use bevy::input::gamepad::{GamepadAxis, GamepadButton};
use bevy::input::keyboard::KeyCode;
use bevy::input::mouse::MouseButton;
use bevy::prelude::*;

use crate::assets::assets_root;

/// The shared input state: read by systems through the manager and by
/// script host functions through this handle.
pub type InputHandle = Arc<Mutex<InputState>>;

/// The engine's entire button vocabulary, labeled like a DualShock.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PadButton {
    Cross,
    Circle,
    Triangle,
    Square,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
    L1,
    R1,
    L2,
    R2,
    L3,
    R3,
    Start,
    Select,
}

impl PadButton {
    /// The config name: `cross`, `dpad_up`, `l2`, ...
    fn from_config(name: &str) -> Option<Self> {
        Some(match name {
            "cross" => Self::Cross,
            "circle" => Self::Circle,
            "triangle" => Self::Triangle,
            "square" => Self::Square,
            "dpad_up" => Self::DPadUp,
            "dpad_down" => Self::DPadDown,
            "dpad_left" => Self::DPadLeft,
            "dpad_right" => Self::DPadRight,
            "l1" => Self::L1,
            "r1" => Self::R1,
            "l2" => Self::L2,
            "r2" => Self::R2,
            "l3" => Self::L3,
            "r3" => Self::R3,
            "start" => Self::Start,
            "select" => Self::Select,
            _ => return None,
        })
    }

    fn to_gamepad(self) -> GamepadButton {
        match self {
            Self::Cross => GamepadButton::South,
            Self::Circle => GamepadButton::East,
            Self::Triangle => GamepadButton::North,
            Self::Square => GamepadButton::West,
            Self::DPadUp => GamepadButton::DPadUp,
            Self::DPadDown => GamepadButton::DPadDown,
            Self::DPadLeft => GamepadButton::DPadLeft,
            Self::DPadRight => GamepadButton::DPadRight,
            Self::L1 => GamepadButton::LeftTrigger,
            Self::R1 => GamepadButton::RightTrigger,
            Self::L2 => GamepadButton::LeftTrigger2,
            Self::R2 => GamepadButton::RightTrigger2,
            Self::L3 => GamepadButton::LeftThumb,
            Self::R3 => GamepadButton::RightThumb,
            Self::Start => GamepadButton::Start,
            Self::Select => GamepadButton::Select,
        }
    }
}

/// The engine's axis vocabulary: the two analog sticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PadAxis {
    LeftStickX,
    LeftStickY,
    RightStickX,
    RightStickY,
}

impl PadAxis {
    const ALL: [PadAxis; 4] = [
        PadAxis::LeftStickX,
        PadAxis::LeftStickY,
        PadAxis::RightStickX,
        PadAxis::RightStickY,
    ];

    /// The config and script name: `left_stick_x`, ...
    fn from_config(name: &str) -> Option<Self> {
        Some(match name {
            "left_stick_x" => Self::LeftStickX,
            "left_stick_y" => Self::LeftStickY,
            "right_stick_x" => Self::RightStickX,
            "right_stick_y" => Self::RightStickY,
            _ => return None,
        })
    }

    fn to_gamepad(self) -> GamepadAxis {
        match self {
            Self::LeftStickX => GamepadAxis::LeftStickX,
            Self::LeftStickY => GamepadAxis::LeftStickY,
            Self::RightStickX => GamepadAxis::RightStickX,
            Self::RightStickY => GamepadAxis::RightStickY,
        }
    }
}

/// A physical input bound to an action.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Raw {
    Key(KeyCode),
    Mouse(MouseButton),
    Pad(PadButton),
}

impl Raw {
    /// Parses a config value: bevy key names (`"KeyZ"`, `"ArrowUp"`),
    /// mouse buttons (`"MouseLeft"`), or pad buttons (`"Cross"`).
    fn from_config(name: &str) -> Option<Self> {
        // Binding values use the pad buttons' own PS-letter names
        // ("Cross", "DPadUp", "L2"); the lowercase config forms also
        // pass through so either reads fine.
        if let Some(button) =
            PadButton::from_config(name).or_else(|| PadButton::from_config(&name.to_lowercase()))
        {
            return Some(Self::Pad(button));
        }
        if let Some(key) = parse_key(name) {
            return Some(Self::Key(key));
        }
        let mouse = match name {
            "MouseLeft" => MouseButton::Left,
            "MouseRight" => MouseButton::Right,
            "MouseMiddle" => MouseButton::Middle,
            "MouseBack" => MouseButton::Back,
            "MouseForward" => MouseButton::Forward,
            _ => return None,
        };
        Some(Self::Mouse(mouse))
    }
}

/// Keyboard names follow bevy's own variant spellings. Full coverage of
/// the everyday keys; exotics can be added as they're wanted.
fn parse_key(name: &str) -> Option<KeyCode> {
    let code = match name {
        "Enter" => KeyCode::Enter,
        "Escape" => KeyCode::Escape,
        "Space" => KeyCode::Space,
        "Tab" => KeyCode::Tab,
        "Backspace" => KeyCode::Backspace,
        "ArrowUp" => KeyCode::ArrowUp,
        "ArrowDown" => KeyCode::ArrowDown,
        "ArrowLeft" => KeyCode::ArrowLeft,
        "ArrowRight" => KeyCode::ArrowRight,
        "ShiftLeft" => KeyCode::ShiftLeft,
        "ShiftRight" => KeyCode::ShiftRight,
        "ControlLeft" => KeyCode::ControlLeft,
        "ControlRight" => KeyCode::ControlRight,
        "AltLeft" => KeyCode::AltLeft,
        "AltRight" => KeyCode::AltRight,
        "Minus" => KeyCode::Minus,
        "Equal" => KeyCode::Equal,
        "Backquote" => KeyCode::Backquote,
        "Comma" => KeyCode::Comma,
        "Period" => KeyCode::Period,
        "Slash" => KeyCode::Slash,
        "Backslash" => KeyCode::Backslash,
        "Semicolon" => KeyCode::Semicolon,
        "Quote" => KeyCode::Quote,
        "BracketLeft" => KeyCode::BracketLeft,
        "BracketRight" => KeyCode::BracketRight,
        _ => return parse_prefixed_key(name),
    };
    Some(code)
}

/// "KeyA".."KeyZ", "Digit0".."Digit9", "Numpad0".."Numpad9", "F1".."F12".
fn parse_prefixed_key(name: &str) -> Option<KeyCode> {
    if let Some(c) = name.strip_prefix("Key") {
        return letter_key(c.chars().next()?);
    }
    if let Some(d) = name.strip_prefix("Digit") {
        return digit_code(d.chars().next()?.to_digit(10)? as usize);
    }
    if let Some(d) = name.strip_prefix("Numpad") {
        return numpad_code(d.chars().next()?.to_digit(10)? as usize);
    }
    if let Some(n) = name.strip_prefix("F") {
        return function_key(n.parse::<usize>().ok()?);
    }
    None
}

fn letter_key(c: char) -> Option<KeyCode> {
    Some(match c {
        'A' => KeyCode::KeyA,
        'B' => KeyCode::KeyB,
        'C' => KeyCode::KeyC,
        'D' => KeyCode::KeyD,
        'E' => KeyCode::KeyE,
        'F' => KeyCode::KeyF,
        'G' => KeyCode::KeyG,
        'H' => KeyCode::KeyH,
        'I' => KeyCode::KeyI,
        'J' => KeyCode::KeyJ,
        'K' => KeyCode::KeyK,
        'L' => KeyCode::KeyL,
        'M' => KeyCode::KeyM,
        'N' => KeyCode::KeyN,
        'O' => KeyCode::KeyO,
        'P' => KeyCode::KeyP,
        'Q' => KeyCode::KeyQ,
        'R' => KeyCode::KeyR,
        'S' => KeyCode::KeyS,
        'T' => KeyCode::KeyT,
        'U' => KeyCode::KeyU,
        'V' => KeyCode::KeyV,
        'W' => KeyCode::KeyW,
        'X' => KeyCode::KeyX,
        'Y' => KeyCode::KeyY,
        'Z' => KeyCode::KeyZ,
        _ => return None,
    })
}

fn digit_code(digit: usize) -> Option<KeyCode> {
    Some(match digit {
        0 => KeyCode::Digit0,
        1 => KeyCode::Digit1,
        2 => KeyCode::Digit2,
        3 => KeyCode::Digit3,
        4 => KeyCode::Digit4,
        5 => KeyCode::Digit5,
        6 => KeyCode::Digit6,
        7 => KeyCode::Digit7,
        8 => KeyCode::Digit8,
        9 => KeyCode::Digit9,
        _ => return None,
    })
}

fn numpad_code(digit: usize) -> Option<KeyCode> {
    Some(match digit {
        0 => KeyCode::Numpad0,
        1 => KeyCode::Numpad1,
        2 => KeyCode::Numpad2,
        3 => KeyCode::Numpad3,
        4 => KeyCode::Numpad4,
        5 => KeyCode::Numpad5,
        6 => KeyCode::Numpad6,
        7 => KeyCode::Numpad7,
        8 => KeyCode::Numpad8,
        9 => KeyCode::Numpad9,
        _ => return None,
    })
}

fn function_key(n: usize) -> Option<KeyCode> {
    Some(match n {
        1 => KeyCode::F1,
        2 => KeyCode::F2,
        3 => KeyCode::F3,
        4 => KeyCode::F4,
        5 => KeyCode::F5,
        6 => KeyCode::F6,
        7 => KeyCode::F7,
        8 => KeyCode::F8,
        9 => KeyCode::F9,
        10 => KeyCode::F10,
        11 => KeyCode::F11,
        12 => KeyCode::F12,
        _ => return None,
    })
}

/// Action → physical bindings.
#[derive(Clone, Debug, Default)]
pub struct Bindings {
    buttons: BTreeMap<PadButton, Vec<Raw>>,
}

impl Bindings {
    /// The shipped defaults, mirrored by `assets/input.ron`.
    pub fn standard() -> Self {
        let mut bindings = Self::default();
        bindings.bind(
            PadButton::Cross,
            &[
                Raw::Key(KeyCode::KeyZ),
                Raw::Key(KeyCode::Enter),
                Raw::Pad(PadButton::Cross),
            ],
        );
        bindings.bind(
            PadButton::Circle,
            &[Raw::Key(KeyCode::KeyX), Raw::Pad(PadButton::Circle)],
        );
        bindings.bind(
            PadButton::Triangle,
            &[Raw::Key(KeyCode::KeyI), Raw::Pad(PadButton::Triangle)],
        );
        bindings.bind(
            PadButton::Square,
            &[Raw::Key(KeyCode::KeyC), Raw::Pad(PadButton::Square)],
        );
        bindings.bind(
            PadButton::DPadUp,
            &[
                Raw::Key(KeyCode::KeyW),
                Raw::Key(KeyCode::ArrowUp),
                Raw::Pad(PadButton::DPadUp),
            ],
        );
        bindings.bind(
            PadButton::DPadDown,
            &[
                Raw::Key(KeyCode::KeyS),
                Raw::Key(KeyCode::ArrowDown),
                Raw::Pad(PadButton::DPadDown),
            ],
        );
        bindings.bind(
            PadButton::DPadLeft,
            &[
                Raw::Key(KeyCode::KeyA),
                Raw::Key(KeyCode::ArrowLeft),
                Raw::Pad(PadButton::DPadLeft),
            ],
        );
        bindings.bind(
            PadButton::DPadRight,
            &[
                Raw::Key(KeyCode::KeyD),
                Raw::Key(KeyCode::ArrowRight),
                Raw::Pad(PadButton::DPadRight),
            ],
        );
        bindings.bind(
            PadButton::L1,
            &[Raw::Key(KeyCode::KeyQ), Raw::Pad(PadButton::L1)],
        );
        bindings.bind(
            PadButton::R1,
            &[Raw::Key(KeyCode::KeyE), Raw::Pad(PadButton::R1)],
        );
        bindings.bind(PadButton::L2, &[Raw::Pad(PadButton::L2)]);
        bindings.bind(
            PadButton::R2,
            &[
                Raw::Key(KeyCode::ShiftLeft),
                Raw::Key(KeyCode::ShiftRight),
                Raw::Pad(PadButton::R2),
            ],
        );
        bindings.bind(PadButton::L3, &[Raw::Pad(PadButton::L3)]);
        bindings.bind(PadButton::R3, &[Raw::Pad(PadButton::R3)]);
        bindings.bind(PadButton::Start, &[Raw::Pad(PadButton::Start)]);
        bindings.bind(PadButton::Select, &[Raw::Pad(PadButton::Select)]);
        bindings
    }

    fn bind(&mut self, button: PadButton, raws: &[Raw]) {
        self.buttons.insert(button, raws.to_vec());
    }

    fn parse_config(buttons: BTreeMap<String, Vec<String>>) -> (Self, usize) {
        let mut bindings = Self::default();
        let mut skipped = 0;
        for (action, raws) in buttons {
            let Some(button) = PadButton::from_config(&action) else {
                warn!("input.ron: unknown action '{action}'");
                skipped += 1;
                continue;
            };
            let mut parsed = Vec::new();
            for raw in &raws {
                match Raw::from_config(raw) {
                    Some(raw) => parsed.push(raw),
                    None => {
                        warn!("input.ron: unknown binding '{raw}' for {action}");
                        skipped += 1;
                    }
                }
            }
            bindings.buttons.insert(button, parsed);
        }
        (bindings, skipped)
    }

    fn active(
        &self,
        keys: &ButtonInput<KeyCode>,
        mouse: &ButtonInput<MouseButton>,
        pad: &ButtonInput<GamepadButton>,
    ) -> BTreeSet<PadButton> {
        let mut active = BTreeSet::new();
        for (button, raws) in &self.buttons {
            if raws.iter().any(|raw| match *raw {
                Raw::Key(key) => keys.pressed(key),
                Raw::Mouse(mouse_button) => mouse.pressed(mouse_button),
                Raw::Pad(pad_button) => pad.pressed(pad_button.to_gamepad()),
            }) {
                active.insert(*button);
            }
        }
        active
    }
}

/// The per-action state all readers share.
#[derive(Clone, Debug, Default)]
pub struct InputState {
    pressed: BTreeSet<PadButton>,
    just_pressed: BTreeSet<PadButton>,
    just_released: BTreeSet<PadButton>,
    axes: BTreeMap<PadAxis, f32>,
}

impl InputState {
    pub fn pressed(&self, button: PadButton) -> bool {
        self.pressed.contains(&button)
    }

    pub fn just_pressed(&self, button: PadButton) -> bool {
        self.just_pressed.contains(&button)
    }

    pub fn just_released(&self, button: PadButton) -> bool {
        self.just_released.contains(&button)
    }

    pub fn axis(&self, axis: PadAxis) -> f32 {
        self.axes.get(&axis).copied().unwrap_or(0.0)
    }

    /// Name-based lookups for the script API; `None` is an unknown name.
    pub fn pressed_by_name(&self, name: &str) -> Option<bool> {
        Some(self.pressed(PadButton::from_config(name)?))
    }

    pub fn just_pressed_by_name(&self, name: &str) -> Option<bool> {
        Some(self.just_pressed(PadButton::from_config(name)?))
    }

    pub fn just_released_by_name(&self, name: &str) -> Option<bool> {
        Some(self.just_released(PadButton::from_config(name)?))
    }

    pub fn axis_by_name(&self, name: &str) -> Option<f32> {
        Some(self.axis(PadAxis::from_config(name)?))
    }

    #[cfg(test)]
    pub(crate) fn inject(
        &mut self,
        pressed: &[PadButton],
        just_pressed: &[PadButton],
        axes: &[(PadAxis, f32)],
    ) {
        self.pressed = pressed.iter().copied().collect();
        self.just_pressed = just_pressed.iter().copied().collect();
        self.axes = axes.iter().copied().collect();
    }
}

/// A handle to a never-updated state: for scripts compiled outside the
/// running game (tests, detached content).
pub fn detached() -> InputHandle {
    Arc::new(Mutex::new(InputState::default()))
}

/// Aggregated input for one fixed tick. Systems read the manager;
/// script engines hold a [`InputHandle`] to the same state.
#[derive(Resource)]
pub struct InputManager {
    bindings: Bindings,
    state: InputHandle,
    previous: BTreeSet<PadButton>,
}

impl InputManager {
    /// The shipped defaults; `assets/input.ron` isn't read.
    pub fn standard() -> Self {
        Self::with_bindings(Bindings::standard())
    }

    /// Loads `assets/input.ron`, falling back to the standard bindings
    /// per-entry: unknown names warn and keep the default binding.
    pub fn load() -> Self {
        let path = assets_root().join("input.ron");
        let bindings = match std::fs::read_to_string(&path) {
            Ok(text) => match ron::from_str::<InputConfigFile>(&text) {
                Ok(config) => {
                    let (bindings, skipped) = Bindings::parse_config(config.buttons);
                    if skipped > 0 {
                        warn!("input.ron: {skipped} unusable entries fell back to defaults");
                    }
                    bindings
                }
                Err(e) => {
                    warn!("input.ron failed to parse, using defaults: {e}");
                    return Self::standard();
                }
            },
            Err(e) => {
                warn!("input.ron could not be read, using defaults: {e}");
                return Self::standard();
            }
        };
        Self::with_bindings(bindings)
    }

    fn with_bindings(bindings: Bindings) -> Self {
        Self {
            bindings,
            state: detached(),
            previous: BTreeSet::new(),
        }
    }

    /// The handle script engines bind their host functions against.
    pub fn handle(&self) -> InputHandle {
        self.state.clone()
    }

    pub fn pressed(&self, button: PadButton) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pressed(button)
    }

    pub fn just_pressed(&self, button: PadButton) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .just_pressed(button)
    }

    /// The gated movement vector: dpad bits plus the left stick, each
    /// stick direction counting only past the deadzone.
    pub fn movement(&self) -> Vec2 {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        movement_vector(&state.pressed, &state.axes)
    }

    /// Rebuilds the shared state from raw device resources.
    pub fn aggregate(
        &mut self,
        keys: &ButtonInput<KeyCode>,
        mouse: &ButtonInput<MouseButton>,
        pad_buttons: &ButtonInput<GamepadButton>,
        pad_axes: &Axis<GamepadAxis>,
    ) {
        let pressed = self.bindings.active(keys, mouse, pad_buttons);
        let just_pressed = &pressed - &self.previous;
        let just_released = &self.previous - &pressed;
        let axes: BTreeMap<PadAxis, f32> = PadAxis::ALL
            .into_iter()
            .map(|axis| (axis, pad_axes.get(axis.to_gamepad()).unwrap_or(0.0)))
            .collect();
        {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            *state = InputState {
                pressed: pressed.clone(),
                just_pressed,
                just_released,
                axes,
            };
            self.previous = state.pressed.clone();
        }
    }
}

#[derive(serde::Deserialize)]
struct InputConfigFile {
    #[serde(default)]
    buttons: BTreeMap<String, Vec<String>>,
}

/// The gameplay movement vector: dpad bits plus the gated left stick.
pub fn movement_vector(pressed: &BTreeSet<PadButton>, axes: &BTreeMap<PadAxis, f32>) -> Vec2 {
    let mut vector = Vec2::ZERO;
    if pressed.contains(&PadButton::DPadLeft) {
        vector.x -= 1.0;
    }
    if pressed.contains(&PadButton::DPadRight) {
        vector.x += 1.0;
    }
    if pressed.contains(&PadButton::DPadUp) {
        vector.y += 1.0;
    }
    if pressed.contains(&PadButton::DPadDown) {
        vector.y -= 1.0;
    }
    let stick_x = digital(axes.get(&PadAxis::LeftStickX).copied().unwrap_or(0.0));
    let stick_y = digital(axes.get(&PadAxis::LeftStickY).copied().unwrap_or(0.0));
    vector.x += stick_x;
    vector.y += stick_y;
    vector
}

/// Sticks are position sensors; most gameplay wants a direction. Full
/// deflection past the deadzone reads as a full press.
pub fn digital(v: f32) -> f32 {
    if v.abs() >= 0.2 { v.signum() } else { 0.0 }
}

/// Rebuilds the shared input state from raw devices; runs at the head
/// of `FixedUpdate` so gameplay and scripts always see this tick's
/// fresh edges.
#[allow(clippy::type_complexity)]
pub fn aggregate_inputs(
    mut manager: ResMut<InputManager>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    pad_buttons: Res<ButtonInput<GamepadButton>>,
    pad_axes: Res<Axis<GamepadAxis>>,
) {
    manager.aggregate(&keys, &mouse, &pad_buttons, &pad_axes);
}

#[cfg(test)]
impl InputManager {
    /// A manager whose state claims these buttons were just pressed —
    /// for driving systems that read input without raw devices.
    pub(crate) fn with_just_pressed(buttons: &[PadButton]) -> Self {
        let manager = Self::standard();
        let mut state = manager.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.just_pressed = buttons.iter().copied().collect();
        state.pressed = buttons.iter().copied().collect();
        drop(state);
        manager
    }
}

#[cfg(test)]
mod tests {
    use bevy::input::ButtonInput;

    use super::*;

    fn lock(handle: &InputHandle) -> std::sync::MutexGuard<'_, InputState> {
        handle.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[test]
    fn config_names_parse_round_trip() {
        assert_eq!(PadButton::from_config("cross"), Some(PadButton::Cross));
        assert_eq!(PadButton::from_config("dpad_up"), Some(PadButton::DPadUp));
        assert_eq!(PadButton::from_config("r2"), Some(PadButton::R2));
        assert_eq!(PadButton::from_config("south"), None);
        assert_eq!(
            PadAxis::from_config("left_stick_x"),
            Some(PadAxis::LeftStickX)
        );

        assert_eq!(Raw::from_config("KeyZ"), Some(Raw::Key(KeyCode::KeyZ)));
        assert_eq!(
            Raw::from_config("ArrowUp"),
            Some(Raw::Key(KeyCode::ArrowUp))
        );
        assert_eq!(Raw::from_config("F5"), Some(Raw::Key(KeyCode::F5)));
        assert_eq!(Raw::from_config("Digit3"), Some(Raw::Key(KeyCode::Digit3)));
        assert_eq!(
            Raw::from_config("Numpad7"),
            Some(Raw::Key(KeyCode::Numpad7))
        );
        assert_eq!(
            Raw::from_config("MouseLeft"),
            Some(Raw::Mouse(MouseButton::Left))
        );
        assert_eq!(Raw::from_config("Cross"), Some(Raw::Pad(PadButton::Cross)));
        assert_eq!(Raw::from_config("Bogus"), None);
        assert_eq!(
            Raw::from_config("Key1"),
            None,
            "digits are Digit1, not Key1"
        );
    }

    #[test]
    fn standard_bindings_keep_todays_keyboard() {
        let bindings = Bindings::standard();
        // Movement: WASD + arrows on the dpad; run on Shift via R2.
        assert!(bindings.buttons[&PadButton::DPadUp].contains(&Raw::Key(KeyCode::ArrowUp)));
        assert!(bindings.buttons[&PadButton::DPadUp].contains(&Raw::Key(KeyCode::KeyW)));
        assert!(bindings.buttons[&PadButton::R2].contains(&Raw::Key(KeyCode::ShiftLeft)));
        // Confirm: Z / Enter / the cross itself.
        assert!(bindings.buttons[&PadButton::Cross].contains(&Raw::Key(KeyCode::KeyZ)));
        assert!(bindings.buttons[&PadButton::Cross].contains(&Raw::Pad(PadButton::Cross)));
    }

    #[test]
    fn edges_fire_once_across_ticks() {
        let mut manager = InputManager::standard();
        let mut keys = ButtonInput::default();
        let mouse = ButtonInput::default();
        let pad = ButtonInput::default();
        let mut axes = Axis::default();
        axes.set(PadAxis::LeftStickX.to_gamepad(), 0.9);

        let handle = manager.handle();

        // Press: the edge, the held state, and the raw axis all land.
        keys.press(KeyCode::KeyZ);
        manager.aggregate(&keys, &mouse, &pad, &axes);
        assert!(manager.just_pressed(PadButton::Cross));
        assert!(manager.pressed(PadButton::Cross));
        assert_eq!(lock(&handle).axis_by_name("left_stick_x"), Some(0.9));

        // Hold: no new edge.
        manager.aggregate(&keys, &mouse, &pad, &axes);
        assert!(!manager.just_pressed(PadButton::Cross));
        assert!(manager.pressed(PadButton::Cross));

        // Release: the release edge fires exactly once.
        keys.release(KeyCode::KeyZ);
        manager.aggregate(&keys, &mouse, &pad, &axes);
        assert!(lock(&handle).just_released(PadButton::Cross));
        assert!(!manager.pressed(PadButton::Cross));
        manager.aggregate(&keys, &mouse, &pad, &axes);
        assert!(!lock(&handle).just_released(PadButton::Cross));
    }

    #[test]
    fn pad_buttons_and_axes_flow_through_the_mapping() {
        let mut manager = InputManager::standard();
        let keys = ButtonInput::default();
        let mouse = ButtonInput::default();
        let mut pad = ButtonInput::default();
        let mut axes = Axis::default();
        pad.press(GamepadButton::South);
        pad.press(GamepadButton::RightTrigger2);
        axes.set(PadAxis::LeftStickY.to_gamepad(), -0.95);

        manager.aggregate(&keys, &mouse, &pad, &axes);
        assert!(manager.just_pressed(PadButton::Cross), "South is the cross");
        assert!(manager.pressed(PadButton::R2), "RightTrigger2 is R2");
        let handle = manager.handle();
        assert_eq!(lock(&handle).axis_by_name("left_stick_y"), Some(-0.95));
    }

    #[test]
    fn the_movement_vector_combines_dpad_and_gated_stick() {
        let mut pressed = BTreeSet::new();
        let mut axes = BTreeMap::new();
        assert_eq!(movement_vector(&pressed, &axes), Vec2::ZERO);

        pressed.insert(PadButton::DPadRight);
        pressed.insert(PadButton::DPadUp);
        assert_eq!(movement_vector(&pressed, &axes), Vec2::new(1.0, 1.0));

        // A soft stick deflects to nothing; a firm one counts once.
        axes.insert(PadAxis::LeftStickX, 0.15);
        assert_eq!(movement_vector(&pressed, &axes), Vec2::new(1.0, 1.0));
        axes.insert(PadAxis::LeftStickX, 0.9);
        assert_eq!(movement_vector(&pressed, &axes), Vec2::new(2.0, 1.0));
    }

    #[test]
    fn the_shared_handle_sees_every_aggregate() {
        let mut manager = InputManager::standard();
        let handle = manager.handle();
        let mut keys = ButtonInput::default();
        let mouse = ButtonInput::default();
        let pad = ButtonInput::default();
        let axes = Axis::default();

        keys.press(KeyCode::Enter);
        manager.aggregate(&keys, &mouse, &pad, &axes);

        let state = lock(&handle);

        assert!(state.just_pressed_by_name("cross").unwrap());
        assert_eq!(state.axis_by_name("left_stick_x"), Some(0.0));
        assert_eq!(state.pressed_by_name("triangle"), Some(false));
        assert_eq!(state.pressed_by_name("y2"), None, "unknown names are None");
    }
}
