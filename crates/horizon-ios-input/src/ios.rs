use crate::Input;
use block2::RcBlock;
use objc2::encode::Encode;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Imp, Sel};
use objc2::{msg_send, sel};
use objc2_core_foundation::CGPoint;
use objc2_foundation::{NSArray, NSNumber, NSString};
use objc2_game_controller::{
    GCControllerButtonInput, GCControllerDirectionPad, GCDevice, GCKeyCode, GCKeyboard,
    GCKeyboardInput, GCMouse, GCMouseInput,
};
use std::cell::RefCell;
use std::ffi::{CStr, CString, c_float};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, Once, PoisonError};
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
                    push(Input::Wheel {
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

/// The window's root view controller, which UIKit asks whether to lock the pointer.
///
/// # Safety
///
/// `view` is a live UIView, on the main thread.
unsafe fn root_controller(view: &AnyObject) -> Option<Retained<AnyObject>> {
    // SAFETY: UIKit property reads.
    unsafe {
        let window: Option<Retained<AnyObject>> = msg_send![view, window];
        msg_send![&*window?, rootViewController]
    }
}

/// The scene's state for the log: whether it is there, its activation, the lock.
///
/// # Safety
///
/// As [`root_controller`].
unsafe fn describe_scene(view: &AnyObject) -> String {
    // SAFETY: UIKit property reads (iOS 13 and 14).
    unsafe {
        let window: Option<Retained<AnyObject>> = msg_send![view, window];
        let Some(window) = window else {
            return "no window".to_owned();
        };
        let scene: Option<Retained<AnyObject>> = msg_send![&*window, windowScene];
        let Some(scene) = scene else {
            return "no window scene".to_owned();
        };
        let activation: isize = msg_send![&*scene, activationState];
        let activation = match activation {
            0 => "foreground active",
            1 => "foreground inactive",
            2 => "background",
            _ => "unattached",
        };
        format!("scene {activation}, locked {:?}", view_locked(view))
    }
}

/// Logs each change of the pointer lock the system makes (`UIPointerLockStateDidChangeNotification`).
fn watch_lock_state() {
    static WATCHING: Once = Once::new();
    WATCHING.call_once(|| {
        #[link(name = "UIKit", kind = "framework")]
        unsafe extern "C" {
            static UIPointerLockStateDidChangeNotification: &'static NSString;
        }
        let changed = RcBlock::new(|note: NonNull<AnyObject>| {
            // SAFETY: the notification's object is the scene's `UIPointerLockState`.
            unsafe {
                let state: Option<Retained<AnyObject>> = msg_send![note.as_ref(), object];
                let Some(state) = state else {
                    return;
                };
                let answers: Bool = msg_send![&*state, respondsToSelector: sel!(isLocked)];
                if answers.as_bool() {
                    let locked: Bool = msg_send![&*state, isLocked];
                    log::info!(
                        "Horizon: the system's pointer lock changed: locked {}",
                        locked.as_bool()
                    );
                }
            }
        });
        // SAFETY: NSNotificationCenter on the main thread; the observer lives as long as the app.
        unsafe {
            let center: Retained<AnyObject> = msg_send![
                AnyClass::get(c"NSNotificationCenter").expect("Foundation"),
                defaultCenter
            ];
            let nil = std::ptr::null_mut::<AnyObject>();
            let observer: Option<Retained<AnyObject>> = msg_send![
                &*center,
                addObserverForName: UIPointerLockStateDidChangeNotification,
                object: nil,
                queue: nil,
                usingBlock: &*changed
            ];
            std::mem::forget(observer);
        }
    });
}

/// Locks the pointer (hidden, only GameController's movement) or frees it. UIKit asks the window's
/// root view controller, and winit's doesn't answer, so its class gets the method.
pub fn set_pointer_locked(window: &Window, locked: bool) {
    let Some(handle) = ui_handle(window) else {
        return;
    };
    // SAFETY: winit's handle points at its live view.
    let view: &AnyObject = unsafe { handle.ui_view.cast().as_ref() };
    // SAFETY: as above.
    let Some(controller) = (unsafe { root_controller(view) }) else {
        log::warn!("Horizon: no root view controller to lock the pointer with");
        return;
    };
    watch_lock_state();
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
        let winit = handle.ui_view_controller.is_some_and(|c| {
            c.as_ptr().cast::<AnyObject>() == Retained::as_ptr(&controller).cast_mut()
        });
        log::info!(
            "Horizon: root view controller {} (winit's: {winit}) answers prefersPointerLocked: {}",
            controller.class().name().to_string_lossy(),
            if added.as_bool() {
                "added"
            } else {
                "it already did"
            },
        );
    });
    POINTER_LOCKED.store(locked, Ordering::Relaxed);
    // SAFETY: a UIViewController method (iOS 14), on the main thread; then property reads.
    unsafe {
        let _: () = msg_send![&*controller, setNeedsUpdateOfPrefersPointerLocked];
        let answer: Bool = msg_send![&*controller, prefersPointerLocked];
        log::info!(
            "Horizon: pointer lock requested: {locked} (the view controller answers {}; {})",
            answer.as_bool(),
            describe_scene(view)
        );
    }
}

