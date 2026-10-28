//! Input state: keyboard, mouse buttons, cursor and wheel.
//!
//! The platform layer owns the winit types; the rest of the engine sees a small, stable
//! [`Key`] enum and a snapshot of the current frame. Edge state (`pressed`/`released`) is
//! cleared after the frame is presented, so a system can ask "was Q pressed this frame?"
//! without consuming an event queue.

use moba_core::bitset::BitSet64;
use moba_core::math::Vec2;
use winit::keyboard::KeyCode;

/// Physical keys the engine understands.
///
/// These are intentionally physical (layout-independent) bindings: `W` is the key labelled
/// W on a QWERTY keyboard, which is what MOBA players expect for movement.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
#[non_exhaustive]
pub enum Key {
    /// Move up / W key.
    W,
    /// Move left / A key.
    A,
    /// Move down / S key.
    S,
    /// Move right / D key.
    D,
    /// Ability Q.
    Q,
    /// Ability E.
    E,
    /// Reserved ability R.
    R,
    /// Reserved key F.
    F,
    /// Open shop.
    B,
    /// Reserved map key.
    M,
    /// Digit 1 (shop slot 1).
    Digit1,
    /// Digit 2 (shop slot 2).
    Digit2,
    /// Digit 3 (shop slot 3).
    Digit3,
    /// Digit 4 (shop slot 4).
    Digit4,
    /// Digit 5 (shop slot 5).
    Digit5,
    /// Digit 6 (shop slot 6).
    Digit6,
    /// Digit 7 (shop slot 7).
    Digit7,
    /// Digit 8 (shop slot 8).
    Digit8,
    /// Stop orders.
    Space,
    /// Shift modifier.
    Shift,
    /// Control modifier.
    Ctrl,
    /// Alt modifier.
    Alt,
    /// Toggle scoreboard.
    Tab,
    /// Quit / close overlay.
    Escape,
    /// Confirm.
    Enter,
    /// Backspace.
    Backspace,
    /// UI navigation up.
    ArrowUp,
    /// UI navigation down.
    ArrowDown,
    /// UI navigation left.
    ArrowLeft,
    /// UI navigation right.
    ArrowRight,
    /// Function key F1.
    F1,
    /// Function key F2.
    F2,
    /// Function key F3.
    F3,
    /// Function key F4.
    F4,
    /// Function key F5.
    F5,
}

impl Key {
    /// Number of variants; used to size bit sets.
    pub const COUNT: usize = 35;

    /// Dense index of this key.
    #[inline]
    #[must_use]
    pub const fn index(self) -> u32 {
        self as u32
    }
}

/// Mouse buttons the engine understands.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
#[non_exhaustive]
pub enum MouseButton {
    /// Primary mouse button (attack/select).
    Left,
    /// Secondary mouse button (move).
    Right,
    /// Middle mouse button.
    Middle,
    /// Back mouse button.
    Back,
    /// Forward mouse button.
    Forward,
}

impl MouseButton {
    /// Number of variants.
    pub const COUNT: usize = 5;

    /// Dense index of this button.
    #[inline]
    #[must_use]
    pub const fn index(self) -> u32 {
        self as u32
    }
}

/// Maps a winit key code to an engine key, if the key is bound.
#[must_use]
pub(crate) fn map_key(code: KeyCode) -> Option<Key> {
    Some(match code {
        KeyCode::KeyW => Key::W,
        KeyCode::KeyA => Key::A,
        KeyCode::KeyS => Key::S,
        KeyCode::KeyD => Key::D,
        KeyCode::KeyQ => Key::Q,
        KeyCode::KeyE => Key::E,
        KeyCode::KeyR => Key::R,
        KeyCode::KeyF => Key::F,
        KeyCode::KeyB => Key::B,
        KeyCode::KeyM => Key::M,
        KeyCode::Digit1 => Key::Digit1,
        KeyCode::Digit2 => Key::Digit2,
        KeyCode::Digit3 => Key::Digit3,
        KeyCode::Digit4 => Key::Digit4,
        KeyCode::Digit5 => Key::Digit5,
        KeyCode::Digit6 => Key::Digit6,
        KeyCode::Digit7 => Key::Digit7,
        KeyCode::Digit8 => Key::Digit8,
        KeyCode::Space => Key::Space,
        KeyCode::ShiftLeft | KeyCode::ShiftRight => Key::Shift,
        KeyCode::ControlLeft | KeyCode::ControlRight => Key::Ctrl,
        KeyCode::AltLeft | KeyCode::AltRight => Key::Alt,
        KeyCode::Tab => Key::Tab,
        KeyCode::Escape => Key::Escape,
        KeyCode::Enter => Key::Enter,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::ArrowUp => Key::ArrowUp,
        KeyCode::ArrowDown => Key::ArrowDown,
        KeyCode::ArrowLeft => Key::ArrowLeft,
        KeyCode::ArrowRight => Key::ArrowRight,
        KeyCode::F1 => Key::F1,
        KeyCode::F2 => Key::F2,
        KeyCode::F3 => Key::F3,
        KeyCode::F4 => Key::F4,
        KeyCode::F5 => Key::F5,
        _ => return None,
    })
}

