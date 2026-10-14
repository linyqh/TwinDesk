//! Cross-machine drag hand-off decisions (report §10.3).
//!
//! Pure like `router`: no windows, sockets or event taps. The engine feeds in
//! what happened and carries out the returned `DragEffect`s.
//!
//! Vocabulary, for a drag that starts on Mac A and is dropped on Mac B:
//!
//! - **Hold**: A's own drag (e.g. Finder's), left suspended while the cursor
//!   is on B. A shows an invisible *shield* panel under its frozen cursor.
//!   If the cursor comes back with the button still down, the shield goes
//!   away and the drag simply continues. If the button is released on B, A
//!   releases its drag over the shield, which rejects it, so nothing is ever
//!   dropped at A's screen edge.
//! - **Proxy**: B's stand-in drag of file promises for A's offer, started on
//!   an invisible *pickup* panel under B's cursor. Dropping it makes the
//!   target (Finder, Mail, …) ask for the files, which are then fetched from
//!   A. Taking the cursor back to A cancels the proxy over the shield.
//!
//! The button can be held two ways, which decides how it is pressed and
//! released: physically (the user's hand on this Mac's mouse, while the OS
//! never saw the press because it was forwarded) or by our injector (the
//! peer's mouse drives this Mac).

use std::path::PathBuf;

use osm_model::LogicalPoint;
use osm_protocol::v1::{self, envelope::Payload};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Driver {
    /// The user's hand on this Mac's own mouse holds the button.
    Physical,
    /// Our injector holds the button on behalf of the peer.
    Injected,
}

/// A file drag this Mac can serve, prepared by the engine at the crossing.
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedOffer {
    pub drag_id: u64,
    pub token: Vec<u8>,
    pub items: Vec<v1::DragItem>,
    pub paths: Vec<PathBuf>,
}

/// What the drag pasteboard said when the cursor left with the button held.
#[derive(Clone, Debug, PartialEq)]
pub enum Probe {
    /// No drag session began since the press (window move, selection, …).
    NoDrag,
    /// A drag of something other than local files (text, promises, …).
    OtherDrag,
    Files(PreparedOffer),
}

#[derive(Clone, Debug, PartialEq)]
pub enum DragEffect {
    ShowShield {
        request: u64,
        at: LogicalPoint,
    },
    HideShield {
        request: u64,
    },
    /// Hide once the window server has delivered the release to the shield.
    HideShieldSoon {
        request: u64,
    },
    /// Release the left button where the cursor is, after a zero-length drag
    /// so the drag session re-targets whatever is under it (the shield).
    ReleaseLeft(Driver),
    /// Press the left button where the cursor is, on the pickup panel.
    PressLeft(Driver),
    /// Post Escape: cancels a drag session before a release that has no
    /// shield under it could drop anything.
    Escape(Driver),
    /// Put a pickup panel at `at` and start a promise drag on the next press.
    BeginProxy {
        request: u64,
        drag_id: u64,
        items: Vec<v1::DragItem>,
        at: LogicalPoint,
    },
    /// Remove the pickup panel and forget the proxy session.
    EndProxy {
        drag_id: u64,
    },
    /// Serve these files and tell the peer about them.
    Offer(PreparedOffer),
    Send(Payload),
}

#[derive(Debug)]
struct Hold {
    driver: Driver,
    /// Set for file drags offered to the peer.
    drag_id: Option<u64>,
    shield: u64,
    shield_ready: bool,
    /// Release requested before the shield was up.
    releasing: bool,
    /// The shield never appeared; cancel with Escape before releasing.
    escape: bool,
    /// A drag session exists (Escape would cancel it rather than reach
    /// the frontmost app).
    dragging: bool,
}

#[derive(Debug, PartialEq)]
enum ProxyPhase {
    Opening {
        request: u64,
    },
    Active,
    /// Being released over the shield; ends silently.
    Cancelling {
        request: u64,
        ready: bool,
    },
}