/// Whether the system has locked the pointer (it may refuse); `None` before the window has a
/// scene.
pub fn pointer_lock_state(window: &Window) -> Option<bool> {
    let view = ui_handle(window)?.ui_view;
    // SAFETY: winit's handle points at its live view.
    unsafe { view_locked(view.cast().as_ref()) }
}

/// Where the system's pointer is, in window pixels; `None` while the pointer is locked, when the
/// system still sends the pointer's clicks as touches, at a place that isn't where anything is
/// (the window's corner).
///
/// # Safety
///
/// `view` is a live UIView, on the main thread.
unsafe fn pointer_place(view: &AnyObject, at: CGPoint) -> Option<(f64, f64)> {
    if POINTER_LOCKED.load(Ordering::Relaxed) || unsafe { view_locked(view) } == Some(true) {
        return None;
    }
    // SAFETY: a UIView property read.
    let scale: f64 = unsafe { msg_send![view, contentScaleFactor] };
    Some((at.x * scale, at.y * scale))
}

/// # Safety
///
/// As [`pointer_place`].
unsafe fn view_locked(view: &AnyObject) -> Option<bool> {
    // SAFETY: UIKit property reads (iOS 14).
    unsafe {
        let window: Option<Retained<AnyObject>> = msg_send![view, window];
        let scene: Option<Retained<AnyObject>> = msg_send![&*window?, windowScene];
        let state: Option<Retained<AnyObject>> = msg_send![&*scene?, pointerLockState];
        let locked: Bool = msg_send![&*state?, isLocked];
        Some(locked.as_bool())
    }
}

/// # Safety
///
/// `recognizer` is a live gesture recognizer, on the main thread.
unsafe fn recognizer_view(recognizer: *mut AnyObject) -> Option<Retained<AnyObject>> {
    // SAFETY: a UIGestureRecognizer property read.
    unsafe { msg_send![recognizer, view] }
}

/// `UIEventButtonMask` bits: primary, secondary, then the buttons by number.
const BUTTONS: [MouseButton; 5] = [
    MouseButton::Left,
    MouseButton::Right,
    MouseButton::Middle,
    MouseButton::Back,
    MouseButton::Forward,
];

/// The pointer's touch's buttons as last reported.
static TOUCH_BUTTONS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Began,
    Moved,
    Ended,
}

