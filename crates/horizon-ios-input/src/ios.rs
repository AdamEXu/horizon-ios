use crate::Input;
use block2::RcBlock;
use objc2::encode::Encode;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Imp, Sel};
use objc2::{msg_send, sel};
use objc2_foundation::{NSArray, NSNumber};
use objc2_game_controller::{
    GCControllerButtonInput, GCControllerDirectionPad, GCDevice, GCKeyCode, GCKeyboard,
    GCKeyboardInput, GCMouse, GCMouseInput,
};
use std::cell::RefCell;
use std::ffi::{CString, c_float};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Once, OnceLock, PoisonError};
use std::time::{Duration, Instant};
use winit::event::{ElementState, KeyEvent, MouseButton};
use winit::keyboard::{Key, KeyCode, KeyLocation, PhysicalKey, SmolStr};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

/// What the GameController handlers saw since the last `poll`. They run on the main queue, between
/// the event loop's turns.
static INPUTS: Mutex<Vec<Input>> = Mutex::new(Vec::new());
/// What winit's view controller answers to `prefersPointerLocked`.
static POINTER_LOCKED: AtomicBool = AtomicBool::new(false);
/// The touches the pointer made (`UITouchTypeIndirectPointer`), by address: winit's touch id.
static POINTER_TOUCHES: Mutex<Vec<u64>> = Mutex::new(Vec::new());
/// How often the connected keyboards and mice are listed again.
const SCAN: Duration = Duration::from_secs(1);

fn push(input: Input) {
    INPUTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(input);
}

#[derive(Default)]
struct Devices {
    keyboard: Option<Retained<GCKeyboard>>,
    mice: Vec<Retained<GCMouse>>,
    scanned: Option<Instant>,
}

thread_local! {
    static DEVICES: RefCell<Devices> = RefCell::default();
}

fn describe(device: &(impl GCDevice + objc2::Message)) -> String {
    // SAFETY: plain property reads on a device the framework handed out.
    let (vendor, category) = unsafe { (device.vendorName(), device.productCategory()) };
    match vendor {
        Some(vendor) => format!("{vendor} ({category})"),
        None => category.to_string(),
    }
}

impl Devices {
    fn scan(&mut self) {
        self.scanned = Some(Instant::now());
        // SAFETY: class property reads, on the main thread like every GameController call here.
        let keyboard = unsafe { GCKeyboard::coalescedKeyboard() };
        if keyboard.as_ref().map(Retained::as_ptr) != self.keyboard.as_ref().map(Retained::as_ptr) {
            match &keyboard {
                Some(keyboard) => {
                    watch_keyboard(keyboard);
                    log::info!("Horizon: keyboard connected: {}", describe(&**keyboard));
                }
                None => log::info!("Horizon: keyboard disconnected"),
            }
            self.keyboard = keyboard;
        }
        // SAFETY: as above.
        let mice = unsafe { GCMouse::mice() }.to_vec();
        for mouse in &mice {
            if !self
                .mice
                .iter()
                .any(|m| Retained::as_ptr(m) == Retained::as_ptr(mouse))
            {
                watch_mouse(mouse);
                log::info!("Horizon: mouse connected: {}", describe(&**mouse));
            }
        }
        for mouse in &self.mice {
            if !mice
                .iter()
                .any(|m| Retained::as_ptr(m) == Retained::as_ptr(mouse))
            {
                log::info!("Horizon: mouse disconnected: {}", describe(&**mouse));
            }
        }
        if mice.is_empty() && !self.mice.is_empty() {
            for button in [MouseButton::Left, MouseButton::Right, MouseButton::Middle] {
                push(Input::Button {
                    button,
                    pressed: false,
                });
            }
        }
        self.mice = mice;
    }
}

