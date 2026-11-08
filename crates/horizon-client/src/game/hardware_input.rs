//! Hardware keyboards and mice on iPadOS (`horizon-ios-input`), turned into the window events the
//! desktop's keyboard and mouse send, so every screen and binding takes them as it does there.
//!
//! The pointer stays locked while a mouse is in use: in play the mouse's movement turns the view
//! (`MouseHandler.onMove`, as the desktop's raw motion), and over screens it moves the game's own
//! cursor, which the game draws, so hover works without a button held. Keys come from
//! GameController; typed text comes from UIKit (winit's `insertText:`), which knows the layout,
//! while a text field has the keyboard focus. Touching the screen hands control back to the touch
//! controls.
use super::Game;
use super::event_loop::EventLoopCtl;
use super::loading_screen::Loading;
use horizon_client::hud::{HudAtlas, HudBatch, rgba};
use horizon_ios_input::Input;
use std::time::{Duration, Instant};
use winit::dpi::PhysicalPosition;
use winit::event::{DeviceId, ElementState, MouseScrollDelta, TouchPhase, WindowEvent};
use winit::keyboard::{KeyCode, ModifiersState};
use winit::window::{Window, WindowId};

/// A held key starts repeating after this, then repeats this often (iPadOS' defaults).
const REPEAT_DELAY: Duration = Duration::from_millis(500);
const REPEAT_EVERY: Duration = Duration::from_millis(50);
/// How far the mouse moves (points) before it takes over from the touch controls: a palm
/// brushing the trackpad while playing by touch does nothing.
const WAKE_MOTION: f64 = 40.0;

#[derive(Default)]
pub(super) struct HardwareInput {
    /// A keyboard or mouse is what the player uses now: the touch controls hide.
    active: bool,
    held: Vec<KeyCode>,
    modifiers: ModifiersState,
    repeat: Option<(KeyCode, Instant)>,
    lock_wanted: bool,
    lock_seen: Option<bool>,
    /// Mouse movement while the touch controls are in use, towards [`WAKE_MOTION`].
    stray_motion: f64,
}

/// Where the pointer is for the input being translated.
#[derive(Clone, Copy)]
pub(super) struct Pointer {
    /// The game holds the pointer: the mouse turns the view.
    pub(super) grabbed: bool,
    /// The cursor, in window pixels.
    pub(super) at: (f64, f64),
    pub(super) size: [u32; 2],
    pub(super) scale_factor: f64,
}

pub(super) enum Out {
    Event(WindowEvent),
    /// Raw mouse motion with the pointer grabbed (`DeviceEvent::MouseMotion`).
    Look(f64, f64),
}

fn modifier(code: KeyCode) -> Option<ModifiersState> {
    Some(match code {
        KeyCode::ShiftLeft | KeyCode::ShiftRight => ModifiersState::SHIFT,
        KeyCode::ControlLeft | KeyCode::ControlRight => ModifiersState::CONTROL,
        KeyCode::AltLeft | KeyCode::AltRight => ModifiersState::ALT,
        KeyCode::SuperLeft | KeyCode::SuperRight => ModifiersState::SUPER,
        _ => return None,
    })
}

impl HardwareInput {
    pub(super) fn active(&self) -> bool {
        self.active
    }

    /// A finger touched the screen: the touch controls take over.
    pub(super) fn touched(&mut self) {
        if self.active {
            log::info!("Horizon: touch is the input in use");
        }
        self.active = false;
        self.stray_motion = 0.0;
    }

    /// The game draws the cursor: a mouse in use and the pointer not grabbed.
    pub(super) fn cursor_shown(&self, grabbed: bool) -> bool {
        self.active && !grabbed && horizon_ios_input::mouse_connected()
    }