/// A pointer touch's place, and its buttons pressed or let go there (the event's `buttonMask`).
///
/// # Safety
///
/// UIKit's arguments to a touch method of the live recognizer `this`, on the main thread.
unsafe fn pointer_touches(
    this: *mut AnyObject,
    touches: *mut AnyObject,
    event: *mut AnyObject,
    phase: Phase,
) {
    // SAFETY: as the function's.
    unsafe {
        let touches: Option<Retained<NSArray<AnyObject>>> = msg_send![touches, allObjects];
        let (Some(touches), Some(view)) = (touches, recognizer_view(this)) else {
            return;
        };
        let Some(touch) = touches.to_vec().pop() else {
            return;
        };
        let at: CGPoint = msg_send![&*touch, locationInView: std::ptr::null_mut::<AnyObject>()];
        let before = match phase {
            Phase::Began => 0,
            _ => TOUCH_BUTTONS.load(Ordering::Relaxed),
        };
        let now = match phase {
            Phase::Ended => 0,
            _ if event.is_null() => before,
            _ => msg_send![event, buttonMask],
        };
        TOUCH_BUTTONS.store(now, Ordering::Relaxed);
        let Some((x, y)) = pointer_place(&view, at) else {
            return;
        };
        if before == now {
            push(Input::Pointer { x, y });
        }
        for (bit, button) in BUTTONS.into_iter().enumerate() {
            let (was, is) = (before >> bit & 1 == 1, now >> bit & 1 == 1);
            if was != is {
                push(Input::Click {
                    x,
                    y,
                    button,
                    pressed: is,
                });
            }
        }
    }
}

extern "C-unwind" fn pointer_touches_moved(
    this: *mut AnyObject,
    _: Sel,
    touches: *mut AnyObject,
    event: *mut AnyObject,
) {
    // SAFETY: UIKit calls with its own arguments, on the main thread.
    unsafe { pointer_touches(this, touches, event, Phase::Moved) }
}

extern "C-unwind" fn pointer_touches_began(
    this: *mut AnyObject,
    _: Sel,
    touches: *mut AnyObject,
    event: *mut AnyObject,
) {
    // SAFETY: UIKit passes an `NSSet<UITouch *>`.
    let all: Option<Retained<NSArray<AnyObject>>> = unsafe { msg_send![touches, allObjects] };
    if let Some(all) = all {
        let mut ids = POINTER_TOUCHES
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for touch in all.to_vec() {
            ids.push(Retained::as_ptr(&touch) as usize as u64);
        }
        // Touches whose end never reached the game: keep the list short.
        let excess = ids.len().saturating_sub(16);
        ids.drain(..excess);
    }
    // SAFETY: as in `pointer_touches_moved`.
    unsafe { pointer_touches(this, touches, event, Phase::Began) }
}

extern "C-unwind" fn pointer_touches_ended(
    this: *mut AnyObject,
    _: Sel,
    touches: *mut AnyObject,
    event: *mut AnyObject,
) {
    // SAFETY: as in `pointer_touches_moved`, then `setState:`, UIGestureRecognizer's for
    // subclasses: UIGestureRecognizerStateFailed resets the recognizer for the next touch,
    // having never recognized anything or held a touch back from the view.
    unsafe {
        pointer_touches(this, touches, event, Phase::Ended);
        let _: () = msg_send![this, setState: 5isize];
    }
}

/// UIGestureRecognizerStateBegan or Changed.
fn moving(state: isize) -> bool {
    matches!(state, 1 | 2)
}

extern "C-unwind" fn hovered(_: *mut AnyObject, _: Sel, recognizer: *mut AnyObject) {
    // SAFETY: UIKit calls the action with its live recognizer, on the main thread.
    unsafe {
        let state: isize = msg_send![recognizer, state];
        let Some(view) = recognizer_view(recognizer).filter(|_| moving(state)) else {
            return;
        };
        let at: CGPoint = msg_send![recognizer, locationInView: std::ptr::null_mut::<AnyObject>()];
        if let Some((x, y)) = pointer_place(&view, at) {
            push(Input::Pointer { x, y });
        }
    }
}

extern "C-unwind" fn scrolled(_: *mut AnyObject, _: Sel, recognizer: *mut AnyObject) {
    // SAFETY: as in `hovered`.
    unsafe {
        let state: isize = msg_send![recognizer, state];
        let Some(view) = recognizer_view(recognizer).filter(|_| moving(state)) else {
            return;
        };
        let nil = std::ptr::null_mut::<AnyObject>();
        let by: CGPoint = msg_send![recognizer, translationInView: nil];
        let _: () = msg_send![recognizer, setTranslation: CGPoint::ZERO, inView: nil];
        let scale: f64 = msg_send![&*view, contentScaleFactor];
        if by != CGPoint::ZERO {
            push(Input::Scroll {
                dx: by.x * scale,
                dy: by.y * scale,
            });
        }
    }
}

