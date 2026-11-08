//! Touch screens (iOS): basic controls to get playing, to be tuned later.
//!
//! In play: the left part of the screen is a floating stick (the virtual pad's left stick),
//! dragging elsewhere turns the view as the mouse does, a quick tap there uses the item and a
//! still press mines (the triggers), and buttons on the right jump, sneak, sprint, attack, use,
//! open the inventory, change the camera and pause (the virtual pad's buttons, so they act as
//! a controller's would, with its bindings). Tapping the hotbar selects a slot.
//!
//! Everywhere else (the title, menus, the inventory) the first finger is the mouse: down is a
//! left press where it touches, moving drags, up releases. Two fingers moving together scroll.
use super::event_loop::EventLoopCtl;
use super::{Game, Screen};
use horizon_client::gamepad::Button;
use horizon_client::hud::{HudBatch, rgba};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use winit::dpi::PhysicalPosition;
use winit::event::{
    DeviceId, ElementState, MouseButton, MouseScrollDelta, Touch, TouchPhase, WindowEvent,
};
use winit::window::WindowId;

/// A press held this long without moving mines (holds the attack trigger).
const HOLD: Duration = Duration::from_millis(300);
/// A finger moving less than this (GUI pixels) from where it touched is still a tap.
const TAP_SLOP: f32 = 4.0;
/// The stick's reach from where the finger touched, in GUI pixels.
const STICK_RADIUS: f32 = 24.0;
/// The view's turn per physical pixel of finger travel, in mouse counts.
const LOOK_SPEED: f64 = 0.55;
/// Button size, GUI pixels.
const SIZE: f32 = 26.0;
const SMALL: f32 = 18.0;
const GAP: f32 = 6.0;

/// The on-screen buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Control {
    Jump,
    Sneak,
    Sprint,
    Attack,
    Use,
    Inventory,
    Camera,
    Pause,
}

impl Control {
    const ALL: [Control; 8] = [
        Control::Jump,
        Control::Sneak,
        Control::Sprint,
        Control::Attack,
        Control::Use,
        Control::Inventory,
        Control::Camera,
        Control::Pause,
    ];
    /// The pad button it presses (the controller bindings decide what that does).
    fn button(self) -> Button {
        match self {
            Control::Jump => Button::South,
            Control::Sneak => Button::RightStick,
            Control::Sprint => Button::LeftStick,
            Control::Attack => Button::RightTrigger,
            Control::Use => Button::LeftTrigger,
            Control::Inventory => Button::North,
            Control::Camera => Button::Select,
            Control::Pause => Button::Start,
        }
    }
    /// Its glyph: symbols, not words, so there is nothing to translate.
    fn glyph(self) -> &'static str {
        match self {
            Control::Jump => "^",
            Control::Sneak => "v",
            Control::Sprint => ">>",
            Control::Attack => "X",
            Control::Use => "O",
            Control::Inventory => "...",
            Control::Camera => "@",
            Control::Pause => "||",
        }
    }
    /// Where it is on a GUI of `w` by `h`: x, y, size.
    fn rect(self, w: f32, h: f32) -> [f32; 3] {
        let col = |i: f32| w - (SIZE + GAP) * (i + 1.0) - 4.0;
        let row = |i: f32| h - (SIZE + GAP) * (i + 1.0) - 4.0;
        // Down the right edge, under the toasts and above the big buttons.
        let top = |i: f32| [w - SMALL - 6.0, 46.0 + (SMALL + GAP) * i, SMALL];
        match self {
            Control::Jump => [col(0.0), row(0.0), SIZE],
            Control::Sneak => [col(1.0), row(0.0), SIZE],
            Control::Use => [col(0.0), row(1.0), SIZE],
            Control::Attack => [col(1.0), row(1.0), SIZE],
            Control::Sprint => [col(0.0), row(2.0), SIZE],
            Control::Pause => top(0.0),
            Control::Inventory => top(1.0),
            Control::Camera => top(2.0),
        }
    }
    /// Held while the finger stays down; the others are taps.
    fn holds(self) -> bool {
        matches!(
            self,
            Control::Jump | Control::Sneak | Control::Sprint | Control::Attack | Control::Use
        )
    }
}