    fn key(&self, code: KeyCode, state: ElementState, repeat: bool, text: bool) -> Option<Out> {
        #[cfg(target_os = "ios")]
        {
            use winit::keyboard::{Key, NativeKey, SmolStr};
            let shift = self.modifiers.shift_key();
            let typed = horizon_ios_input::us_char(code, shift);
            let logical = match (horizon_ios_input::named_key(code), typed) {
                (Some(named), _) => Key::Named(named),
                (None, Some(c)) => Key::Character(SmolStr::from(c.encode_utf8(&mut [0; 4]))),
                (None, None) => Key::Unidentified(NativeKey::Unidentified),
            };
            let shortcut = self.modifiers.control_key() || self.modifiers.super_key();
            let text = typed
                .filter(|_| text && !shortcut && state == ElementState::Pressed)
                .map(|c| SmolStr::from(c.encode_utf8(&mut [0; 4])));
            Some(Out::Event(WindowEvent::KeyboardInput {
                device_id: DeviceId::dummy(),
                event: horizon_ios_input::key_event(code, logical, text, state, repeat),
                is_synthetic: false,
            }))
        }
        #[cfg(not(target_os = "ios"))]
        {
            let _ = (code, state, repeat, text);
            None
        }
    }

    fn press(&mut self, code: KeyCode, pressed: bool, text: bool, out: &mut Vec<Out>) {
        let state = if pressed {
            ElementState::Pressed
        } else {
            ElementState::Released
        };
        if pressed {
            if self.held.contains(&code) {
                return;
            }
            self.held.push(code);
        } else {
            if !self.held.contains(&code) {
                return;
            }
            self.held.retain(|&k| k != code);
        }
        if modifier(code).is_some() {
            let modifiers = self
                .held
                .iter()
                .filter_map(|&k| modifier(k))
                .fold(ModifiersState::empty(), |all, m| all | m);
            if modifiers != self.modifiers {
                self.modifiers = modifiers;
                out.push(Out::Event(WindowEvent::ModifiersChanged(modifiers.into())));
            }
        } else if pressed {
            self.repeat = Some((code, Instant::now() + REPEAT_DELAY));
        } else if self.repeat.is_some_and(|(k, _)| k == code) {
            self.repeat = None;
        }
        out.extend(self.key(code, state, false, text));
    }

    /// One input as window events. `text`: the keys carry the characters they type on a US
    /// layout, for a window UIKit sends no text to.
    pub(super) fn translate(&mut self, input: Input, pointer: Pointer, text: bool) -> Vec<Out> {
        let mut out = Vec::new();
        // A key let go still reaches the game, whichever input is in use.
        let release = matches!(input, Input::Key { pressed: false, .. });
        if !self.active && !release {
            let wakes = match input {
                Input::Key { .. } => true,
                Input::Motion { dx, dy } => {
                    self.stray_motion += dx.abs() + dy.abs();
                    self.stray_motion >= WAKE_MOTION
                }
                Input::Button { .. } | Input::Scroll { .. } => true,
            };
            if !wakes {
                return out;
            }
            log::info!("Horizon: keyboard and mouse are the input in use");
            self.active = true;
        }
        match input {
            Input::Key { usage, pressed } => {
                if let Some(code) = horizon_ios_input::key_code(usage) {
                    self.press(code, pressed, text, &mut out);
                }
            }
            Input::Motion { dx, dy } if pointer.grabbed => out.push(Out::Look(dx, dy)),
            Input::Motion { dx, dy } => {
                let max = |n: u32| f64::from(n.saturating_sub(1));
                let x = (pointer.at.0 + dx * pointer.scale_factor).clamp(0.0, max(pointer.size[0]));
                let y = (pointer.at.1 + dy * pointer.scale_factor).clamp(0.0, max(pointer.size[1]));
                out.push(Out::Event(WindowEvent::CursorMoved {
                    device_id: DeviceId::dummy(),
                    position: PhysicalPosition::new(x, y),
                }));
            }
            Input::Button { button, pressed } => out.push(Out::Event(WindowEvent::MouseInput {
                device_id: DeviceId::dummy(),
                state: if pressed {
                    ElementState::Pressed
                } else {
                    ElementState::Released
                },
                button,
            })),
            Input::Scroll { dx, dy } => out.push(Out::Event(WindowEvent::MouseWheel {
                device_id: DeviceId::dummy(),
                delta: MouseScrollDelta::LineDelta(dx as f32, dy as f32),
                phase: TouchPhase::Moved,
            })),
        }
        out
    }

    /// Once a frame: the held key's repeat, and every key let go if the keyboard went away.
    pub(super) fn frame(&mut self, text: bool) -> Vec<Out> {
        let mut out = Vec::new();
        if !horizon_ios_input::keyboard_connected() && !self.held.is_empty() {
            for code in self.held.clone() {
                self.press(code, false, text, &mut out);
            }
        }
        if let Some((code, at)) = self.repeat
            && Instant::now() >= at
        {
            self.repeat = Some((code, Instant::now() + REPEAT_EVERY));
            out.extend(self.key(code, ElementState::Pressed, true, text));
        }
        out
    }