#[derive(Debug)]
struct Proxy {
    drag_id: u64,
    driver: Driver,
    phase: ProxyPhase,
}

#[derive(Debug, Default)]
pub struct DragCoordinator {
    hold: Option<Hold>,
    proxy: Option<Proxy>,
    next_request: u64,
}

impl DragCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    fn request(&mut self) -> u64 {
        self.next_request += 1;
        self.next_request
    }

    /// True while releasing the injected left button must go through the
    /// shield instead of happening wherever the cursor is.
    pub fn keeps_injected_left(&self) -> bool {
        self.hold
            .as_ref()
            .is_some_and(|h| h.driver == Driver::Injected)
            || self.proxy.as_ref().is_some_and(|p| {
                p.driver == Driver::Injected && !matches!(p.phase, ProxyPhase::Opening { .. })
            })
    }

    pub fn has_proxy(&self) -> bool {
        self.proxy.is_some()
    }

    pub fn is_idle(&self) -> bool {
        self.hold.is_none() && self.proxy.is_none()
    }

    /// The cursor left this Mac with the left button held. `at` is where the
    /// drag now sits on this Mac.
    pub fn crossing_out(
        &mut self,
        driver: Driver,
        probe: Probe,
        at: LogicalPoint,
    ) -> Vec<DragEffect> {
        if self.proxy.is_some() {
            // Carrying a proxy back toward its origin: the origin resumes its
            // own drag, so the proxy just goes away.
            return self.cancel_proxy(at);
        }
        if self.hold.is_some() {
            return Vec::new();
        }
        let request = self.request();
        let show = DragEffect::ShowShield { request, at };
        match (driver, probe) {
            // Nothing visible is in flight on the peer's side; the release
            // still has to end this press safely, which the shield does.
            (Driver::Physical, probe @ (Probe::NoDrag | Probe::OtherDrag)) => {
                self.hold = Some(Hold {
                    driver,
                    drag_id: None,
                    shield: request,
                    shield_ready: false,
                    releasing: false,
                    escape: false,
                    dragging: probe == Probe::OtherDrag,
                });
                vec![show]
            }
            (_, Probe::Files(offer)) => {
                self.hold = Some(Hold {
                    driver,
                    drag_id: Some(offer.drag_id),
                    shield: request,
                    shield_ready: false,
                    releasing: false,
                    escape: false,
                    dragging: true,
                });
                vec![show, DragEffect::Offer(offer)]
            }
            // An injected non-file drag cannot be finished on the peer and
            // nobody would ever release it: cancel it right away.
            (Driver::Injected, Probe::OtherDrag) => {
                self.hold = Some(Hold {
                    driver,
                    drag_id: None,
                    shield: request,
                    shield_ready: false,
                    releasing: true,
                    escape: false,
                    dragging: true,
                });
                vec![show]
            }
            // Selection or similar: the normal release at the edge is fine.
            (Driver::Injected, Probe::NoDrag) => Vec::new(),
        }
    }

    /// The cursor came back to this Mac (or the session ended) while a hold
    /// may be suspended: let the drag continue under the user's hand.
    pub fn crossing_in(&mut self) -> Vec<DragEffect> {
        match &self.hold {
            Some(h) if !h.releasing => {
                let request = h.shield;
                self.hold = None;
                vec![DragEffect::HideShield { request }]
            }
            _ => Vec::new(),
        }
    }

    /// The peer offered files for the drag that just arrived here.
    pub fn offer_received(
        &mut self,
        drag_id: u64,
        items: Vec<v1::DragItem>,
        driver: Driver,
        at: LogicalPoint,
    ) -> Vec<DragEffect> {
        if self.hold.is_some() || items.is_empty() {
            return Vec::new();
        }
        let mut fx = Vec::new();
        if let Some(old) = self.proxy.take() {
            fx.push(DragEffect::EndProxy {
                drag_id: old.drag_id,
            });
        }
        let request = self.request();
        self.proxy = Some(Proxy {
            drag_id,
            driver,
            phase: ProxyPhase::Opening { request },
        });
        fx.push(DragEffect::BeginProxy {
            request,
            drag_id,
            items,
            at,
        });
        fx
    }

    /// The UI finished showing the panel for `request`. `physical_left_held`
    /// is the hardware button state right now.
    pub fn overlay_ready(&mut self, request: u64, physical_left_held: bool) -> Vec<DragEffect> {
        if let Some(h) = &mut self.hold
            && h.shield == request
        {
            h.shield_ready = true;
            if h.releasing {
                h.releasing = false;
                return self.release_hold();
            }
            return Vec::new();
        }
        let Some(p) = &mut self.proxy else {
            return Vec::new();
        };
        match p.phase {
            ProxyPhase::Opening { request: r } if r == request => {
                if p.driver == Driver::Physical && !physical_left_held {
                    return self.abort_opening_proxy();
                }
                p.phase = ProxyPhase::Active;
                vec![DragEffect::PressLeft(p.driver)]
            }
            ProxyPhase::Cancelling {
                request: r,
                ref mut ready,
            } if r == request && !*ready => {
                *ready = true;
                vec![
                    DragEffect::ReleaseLeft(p.driver),
                    DragEffect::HideShieldSoon { request },
                ]
            }
            _ => Vec::new(),
        }
    }

    /// The panel for `request` did not appear in time (UI stalled or gone).
    pub fn overlay_timeout(&mut self, request: u64) -> Vec<DragEffect> {
        if let Some(h) = &mut self.hold
            && h.shield == request
            && !h.shield_ready
        {
            h.shield_ready = true;
            h.escape = h.dragging;
            // A physical hold may still come back and resume; anything else
            // has nobody left to finish it.
            if h.releasing || h.driver == Driver::Injected {
                h.releasing = false;
                return self.release_hold();
            }
            return Vec::new();
        }
        let Some(p) = &mut self.proxy else {
            return Vec::new();
        };
        match p.phase {
            ProxyPhase::Opening { request: r } if r == request => self.abort_opening_proxy(),
            ProxyPhase::Cancelling {
                request: r,
                ref mut ready,
            } if r == request && !*ready => {
                *ready = true;
                vec![
                    DragEffect::Escape(p.driver),
                    DragEffect::ReleaseLeft(p.driver),
                    DragEffect::HideShield { request },
                ]
            }
            _ => Vec::new(),
        }
    }

    /// Physical left button released while this Mac forwards to the peer:
    /// the drop happens there, so end the suspended local drag.
    pub fn physical_left_up_forwarding(&mut self) -> Vec<DragEffect> {
        if self
            .hold
            .as_ref()
            .is_some_and(|h| h.driver == Driver::Physical)
        {
            self.release_hold()
        } else {
            Vec::new()
        }
    }

    /// Physical left button released while this Mac is used locally.
    pub fn physical_left_up_local(&mut self) -> Vec<DragEffect> {
        self.left_up_before_press(Driver::Physical)
    }

    /// The peer's left button was released while it controls this Mac.
    pub fn forwarded_left_up(&mut self) -> Vec<DragEffect> {
        self.left_up_before_press(Driver::Injected)
    }

    /// A release that beats the pickup panel means the user dropped before
    /// the proxy existed; there is nothing to drop.
    fn left_up_before_press(&mut self, driver: Driver) -> Vec<DragEffect> {
        match &self.proxy {
            Some(p) if p.driver == driver && matches!(p.phase, ProxyPhase::Opening { .. }) => {
                self.abort_opening_proxy()
            }
            _ => Vec::new(),
        }
    }

    /// The UI reports that the proxy drag session ended.
    pub fn proxy_ended(&mut self, drag_id: u64, dropped: bool) -> Vec<DragEffect> {
        let Some(p) = &self.proxy else {
            return Vec::new();
        };
        if p.drag_id != drag_id {
            return Vec::new();
        }
        let silent = matches!(p.phase, ProxyPhase::Cancelling { .. });
        self.proxy = None;
        let mut fx = vec![DragEffect::EndProxy { drag_id }];
        if !silent {
            fx.push(drag_end(drag_id, dropped));
        }
        fx
    }

    /// The peer's proxy for our offer ended (dropped or not): our injected
    /// hold can never be finished here, so release it over the shield.
    pub fn drag_end_received(&mut self, drag_id: u64) -> Vec<DragEffect> {
        match &self.hold {
            Some(h) if h.driver == Driver::Injected && h.drag_id == Some(drag_id) => {
                self.release_hold()
            }
            _ => Vec::new(),
        }
    }

    /// Session aborted or connection lost. Nothing can be handed over any
    /// more, so cancel what needs cancelling and resume what can resume.
    pub fn reset(&mut self, at: LogicalPoint) -> Vec<DragEffect> {
        let mut fx = Vec::new();
        if let Some(h) = &self.hold {
            if h.driver == Driver::Physical {
                if !h.releasing {
                    fx.extend(self.crossing_in());
                }
            } else if !h.releasing {
                fx.extend(self.release_hold());
            }
        }
        if self.proxy.is_some() {
            fx.extend(self.cancel_proxy(at));
        }
        fx
    }

    fn release_hold(&mut self) -> Vec<DragEffect> {
        let Some(h) = &mut self.hold else {
            return Vec::new();
        };
        if h.releasing {
            return Vec::new();
        }
        if h.shield_ready {
            let (driver, request, escape) = (h.driver, h.shield, h.escape);
            self.hold = None;
            let mut fx = Vec::new();
            if escape {
                fx.push(DragEffect::Escape(driver));
            }
            fx.push(DragEffect::ReleaseLeft(driver));
            fx.push(DragEffect::HideShieldSoon { request });
            fx
        } else {
            h.releasing = true;
            Vec::new()
        }
    }

    fn cancel_proxy(&mut self, at: LogicalPoint) -> Vec<DragEffect> {
        let Some(p) = &mut self.proxy else {
            return Vec::new();
        };
        match p.phase {
            ProxyPhase::Opening { .. } => {
                let drag_id = p.drag_id;
                self.proxy = None;
                vec![DragEffect::EndProxy { drag_id }]
            }
            ProxyPhase::Active => {
                self.next_request += 1;
                let request = self.next_request;
                p.phase = ProxyPhase::Cancelling {
                    request,
                    ready: false,
                };
                vec![DragEffect::ShowShield { request, at }]
            }
            ProxyPhase::Cancelling { .. } => Vec::new(),
        }
    }

    fn abort_opening_proxy(&mut self) -> Vec<DragEffect> {
        let Some(p) = self.proxy.take() else {
            return Vec::new();
        };
        vec![
            DragEffect::EndProxy { drag_id: p.drag_id },
            drag_end(p.drag_id, false),
        ]
    }
}