/// The back button over a screen opened in play (the pad's B: close, back), top left.
const BACK: [f32; 3] = [10.0, 30.0, SIZE];

/// What one finger is doing.
#[derive(Clone, Copy, Debug)]
enum Role {
    /// The left stick, centred where the finger touched (GUI pixels).
    Stick {
        origin: [f32; 2],
        at: [f32; 2],
    },
    /// Turning the view; a tap uses, a still press mines.
    Look {
        last: [f64; 2],
        start: [f32; 2],
        since: Instant,
        moved: bool,
        mining: bool,
    },
    Button(Control),
    /// The hotbar: nothing more once the slot is chosen.
    Hotbar,
    /// The mouse, in menus.
    Mouse,
    /// A second finger in a menu: scrolls with the first.
    Scroll {
        last: [f64; 2],
    },
    /// Lifted out of play (a screen opened): ignored until it lifts.
    Ignored,
}

#[derive(Default)]
pub(super) struct TouchControls {
    fingers: HashMap<u64, Role>,
    /// Pad buttons tapped since the last frame: the pads read them down once, and they are let
    /// go the frame after (`releasing`).
    taps: Vec<Button>,
    releasing: Vec<Button>,
    /// The touch screen has been used: draw the controls.
    seen: bool,
    /// The last frame was in play.
    was_playing: bool,
    /// The focused text field's box, in window pixels (x, y, width, height).
    text_area: Option<[i32; 4]>,
    /// The on-screen keyboard is wanted: the text field was tapped.
    keyboard: bool,
}

/// A touch as the mouse: where it is, then a left press or release.
pub(super) fn as_mouse(touch: Touch) -> Vec<WindowEvent> {
    let device_id = DeviceId::dummy();
    let moved = WindowEvent::CursorMoved {
        device_id,
        position: touch.location,
    };
    let button = |state| WindowEvent::MouseInput {
        device_id,
        state,
        button: MouseButton::Left,
    };
    match touch.phase {
        TouchPhase::Started => vec![moved, button(ElementState::Pressed)],
        TouchPhase::Moved => vec![moved],
        TouchPhase::Ended | TouchPhase::Cancelled => vec![moved, button(ElementState::Released)],
    }
}

/// A touch the pointer of a connected mouse made (a click, a trackpad press): the mouse's own
/// input (game/hardware_input.rs) has it already.
pub(super) fn is_pointer(touch: &Touch) -> bool {
    let ended = matches!(touch.phase, TouchPhase::Ended | TouchPhase::Cancelled);
    horizon_ios_input::pointer_touch(touch.id, ended) && horizon_ios_input::mouse_connected()
}

pub(super) fn pointer_at(touch: &Touch) -> WindowEvent {
    WindowEvent::CursorMoved {
        device_id: DeviceId::dummy(),
        position: touch.location,
    }
}

fn inside(p: [f32; 2], r: [f32; 3]) -> bool {
    // A little larger than drawn: fingers are wide.
    let pad = 3.0;
    p[0] >= r[0] - pad && p[0] < r[0] + r[2] + pad && p[1] >= r[1] - pad && p[1] < r[1] + r[2] + pad
}

