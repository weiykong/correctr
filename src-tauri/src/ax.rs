//! Where on screen is the text the user selected?
//!
//! Used to drop the preview HUD next to the selection instead of dead centre of
//! the display. Everything here is macOS Accessibility API: a system-wide
//! focused-element query, then the bounds of the selected range. Results are in
//! global display *points* with a top-left origin — the same space tao's
//! `set_outer_position` expects, so no y-flip is needed.

#![cfg(target_os = "macos")]
#![allow(non_camel_case_types, non_snake_case)]

use std::ffi::c_void;
use std::os::raw::c_char;

type AXUIElementRef = *const c_void;
type CFTypeRef = *const c_void;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CGSize {
    pub width: f64,
    pub height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CGRect {
    pub origin: CGPoint,
    pub size: CGSize,
}

/// A screen rectangle in global points, origin top-left.
pub type Rect = (f64, f64, f64, f64); // x, y, w, h

const AX_VALUE_CG_RECT: u32 = 3;
const UTF8: u32 = 0x0800_0100;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFTypeRef,
        out: *mut CFTypeRef,
    ) -> i32;
    fn AXUIElementCopyParameterizedAttributeValue(
        element: AXUIElementRef,
        attribute: CFTypeRef,
        parameter: CFTypeRef,
        out: *mut CFTypeRef,
    ) -> i32;
    fn AXValueGetValue(value: CFTypeRef, the_type: u32, out: *mut c_void) -> u8;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringCreateWithCString(
        alloc: *const c_void,
        s: *const c_char,
        enc: u32,
    ) -> CFTypeRef;
    fn CFRelease(cf: *mut c_void);
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventCreate(source: *const c_void) -> CFTypeRef;
    fn CGEventGetLocation(event: CFTypeRef) -> CGPoint;
}

fn cfstring(s: &str) -> Option<CFTypeRef> {
    let c = std::ffi::CString::new(s).ok()?;
    let r = unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8) };
    if r.is_null() {
        None
    } else {
        Some(r)
    }
}

unsafe fn attr(element: AXUIElementRef, name: &str) -> Option<CFTypeRef> {
    let key = cfstring(name)?;
    let mut out: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(element, key, &mut out);
    CFRelease(key as *mut c_void);
    if err == 0 && !out.is_null() {
        Some(out)
    } else {
        None
    }
}

fn rect_from(value: CFTypeRef) -> Option<Rect> {
    let mut r = CGRect::default();
    let ok = unsafe { AXValueGetValue(value, AX_VALUE_CG_RECT, &mut r as *mut _ as *mut c_void) };
    if ok == 0 || (r.size.width <= 0.0 && r.size.height <= 0.0) {
        return None;
    }
    Some((r.origin.x, r.origin.y, r.size.width, r.size.height))
}

/// Bounds of the current text selection in the frontmost app, in global points.
///
/// Falls back to the focused element's frame (a text field, a web view) and
/// finally to the mouse pointer, so there is always *something* to point at.
pub fn selection_rect() -> Option<Rect> {
    unsafe {
        let sys = AXUIElementCreateSystemWide();
        if !sys.is_null() {
            if let Some(app) = attr(sys, "AXFocusedApplication") {
                if let Some(el) = attr(app as AXUIElementRef, "AXFocusedUIElement") {
                    // 1. The selection itself.
                    if let Some(range) = attr(el as AXUIElementRef, "AXSelectedTextRange") {
                        if let Some(key) = cfstring("AXBoundsForRange") {
                            let mut out: CFTypeRef = std::ptr::null();
                            let err = AXUIElementCopyParameterizedAttributeValue(
                                el as AXUIElementRef,
                                key,
                                range,
                                &mut out,
                            );
                            CFRelease(key as *mut c_void);
                            if err == 0 && !out.is_null() {
                                let r = rect_from(out);
                                CFRelease(out as *mut c_void);
                                CFRelease(range as *mut c_void);
                                CFRelease(el as *mut c_void);
                                CFRelease(app as *mut c_void);
                                CFRelease(sys as *mut c_void);
                                if let Some(r) = r {
                                    if r.2 > 0.5 && r.3 > 0.5 {
                                        return Some(r);
                                    }
                                }
                                return mouse_point();
                            }
                            CFRelease(out as *mut c_void);
                        }
                        CFRelease(range as *mut c_void);
                    }

                    // 2. The control the caret sits in.
                    if let Some(frame) = attr(el as AXUIElementRef, "AXFrame") {
                        let r = rect_from(frame);
                        CFRelease(frame as *mut c_void);
                        CFRelease(el as *mut c_void);
                        CFRelease(app as *mut c_void);
                        CFRelease(sys as *mut c_void);
                        if r.is_some() {
                            return r;
                        }
                        return mouse_point();
                    }
                    CFRelease(el as *mut c_void);
                }
                CFRelease(app as *mut c_void);
            }
            CFRelease(sys as *mut c_void);
        }
    }
    mouse_point()
}

fn mouse_point() -> Option<Rect> {
    unsafe {
        let ev = CGEventCreate(std::ptr::null());
        if ev.is_null() {
            return None;
        }
        let p = CGEventGetLocation(ev);
        CFRelease(ev as *mut c_void);
        Some((p.x, p.y, 1.0, 18.0))
    }
}
