//! Hardware keyboards and mice on iPadOS (`horizon-ios-input`), turned into the window events the
//! desktop's keyboard and mouse send, so every screen and binding takes them as it does there.
//!
//! As the desktop grabs the mouse only in play, the pointer is locked only while the game holds it:
//! there GameController's raw movement turns the view (`MouseHandler.onMove`), and its buttons
//! and wheel act. Over screens the system's own pointer shows, and everything comes from UIKit's
//! pointer events: hovering and dragging move the cursor, and each press and release first moves
//! it to where that event happened, as a desktop's mouse reports a move before a click. Keys come
//! from GameController; typed text comes from UIKit (winit's `insertText:`), which knows the
//! layout, while a text field has the keyboard focus. Touching the screen hands control back to
//! the touch controls.
use super::Game;
use super::event_loop::EventLoopCtl;
use super::loading_screen::Loading;
use horizon_ios_input::Input;
use std::time::{Duration, Instant};
use winit::dpi::PhysicalPosition;
use winit::event::{
    DeviceId, ElementState, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent,
};
use winit::keyboard::{KeyCode, ModifiersState};
use winit::window::{Window, WindowId};

/// A held key starts repeating after this, then repeats this often (iPadOS' defaults).
const REPEAT_DELAY: Duration = Duration::from_millis(500);
const REPEAT_EVERY: Duration = Duration::from_millis(50);
/// How far the mouse moves (GameController's units) before it takes over from the touch
/// controls: a palm brushing the trackpad while playing by touch does nothing.
const WAKE_MOTION: f64 = 40.0;
/// How long the system may take to lock or free the pointer before it is asked again.
const LOCK_RETRY: Duration = Duration::from_secs(1);

pub(super) struct HardwareInput {
    /// A keyboard or mouse is what the player uses now: the touch controls hide.
    active: bool,
    held: Vec<KeyCode>,
    modifiers: ModifiersState,
    repeat: Option<(KeyCode, Instant)>,
    lock_wanted: bool,
    /// When the lock was last asked for, and how many times since `lock_wanted` changed.
    lock_asked: (Instant, u32),
    lock_seen: Option<bool>,
    /// Mouse movement while the touch controls are in use, towards [`WAKE_MOTION`].
    stray_motion: f64,
    /// The game holds the pointer (the last input's view of it).
    grabbed: bool,
    /// Mouse buttons the game has down.
    buttons: Vec<MouseButton>,
}

