//! Hardware keyboards and mice on iPadOS, which winit 0.30's iOS backend leaves out: keys and the
//! mouse from the GameController framework (`GCKeyboard`, `GCMouse`), the pointer lock, and which
//! touches the pointer made rather than a finger. On other systems nothing is ever connected.
mod hid;
#[cfg(target_os = "ios")]
mod ios;

pub use hid::{key_code, named_key, us_char};
#[cfg(target_os = "ios")]
pub use ios::{
    key_event, keyboard_connected, mouse_connected, pointer_lock_state, pointer_touch, poll,
    set_pointer_locked, watch_pointer,
};
use winit::event::MouseButton;

/// One input from a hardware keyboard or mouse, in the order they came.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Input {
    /// A key by its HID usage (keyboard page), the number `GCKeyCode` gives it.
    Key { usage: u16, pressed: bool },
    /// The mouse moved, in GameController's units, y down: whether or not the pointer is locked.
    Motion { dx: f64, dy: f64 },
    /// GameController's button, whether or not the pointer is locked.
    Button { button: MouseButton, pressed: bool },
    /// GameController's wheel, in lines, in winit's `LineDelta` directions.
    Wheel { dx: f64, dy: f64 },
    /// Where the system's pointer is, in window pixels, while it isn't locked: hovering, or
    /// dragging with a button held.
    Pointer { x: f64, y: f64 },
    /// A button of the system's pointer pressed or let go, while it isn't locked, where the
    /// pointer is then (UIKit's pointer touch and its event's `buttonMask`).
    Click {
        x: f64,
        y: f64,
        button: MouseButton,
        pressed: bool,
    },
    /// UIKit's scrolling (the wheel, two fingers on a trackpad) while the pointer isn't locked, in
    /// window pixels, in winit's `PixelDelta` directions and the system's scroll direction.
    Scroll { dx: f64, dy: f64 },
}

#[cfg(not(target_os = "ios"))]
mod other {
    use super::Input;
    use winit::window::Window;

    pub fn poll() -> Vec<Input> {
        Vec::new()
    }
    pub fn keyboard_connected() -> bool {
        false
    }
    pub fn mouse_connected() -> bool {
        false
    }
    pub fn set_pointer_locked(_: &Window, _: bool) {}
    pub fn pointer_lock_state(_: &Window) -> Option<bool> {
        None
    }
    pub fn watch_pointer(_: &Window) {}
    pub fn pointer_touch(_: u64, _: bool) -> bool {
        false
    }
}
#[cfg(not(target_os = "ios"))]
pub use other::*;