fn watch_keyboard(keyboard: &GCKeyboard) {
    // SAFETY: a property read.
    let Some(input) = (unsafe { keyboard.keyboardInput() }) else {
        return;
    };
    let changed = RcBlock::new(
        |_: NonNull<GCKeyboardInput>,
         _: NonNull<GCControllerButtonInput>,
         code: GCKeyCode,
         pressed: Bool| {
            if let Ok(usage) = u16::try_from(code) {
                push(Input::Key {
                    usage,
                    pressed: pressed.as_bool(),
                });
            }
        },
    );
    // SAFETY: the property copies the block; the block is `'static` and only touches statics.
    unsafe { input.setKeyChangedHandler(RcBlock::as_ptr(&changed)) };
}

fn watch_button(button: &GCControllerButtonInput, which: MouseButton) {
    let changed = RcBlock::new(
        move |_: NonNull<GCControllerButtonInput>, _: c_float, pressed: Bool| {
            push(Input::Button {
                button: which,
                pressed: pressed.as_bool(),
            });
        },
    );
    // SAFETY: as in `watch_keyboard`.
    unsafe { button.setPressedChangedHandler(RcBlock::as_ptr(&changed)) };
}

fn watch_mouse(mouse: &GCMouse) {
    // SAFETY: property reads and handler blocks as in `watch_keyboard`.
    unsafe {
        let Some(input) = mouse.mouseInput() else {
            return;
        };
        let moved = RcBlock::new(|_: NonNull<GCMouseInput>, dx: c_float, dy: c_float| {
            push(Input::Motion {
                dx: f64::from(dx),
                dy: -f64::from(dy),
            });
        });
        input.setMouseMovedHandler(RcBlock::as_ptr(&moved));
        watch_button(&input.leftButton(), MouseButton::Left);
        if let Some(button) = input.rightButton() {
            watch_button(&button, MouseButton::Right);
        }
        if let Some(button) = input.middleButton() {
            watch_button(&button, MouseButton::Middle);
        }
        if let Some(buttons) = input.auxiliaryButtons() {
            for (i, button) in buttons.to_vec().iter().enumerate() {
                let which = match i {
                    0 => MouseButton::Back,
                    1 => MouseButton::Forward,
                    n => MouseButton::Other(5 + n as u16),
                };
                watch_button(button, which);
            }
        }
        // GameController reports the wheel raw: the vertical axis first, positive towards the
        // user, the horizontal second. iPadOS scrolls naturally by default, which flips both.
        let scrolled = RcBlock::new(
            |_: NonNull<GCControllerDirectionPad>, x: c_float, y: c_float| {
                if x != 0.0 || y != 0.0 {
                    push(Input::Scroll {
                        dx: -f64::from(y),
                        dy: f64::from(x),
                    });
                }
            },
        );
        input
            .scroll()
            .setValueChangedHandler(RcBlock::as_ptr(&scrolled));
    }
}

/// The keyboard and mouse input since the last call. Lists the connected keyboards and mice
/// again now and then, and starts listening to new ones. Call it on the main thread.
pub fn poll() -> Vec<Input> {
    DEVICES.with_borrow_mut(|devices| {
        if devices.scanned.is_none_or(|at| at.elapsed() >= SCAN) {
            devices.scan();
        }
    });
    std::mem::take(&mut *INPUTS.lock().unwrap_or_else(PoisonError::into_inner))
}

pub fn keyboard_connected() -> bool {
    DEVICES.with_borrow(|devices| devices.keyboard.is_some())
}

pub fn mouse_connected() -> bool {
    DEVICES.with_borrow(|devices| !devices.mice.is_empty())
}