fn drag_end(drag_id: u64, dropped: bool) -> DragEffect {
    DragEffect::Send(Payload::DragEnd(v1::DragEnd { drag_id, dropped }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: LogicalPoint = LogicalPoint { x: 5.0, y: 6.0 };

    fn offer(id: u64) -> PreparedOffer {
        PreparedOffer {
            drag_id: id,
            token: vec![1; 16],
            items: vec![item()],
            paths: vec![PathBuf::from("/tmp/a.txt")],
        }
    }

    fn item() -> v1::DragItem {
        v1::DragItem {
            name: "a.txt".into(),
            kind: v1::EntryKind::File as i32,
            size: 1,
        }
    }

    fn shield_request(fx: &[DragEffect]) -> u64 {
        fx.iter()
            .find_map(|f| match f {
                DragEffect::ShowShield { request, .. } => Some(*request),
                _ => None,
            })
            .expect("shield shown")
    }

    fn proxy_request(fx: &[DragEffect]) -> u64 {
        fx.iter()
            .find_map(|f| match f {
                DragEffect::BeginProxy { request, .. } => Some(*request),
                _ => None,
            })
            .expect("proxy begun")
    }

    // ---- Origin side, physical hold (drag from this Mac to the peer) ----

    #[test]
    fn file_drag_out_shields_and_offers() {
        let mut d = DragCoordinator::new();
        let fx = d.crossing_out(Driver::Physical, Probe::Files(offer(7)), AT);
        assert!(matches!(fx[0], DragEffect::ShowShield { at, .. } if at == AT));
        assert!(matches!(&fx[1], DragEffect::Offer(o) if o.drag_id == 7));
        assert!(!d.keeps_injected_left());
    }

    #[test]
    fn release_on_peer_cancels_local_drag_over_shield() {
        let mut d = DragCoordinator::new();
        let req = shield_request(&d.crossing_out(Driver::Physical, Probe::Files(offer(7)), AT));
        assert!(d.overlay_ready(req, true).is_empty());
        let fx = d.physical_left_up_forwarding();
        assert_eq!(
            fx,
            vec![
                DragEffect::ReleaseLeft(Driver::Physical),
                DragEffect::HideShieldSoon { request: req }
            ]
        );
        assert!(d.is_idle());
    }

    #[test]
    fn release_before_shield_ready_waits_for_it() {
        let mut d = DragCoordinator::new();
        let req = shield_request(&d.crossing_out(Driver::Physical, Probe::NoDrag, AT));
        assert!(d.physical_left_up_forwarding().is_empty());
        // Coming back now must not resume a drag whose button is released.
        assert!(d.crossing_in().is_empty());
        assert_eq!(
            d.overlay_ready(req, false),
            vec![
                DragEffect::ReleaseLeft(Driver::Physical),
                DragEffect::HideShieldSoon { request: req }
            ]
        );
        assert!(d.is_idle());
    }

    #[test]
    fn coming_back_with_button_held_resumes_the_drag() {
        let mut d = DragCoordinator::new();
        let req = shield_request(&d.crossing_out(Driver::Physical, Probe::Files(offer(7)), AT));
        d.overlay_ready(req, true);
        assert_eq!(
            d.crossing_in(),
            vec![DragEffect::HideShield { request: req }]
        );
        assert!(d.is_idle());
        // A later release is just a local drop.
        assert!(d.physical_left_up_local().is_empty());
    }

    #[test]
    fn peer_drag_end_does_not_touch_a_physical_hold() {
        let mut d = DragCoordinator::new();
        d.crossing_out(Driver::Physical, Probe::Files(offer(7)), AT);
        assert!(d.drag_end_received(7).is_empty());
        assert!(!d.is_idle());
    }

    // ---- Proxy side, injected (peer's mouse drives this Mac) ----

    #[test]
    fn injected_proxy_presses_after_pickup_is_ready() {
        let mut d = DragCoordinator::new();
        let fx = d.offer_received(7, vec![item()], Driver::Injected, AT);
        let req = proxy_request(&fx);
        assert!(!d.keeps_injected_left());
        assert_eq!(
            d.overlay_ready(req, false),
            vec![DragEffect::PressLeft(Driver::Injected)]
        );
        assert!(d.keeps_injected_left());
        // Dropped on a Finder window.
        assert_eq!(
            d.proxy_ended(7, true),
            vec![
                DragEffect::EndProxy { drag_id: 7 },
                DragEffect::Send(Payload::DragEnd(v1::DragEnd {
                    drag_id: 7,
                    dropped: true
                }))
            ]
        );
        assert!(d.is_idle());
    }

    #[test]
    fn release_before_pickup_ready_aborts_proxy() {
        let mut d = DragCoordinator::new();
        let req = proxy_request(&d.offer_received(7, vec![item()], Driver::Injected, AT));
        let fx = d.forwarded_left_up();
        assert!(matches!(fx[0], DragEffect::EndProxy { drag_id: 7 }));
        assert!(matches!(&fx[1], DragEffect::Send(Payload::DragEnd(e)) if !e.dropped));
        assert!(
            d.overlay_ready(req, false).is_empty(),
            "late ready presses nothing"
        );
    }

    #[test]
    fn carrying_proxy_back_cancels_it_silently() {
        let mut d = DragCoordinator::new();
        let req = proxy_request(&d.offer_received(7, vec![item()], Driver::Injected, AT));
        d.overlay_ready(req, false);
        let fx = d.crossing_out(Driver::Injected, Probe::OtherDrag, AT);
        let shield = shield_request(&fx);
        assert!(d.keeps_injected_left());
        assert_eq!(
            d.overlay_ready(shield, false),
            vec![
                DragEffect::ReleaseLeft(Driver::Injected),
                DragEffect::HideShieldSoon { request: shield }
            ]
        );
        // No DragEnd: the origin resumes its own drag instead.
        assert_eq!(
            d.proxy_ended(7, false),
            vec![DragEffect::EndProxy { drag_id: 7 }]
        );
        assert!(d.is_idle());
    }

    // ---- Reverse direction: files on the controlled Mac ----

    #[test]
    fn injected_file_drag_out_is_held_until_peer_drops() {
        let mut d = DragCoordinator::new();
        let fx = d.crossing_out(Driver::Injected, Probe::Files(offer(9)), AT);
        let req = shield_request(&fx);
        assert!(
            d.keeps_injected_left(),
            "left stays down while the cursor is away"
        );
        d.overlay_ready(req, false);
        assert!(
            d.drag_end_received(8).is_empty(),
            "other drag ids are ignored"
        );
        assert_eq!(
            d.drag_end_received(9),
            vec![
                DragEffect::ReleaseLeft(Driver::Injected),
                DragEffect::HideShieldSoon { request: req }
            ]
        );
        assert!(!d.keeps_injected_left());
    }

    #[test]
    fn injected_non_file_drag_is_cancelled_at_once() {
        let mut d = DragCoordinator::new();
        let req = shield_request(&d.crossing_out(Driver::Injected, Probe::OtherDrag, AT));
        assert!(d.keeps_injected_left());
        assert_eq!(
            d.overlay_ready(req, false),
            vec![
                DragEffect::ReleaseLeft(Driver::Injected),
                DragEffect::HideShieldSoon { request: req }
            ]
        );
    }

    #[test]
    fn injected_selection_crossing_needs_nothing() {
        let mut d = DragCoordinator::new();
        assert!(
            d.crossing_out(Driver::Injected, Probe::NoDrag, AT)
                .is_empty()
        );
        assert!(!d.keeps_injected_left());
    }

    #[test]
    fn physical_proxy_needs_button_still_held() {
        let mut d = DragCoordinator::new();
        let req = proxy_request(&d.offer_received(9, vec![item()], Driver::Physical, AT));
        let fx = d.overlay_ready(req, false);
        assert!(matches!(fx[0], DragEffect::EndProxy { drag_id: 9 }));
        assert!(matches!(&fx[1], DragEffect::Send(Payload::DragEnd(e)) if !e.dropped));

        let req = proxy_request(&d.offer_received(10, vec![item()], Driver::Physical, AT));
        assert_eq!(
            d.overlay_ready(req, true),
            vec![DragEffect::PressLeft(Driver::Physical)]
        );
        assert!(!d.keeps_injected_left());
    }

    #[test]
    fn physical_release_before_pickup_aborts() {
        let mut d = DragCoordinator::new();
        d.offer_received(9, vec![item()], Driver::Physical, AT);
        assert!(!d.physical_left_up_local().is_empty());
        assert!(d.is_idle());
    }

    // ---- Failures ----

    #[test]
    fn reset_resumes_physical_and_cancels_injected() {
        let mut d = DragCoordinator::new();
        let req = shield_request(&d.crossing_out(Driver::Physical, Probe::Files(offer(1)), AT));
        assert_eq!(d.reset(AT), vec![DragEffect::HideShield { request: req }]);
        assert!(d.is_idle());

        let mut d = DragCoordinator::new();
        let req = shield_request(&d.crossing_out(Driver::Injected, Probe::Files(offer(1)), AT));
        assert!(d.reset(AT).is_empty(), "release waits for the shield");
        assert!(matches!(
            d.overlay_ready(req, false)[0],
            DragEffect::ReleaseLeft(Driver::Injected)
        ));

        let mut d = DragCoordinator::new();
        let req = proxy_request(&d.offer_received(3, vec![item()], Driver::Injected, AT));
        d.overlay_ready(req, false);
        let shield = shield_request(&d.reset(AT));
        assert!(matches!(
            d.overlay_ready(shield, false)[0],
            DragEffect::ReleaseLeft(Driver::Injected)
        ));
    }

    #[test]
    fn missing_shield_cancels_injected_hold_with_escape() {
        let mut d = DragCoordinator::new();
        let req = shield_request(&d.crossing_out(Driver::Injected, Probe::Files(offer(1)), AT));
        assert_eq!(
            d.overlay_timeout(req),
            vec![
                DragEffect::Escape(Driver::Injected),
                DragEffect::ReleaseLeft(Driver::Injected),
                DragEffect::HideShieldSoon { request: req }
            ]
        );
        assert!(d.is_idle());
    }

    #[test]
    fn missing_shield_on_physical_hold_escapes_at_release() {
        let mut d = DragCoordinator::new();
        let req = shield_request(&d.crossing_out(Driver::Physical, Probe::Files(offer(1)), AT));
        assert!(
            d.overlay_timeout(req).is_empty(),
            "user may still come back"
        );
        let fx = d.physical_left_up_forwarding();
        assert_eq!(fx[0], DragEffect::Escape(Driver::Physical));
        assert_eq!(fx[1], DragEffect::ReleaseLeft(Driver::Physical));
        // A late ready for a finished request does nothing.
        assert!(d.overlay_ready(req, false).is_empty());
    }

    #[test]
    fn missing_shield_without_drag_session_never_sends_escape() {
        let mut d = DragCoordinator::new();
        let req = shield_request(&d.crossing_out(Driver::Physical, Probe::NoDrag, AT));
        d.overlay_timeout(req);
        assert_eq!(
            d.physical_left_up_forwarding(),
            vec![
                DragEffect::ReleaseLeft(Driver::Physical),
                DragEffect::HideShieldSoon { request: req }
            ]
        );
    }

    #[test]
    fn missing_pickup_aborts_proxy() {
        let mut d = DragCoordinator::new();
        let req = proxy_request(&d.offer_received(4, vec![item()], Driver::Injected, AT));
        let fx = d.overlay_timeout(req);
        assert!(matches!(fx[0], DragEffect::EndProxy { drag_id: 4 }));
        assert!(matches!(&fx[1], DragEffect::Send(Payload::DragEnd(e)) if !e.dropped));
    }

    #[test]
    fn new_offer_replaces_stale_proxy() {
        let mut d = DragCoordinator::new();
        d.offer_received(1, vec![item()], Driver::Injected, AT);
        let fx = d.offer_received(2, vec![item()], Driver::Injected, AT);
        assert!(matches!(fx[0], DragEffect::EndProxy { drag_id: 1 }));
        assert!(matches!(fx[1], DragEffect::BeginProxy { drag_id: 2, .. }));
        assert!(d.offer_received(3, vec![], Driver::Injected, AT).is_empty());
    }

    #[test]
    fn second_crossing_while_holding_is_ignored() {
        let mut d = DragCoordinator::new();
        d.crossing_out(Driver::Physical, Probe::Files(offer(1)), AT);
        assert!(
            d.crossing_out(Driver::Physical, Probe::Files(offer(2)), AT)
                .is_empty()
        );
        assert!(
            d.offer_received(5, vec![item()], Driver::Physical, AT)
                .is_empty()
        );
    }
}