/// Maps a winit mouse button to an engine button.
#[must_use]
pub(crate) fn map_mouse_button(button: winit::event::MouseButton) -> Option<MouseButton> {
    Some(match button {
        winit::event::MouseButton::Left => MouseButton::Left,
        winit::event::MouseButton::Right => MouseButton::Right,
        winit::event::MouseButton::Middle => MouseButton::Middle,
        winit::event::MouseButton::Back => MouseButton::Back,
        winit::event::MouseButton::Forward => MouseButton::Forward,
        winit::event::MouseButton::Other(_) => return None,
    })
}

/// Per-frame input snapshot.
///
/// The struct is plain data: no allocation, no locks, cheap to query from systems.
#[derive(Clone, Debug, Default)]
pub struct InputState {
    keys_down: BitSet64,
    keys_pressed: BitSet64,
    keys_released: BitSet64,
    mouse_down: BitSet64,
    mouse_pressed: BitSet64,
    mouse_released: BitSet64,
    cursor: Option<Vec2>,
    wheel_y: f32,
    focused: bool,
}

impl InputState {
    /// Creates an empty input state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` while a key is held.
    #[inline]
    #[must_use]
    pub fn is_key_down(&self, key: Key) -> bool {
        self.keys_down.contains(key.index())
    }

    /// Returns `true` on the frame a key went down.
    #[inline]
    #[must_use]
    pub fn was_key_pressed(&self, key: Key) -> bool {
        self.keys_pressed.contains(key.index())
    }

    /// Returns `true` on the frame a key went up.
    #[inline]
    #[must_use]
    pub fn was_key_released(&self, key: Key) -> bool {
        self.keys_released.contains(key.index())
    }

    /// Returns `true` while a mouse button is held.
    #[inline]
    #[must_use]
    pub fn is_mouse_down(&self, button: MouseButton) -> bool {
        self.mouse_down.contains(button.index())
    }

    /// Returns `true` on the frame a mouse button went down.
    #[inline]
    #[must_use]
    pub fn was_mouse_pressed(&self, button: MouseButton) -> bool {
        self.mouse_pressed.contains(button.index())
    }

    /// Returns `true` on the frame a mouse button went up.
    #[inline]
    #[must_use]
    pub fn was_mouse_released(&self, button: MouseButton) -> bool {
        self.mouse_released.contains(button.index())
    }

    /// Cursor position in physical pixels relative to the window, if it is inside.
    #[inline]
    #[must_use]
    pub fn cursor(&self) -> Option<Vec2> {
        self.cursor
    }

    /// Returns a copy with the cursor divided by `scale` for logical UI hit-testing.
    #[must_use]
    pub fn scaled_cursor(&self, scale: f32) -> Self {
        let mut scaled = self.clone();
        if scale > 0.0 {
            scaled.cursor = scaled
                .cursor
                .map(|cursor| Vec2::new(cursor.x / scale, cursor.y / scale));
        }
        scaled
    }

    /// Accumulated vertical wheel delta for this frame.
    ///
    /// Positive is scroll up. Line-based platforms report 120 units per notch.
    #[inline]
    #[must_use]
    pub fn wheel_y(&self) -> f32 {
        self.wheel_y
    }

    /// Returns `true` if the window currently has keyboard focus.
    #[inline]
    #[must_use]
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Convenience: `Shift` held.
    #[inline]
    #[must_use]
    pub fn shift(&self) -> bool {
        self.is_key_down(Key::Shift)
    }

    /// Convenience: `Ctrl` held.
    #[inline]
    #[must_use]
    pub fn ctrl(&self) -> bool {
        self.is_key_down(Key::Ctrl)
    }

    /// Convenience: `Alt` held.
    #[inline]
    #[must_use]
    pub fn alt(&self) -> bool {
        self.is_key_down(Key::Alt)
    }

    pub(crate) fn handle_key(&mut self, key: Key, pressed: bool, repeat: bool) {
        let index = key.index();
        if pressed {
            if !repeat && !self.keys_down.contains(index) {
                self.keys_pressed.insert(index);
            }
            self.keys_down.insert(index);
        } else if self.keys_down.remove(index) {
            self.keys_released.insert(index);
        }
    }