/// A key event as winit's desktop backends make them, which winit's iOS backend never sends.
pub fn key_event(
    physical_key: KeyCode,
    logical_key: Key,
    text: Option<SmolStr>,
    state: ElementState,
    repeat: bool,
) -> KeyEvent {
    /// `KeyEvent` without its `platform_specific` field, which is empty on iOS.
    #[allow(dead_code)]
    struct Public(
        PhysicalKey,
        Key,
        Option<SmolStr>,
        KeyLocation,
        ElementState,
        bool,
    );
    const _: () = assert!(size_of::<KeyEvent>() == size_of::<Public>());
    let location = match physical_key {
        KeyCode::ShiftRight | KeyCode::ControlRight | KeyCode::AltRight | KeyCode::SuperRight => {
            KeyLocation::Right
        }
        KeyCode::ShiftLeft | KeyCode::ControlLeft | KeyCode::AltLeft | KeyCode::SuperLeft => {
            KeyLocation::Left
        }
        KeyCode::Numpad0
        | KeyCode::Numpad1
        | KeyCode::Numpad2
        | KeyCode::Numpad3
        | KeyCode::Numpad4
        | KeyCode::Numpad5
        | KeyCode::Numpad6
        | KeyCode::Numpad7
        | KeyCode::Numpad8
        | KeyCode::Numpad9
        | KeyCode::NumpadAdd
        | KeyCode::NumpadComma
        | KeyCode::NumpadDecimal
        | KeyCode::NumpadDivide
        | KeyCode::NumpadEnter
        | KeyCode::NumpadEqual
        | KeyCode::NumpadMultiply
        | KeyCode::NumpadSubtract => KeyLocation::Numpad,
        _ => KeyLocation::Standard,
    };
    let mut event = std::mem::MaybeUninit::<KeyEvent>::uninit();
    let at = event.as_mut_ptr();
    // SAFETY: every public field is written. The one private field, `platform_specific`, is
    // winit's iOS `KeyEventExtra {}`: zero-sized, so it has no bytes to initialise (the size
    // assertion above fails the build if a winit update gives it any).
    unsafe {
        (&raw mut (*at).physical_key).write(PhysicalKey::Code(physical_key));
        (&raw mut (*at).logical_key).write(logical_key);
        (&raw mut (*at).text).write(text);
        (&raw mut (*at).location).write(location);
        (&raw mut (*at).state).write(state);
        (&raw mut (*at).repeat).write(repeat);
        event.assume_init()
    }
}

fn ui_handle(window: &Window) -> Option<winit::raw_window_handle::UiKitWindowHandle> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::UiKit(handle) => Some(handle),
        _ => None,
    }
}

extern "C-unwind" fn prefers_pointer_locked(_: &AnyObject, _: Sel) -> Bool {
    Bool::new(POINTER_LOCKED.load(Ordering::Relaxed))
}

/// Locks the pointer (hidden, only GameController's movement) or frees it. UIKit asks the window's
/// root view controller, and winit's doesn't answer, so its class gets the method.
pub fn set_pointer_locked(window: &Window, locked: bool) {
    let Some(controller) = ui_handle(window).and_then(|h| h.ui_view_controller) else {
        return;
    };
    // SAFETY: winit's handle points at its live `WinitUIViewController`.
    let controller: &AnyObject = unsafe { controller.cast().as_ref() };
    static ADD_METHOD: Once = Once::new();
    ADD_METHOD.call_once(|| {
        let types = CString::new(format!("{}@:", Bool::ENCODING)).expect("no NUL in an encoding");
        let method: extern "C-unwind" fn(&AnyObject, Sel) -> Bool = prefers_pointer_locked;
        // SAFETY: `method` has the signature `types` describes, and `prefersPointerLocked` is a
        // `BOOL` getter taking no arguments, as UIViewController declares it.
        let added = unsafe {
            let imp: Imp = std::mem::transmute(method);
            let class: *const AnyClass = controller.class();
            objc2::ffi::class_addMethod(
                class.cast_mut(),
                sel!(prefersPointerLocked),
                imp,
                types.as_ptr(),
            )
        };
        if !added.as_bool() {
            log::warn!("Horizon: winit's view controller already answers prefersPointerLocked");
        }
    });
    if POINTER_LOCKED.swap(locked, Ordering::Relaxed) != locked {
        // SAFETY: a UIViewController method (iOS 14), on the main thread.
        unsafe {
            let _: () = msg_send![controller, setNeedsUpdateOfPrefersPointerLocked];
        }
    }
}

