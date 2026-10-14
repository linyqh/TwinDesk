//! Input capture and injection on macOS.
//!
//! One background thread owns a filtering Quartz event tap on its own run
//! loop. The tap callback does fixed-size work only: read the current mode,
//! translate the event, push it to a channel, and decide whether the local
//! OS sees it. Network and UI code never run on that thread.
//!
//! Modes:
//! - `Local`: everything passes through; mouse motion is reported so the
//!   engine can detect the screen edge.
//! - `Forwarding`: this Mac controls the peer. Local delivery is suppressed,
//!   the cursor is frozen and hidden, raw deltas are forwarded.
//! - `Controlled`: the peer controls this Mac. Injected events pass; any
//!   physical input is reported so the user can take the Mac back.

#![allow(non_upper_case_globals)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use osm_core::keys::modifier_is_down;
use osm_model::{LogicalPoint, LogicalRect};

use crate::sys::*;

/// Tag stamped into `kCGEventSourceUserData` of every event we post, so the
/// tap recognises its own injections and never forwards them back.
pub const INJECTED_TAG: i64 = 0x004F_534D_3149_4E4A; // "OSM1INJ"

/// Bounded so a stalled consumer can never grow memory or block the tap.
const CHANNEL_CAPACITY: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CaptureMode {
    Local = 0,
    Forwarding = 1,
    Controlled = 2,
}

impl CaptureMode {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Forwarding,
            2 => Self::Controlled,
            _ => Self::Local,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InputEvent {
    /// Local mode: physical motion with the cursor position after it.
    /// `left_down` is set for left-button drags.
    LocalMove {
        pos: LogicalPoint,
        dx: f64,
        dy: f64,
        left_down: bool,
    },
    /// Local mode: the physical left button went down or up.
    LocalLeftButton { down: bool },
    /// Forwarding mode: raw motion delta in points.
    Move { dx: i32, dy: i32 },
    Button {
        button: u32,
        down: bool,
        click_state: i64,
        flags: u64,
    },
    Scroll {
        continuous: bool,
        dy: i32,
        dx: i32,
        scroll_phase: u32,
        momentum_phase: u32,
    },
    Key {
        keycode: u16,
        down: bool,
        repeat: bool,
        flags: u64,
    },
    /// Controlled mode: the user touched this Mac's own keyboard or mouse.
    LocalActivity,
    /// The system disabled the tap (timeout or permission change). The
    /// engine must fall back to local control.
    TapDisabled,
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("无法创建事件监听（请在“系统设置 > 隐私与安全性 > 辅助功能”中允许 OpenShareMouse）")]
    TapCreateFailed,
    #[error("事件监听线程启动失败")]
    ThreadFailed,
}

struct TapState {
    mode: Arc<AtomicU8>,
    tx: SyncSender<InputEvent>,
    /// The tap port, so the callback can re-enable it after a timeout.
    port: std::sync::atomic::AtomicPtr<c_void>,
}

/// Handle to the running tap thread. Dropping it stops the tap and restores
/// local control.
#[derive(Debug)]
pub struct Capture {
    mode: Arc<AtomicU8>,
    run_loop: Arc<Mutex<usize>>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for TapState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TapState").finish_non_exhaustive()
    }
}

impl Capture {
    pub fn start() -> Result<(Self, Receiver<InputEvent>), CaptureError> {
        let (tx, rx) = sync_channel(CHANNEL_CAPACITY);
        let mode = Arc::new(AtomicU8::new(CaptureMode::Local as u8));
        let run_loop = Arc::new(Mutex::new(0usize));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), CaptureError>>();

