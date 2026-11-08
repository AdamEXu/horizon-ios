//! The deck's SwiftUI page view (`swift/DeckPages.swift`), behind `window::PagerHost`.
#![allow(unsafe_code)]

use horizon_client_input::window::PagerHost;
use std::ffi::{CString, c_void};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

unsafe extern "C" {
    fn horizon_deck_attach(view: *mut c_void);
    fn horizon_deck_layout(x: f64, y: f64, width: f64, height: f64, visible: bool);
    fn horizon_deck_titles(text: *const std::ffi::c_char);
    fn horizon_deck_offset() -> f64;
    fn horizon_deck_bands(top: *mut f64, bottom: *mut f64);
    fn horizon_deck_next_tap(page: *mut i32, x: *mut f64, y: *mut f64) -> bool;
}

pub const HOST: PagerHost = PagerHost {
    update,
    offset,
    bands,
    next_tap,
};

// SAFETY (all below): the Swift functions run on the main thread, which winit's event loop and
// so every caller is on; the view is winit's live UIView; pointers are to live locals.

fn update(window: &Window, rect: Option<[f64; 4]>, titles: &[String]) {
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::UiKit(handle) = handle.as_raw() else {
        return;
    };
    unsafe { horizon_deck_attach(handle.ui_view.as_ptr()) };
    let scale = window.scale_factor();
    if let Ok(text) = CString::new(titles.join("\n")) {
        unsafe { horizon_deck_titles(text.as_ptr()) };
    }
    match rect {
        Some([x, y, w, h]) => unsafe {
            horizon_deck_layout(x / scale, y / scale, w / scale, h / scale, true)
        },
        None => unsafe { horizon_deck_layout(0.0, 0.0, 0.0, 0.0, false) },
    }
}

fn offset() -> f64 {
    unsafe { horizon_deck_offset() }
}

fn bands() -> (f64, f64) {
    let (mut top, mut bottom) = (0.0, 0.0);
    unsafe { horizon_deck_bands(&mut top, &mut bottom) };
    (top, bottom)
}

fn next_tap() -> Option<(usize, f64, f64)> {
    let (mut page, mut x, mut y) = (0i32, 0.0, 0.0);
    unsafe { horizon_deck_next_tap(&mut page, &mut x, &mut y) }
        .then(|| (page.max(0) as usize, x, y))
}