    /// Locks the pointer while a mouse is in use and the app is in front; logs what the system
    /// made of it.
    pub(super) fn sync_pointer_lock(&mut self, window: &Window, focused: bool) {
        let wanted = self.active && focused && horizon_ios_input::mouse_connected();
        if wanted != self.lock_wanted {
            self.lock_wanted = wanted;
            horizon_ios_input::set_pointer_locked(window, wanted);
        }
        let seen = horizon_ios_input::pointer_lock_state(window);
        if seen != self.lock_seen {
            self.lock_seen = seen;
            if let Some(locked) = seen {
                log::info!(
                    "Horizon: pointer {}",
                    if locked { "locked" } else { "unlocked" }
                );
            }
        }
    }
}

/// The cursor's arrow, a cell a point: `#` its outline, `o` its inside.
const ARROW: [&str; 13] = [
    "#       ", "##      ", "#o#     ", "#oo#    ", "#ooo#   ", "#oooo#  ", "#ooooo# ", "#oooooo#",
    "#ooo####", "#o#oo#  ", "##  #o# ", "#   #o# ", "     ## ",
];

/// The cursor with its tip at `at` (GUI pixels); `point` is a point in GUI pixels.
pub(super) fn draw_cursor(batch: &mut HudBatch, atlas: &HudAtlas, at: (f32, f32), point: f32) {
    let cell = point * 1.4;
    for (row, line) in ARROW.iter().enumerate() {
        for (col, c) in line.bytes().enumerate() {
            let color = match c {
                b'#' => rgba(0, 0, 0, 255),
                b'o' => rgba(255, 255, 255, 255),
                _ => continue,
            };
            let (x, y) = (at.0 + col as f32 * cell, at.1 + row as f32 * cell);
            batch.fill(atlas, x, y, cell, cell, color);
        }
    }
}

impl Game<'_> {
    /// Once a frame, before the pads: the keyboard's and mouse's input since the last frame, and
    /// the pointer lock.
    pub(super) fn hardware_input(&mut self, event_loop: &dyn EventLoopCtl) {
        let Some(window) = self.window.clone() else {
            return;
        };
        horizon_ios_input::watch_pointer_touches(&window);
        let id = window.id();
        for input in horizon_ios_input::poll() {
            let was_active = self.hardware.active();
            let pointer = self.hardware_pointer(&window);
            for out in self.hardware.translate(input, pointer, false) {
                self.hardware_out(event_loop, id, out);
            }
            if !was_active && self.hardware.active() {
                // The keyboard and mouse take over from the touch buttons' virtual pad.
                self.pad_yield_input(event_loop);
                self.pad.keyboard_took_over = true;
            }
        }
        for out in self.hardware.frame(false) {
            self.hardware_out(event_loop, id, out);
        }
        self.hardware.sync_pointer_lock(&window, self.focused);
    }

    fn hardware_pointer(&self, window: &Window) -> Pointer {
        let size = self.renderer.as_ref().map_or_else(
            || horizon_client::window::surface_size(window).into(),
            |r| [r.config.width, r.config.height],
        );
        let scale = f64::from(HudBatch::new(size).scale);
        Pointer {
            grabbed: self.captured,
            at: (
                f64::from(self.cursor.0) * scale,
                f64::from(self.cursor.1) * scale,
            ),
            size,
            scale_factor: window.scale_factor(),
        }
    }

    fn hardware_out(&mut self, event_loop: &dyn EventLoopCtl, id: WindowId, out: Out) {
        match out {
            Out::Event(event) => self.user_window_event(event_loop, id, event),
            Out::Look(x, y) => self.mouse_motion(event_loop, x, y),
        }
    }

    pub(super) fn draw_hardware_cursor(&self, batch: &mut HudBatch) {
        if !self.hardware.cursor_shown(self.captured) {
            return;
        }
        let point = self
            .window
            .as_ref()
            .map_or(1.0, |w| w.scale_factor() as f32)
            / batch.scale;
        draw_cursor(batch, &self.gui.atlas, self.cursor, point);
    }

    /// The window's keys come from GameController while a keyboard is connected: winit's own key
    /// events (Backspace and Return from UIKit's text input) would come twice. Its text stays.
    pub(super) fn hardware_drops(event: &WindowEvent) -> bool {
        horizon_ios_input::keyboard_connected()
            && matches!(event, WindowEvent::KeyboardInput { event, .. }
                if matches!(event.physical_key, winit::keyboard::PhysicalKey::Code(_)))
    }
}