        let thread_mode = mode.clone();
        let thread_rl = run_loop.clone();
        let thread = std::thread::Builder::new()
            .name("osm-event-tap".into())
            .spawn(move || {
                // Leaked on purpose: the callback may still fire while the
                // run loop winds down, so the state must outlive the thread.
                let state: &'static TapState = Box::leak(Box::new(TapState {
                    mode: thread_mode,
                    tx,
                    port: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
                }));
                let mask = [
                    kCGEventLeftMouseDown,
                    kCGEventLeftMouseUp,
                    kCGEventRightMouseDown,
                    kCGEventRightMouseUp,
                    kCGEventMouseMoved,
                    kCGEventLeftMouseDragged,
                    kCGEventRightMouseDragged,
                    kCGEventKeyDown,
                    kCGEventKeyUp,
                    kCGEventFlagsChanged,
                    kCGEventScrollWheel,
                    kCGEventOtherMouseDown,
                    kCGEventOtherMouseUp,
                    kCGEventOtherMouseDragged,
                ]
                .into_iter()
                .fold(0, |m, t| m | event_mask(t));

                // SAFETY: plain CG/CF calls on this thread; `state` is 'static.
                unsafe {
                    let port = CGEventTapCreate(
                        kCGHIDEventTap,
                        kCGHeadInsertEventTap,
                        kCGEventTapOptionDefault,
                        mask,
                        tap_callback,
                        state as *const TapState as *mut c_void,
                    );
                    if port.is_null() {
                        let _ = ready_tx.send(Err(CaptureError::TapCreateFailed));
                        return;
                    }
                    state.port.store(port, Ordering::Release);
                    let source = CFMachPortCreateRunLoopSource(std::ptr::null(), port, 0);
                    let rl = CFRunLoopGetCurrent();
                    CFRunLoopAddSource(rl, source, kCFRunLoopCommonModes);
                    CGEventTapEnable(port, true);
                    *thread_rl.lock().unwrap() = rl as usize;
                    let _ = ready_tx.send(Ok(()));

                    CFRunLoopRun();

                    CGEventTapEnable(port, false);
                    CFMachPortInvalidate(port);
                    CFRelease(source);
                    CFRelease(port);
                }
            })
            .map_err(|_| CaptureError::ThreadFailed)?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok((
                Self {
                    mode,
                    run_loop,
                    thread: Some(thread),
                },
                rx,
            )),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => Err(CaptureError::ThreadFailed),
        }
    }

    pub fn set_mode(&self, mode: CaptureMode) {
        self.mode.store(mode as u8, Ordering::Release);
    }

    pub fn mode(&self) -> CaptureMode {
        CaptureMode::from_u8(self.mode.load(Ordering::Acquire))
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.set_mode(CaptureMode::Local);
        let rl = *self.run_loop.lock().unwrap();
        if rl != 0 {
            // SAFETY: rl is the tap thread's run loop, still alive until join.
            unsafe { CFRunLoopStop(rl as CFRunLoopRef) };
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn send(state: &TapState, ev: InputEvent) -> bool {
    match state.tx.try_send(ev) {
        Ok(()) => true,
        // Dropping a motion sample is fine; losing a key or button edge is
        // not, so the caller falls back to local delivery for those.
        Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
    }
}

unsafe extern "C" fn tap_callback(
    _proxy: CGEventTapProxy,
    event_type: CGEventType,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef {
    // SAFETY: user_info is the leaked 'static TapState from Capture::start.
    let state = unsafe { &*(user_info as *const TapState) };

    if event_type == kCGEventTapDisabledByTimeout || event_type == kCGEventTapDisabledByUserInput {
        // Re-arm, but tell the engine: while the tap was off, suppression did
        // not happen, so a forwarding session is no longer trustworthy.
        let port = state.port.load(Ordering::Acquire);
        if !port.is_null() {
            // SAFETY: port is the live tap created on this thread.
            unsafe { CGEventTapEnable(port, true) };
        }
        state
            .mode
            .store(CaptureMode::Local as u8, Ordering::Release);
        let _ = state.tx.try_send(InputEvent::TapDisabled);
        return event;
    }

    // SAFETY: event is valid for the duration of the callback.
    let injected =
        unsafe { CGEventGetIntegerValueField(event, kCGEventSourceUserData) } == INJECTED_TAG;
    let mode = CaptureMode::from_u8(state.mode.load(Ordering::Acquire));

    match mode {
        CaptureMode::Local => {
            if !injected && is_motion(event_type) {
                // SAFETY: as above.
                let (pos, dx, dy) = unsafe {
                    let p = CGEventGetLocation(event);
                    (
                        LogicalPoint { x: p.x, y: p.y },
                        CGEventGetIntegerValueField(event, kCGMouseEventDeltaX) as f64,
                        CGEventGetIntegerValueField(event, kCGMouseEventDeltaY) as f64,
                    )
                };
                let _ = state.tx.try_send(InputEvent::LocalMove {
                    pos,
                    dx,
                    dy,
                    left_down: event_type == kCGEventLeftMouseDragged,
                });
            } else if !injected
                && (event_type == kCGEventLeftMouseDown || event_type == kCGEventLeftMouseUp)
            {
                let _ = state.tx.try_send(InputEvent::LocalLeftButton {
                    down: event_type == kCGEventLeftMouseDown,
                });
            }
            event
        }
        CaptureMode::Controlled => {
            if !injected && is_physical_activity(event_type) {
                let _ = state.tx.try_send(InputEvent::LocalActivity);
            }
            event
        }
        CaptureMode::Forwarding => {
            if injected {
                return event;
            }
            // SAFETY: event is valid for the duration of the callback.
            let Some(ev) = (unsafe { translate(event_type, event) }) else {
                return event;
            };
            // If the queue is full, a dropped motion sample is harmless, but
            // a key or button edge must reach some OS rather than vanish.
            if send(state, ev) || matches!(ev, InputEvent::Move { .. }) {
                std::ptr::null_mut()
            } else {
                event
            }
        }
    }
}

fn is_motion(t: CGEventType) -> bool {
    matches!(
        t,
        kCGEventMouseMoved
            | kCGEventLeftMouseDragged
            | kCGEventRightMouseDragged
            | kCGEventOtherMouseDragged
    )
}

fn is_physical_activity(t: CGEventType) -> bool {
    // Small accidental mouse jitter should not steal control back, so only
    // clicks, keys and scrolling count.
    matches!(
        t,
        kCGEventLeftMouseDown
            | kCGEventRightMouseDown
            | kCGEventOtherMouseDown
            | kCGEventKeyDown
            | kCGEventScrollWheel
    )
}

/// # Safety
/// `event` must be a valid CGEventRef.
unsafe fn translate(t: CGEventType, event: CGEventRef) -> Option<InputEvent> {
    // SAFETY: caller guarantees a valid event.
    unsafe {
        let int = |f| CGEventGetIntegerValueField(event, f);
        let flags = CGEventGetFlags(event);
        Some(match t {
            kCGEventMouseMoved
            | kCGEventLeftMouseDragged
            | kCGEventRightMouseDragged
            | kCGEventOtherMouseDragged => InputEvent::Move {
                dx: int(kCGMouseEventDeltaX) as i32,
                dy: int(kCGMouseEventDeltaY) as i32,
            },
            kCGEventLeftMouseDown
            | kCGEventLeftMouseUp
            | kCGEventRightMouseDown
            | kCGEventRightMouseUp
            | kCGEventOtherMouseDown
            | kCGEventOtherMouseUp => InputEvent::Button {
                button: int(kCGMouseEventButtonNumber) as u32,
                down: matches!(
                    t,
                    kCGEventLeftMouseDown | kCGEventRightMouseDown | kCGEventOtherMouseDown
                ),
                click_state: int(kCGMouseEventClickState),
                flags,
            },
            kCGEventScrollWheel => {
                let continuous = int(kCGScrollWheelEventIsContinuous) != 0;
                let (dy, dx) = if continuous {
                    (
                        int(kCGScrollWheelEventPointDeltaAxis1),
                        int(kCGScrollWheelEventPointDeltaAxis2),
                    )
                } else {
                    (
                        int(kCGScrollWheelEventDeltaAxis1),
                        int(kCGScrollWheelEventDeltaAxis2),
                    )
                };
                InputEvent::Scroll {
                    continuous,
                    dy: dy as i32,
                    dx: dx as i32,
                    scroll_phase: int(kCGScrollWheelEventScrollPhase) as u32,
                    momentum_phase: int(kCGScrollWheelEventMomentumPhase) as u32,
                }
            }
            kCGEventKeyDown | kCGEventKeyUp => InputEvent::Key {
                keycode: int(kCGKeyboardEventKeycode) as u16,
                down: t == kCGEventKeyDown,
                repeat: int(kCGKeyboardEventAutorepeat) != 0,
                flags,
            },
            kCGEventFlagsChanged => {
                let keycode = int(kCGKeyboardEventKeycode) as u16;
                InputEvent::Key {
                    keycode,
                    down: modifier_is_down(keycode, flags),
                    repeat: false,
                    flags,
                }
            }
            _ => return None,
        })
    }
}

/// Hardware state of the left button, whether or not our tap let the press
/// through to this Mac.
pub fn physical_left_down() -> bool {
    // SAFETY: plain query.
    unsafe { CGEventSourceButtonState(kCGEventSourceStateHIDSystemState, kCGMouseButtonLeft) }
}

/// Current cursor location in Quartz global coordinates.
pub fn cursor_position() -> LogicalPoint {
    // SAFETY: CGEventCreate(NULL) carries the current location; released
    // right after reading it.
    unsafe {
        let e = CGEventCreate(std::ptr::null_mut());
        if e.is_null() {
            return LogicalPoint { x: 0.0, y: 0.0 };
        }
        let p = CGEventGetLocation(e);
        CFRelease(e);
        LogicalPoint { x: p.x, y: p.y }
    }
}

/// Active displays in Quartz global coordinates (points, y down).
pub fn display_rects() -> Vec<LogicalRect> {
    let mut ids = [0u32; 16];
    let mut count = 0u32;
    // SAFETY: buffer of 16 ids, count written by the call.
    let err = unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) };
    if err != 0 {
        return Vec::new();
    }
    ids[..count as usize]
        .iter()
        .map(|&id| {
            // SAFETY: id comes from the active display list.
            let r = unsafe { CGDisplayBounds(id) };
            LogicalRect {
                x: r.origin.x,
                y: r.origin.y,
                w: r.size.width,
                h: r.size.height,
            }
        })
        .collect()
}

/// Posts events into the HID stream on behalf of the controlling peer and
/// tracks what it holds down so it can release exactly that.
#[derive(Debug)]
pub struct Injector {
    source: usize,
    pos: LogicalPoint,
    buttons_down: [bool; 32],
    keys_down: Vec<u16>,
    flags: u64,
}

// SAFETY: the CGEventSource is only used from the thread owning the Injector;
// the engine moves it between threads but never shares it.
unsafe impl Send for Injector {}

impl Default for Injector {
    fn default() -> Self {
        Self::new()
    }
}

impl Injector {
    pub fn new() -> Self {
        // SAFETY: plain constructor; a private state table keeps our
        // injected modifier state separate from the physical keyboard's.
        let source = unsafe { CGEventSourceCreate(kCGEventSourceStatePrivate) };
        if !source.is_null() {
            // Without this, local hardware events are suppressed for 0.25 s
            // after every injected event, which makes taking control back
            // with the local mouse feel broken.
            // SAFETY: valid source.
            unsafe { CGEventSourceSetLocalEventsSuppressionInterval(source, 0.0) };
        }
        Self {
            source: source as usize,
            pos: LogicalPoint { x: 0.0, y: 0.0 },
            buttons_down: [false; 32],
            keys_down: Vec::new(),
            flags: 0,
        }
    }

    pub fn position(&self) -> LogicalPoint {
        self.pos
    }

    /// Moves the cursor without generating an event (used on enter).
    pub fn warp(&mut self, p: LogicalPoint) {
        self.pos = p;
        // SAFETY: plain call.
        unsafe { CGWarpMouseCursorPosition(CGPoint { x: p.x, y: p.y }) };
    }

    /// Posts motion to an already-clamped absolute position, as a drag when
    /// a button is held so selections and window moves work.
    pub fn move_to(&mut self, p: LogicalPoint, dx: i32, dy: i32) {
        self.pos = p;
        let (t, button) = if self.buttons_down[0] {
            (kCGEventLeftMouseDragged, kCGMouseButtonLeft)
        } else if self.buttons_down[1] {
            (kCGEventRightMouseDragged, kCGMouseButtonRight)
        } else if let Some(b) = self.buttons_down.iter().skip(2).position(|d| *d) {
            (kCGEventOtherMouseDragged, b as u32 + 2)
        } else {
            (kCGEventMouseMoved, kCGMouseButtonLeft)
        };
        self.post_mouse(t, button, |e| {
            // SAFETY: e is a fresh event owned by post_mouse.
            unsafe {
                CGEventSetIntegerValueField(e, kCGMouseEventDeltaX, dx as i64);
                CGEventSetIntegerValueField(e, kCGMouseEventDeltaY, dy as i64);
            }
        });
    }

    pub fn button(&mut self, button: u32, down: bool, click_state: i64) {
        let idx = (button as usize).min(31);
        if self.buttons_down[idx] == down {
            return;
        }
        self.buttons_down[idx] = down;
        let t = match (button, down) {
            (0, true) => kCGEventLeftMouseDown,
            (0, false) => kCGEventLeftMouseUp,
            (1, true) => kCGEventRightMouseDown,
            (1, false) => kCGEventRightMouseUp,
            (_, true) => kCGEventOtherMouseDown,
            (_, false) => kCGEventOtherMouseUp,
        };
        self.post_mouse(t, button, |e| {
            // SAFETY: e is a fresh event owned by post_mouse.
            unsafe {
                CGEventSetIntegerValueField(e, kCGMouseEventClickState, click_state.max(1));
                CGEventSetIntegerValueField(e, kCGMouseEventButtonNumber, button as i64);
            }
        });
    }

    pub fn scroll(
        &mut self,
        continuous: bool,
        dy: i32,
        dx: i32,
        scroll_phase: u32,
        momentum_phase: u32,
    ) {
        let unit = if continuous {
            kCGScrollEventUnitPixel
        } else {
            kCGScrollEventUnitLine
        };
        // SAFETY: fresh event, released below.
        unsafe {
            let e = CGEventCreateScrollWheelEvent2(self.source(), unit, 2, dy, dx, 0);
            if e.is_null() {
                return;
            }
            if continuous {
                CGEventSetIntegerValueField(e, kCGScrollWheelEventIsContinuous, 1);
                CGEventSetIntegerValueField(e, kCGScrollWheelEventScrollPhase, scroll_phase as i64);
                CGEventSetIntegerValueField(
                    e,
                    kCGScrollWheelEventMomentumPhase,
                    momentum_phase as i64,
                );
            }
            CGEventSetLocation(
                e,
                CGPoint {
                    x: self.pos.x,
                    y: self.pos.y,
                },
            );
            self.finish(e);
        }
    }

    pub fn key(&mut self, keycode: u16, down: bool, repeat: bool, flags: u64) {
        if down {
            if !repeat && !self.keys_down.contains(&keycode) {
                self.keys_down.push(keycode);
            }
        } else {
            self.keys_down.retain(|k| *k != keycode);
        }
        self.flags = flags;
        // SAFETY: fresh event, released in finish.
        unsafe {
            let e = CGEventCreateKeyboardEvent(self.source(), keycode, down);
            if e.is_null() {
                return;
            }
            // Explicit flags keep modifiers from sticking when the remote
            // and local modifier state disagree.
            CGEventSetFlags(e, flags);
            if repeat {
                CGEventSetIntegerValueField(e, kCGKeyboardEventAutorepeat, 1);
            }
            self.finish(e);
        }
    }

    /// Presses and releases Escape with no modifiers.
    pub fn tap_escape(&mut self) {
        const KEY_ESCAPE: u16 = 53;
        self.key(KEY_ESCAPE, true, false, 0);
        self.key(KEY_ESCAPE, false, false, 0);
    }

    /// Releases every key and button this injector pressed, and nothing else.
    pub fn release_all(&mut self) {
        self.release_all_but_left(false);
    }

    /// Like [`release_all`](Self::release_all), but with `keep_left` the left
    /// button stays down: a file drag started on this Mac is still held while
    /// the cursor is on the peer, and releasing it here would drop the files.
    pub fn release_all_but_left(&mut self, keep_left: bool) {
        for keycode in std::mem::take(&mut self.keys_down).into_iter().rev() {
            self.flags = 0;
            // SAFETY: fresh event, released in finish.
            unsafe {
                let e = CGEventCreateKeyboardEvent(self.source(), keycode, false);
                if !e.is_null() {
                    CGEventSetFlags(e, 0);
                    self.finish(e);
                }
            }
        }
        for b in 0..self.buttons_down.len() {
            if b == 0 && keep_left {
                continue;
            }
            if self.buttons_down[b] {
                self.button(b as u32, false, 1);
            }
        }
    }

    pub fn left_down(&self) -> bool {
        self.buttons_down[0]
    }

    /// Re-posts the current position so an in-progress drag re-targets
    /// whatever window is under the cursor now (e.g. a shield that just
    /// appeared under a stationary drag).
    pub fn nudge(&mut self) {
        self.move_to(self.pos, 0, 0);
    }

    /// Posts a left-button edge at the current cursor position without
    /// touching the tracked button state. Used on the Mac whose physical
    /// button is held: the OS never saw its press (it was forwarded), so the
    /// drag hand-off has to synthesize one, and later end it.
    pub fn post_untracked_left(&mut self, edge: UntrackedLeft) {
        let at = cursor_position();
        let t = match edge {
            UntrackedLeft::Down => kCGEventLeftMouseDown,
            UntrackedLeft::Drag => kCGEventLeftMouseDragged,
            UntrackedLeft::Up => kCGEventLeftMouseUp,
        };
        let saved = self.pos;
        self.pos = at;
        self.post_mouse(t, kCGMouseButtonLeft, |e| {
            // SAFETY: e is a fresh event owned by post_mouse.
            unsafe { CGEventSetIntegerValueField(e, kCGMouseEventClickState, 1) };
        });
        self.pos = saved;
    }

    fn source(&self) -> CGEventSourceRef {
        self.source as CGEventSourceRef
    }

    fn post_mouse(&mut self, t: CGEventType, button: u32, configure: impl FnOnce(CGEventRef)) {
        // SAFETY: fresh event, released in finish.
        unsafe {
            let e = CGEventCreateMouseEvent(
                self.source(),
                t,
                CGPoint {
                    x: self.pos.x,
                    y: self.pos.y,
                },
                button,
            );
            if e.is_null() {
                return;
            }
            CGEventSetFlags(e, self.flags);
            configure(e);
            self.finish(e);
        }
    }

    /// # Safety
    /// `e` must be a fresh event this function takes ownership of.
    unsafe fn finish(&self, e: CGEventRef) {
        // SAFETY: caller hands over a valid, owned event.
        unsafe {
            CGEventSetIntegerValueField(e, kCGEventSourceUserData, INJECTED_TAG);
            CGEventPost(kCGHIDEventTap, e);
            CFRelease(e);
        }
    }
}

impl Drop for Injector {
    fn drop(&mut self) {
        self.release_all();
        if self.source != 0 {
            // SAFETY: source came from CGEventSourceCreate.
            unsafe { CFRelease(self.source()) };
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UntrackedLeft {
    Down,
    Drag,
    Up,
}

/// Freezes and hides the local cursor while this Mac controls the peer.
#[derive(Debug, Default)]
pub struct CursorLock {
    locked: Option<LogicalPoint>,
}

impl CursorLock {
    pub fn is_locked(&self) -> bool {
        self.locked.is_some()
    }

    /// Detaches the cursor from mouse motion at `park` and hides it.
    pub fn lock(&mut self, park: LogicalPoint) {
        if self.locked.is_some() {
            return;
        }
        self.locked = Some(park);
        // SAFETY: plain calls; hide/show are balanced via `locked`.
        unsafe {
            CGWarpMouseCursorPosition(CGPoint {
                x: park.x,
                y: park.y,
            });
            CGAssociateMouseAndMouseCursorPosition(0);
            hide_cursor_in_background();
            CGDisplayHideCursor(0);
        }
    }

    /// Reattaches and shows the cursor, optionally placing it at `at`.
    pub fn unlock(&mut self, at: Option<LogicalPoint>) {
        let Some(park) = self.locked.take() else {
            return;
        };
        let p = at.unwrap_or(park);
        // SAFETY: plain calls; balanced with lock().
        unsafe {
            CGWarpMouseCursorPosition(CGPoint { x: p.x, y: p.y });
            CGAssociateMouseAndMouseCursorPosition(1);
            CGDisplayShowCursor(0);
        }
    }
}

impl Drop for CursorLock {
    fn drop(&mut self) {
        self.unlock(None);
    }
}

/// A background app cannot hide the cursor without the private
/// `SetsCursorInBackground` connection property (what Synergy/Barrier do).
/// Looked up dynamically so an OS that removes it just leaves the cursor
/// visible instead of failing to launch.
fn hide_cursor_in_background() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        type MainConn = unsafe extern "C" fn() -> i32;
        type SetProp = unsafe extern "C" fn(i32, i32, *const c_void, *const c_void) -> i32;
        let main = libc::dlsym(libc::RTLD_DEFAULT, c"CGSMainConnectionID".as_ptr());
        let set = libc::dlsym(libc::RTLD_DEFAULT, c"CGSSetConnectionProperty".as_ptr());
        if main.is_null() || set.is_null() {
            tracing::warn!("SetsCursorInBackground unavailable; cursor stays visible");
            return;
        }
        let main: MainConn = std::mem::transmute(main);
        let set: SetProp = std::mem::transmute(set);
        unsafe extern "C" {
            static kCFBooleanTrue: *const c_void;
            fn CFStringCreateWithCString(
                a: *const c_void,
                s: *const libc::c_char,
                enc: u32,
            ) -> *const c_void;
        }
        const UTF8: u32 = 0x0800_0100;
        let key =
            CFStringCreateWithCString(std::ptr::null(), c"SetsCursorInBackground".as_ptr(), UTF8);
        let conn = main();
        set(conn, conn, key, kCFBooleanTrue);
        CFRelease(key);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_round_trips() {
        for m in [
            CaptureMode::Local,
            CaptureMode::Forwarding,
            CaptureMode::Controlled,
        ] {
            assert_eq!(CaptureMode::from_u8(m as u8), m);
        }
    }

    #[test]
    fn displays_are_enumerable() {
        // Headless CI may have none; a desktop Mac must have at least one.
        for r in display_rects() {
            assert!(r.w > 0.0 && r.h > 0.0);
        }
    }

    #[test]
    fn motion_and_activity_sets() {
        assert!(is_motion(kCGEventLeftMouseDragged));
        assert!(!is_motion(kCGEventLeftMouseDown));
        assert!(is_physical_activity(kCGEventKeyDown));
        assert!(!is_physical_activity(kCGEventMouseMoved));
    }
}
