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
    set_pointer_locked, watch_pointer_touches,
};
use winit::event::MouseButton;

/// One input from a hardware keyboard or mouse, in the order they came.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Input {
    /// A key by its HID usage (keyboard page), the number `GCKeyCode` gives it.
    Key {
        usage: u16,
        pressed: bool,
    },
    /// The mouse moved, in GameController's units, y down.
    Motion {
        dx: f64,
        dy: f64,
    },
    Button {
        button: MouseButton,
        pressed: bool,
    },
    /// The wheel or a two-finger scroll, in lines, in winit's `LineDelta` directions.
    Scroll {
        dx: f64,
        dy: f64,
    },
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
    pub fn watch_pointer_touches(_: &Window) {}
    pub fn pointer_touch(_: u64, _: bool) -> bool {
        false
    }
}
#[cfg(not(target_os = "ios"))]
pub use other::*;