fn define(
    name: &CStr,
    superclass: &CStr,
    methods: impl FnOnce(&mut ClassBuilder),
) -> &'static AnyClass {
    let superclass = AnyClass::get(superclass).expect("UIKit is loaded");
    let mut builder = ClassBuilder::new(name, superclass).expect("the class is defined once");
    methods(&mut builder);
    builder.register()
}

/// Watches the system's pointer on winit's view while it isn't locked: where it hovers, which
/// touches it makes (winit's `Touch` doesn't say what made it) and where they go, and scrolling
/// (the wheel, two fingers on the trackpad). None of it holds a touch back from the view.
pub fn watch_pointer(window: &Window) {
    static WATCHING: Once = Once::new();
    let Some(handle) = ui_handle(window) else {
        return;
    };
    WATCHING.call_once(|| {
        type Touches = extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject);
        type Action = extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject);
        let touches = define(
            c"HorizonPointerTouches",
            c"UIGestureRecognizer",
            |builder| {
                // SAFETY: the signatures of UIResponder's touch methods.
                unsafe {
                    builder.add_method(
                        sel!(touchesBegan:withEvent:),
                        pointer_touches_began as Touches,
                    );
                    builder.add_method(
                        sel!(touchesMoved:withEvent:),
                        pointer_touches_moved as Touches,
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
            },
        );
        let target = define(c"HorizonPointerTarget", c"NSObject", |builder| {
            // SAFETY: gesture recognizer actions take the recognizer.
            unsafe {
                builder.add_method(sel!(hover:), hovered as Action);
                builder.add_method(sel!(scroll:), scrolled as Action);
            }
        });
        let pointer_only = NSArray::from_retained_slice(&[NSNumber::numberWithInteger(3)]);
        let no_touches = NSArray::<NSNumber>::new();
        // SAFETY: UIKit calls on the main thread. The view retains its recognizers; they don't
        // retain their target, which lives as long as the app.
        unsafe {
            let view: &AnyObject = handle.ui_view.cast().as_ref();
            let target: Retained<AnyObject> = msg_send![target, new];
            let target = &*Retained::into_raw(target);
            let quiet = |recognizer: &AnyObject| {
                let _: () = msg_send![recognizer, setCancelsTouchesInView: false];
                let _: () = msg_send![recognizer, setDelaysTouchesEnded: false];
                let _: () = msg_send![view, addGestureRecognizer: recognizer];
            };
            let recognizer: Retained<AnyObject> = msg_send![touches, new];
            // UITouchTypeIndirectPointer.
            let _: () = msg_send![&*recognizer, setAllowedTouchTypes: &*pointer_only];
            quiet(&recognizer);
            let class = AnyClass::get(c"UIHoverGestureRecognizer").expect("UIKit is loaded");
            let recognizer: Allocated<AnyObject> = msg_send![class, alloc];
            let recognizer: Retained<AnyObject> =
                msg_send![recognizer, initWithTarget: target, action: sel!(hover:)];
            quiet(&recognizer);
            let class = AnyClass::get(c"UIPanGestureRecognizer").expect("UIKit is loaded");
            let recognizer: Allocated<AnyObject> = msg_send![class, alloc];
            let recognizer: Retained<AnyObject> =
                msg_send![recognizer, initWithTarget: target, action: sel!(scroll:)];
            // Scrolling only (UIScrollTypeMaskAll), no touches: fingers stay the game's.
            let _: () = msg_send![&*recognizer, setAllowedScrollTypesMask: 3usize];
            let _: () = msg_send![&*recognizer, setAllowedTouchTypes: &*no_touches];
            quiet(&recognizer);
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