    pub(crate) fn handle_mouse_button(&mut self, button: MouseButton, pressed: bool) {
        let index = button.index();
        if pressed {
            if !self.mouse_down.contains(index) {
                self.mouse_pressed.insert(index);
            }
            self.mouse_down.insert(index);
        } else if self.mouse_down.remove(index) {
            self.mouse_released.insert(index);
        }
    }

    pub(crate) fn handle_cursor(&mut self, position: Vec2) {
        self.cursor = Some(position);
    }

    pub(crate) fn handle_wheel(&mut self, delta_y: f32) {
        self.wheel_y += delta_y;
    }

    pub(crate) fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        if !focused {
            // Focus loss can swallow the matching key-up events. Treat every held key as
            // released so the simulation cannot get stuck moving in one direction.
            for key in 0..Key::COUNT as u32 {
                if self.keys_down.remove(key) {
                    self.keys_released.insert(key);
                }
            }
            for button in 0..MouseButton::COUNT as u32 {
                if self.mouse_down.remove(button) {
                    self.mouse_released.insert(button);
                }
            }
        }
    }

    pub(crate) fn clear_edges(&mut self) {
        self.keys_pressed = BitSet64::EMPTY;
        self.keys_released = BitSet64::EMPTY;
        self.mouse_pressed = BitSet64::EMPTY;
        self.mouse_released = BitSet64::EMPTY;
        self.wheel_y = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_mapping_covers_bindings() {
        assert_eq!(map_key(KeyCode::KeyW), Some(Key::W));
        assert_eq!(map_key(KeyCode::ShiftLeft), Some(Key::Shift));
        assert_eq!(map_key(KeyCode::ShiftRight), Some(Key::Shift));
        assert_eq!(map_key(KeyCode::Digit4), Some(Key::Digit4));
        assert_eq!(map_key(KeyCode::KeyZ), None);
        assert!(Key::COUNT as u32 > Key::F5.index());
    }

    #[test]
    fn scaled_cursor_maps_physical_to_logical_coordinates() {
        let input = InputState {
            cursor: Some(Vec2::new(400.0, 300.0)),
            ..InputState::new()
        };
        let scaled = input.scaled_cursor(2.0);
        assert_eq!(scaled.cursor(), Some(Vec2::new(200.0, 150.0)));
        assert_eq!(input.cursor(), Some(Vec2::new(400.0, 300.0)));
        assert_eq!(input.scaled_cursor(0.0).cursor(), input.cursor());
    }

    #[test]
    fn press_edges_ignore_autorepeat() {
        let mut input = InputState::new();
        input.handle_key(Key::Q, true, false);
        assert!(input.is_key_down(Key::Q));
        assert!(input.was_key_pressed(Key::Q));
        assert!(!input.was_key_released(Key::Q));

        input.clear_edges();
        assert!(input.is_key_down(Key::Q));
        assert!(!input.was_key_pressed(Key::Q));

        input.handle_key(Key::Q, true, true);
        assert!(
            !input.was_key_pressed(Key::Q),
            "autorepeat must not re-trigger"
        );

        input.handle_key(Key::Q, false, false);
        assert!(!input.is_key_down(Key::Q));
        assert!(input.was_key_released(Key::Q));
        input.handle_key(Key::Q, false, false);
        input.clear_edges();
        input.handle_key(Key::Q, false, false);
        assert!(!input.was_key_released(Key::Q));
    }

    #[test]
    fn focus_loss_releases_held_input() {
        let mut input = InputState::new();
        input.set_focused(true);
        input.handle_key(Key::W, true, false);
        input.handle_mouse_button(MouseButton::Left, true);
        input.set_focused(false);
        assert!(!input.is_key_down(Key::W));
        assert!(input.was_key_released(Key::W));
        assert!(!input.is_mouse_down(MouseButton::Left));
        assert!(input.was_mouse_released(MouseButton::Left));
        assert!(!input.is_focused());
    }

    #[test]
    fn mouse_and_wheel_accumulate() {
        let mut input = InputState::new();
        input.handle_mouse_button(MouseButton::Right, true);
        assert!(input.is_mouse_down(MouseButton::Right));
        assert!(input.was_mouse_pressed(MouseButton::Right));
        input.handle_cursor(Vec2::new(10.0, 20.0));
        assert_eq!(input.cursor(), Some(Vec2::new(10.0, 20.0)));
        input.handle_wheel(120.0);
        input.handle_wheel(120.0);
        assert_eq!(input.wheel_y(), 240.0);
        input.clear_edges();
        assert_eq!(input.wheel_y(), 0.0);
    }
}