impl Loading {
    /// The startup menu's share of [`Game::hardware_input`]: no world, so never grabbed, and no
    /// text input from UIKit, so the keys type.
    pub(super) fn hardware_input(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        let Some(window) = self.window.clone() else {
            return;
        };
        horizon_ios_input::watch_pointer_touches(&window);
        let id = window.id();
        let size = self.renderer.as_ref().map_or_else(
            || horizon_client::window::surface_size(&window).into(),
            |r| [r.config.width, r.config.height],
        );
        for input in horizon_ios_input::poll() {
            let pointer = Pointer {
                grabbed: false,
                at: self.menu.pointer(),
                size,
                scale_factor: window.scale_factor(),
            };
            for out in self.hardware.translate(input, pointer, true) {
                if let Out::Event(event) = out {
                    self.window_event(event_loop, id, event);
                }
            }
        }
        for out in self.hardware.frame(true) {
            if let Out::Event(event) = out {
                self.window_event(event_loop, id, event);
            }
        }
        self.hardware.sync_pointer_lock(&window, true);
    }

    pub(super) fn draw_hardware_cursor(&self, batch: &mut HudBatch) {
        if !self.hardware.cursor_shown(false) {
            return;
        }
        let atlas = self.gui.as_ref().map_or(&self.boot, |gui| &gui.atlas);
        let point = self
            .window
            .as_ref()
            .map_or(1.0, |w| w.scale_factor() as f32)
            / batch.scale;
        let (x, y) = self.menu.pointer();
        let at = (x as f32 / batch.scale, y as f32 / batch.scale);
        draw_cursor(batch, atlas, at, point);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pointer(grabbed: bool) -> Pointer {
        Pointer {
            grabbed,
            at: (100.0, 100.0),
            size: [400, 300],
            scale_factor: 2.0,
        }
    }

    #[test]
    fn the_mouse_turns_the_view_when_grabbed_and_moves_the_cursor_otherwise() {
        let mut input = HardwareInput::default();
        let motion = Input::Motion { dx: 3.0, dy: -4.0 };
        input.translate(Input::Motion { dx: 50.0, dy: 0.0 }, pointer(true), false);
        let out = input.translate(motion, pointer(true), false);
        assert!(matches!(out[..], [Out::Look(x, y)] if x == 3.0 && y == -4.0));
        assert!(input.active());
        let out = input.translate(motion, pointer(false), false);
        let [Out::Event(WindowEvent::CursorMoved { position, .. })] = &out[..] else {
            panic!("a cursor move");
        };
        assert_eq!((position.x, position.y), (106.0, 92.0));
        let far = Input::Motion {
            dx: -1000.0,
            dy: 1000.0,
        };
        let out = input.translate(far, pointer(false), false);
        let [Out::Event(WindowEvent::CursorMoved { position, .. })] = &out[..] else {
            panic!("a cursor move");
        };
        assert_eq!((position.x, position.y), (0.0, 299.0));
    }

    #[test]
    fn a_brush_of_the_trackpad_leaves_the_touch_controls() {
        let mut input = HardwareInput::default();
        let nudge = Input::Motion { dx: 6.0, dy: 5.0 };
        for _ in 0..3 {
            assert!(input.translate(nudge, pointer(true), false).is_empty());
        }
        assert!(!input.active());
        assert_eq!(input.translate(nudge, pointer(true), false).len(), 1);
        assert!(input.active());
    }

    #[test]
    fn a_touch_hands_back_to_the_touch_controls() {
        let mut input = HardwareInput::default();
        let click = Input::Button {
            button: winit::event::MouseButton::Left,
            pressed: true,
        };
        input.translate(click, pointer(false), false);
        assert!(input.active());
        input.touched();
        assert!(!input.active());
    }
}