impl Default for HardwareInput {
    fn default() -> Self {
        Self {
            active: false,
            held: Vec::new(),
            modifiers: ModifiersState::empty(),
            repeat: None,
            lock_wanted: false,
            lock_asked: (Instant::now(), 0),
            lock_seen: None,
            stray_motion: 0.0,
            grabbed: false,
            buttons: Vec::new(),
        }
    }
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

fn click(button: MouseButton, pressed: bool) -> Out {
    Out::Event(WindowEvent::MouseInput {
        device_id: DeviceId::dummy(),
        state: if pressed {
            ElementState::Pressed
        } else {
            ElementState::Released
        },
        button,
    })
}

fn wheel(delta: MouseScrollDelta) -> Out {
    Out::Event(WindowEvent::MouseWheel {
        device_id: DeviceId::dummy(),
        delta,
        phase: TouchPhase::Moved,
    })
}

fn moved(x: f64, y: f64) -> Out {
    Out::Event(WindowEvent::CursorMoved {
        device_id: DeviceId::dummy(),
        position: PhysicalPosition::new(x, y),
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

    /// The game took or let go of the pointer: the buttons held through it are let go, since
    /// their releases would come from the source the game no longer reads (GameController's in
    /// play, UIKit's over screens).
    fn grab(&mut self, grabbed: bool, out: &mut Vec<Out>) {
        if grabbed != self.grabbed {
            self.grabbed = grabbed;
            out.extend(self.buttons.drain(..).map(|b| click(b, false)));
        }
    }

    fn button(&mut self, button: MouseButton, pressed: bool, out: &mut Vec<Out>) {
        if pressed == self.buttons.contains(&button) {
            return;
        }
        if pressed {
            self.buttons.push(button);
        } else {
            self.buttons.retain(|&b| b != button);
        }
        out.push(click(button, pressed));
    }

    /// One input as window events. `grabbed`: the game holds the pointer now. `text`: the keys
    /// carry the characters they type on a US layout, for a window UIKit sends no text to.
    pub(super) fn translate(&mut self, input: Input, grabbed: bool, text: bool) -> Vec<Out> {
        let mut out = Vec::new();
        self.grab(grabbed, &mut out);
        // A key or button let go still reaches the game, whichever input is in use.
        let release = matches!(
            input,
            Input::Key { pressed: false, .. }
                | Input::Button { pressed: false, .. }
                | Input::Click { pressed: false, .. }
        );
        if !self.active && !release {
            let wakes = match input {
                Input::Motion { dx, dy } => {
                    self.stray_motion += dx.abs() + dy.abs();
                    self.stray_motion >= WAKE_MOTION
                }
                // An Apple Pencil hovering moves the pointer too.
                Input::Pointer { .. } => false,
                _ => true,
            };
            if wakes {
                log::info!("Horizon: keyboard and mouse are the input in use");
                self.active = true;
            } else if !matches!(input, Input::Pointer { .. }) {
                return out;
            }
        }
        match input {
            Input::Key { usage, pressed } => {
                if let Some(code) = horizon_ios_input::key_code(usage) {
                    self.press(code, pressed, text, &mut out);
                }
            }
            Input::Motion { dx, dy } if grabbed => out.push(Out::Look(dx, dy)),
            Input::Button { button, pressed } if grabbed => self.button(button, pressed, &mut out),
            Input::Wheel { dx, dy } if grabbed => {
                out.push(wheel(MouseScrollDelta::LineDelta(dx as f32, dy as f32)));
            }
            Input::Pointer { x, y } if !grabbed => out.push(moved(x, y)),
            Input::Click {
                x,
                y,
                button,
                pressed,
            } if !grabbed => {
                out.push(moved(x, y));
                self.button(button, pressed, &mut out);
            }
            Input::Scroll { dx, dy } if !grabbed => {
                let delta = MouseScrollDelta::PixelDelta(PhysicalPosition::new(dx, dy));
                out.push(wheel(delta));
            }
            _ => {}
        }
        out
    }

    /// Once a frame: the held key's repeat, and every key let go if the keyboard went away.
    pub(super) fn frame(&mut self, grabbed: bool, text: bool) -> Vec<Out> {
        let mut out = Vec::new();
        self.grab(grabbed, &mut out);
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

    /// Locks the pointer while the game holds it (in play) and the app is in front, as the
    /// desktop grabs the mouse; logs what the system made of it.
    pub(super) fn sync_pointer_lock(&mut self, window: &Window, grabbed: bool, focused: bool) {
        let mouse = horizon_ios_input::mouse_connected();
        let wanted = grabbed && focused && mouse;
        if wanted != self.lock_wanted {
            log::info!(
                "Horizon: pointer lock wanted: {wanted} (in play {grabbed}, window focused \
                 {focused}, mouse {mouse})"
            );
            self.lock_wanted = wanted;
            self.lock_asked = (Instant::now(), 0);
            horizon_ios_input::set_pointer_locked(window, wanted);
        }
        let seen = horizon_ios_input::pointer_lock_state(window);
        // The lock is a request the system may pass over (the app was busy becoming active, a
        // button was held): ask again until the system's state matches.
        let (asked, tries) = self.lock_asked;
        if seen.is_some_and(|locked| locked != wanted) && asked.elapsed() >= LOCK_RETRY && tries < 5
        {
            log::info!("Horizon: the pointer lock is still {seen:?}: asking again");
            self.lock_asked = (Instant::now(), tries + 1);
            horizon_ios_input::set_pointer_locked(window, wanted);
        }
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

impl Game<'_> {
    /// Once a frame, before the pads: the keyboard's and mouse's input since the last frame, and
    /// the pointer lock.
    pub(super) fn hardware_input(&mut self, event_loop: &dyn EventLoopCtl) {
        let Some(window) = self.window.clone() else {
            return;
        };
        horizon_ios_input::watch_pointer(&window);
        let id = window.id();
        for input in horizon_ios_input::poll() {
            let was_active = self.hardware.active();
            for out in self.hardware.translate(input, self.captured, false) {
                self.hardware_out(event_loop, id, out);
            }
            if !was_active && self.hardware.active() {
                // The keyboard and mouse take over from the touch buttons' virtual pad.
                self.pad_yield_input(event_loop);
                self.pad.keyboard_took_over = true;
            }
        }
        for out in self.hardware.frame(self.captured, false) {
            self.hardware_out(event_loop, id, out);
        }
        self.hardware
            .sync_pointer_lock(&window, self.captured, self.focused);
    }

    fn hardware_out(&mut self, event_loop: &dyn EventLoopCtl, id: WindowId, out: Out) {
        match out {
            Out::Event(event) => self.user_window_event(event_loop, id, event),
            Out::Look(x, y) => self.mouse_motion(event_loop, x, y),
        }
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
        horizon_ios_input::watch_pointer(&window);
        let id = window.id();
        let inputs = horizon_ios_input::poll();
        let mut outs = Vec::new();
        for input in inputs {
            outs.extend(self.hardware.translate(input, false, true));
        }
        outs.extend(self.hardware.frame(false, true));
        for out in outs {
            if let Out::Event(event) = out {
                self.window_event(event_loop, id, event);
            }
        }
        self.hardware.sync_pointer_lock(&window, false, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moved_to(out: &[Out]) -> Option<(f64, f64)> {
        out.iter().find_map(|o| match o {
            Out::Event(WindowEvent::CursorMoved { position, .. }) => Some((position.x, position.y)),
            _ => None,
        })
    }

    fn clicks(out: &[Out]) -> usize {
        out.iter()
            .filter(|o| matches!(o, Out::Event(WindowEvent::MouseInput { .. })))
            .count()
    }

    fn click_at(x: f64, y: f64, pressed: bool) -> Input {
        Input::Click {
            x,
            y,
            button: MouseButton::Left,
            pressed,
        }
    }

    const LEFT: Input = Input::Button {
        button: MouseButton::Left,
        pressed: true,
    };

    #[test]
    fn in_play_the_mouse_turns_the_view_and_over_screens_the_pointer_places_the_cursor() {
        let mut input = HardwareInput::default();
        input.translate(Input::Motion { dx: 50.0, dy: 0.0 }, true, false);
        let out = input.translate(Input::Motion { dx: 3.0, dy: -4.0 }, true, false);
        assert!(matches!(out[..], [Out::Look(x, y)] if x == 3.0 && y == -4.0));
        assert!(
            input
                .translate(Input::Pointer { x: 9.0, y: 9.0 }, true, false)
                .is_empty()
        );
        let out = input.translate(Input::Motion { dx: 3.0, dy: -4.0 }, false, false);
        assert!(
            out.is_empty(),
            "the system's pointer moves the cursor over screens"
        );
        let out = input.translate(Input::Pointer { x: 640.0, y: 360.0 }, false, false);
        assert_eq!(moved_to(&out), Some((640.0, 360.0)));
    }

    #[test]
    fn a_click_over_screens_moves_the_cursor_to_the_click_first() {
        let mut input = HardwareInput::default();
        input.translate(Input::Pointer { x: 10.0, y: 10.0 }, false, false);
        for pressed in [true, false] {
            let out = input.translate(click_at(500.0, 200.0, pressed), false, false);
            assert_eq!(moved_to(&out[..1]), Some((500.0, 200.0)));
            assert_eq!(clicks(&out[1..]), 1);
        }
        // GameController's button is the locked pointer's: over screens, UIKit's has the click.
        assert_eq!(clicks(&input.translate(LEFT, false, false)), 0);
        assert_eq!(clicks(&input.translate(LEFT, true, false)), 1);
    }

    #[test]
    fn a_button_held_as_the_game_takes_the_pointer_is_let_go() {
        let mut input = HardwareInput::default();
        input.translate(click_at(5.0, 5.0, true), false, false);
        // The press closed the screen: UIKit, which had its release, no longer reports it.
        let out = input.translate(Input::Motion { dx: 1.0, dy: 0.0 }, true, false);
        assert_eq!(clicks(&out), 1);
        assert!(moved_to(&out).is_none());
    }

    #[test]
    fn a_brush_of_the_trackpad_leaves_the_touch_controls() {
        let mut input = HardwareInput::default();
        let nudge = Input::Motion { dx: 6.0, dy: 5.0 };
        for _ in 0..3 {
            assert!(input.translate(nudge, true, false).is_empty());
        }
        assert!(!input.active());
        assert_eq!(input.translate(nudge, true, false).len(), 1);
        assert!(input.active());
    }

    #[test]
    fn a_touch_hands_back_to_the_touch_controls() {
        let mut input = HardwareInput::default();
        input.translate(click_at(1.0, 1.0, true), false, false);
        assert!(input.active());
        input.touched();
        assert!(!input.active());
    }
}