impl Game<'_> {
    fn touch_playing(&self) -> bool {
        self.title.is_none() && self.screen == Screen::Playing && self.player.is_some()
    }

    /// A screen open over play (the inventory, a container, the pause menu, chat).
    fn touch_screen_over_play(&self) -> bool {
        self.title.is_none() && self.screen != Screen::Playing && self.player.is_some()
    }

    fn touch_scale(&self) -> f32 {
        self.renderer.as_ref().map_or(1.0, |r| {
            HudBatch::new([r.config.width, r.config.height]).scale
        })
    }

    /// The GUI's size, in GUI pixels.
    fn touch_gui(&self) -> (f32, f32) {
        self.gui_size()
    }

    fn touch_press(&mut self, button: Button, down: bool) {
        let state = self.pad.injected_mut();
        match button {
            Button::LeftTrigger => state.triggers[0] = if down { 1.0 } else { 0.0 },
            Button::RightTrigger => state.triggers[1] = if down { 1.0 } else { 0.0 },
            _ => state.set(button, down),
        }
    }

    fn touch_tap(&mut self, button: Button) {
        self.touch_press(button, true);
        self.touch.taps.push(button);
    }

    /// Lets go of everything the touch controls hold on the virtual pad.
    fn touch_release_all(&mut self) {
        for control in Control::ALL {
            self.touch_press(control.button(), false);
        }
        self.pad.injected_mut().left = [0.0; 2];
        self.touch.taps.clear();
        self.touch.releasing.clear();
        for role in self.touch.fingers.values_mut() {
            *role = Role::Ignored;
        }
    }

    /// winit's touch event.
    pub(super) fn touch_event(
        &mut self,
        event_loop: &dyn EventLoopCtl,
        id: WindowId,
        touch: Touch,
    ) {
        if is_pointer(&touch) {
            // The system's pointer is showing (unlocked): the game's cursor goes where it is.
            self.user_window_event(event_loop, id, pointer_at(&touch));
            return;
        }
        if touch.phase == TouchPhase::Started {
            self.hardware.touched();
        }
        self.touch.seen = true;
        self.last_input = Instant::now();
        let scale = self.touch_scale();
        let gui = [
            touch.location.x as f32 / scale,
            touch.location.y as f32 / scale,
        ];
        let finger = touch.id;
        if touch.phase == TouchPhase::Started {
            let role = if self.touch_playing() {
                self.touch_role_in_play(gui, touch.location)
            } else if self.touch_screen_over_play() && inside(gui, BACK) {
                self.touch_tap(Button::East);
                Role::Ignored
            } else if self
                .touch
                .fingers
                .values()
                .any(|r| matches!(r, Role::Mouse))
            {
                Role::Scroll {
                    last: [touch.location.x, touch.location.y],
                }
            } else {
                Role::Mouse
            };
            self.touch.fingers.insert(finger, role);
        }
        let Some(role) = self.touch.fingers.get(&finger).copied() else {
            return;
        };
        if touch.phase == TouchPhase::Started
            && matches!(self.touch.fingers.get(&finger), Some(Role::Mouse))
        {
            // A tap on the text field brings the keyboard up; anywhere else puts it away.
            let (x, y) = (touch.location.x as i32, touch.location.y as i32);
            self.touch.keyboard = self.touch.text_area.is_some_and(|[ax, ay, aw, ah]| {
                let slop = 40;
                x >= ax - slop && x < ax + aw + slop && y >= ay - slop && y < ay + ah + slop
            });
        }
        let ended = matches!(touch.phase, TouchPhase::Ended | TouchPhase::Cancelled);
        match role {
            Role::Mouse => {
                for event in as_mouse(touch) {
                    self.handle_window_event(
                        event_loop,
                        id,
                        event,
                        super::input_trace::InputOrigin::User,
                    );
                }
            }
            Role::Scroll { last } => {
                let now = [touch.location.x, touch.location.y];
                let dy = (now[1] - last[1]) / f64::from(scale) / 12.0;
                if dy.abs() >= 1.0 {
                    self.handle_window_event(
                        event_loop,
                        id,
                        WindowEvent::MouseWheel {
                            device_id: DeviceId::dummy(),
                            delta: MouseScrollDelta::LineDelta(0.0, dy.round() as f32),
                            phase: TouchPhase::Moved,
                        },
                        super::input_trace::InputOrigin::User,
                    );
                    self.touch
                        .fingers
                        .insert(finger, Role::Scroll { last: now });
                }
            }
            Role::Stick { origin, .. } => {
                let mut v = [
                    (gui[0] - origin[0]) / STICK_RADIUS,
                    -(gui[1] - origin[1]) / STICK_RADIUS,
                ];
                let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
                if len > 1.0 {
                    v = [v[0] / len, v[1] / len];
                }
                self.pad.injected_mut().left = if ended { [0.0; 2] } else { v };
                self.touch
                    .fingers
                    .insert(finger, Role::Stick { origin, at: gui });
            }
            Role::Look {
                last,
                start,
                since,
                mut moved,
                mining,
            } => {
                let now = [touch.location.x, touch.location.y];
                let (dx, dy) = (now[0] - last[0], now[1] - last[1]);
                if !moved {
                    let (sx, sy) = (gui[0] - start[0], gui[1] - start[1]);
                    moved = (sx * sx + sy * sy).sqrt() > TAP_SLOP;
                }
                if moved && (dx != 0.0 || dy != 0.0) {
                    self.mouse
                        .on_move(0.0, 0.0, dx * LOOK_SPEED, dy * LOOK_SPEED, true, true);
                }
                if ended {
                    if mining {
                        self.touch_press(Button::RightTrigger, false);
                    } else if !moved && since.elapsed() < HOLD {
                        self.touch_tap(Button::LeftTrigger);
                    }
                } else {
                    self.touch.fingers.insert(
                        finger,
                        Role::Look {
                            last: now,
                            start,
                            since,
                            moved,
                            mining,
                        },
                    );
                }
            }
            Role::Button(control) => {
                if ended && control.holds() {
                    self.touch_press(control.button(), false);
                }
            }
            Role::Hotbar | Role::Ignored => {}
        }
        if ended {
            self.touch.fingers.remove(&finger);
        }
    }

    /// What a finger touching down at `gui` does in play.
    fn touch_role_in_play(&mut self, gui: [f32; 2], at: PhysicalPosition<f64>) -> Role {
        let (w, h) = self.touch_gui();
        for control in Control::ALL {
            if inside(gui, control.rect(w, h)) {
                if control.holds() {
                    self.touch_press(control.button(), true);
                } else {
                    self.touch_tap(control.button());
                }
                return Role::Button(control);
            }
        }
        // The hotbar: 182 by 22 GUI pixels, centred at the bottom (Gui.extractItemHotbar).
        let left = (w / 2.0 - 91.0).floor();
        if gui[1] >= h - 22.0 && gui[0] >= left && gui[0] < left + 182.0 {
            let slot = (((gui[0] - left - 1.0) / 20.0) as usize).min(8);
            self.hotbar_presses.push(slot);
            return Role::Hotbar;
        }
        let stick_taken = self
            .touch
            .fingers
            .values()
            .any(|r| matches!(r, Role::Stick { .. }));
        if gui[0] < w * 0.4 && gui[1] > h * 0.25 && !stick_taken {
            return Role::Stick {
                origin: gui,
                at: gui,
            };
        }
        Role::Look {
            last: [at.x, at.y],
            start: gui,
            since: Instant::now(),
            moved: false,
            mining: false,
        }
    }

    /// Whether the input method (the on-screen keyboard) may show while a text field has focus:
    /// on iOS only once the field was tapped.
    pub(super) fn touch_keyboard_wanted(&mut self, focused: bool) -> bool {
        if !focused {
            self.touch.keyboard = false;
            self.touch.text_area = None;
        }
        // A hardware keyboard leaves the screen free: UIKit shows no keyboard, and the field
        // needs its text input.
        !cfg!(target_os = "ios") || self.touch.keyboard || horizon_ios_input::keyboard_connected()
    }

    pub(super) fn touch_keyboard_done(&mut self) {
        self.touch.keyboard = false;
    }

    pub(super) fn touch_text_area(&mut self, area: [i32; 4]) {
        self.touch.text_area = Some(area);
    }

    /// Once a frame, before the pads are read: taps let go, a still press starts mining, and
    /// leaving play lets go of everything.
    pub(super) fn touch_frame(&mut self) {
        for button in std::mem::take(&mut self.touch.releasing) {
            self.touch_press(button, false);
        }
        self.touch.releasing = std::mem::take(&mut self.touch.taps);
        let playing = self.touch_playing();
        if self.touch.was_playing && !playing {
            self.touch_release_all();
        }
        self.touch.was_playing = playing;
        if !playing {
            return;
        }
        let mut mine = false;
        for role in self.touch.fingers.values_mut() {
            if let Role::Look {
                since,
                moved: false,
                mining,
                ..
            } = role
                && !*mining
                && since.elapsed() >= HOLD
            {
                *mining = true;
                mine = true;
            }
        }
        if mine {
            self.touch_press(Button::RightTrigger, true);
        }
    }

    /// The controls over play, once the touch screen is in use (always on iOS).
    pub(super) fn draw_touch_controls(&self, batch: &mut HudBatch) {
        if !(self.touch.seen || cfg!(target_os = "ios")) || self.hardware.active() {
            return;
        }
        let atlas = &self.gui.atlas;
        if self.touch_screen_over_play() {
            draw_button(batch, atlas, BACK, "<", false);
            return;
        }
        if !self.touch_playing() {
            return;
        }
        let (w, h) = (batch.width, batch.height);
        let held: Vec<Control> = self
            .touch
            .fingers
            .values()
            .filter_map(|r| match r {
                Role::Button(c) => Some(*c),
                _ => None,
            })
            .collect();
        for control in Control::ALL {
            let down = held.contains(&control);
            draw_button(batch, atlas, control.rect(w, h), control.glyph(), down);
        }
        for role in self.touch.fingers.values() {
            if let Role::Stick { origin, at } = role {
                let r = STICK_RADIUS;
                batch.fill(
                    atlas,
                    origin[0] - r,
                    origin[1] - r,
                    r * 2.0,
                    r * 2.0,
                    rgba(0, 0, 0, 70),
                );
                let (mut dx, mut dy) = (at[0] - origin[0], at[1] - origin[1]);
                let len = (dx * dx + dy * dy).sqrt();
                if len > r {
                    dx *= r / len;
                    dy *= r / len;
                }
                let k = 9.0;
                batch.fill(
                    atlas,
                    origin[0] + dx - k,
                    origin[1] + dy - k,
                    k * 2.0,
                    k * 2.0,
                    rgba(255, 255, 255, 120),
                );
            }
        }
    }
}

/// One button: a translucent square, its edge and its glyph.
fn draw_button(
    batch: &mut HudBatch,
    atlas: &horizon_client::hud::HudAtlas,
    [x, y, s]: [f32; 3],
    glyph: &str,
    down: bool,
) {
    let fill = if down {
        rgba(255, 255, 255, 110)
    } else {
        rgba(0, 0, 0, 90)
    };
    batch.fill(atlas, x, y, s, s, fill);
    let edge = rgba(255, 255, 255, 120);
    batch.fill(atlas, x, y, s, 1.0, edge);
    batch.fill(atlas, x, y + s - 1.0, s, 1.0, edge);
    batch.fill(atlas, x, y, 1.0, s, edge);
    batch.fill(atlas, x + s - 1.0, y, 1.0, s, edge);
    let tw = HudBatch::text_width(atlas, glyph);
    batch.text(
        atlas,
        glyph,
        (x + (s - tw) / 2.0).round(),
        (y + (s - 8.0) / 2.0).round(),
        u32::MAX,
        true,
    );
}
