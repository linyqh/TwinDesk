//! Raw CoreGraphics / CoreFoundation declarations used by the input layer.
//!
//! Values and signatures are taken from the macOS SDK headers
//! (CGEventTypes.h, CGEvent.h, CGRemoteOperation.h, IOLLEvent.h). Only what
//! this crate actually calls is declared.

#![allow(non_upper_case_globals, non_camel_case_types, dead_code)]

use std::ffi::c_void;

pub type CGEventRef = *mut c_void;
pub type CGEventSourceRef = *mut c_void;
pub type CFMachPortRef = *mut c_void;
pub type CFRunLoopRef = *mut c_void;
pub type CFRunLoopSourceRef = *mut c_void;
pub type CFStringRef = *const c_void;
pub type CGEventTapProxy = *mut c_void;
pub type CGEventType = u32;
pub type CGEventField = u32;
pub type CGEventFlags = u64;
pub type CGEventMask = u64;
pub type CGDirectDisplayID = u32;
pub type CGError = i32;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGSize {
    pub width: f64,
    pub height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGRect {
    pub origin: CGPoint,
    pub size: CGSize,
}

pub type CGEventTapCallBack = unsafe extern "C" fn(
    proxy: CGEventTapProxy,
    event_type: CGEventType,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef;

// CGEventType (IOLLEvent.h NX_* values)
pub const kCGEventLeftMouseDown: CGEventType = 1;
pub const kCGEventLeftMouseUp: CGEventType = 2;
pub const kCGEventRightMouseDown: CGEventType = 3;
pub const kCGEventRightMouseUp: CGEventType = 4;
pub const kCGEventMouseMoved: CGEventType = 5;
pub const kCGEventLeftMouseDragged: CGEventType = 6;
pub const kCGEventRightMouseDragged: CGEventType = 7;
pub const kCGEventKeyDown: CGEventType = 10;
pub const kCGEventKeyUp: CGEventType = 11;
pub const kCGEventFlagsChanged: CGEventType = 12;
pub const kCGEventScrollWheel: CGEventType = 22;
pub const kCGEventOtherMouseDown: CGEventType = 25;
pub const kCGEventOtherMouseUp: CGEventType = 26;
pub const kCGEventOtherMouseDragged: CGEventType = 27;
pub const kCGEventTapDisabledByTimeout: CGEventType = 0xFFFF_FFFE;
pub const kCGEventTapDisabledByUserInput: CGEventType = 0xFFFF_FFFF;

// CGEventField
pub const kCGMouseEventClickState: CGEventField = 1;
pub const kCGMouseEventButtonNumber: CGEventField = 3;
pub const kCGMouseEventDeltaX: CGEventField = 4;
pub const kCGMouseEventDeltaY: CGEventField = 5;
pub const kCGKeyboardEventAutorepeat: CGEventField = 8;
pub const kCGKeyboardEventKeycode: CGEventField = 9;
pub const kCGScrollWheelEventDeltaAxis1: CGEventField = 11;
pub const kCGScrollWheelEventDeltaAxis2: CGEventField = 12;
pub const kCGEventSourceUserData: CGEventField = 42;
pub const kCGScrollWheelEventIsContinuous: CGEventField = 88;
pub const kCGScrollWheelEventPointDeltaAxis1: CGEventField = 96;
pub const kCGScrollWheelEventPointDeltaAxis2: CGEventField = 97;
pub const kCGScrollWheelEventScrollPhase: CGEventField = 99;
pub const kCGScrollWheelEventMomentumPhase: CGEventField = 123;

// Tap configuration
pub const kCGHIDEventTap: u32 = 0;
pub const kCGHeadInsertEventTap: u32 = 0;
pub const kCGEventTapOptionDefault: u32 = 0;
pub const kCGEventSourceStatePrivate: i32 = -1;
pub const kCGEventSourceStateHIDSystemState: i32 = 1;

// CGScrollEventUnit
pub const kCGScrollEventUnitPixel: u32 = 0;
pub const kCGScrollEventUnitLine: u32 = 1;

// CGMouseButton
pub const kCGMouseButtonLeft: u32 = 0;
pub const kCGMouseButtonRight: u32 = 1;

pub const fn event_mask(t: CGEventType) -> CGEventMask {
    1 << t
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    pub fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: CGEventMask,
        callback: CGEventTapCallBack,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
    pub fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);

    pub fn CGEventGetType(event: CGEventRef) -> CGEventType;
    pub fn CGEventSetType(event: CGEventRef, t: CGEventType);
    pub fn CGEventGetLocation(event: CGEventRef) -> CGPoint;
    pub fn CGEventSetLocation(event: CGEventRef, p: CGPoint);
    pub fn CGEventGetFlags(event: CGEventRef) -> CGEventFlags;
    pub fn CGEventSetFlags(event: CGEventRef, flags: CGEventFlags);
    pub fn CGEventGetIntegerValueField(event: CGEventRef, field: CGEventField) -> i64;
    pub fn CGEventSetIntegerValueField(event: CGEventRef, field: CGEventField, value: i64);

    pub fn CGEventSourceCreate(state: i32) -> CGEventSourceRef;
    pub fn CGEventSourceSetLocalEventsSuppressionInterval(source: CGEventSourceRef, seconds: f64);
    pub fn CGEventCreate(source: CGEventSourceRef) -> CGEventRef;
    pub fn CGEventCreateMouseEvent(
        source: CGEventSourceRef,
        mouse_type: CGEventType,
        cursor: CGPoint,
        button: u32,
    ) -> CGEventRef;
    pub fn CGEventCreateKeyboardEvent(
        source: CGEventSourceRef,
        keycode: u16,
        key_down: bool,
    ) -> CGEventRef;
    pub fn CGEventCreateScrollWheelEvent2(
        source: CGEventSourceRef,
        units: u32,
        wheel_count: u32,
        wheel1: i32,
        wheel2: i32,
        wheel3: i32,
    ) -> CGEventRef;
    pub fn CGEventPost(tap: u32, event: CGEventRef);
    pub fn CGEventSourceButtonState(state: i32, button: u32) -> bool;

    pub fn CGWarpMouseCursorPosition(p: CGPoint) -> CGError;
    pub fn CGAssociateMouseAndMouseCursorPosition(connected: i32) -> CGError;
    pub fn CGDisplayHideCursor(display: CGDirectDisplayID) -> CGError;
    pub fn CGDisplayShowCursor(display: CGDirectDisplayID) -> CGError;
    pub fn CGGetActiveDisplayList(
        max: u32,
        displays: *mut CGDirectDisplayID,
        count: *mut u32,
    ) -> CGError;
    pub fn CGDisplayBounds(display: CGDirectDisplayID) -> CGRect;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub static kCFRunLoopCommonModes: CFStringRef;
    pub fn CFMachPortCreateRunLoopSource(
        allocator: *const c_void,
        port: CFMachPortRef,
        order: isize,
    ) -> CFRunLoopSourceRef;
    pub fn CFMachPortInvalidate(port: CFMachPortRef);
    pub fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    pub fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    pub fn CFRunLoopRun();
    pub fn CFRunLoopStop(rl: CFRunLoopRef);
    pub fn CFRelease(cf: *const c_void);
}