/// Whether the system has locked the pointer (it may refuse, as for a window that doesn't fill
/// the screen); `None` before the window has a scene.
pub fn pointer_lock_state(window: &Window) -> Option<bool> {
    let view = ui_handle(window)?.ui_view;
    // SAFETY: winit's handle points at its live view; these are UIKit property reads (iOS 14).
    unsafe {
        let view: &AnyObject = view.cast().as_ref();
        let window: Option<Retained<AnyObject>> = msg_send![view, window];
        let scene: Option<Retained<AnyObject>> = msg_send![&*window?, windowScene];
        let state: Option<Retained<AnyObject>> = msg_send![&*scene?, pointerLockState];
        let locked: Bool = msg_send![&*state?, isLocked];
        Some(locked.as_bool())
    }
}

extern "C-unwind" fn pointer_touches_began(
    _: *mut AnyObject,
    _: Sel,
    touches: *mut AnyObject,
    _: *mut AnyObject,
) {
    // SAFETY: UIKit passes an `NSSet<UITouch *>`.
    let touches: Option<Retained<NSArray<AnyObject>>> = unsafe { msg_send![touches, allObjects] };
    let Some(touches) = touches else {
        return;
    };
    let mut ids = POINTER_TOUCHES
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    for touch in touches.to_vec() {
        ids.push(Retained::as_ptr(&touch) as usize as u64);
    }
    // Touches whose end never reached the game: keep the list short.
    let excess = ids.len().saturating_sub(16);
    ids.drain(..excess);
}

extern "C-unwind" fn pointer_touches_ended(
    this: *mut AnyObject,
    _: Sel,
    _: *mut AnyObject,
    _: *mut AnyObject,
) {
    // UIGestureRecognizerStateFailed: the recognizer resets for the next touch, having never
    // recognized anything or held a touch back from the view.
    // SAFETY: `setState:` is UIGestureRecognizer's, for subclasses.
    unsafe {
        let _: () = msg_send![this, setState: 5isize];
    }
}

/// A gesture recognizer on winit's view that sees only the pointer's touches, before the view
/// does (winit's `Touch` doesn't say what made it), and lets them all through.
pub fn watch_pointer_touches(window: &Window) {
    static WATCHING: Once = Once::new();
    static CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
    let Some(handle) = ui_handle(window) else {
        return;
    };
    WATCHING.call_once(|| {
        let class = CLASS.get_or_init(|| {
            let superclass = AnyClass::get(c"UIGestureRecognizer").expect("UIKit is loaded");
            let mut builder = ClassBuilder::new(c"HorizonPointerTouches", superclass)
                .expect("the class is defined once");
            type Touches =
                extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject);
            // SAFETY: the signatures of UIResponder's touch methods.
            unsafe {
                builder.add_method(
                    sel!(touchesBegan:withEvent:),
                    pointer_touches_began as Touches,
                );
                builder.add_method(
                    sel!(touchesEnded:withEvent:),
                    pointer_touches_ended as Touches,
                );
                builder.add_method(
                    sel!(touchesCancelled:withEvent:),
                    pointer_touches_ended as Touches,
                );
            }
            builder.register()
        });
        // UITouchTypeIndirectPointer.
        let types = NSArray::from_retained_slice(&[NSNumber::numberWithInteger(3)]);
        // SAFETY: UIKit calls on the main thread; the view retains the recognizer.
        unsafe {
            let view: &AnyObject = handle.ui_view.cast().as_ref();
            let recognizer: Retained<AnyObject> = msg_send![*class, new];
            let _: () = msg_send![&*recognizer, setCancelsTouchesInView: false];
            let _: () = msg_send![&*recognizer, setDelaysTouchesEnded: false];
            let _: () = msg_send![&*recognizer, setAllowedTouchTypes: &*types];
            let _: () = msg_send![view, addGestureRecognizer: &*recognizer];
        }
    });
}

/// Whether winit's touch `id` is the pointer's; `ended` forgets it.
pub fn pointer_touch(id: u64, ended: bool) -> bool {
    let mut ids = POINTER_TOUCHES
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let at = ids.iter().position(|&t| t == id);
    if ended && let Some(at) = at {
        ids.remove(at);
    }
    at.is_some()
}
