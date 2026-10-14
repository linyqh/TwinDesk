//! Pure decisions for one two-machine session.
//!
//! `Router` owns no OS or network resources. It consumes local input and
//! peer messages and returns `Effect`s for the engine to carry out, so the
//! switching rules can be tested without event taps or sockets.

use osm_core::keys::{FLAG_COMMAND, FLAG_CONTROL, FnRestore, KEY_ESCAPE, KeyStroke};
use osm_core::{Desktop, Side};
use osm_model::LogicalPoint;
use osm_platform_macos::{InputEvent, REMAPPED_FN_KEYCODE};
use osm_protocol::v1::{self, LeaveReason};

/// Ctrl+Cmd+Esc while controlling the peer returns control immediately,
/// whatever state the connection is in.
pub const EMERGENCY_FLAGS: u64 = FLAG_CONTROL | FLAG_COMMAND;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Using this Mac normally.
    Local,
    /// This Mac's keyboard and mouse drive the peer.
    Controlling { epoch: u64 },
    /// The peer's keyboard and mouse drive this Mac.
    Controlled { epoch: u64 },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Send(v1::envelope::Payload),
    /// Start forwarding: suppress local input and freeze the cursor at `park`.
    BeginForwarding {
        park: LogicalPoint,
    },
    /// Stop forwarding and put the local cursor at `at`.
    EndForwarding {
        at: LogicalPoint,
    },
    /// Accept control: let injected events through, watch for local activity.
    BeginControlled {
        at: LogicalPoint,
    },
    /// Release everything injected and return to local mode. `handoff` is
    /// set when the cursor left through the return edge; otherwise the
    /// session was aborted. Held-button drags cannot leave through an edge.
    EndControlled {
        handoff: bool,
    },
    InjectMove {
        to: LogicalPoint,
        dx: i32,
        dy: i32,
    },
    InjectButton {
        button: u32,
        down: bool,
        click_state: i64,
    },
    InjectScroll {
        continuous: bool,
        dy: i32,
        dx: i32,
        scroll_phase: u32,
        momentum_phase: u32,
    },
    InjectKey {
        keycode: u16,
        down: bool,
        repeat: bool,
        flags: u64,
    },
}

#[derive(Debug)]
pub struct Router {
    role: Role,
    desktop: Desktop,
    /// Which edge of this Mac leads to the peer.
    peer_side: Side,
    next_epoch: u64,
    /// Local cursor position when forwarding began, restored on abort.
    park: LogicalPoint,
    /// Controlled side: tracked position of the injected cursor.
    remote_pos: LogicalPoint,
    sharing_enabled: bool,
    /// Turns the locally remapped Fn key back into Fn for the peer.
    fn_restore: FnRestore,
    /// Epoch of the most recent session in either role.
    last_epoch: u64,
    remote_left_down: bool,
}

impl Router {
    pub fn new(desktop: Desktop, peer_side: Side) -> Self {
        let center = desktop.entry_point(Side::Left, 0.5);
        Self {
            role: Role::Local,
            desktop,
            peer_side,
            next_epoch: 1,
            park: center,
            remote_pos: center,
            sharing_enabled: true,
            fn_restore: FnRestore::default(),
            last_epoch: 0,
            remote_left_down: false,
        }
    }

    /// Where the local cursor is parked while forwarding.
    pub fn park(&self) -> LogicalPoint {
        self.park
    }

    /// Epoch of the current or most recently ended session.
    pub fn last_epoch(&self) -> u64 {
        self.last_epoch
    }

    pub fn role(&self) -> Role {
        self.role
    }

    pub fn peer_side(&self) -> Side {
        self.peer_side
    }

    pub fn set_peer_side(&mut self, side: Side) -> Vec<Effect> {
        let fx = self.abort();
        self.peer_side = side;
        fx
    }

    pub fn set_desktop(&mut self, desktop: Desktop) -> Vec<Effect> {
        if desktop == self.desktop {
            return Vec::new();
        }
        // Coordinates computed against the old layout are meaningless now.
        let fx = self.abort();
        self.desktop = desktop;
        fx
    }

    pub fn set_sharing_enabled(&mut self, enabled: bool) -> Vec<Effect> {
        self.sharing_enabled = enabled;
        if enabled { Vec::new() } else { self.abort() }
    }

    /// Ends whatever session is active and returns to local control.
    pub fn abort(&mut self) -> Vec<Effect> {
        match self.role {
            Role::Local => Vec::new(),
            Role::Controlling { epoch } => {
                self.role = Role::Local;
                vec![
                    Effect::Send(leave(epoch, 0.5, LeaveReason::Abort)),
                    Effect::EndForwarding { at: self.park },
                ]
            }
            Role::Controlled { epoch } => {
                self.role = Role::Local;
                vec![
                    Effect::EndControlled { handoff: false },
                    Effect::Send(leave(epoch, 0.5, LeaveReason::LocalInput)),
                ]
            }
        }
    }

    /// The connection is gone: recover locally without sending anything.
    pub fn disconnected(&mut self) -> Vec<Effect> {
        let fx = match self.role {
            Role::Local => Vec::new(),
            Role::Controlling { .. } => vec![Effect::EndForwarding { at: self.park }],
            Role::Controlled { .. } => vec![Effect::EndControlled { handoff: false }],
        };
        self.role = Role::Local;
        fx
    }

    pub fn on_local(&mut self, ev: InputEvent) -> Vec<Effect> {
        match (self.role, ev) {
            // The tap already fell back to local delivery; end the session
            // and tell the peer.
            (_, InputEvent::TapDisabled) => self.abort(),

            (
                Role::Local,
                InputEvent::LocalMove {
                    pos, dx, left_down, ..
                },
            ) => {
                // Complete local drops before handing input to another computer.
                if left_down
                    || !self.sharing_enabled
                    || !self.desktop.at_edge(self.peer_side, pos, dx)
                {
                    return Vec::new();
                }
                let epoch = self.next_epoch;
                self.next_epoch += 1;
                self.role = Role::Controlling { epoch };
                self.last_epoch = epoch;
                self.fn_restore.reset();
                self.park = self
                    .desktop
                    .entry_point(self.peer_side, self.desktop.y_ratio(pos));
                let enter = v1::Enter {
                    epoch,
                    // The peer's return edge faces us.
                    return_side: wire_side(self.peer_side.opposite()) as i32,
                    y_ratio: self.desktop.y_ratio(pos),
                };
                vec![
                    Effect::BeginForwarding { park: self.park },
                    Effect::Send(v1::envelope::Payload::Enter(enter)),
                ]
            }

            (Role::Controlling { epoch }, ev) => match ev {
                InputEvent::Key {
                    keycode,
                    down: true,
                    flags,
                    ..
                } if keycode == KEY_ESCAPE && flags & EMERGENCY_FLAGS == EMERGENCY_FLAGS => {
                    self.role = Role::Local;
                    vec![
                        Effect::Send(leave(epoch, 0.5, LeaveReason::Abort)),
                        Effect::EndForwarding { at: self.park },
                    ]
                }
                InputEvent::Move { dx, dy } => {
                    vec![Effect::Send(v1::envelope::Payload::MouseMove(
                        v1::MouseMove { dx, dy },
                    ))]
                }
                InputEvent::Button {
                    button,
                    down,
                    click_state,
                    flags,
                } => {
                    vec![Effect::Send(v1::envelope::Payload::MouseButton(
                        v1::MouseButton {
                            button,
                            down,
                            click_state,
                            flags,
                        },
                    ))]
                }
                InputEvent::Scroll {
                    continuous,
                    dy,
                    dx,
                    scroll_phase,
                    momentum_phase,
                } => {
                    vec![Effect::Send(v1::envelope::Payload::Scroll(v1::Scroll {
                        continuous,
                        delta_y: dy,
                        delta_x: dx,
                        scroll_phase,
                        momentum_phase,
                    }))]
                }
                InputEvent::Key {
                    keycode,
                    down,
                    repeat,
                    flags,
                } => {
                    let k = self.fn_restore.apply(
                        KeyStroke {
                            keycode,
                            down,
                            repeat,
                            flags,
                        },
                        REMAPPED_FN_KEYCODE,
                    );
                    vec![Effect::Send(v1::envelope::Payload::Key(v1::Key {
                        keycode: k.keycode.into(),
                        down: k.down,
                        repeat: k.repeat,
                        flags: k.flags,
                    }))]
                }
                _ => Vec::new(),
            },

            (Role::Controlled { .. }, InputEvent::LocalActivity) => self.abort(),

            _ => Vec::new(),
        }
    }

    pub fn on_peer(&mut self, payload: v1::envelope::Payload) -> Vec<Effect> {
        use v1::envelope::Payload as P;
        match (self.role, payload) {
            (Role::Local, P::Enter(e)) => {
                if !self.sharing_enabled {
                    return vec![Effect::Send(leave(e.epoch, 0.5, LeaveReason::Rejected))];
                }
                let side = side_from_wire(e.return_side).unwrap_or(self.peer_side);
                // Both sides agree on geometry: our return edge must be the
                // edge we consider to face the peer.
                self.peer_side = side;
                self.role = Role::Controlled { epoch: e.epoch };
                self.last_epoch = e.epoch;
                self.remote_pos = self.desktop.entry_point(side, e.y_ratio);
                self.remote_left_down = false;
                vec![Effect::BeginControlled {
                    at: self.remote_pos,
                }]
            }
            // Busy with our own session: refuse rather than queue.
            (Role::Controlling { .. } | Role::Controlled { .. }, P::Enter(e)) => {
                vec![Effect::Send(leave(e.epoch, 0.5, LeaveReason::Rejected))]
            }

            (Role::Controlling { epoch }, P::Leave(l)) if l.epoch == epoch => {
                self.role = Role::Local;
                let at = match LeaveReason::try_from(l.reason) {
                    Ok(LeaveReason::Edge) => self.desktop.entry_point(self.peer_side, l.y_ratio),
                    _ => self.park,
                };
                vec![Effect::EndForwarding { at }]
            }
            (Role::Controlled { epoch }, P::Leave(l)) if l.epoch == epoch => {
                self.role = Role::Local;
                vec![Effect::EndControlled { handoff: false }]
            }

            (Role::Controlled { epoch }, P::MouseMove(m)) => {
                let raw = LogicalPoint {
                    x: self.remote_pos.x + m.dx as f64,
                    y: self.remote_pos.y + m.dy as f64,
                };
                if !self.remote_left_down && self.desktop.beyond_edge(self.peer_side, raw) {
                    let y_ratio = self.desktop.y_ratio(raw);
                    self.role = Role::Local;
                    // Notify the controller before restoring local input.
                    return vec![
                        Effect::Send(leave(epoch, y_ratio, LeaveReason::Edge)),
                        Effect::EndControlled { handoff: true },
                    ];
                }
                self.remote_pos = self.desktop.clamp(raw);
                vec![Effect::InjectMove {
                    to: self.remote_pos,
                    dx: m.dx,
                    dy: m.dy,
                }]
            }
            (Role::Controlled { .. }, P::MouseButton(b)) => {
                if b.button == 0 {
                    self.remote_left_down = b.down;
                }
                vec![Effect::InjectButton {
                    button: b.button,
                    down: b.down,
                    click_state: b.click_state,
                }]
            }
            (Role::Controlled { .. }, P::Scroll(s)) => vec![Effect::InjectScroll {
                continuous: s.continuous,
                dy: s.delta_y,
                dx: s.delta_x,
                scroll_phase: s.scroll_phase,
                momentum_phase: s.momentum_phase,
            }],
            (Role::Controlled { .. }, P::Key(k)) => {
                let Ok(keycode) = u16::try_from(k.keycode) else {
                    return Vec::new();
                };
                vec![Effect::InjectKey {
                    keycode,
                    down: k.down,
                    repeat: k.repeat,
                    flags: k.flags,
                }]
            }

            // Stale epochs, input while not controlled, pings: ignored here.
            _ => Vec::new(),
        }
    }
}

fn leave(epoch: u64, y_ratio: f64, reason: LeaveReason) -> v1::envelope::Payload {
    v1::envelope::Payload::Leave(v1::Leave {
        epoch,
        y_ratio,
        reason: reason as i32,
    })
}

fn wire_side(s: Side) -> v1::Side {
    match s {
        Side::Left => v1::Side::Left,
        Side::Right => v1::Side::Right,
    }
}

fn side_from_wire(v: i32) -> Option<Side> {
    match v1::Side::try_from(v).ok()? {
        v1::Side::Left => Some(Side::Left),
        v1::Side::Right => Some(Side::Right),
        v1::Side::Unspecified => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use osm_model::LogicalRect;
    use v1::envelope::Payload as P;

    fn desktop() -> Desktop {
        Desktop::new(vec![LogicalRect {
            x: 0.0,
            y: 0.0,
            w: 1000.0,
            h: 800.0,
        }])
        .unwrap()
    }

    fn pt(x: f64, y: f64) -> LogicalPoint {
        LogicalPoint { x, y }
    }

    fn start_controlling(r: &mut Router) -> u64 {
        let fx = r.on_local(InputEvent::LocalMove {
            pos: pt(999.0, 400.0),
            dx: 5.0,
            dy: 0.0,
            left_down: false,
        });
        let Role::Controlling { epoch } = r.role() else {
            panic!("not controlling: {fx:?}")
        };
        epoch
    }

    #[test]
    fn local_drag_stays_on_source_until_released_on_either_edge() {
        for (side, x, dx) in [(Side::Left, 0.0, -20.0), (Side::Right, 999.0, 20.0)] {
            let mut r = Router::new(desktop(), side);
            assert!(
                r.on_local(InputEvent::LocalMove {
                    pos: pt(x, 400.0),
                    dx,
                    dy: 0.0,
                    left_down: true,
                })
                .is_empty()
            );
            assert_eq!(r.role(), Role::Local);
            assert!(
                !r.on_local(InputEvent::LocalMove {
                    pos: pt(x, 400.0),
                    dx,
                    dy: 0.0,
                    left_down: false,
                })
                .is_empty()
            );
            assert!(matches!(r.role(), Role::Controlling { .. }));
        }
    }

    #[test]
    fn remotely_driven_drag_can_drop_on_portal_without_returning_control() {
        for (side, dx) in [(v1::Side::Left, -2000), (v1::Side::Right, 2000)] {
            let mut r = controlled(side);
            r.on_peer(P::MouseButton(v1::MouseButton {
                button: 0,
                down: true,
                ..Default::default()
            }));
            let fx = r.on_peer(P::MouseMove(v1::MouseMove { dx, dy: 0 }));
            assert!(matches!(&fx[..], [Effect::InjectMove { .. }]));
            assert!(matches!(r.role(), Role::Controlled { .. }));
            let fx = r.on_peer(P::MouseButton(v1::MouseButton {
                button: 0,
                down: false,
                ..Default::default()
            }));
            assert!(matches!(
                &fx[..],
                [Effect::InjectButton { down: false, .. }]
            ));
            r.on_peer(P::MouseMove(v1::MouseMove { dx, dy: 0 }));
            assert_eq!(r.role(), Role::Local);
        }
    }

    #[test]
    fn crossing_right_edge_starts_controlling() {
        let mut r = Router::new(desktop(), Side::Right);
        let fx = r.on_local(InputEvent::LocalMove {
            pos: pt(999.0, 200.0),
            dx: 3.0,
            dy: 0.0,
            left_down: false,
        });
        assert!(matches!(fx[0], Effect::BeginForwarding { .. }));
        let Effect::Send(P::Enter(e)) = &fx[1] else {
            panic!()
        };
        assert_eq!(e.return_side, v1::Side::Left as i32);
        assert!((e.y_ratio - 0.25).abs() < 1e-9);
    }

    #[test]
    fn wrong_edge_or_inward_motion_does_nothing() {
        let mut r = Router::new(desktop(), Side::Right);
        assert!(
            r.on_local(InputEvent::LocalMove {
                pos: pt(0.0, 200.0),
                dx: -3.0,
                dy: 0.0,
                left_down: false,
            })
            .is_empty()
        );
        assert!(
            r.on_local(InputEvent::LocalMove {
                pos: pt(999.0, 200.0),
                dx: -3.0,
                dy: 0.0,
                left_down: false,
            })
            .is_empty()
        );
        assert_eq!(r.role(), Role::Local);
    }

    #[test]
    fn disabled_sharing_never_switches() {
        let mut r = Router::new(desktop(), Side::Right);
        r.set_sharing_enabled(false);
        assert!(
            r.on_local(InputEvent::LocalMove {
                pos: pt(999.0, 200.0),
                dx: 3.0,
                dy: 0.0,
                left_down: false,
            })
            .is_empty()
        );
    }

    #[test]
    fn epochs_are_remembered_after_sessions_end() {
        let mut r = Router::new(desktop(), Side::Right);
        let epoch = start_controlling(&mut r);
        r.abort();
        assert_eq!(r.last_epoch(), epoch);
        let mut r = controlled(v1::Side::Left);
        r.abort();
        assert_eq!(r.last_epoch(), 9);
    }

    #[test]
    fn controlling_forwards_input() {
        let mut r = Router::new(desktop(), Side::Right);
        start_controlling(&mut r);
        let fx = r.on_local(InputEvent::Key {
            keycode: 0,
            down: true,
            repeat: false,
            flags: 0,
        });
        assert!(matches!(&fx[..], [Effect::Send(P::Key(k))] if k.keycode == 0 && k.down));
        let fx = r.on_local(InputEvent::Move { dx: 4, dy: -2 });
        assert!(matches!(&fx[..], [Effect::Send(P::MouseMove(m))] if m.dx == 4 && m.dy == -2));
    }

    #[test]
    fn emergency_hotkey_returns_control() {
        let mut r = Router::new(desktop(), Side::Right);
        let epoch = start_controlling(&mut r);
        let fx = r.on_local(InputEvent::Key {
            keycode: KEY_ESCAPE,
            down: true,
            repeat: false,
            flags: EMERGENCY_FLAGS,
        });
        assert_eq!(r.role(), Role::Local);
        assert!(matches!(&fx[0], Effect::Send(P::Leave(l)) if l.epoch == epoch));
        assert!(matches!(fx[1], Effect::EndForwarding { .. }));
    }

    #[test]
    fn remapped_fn_is_forwarded_as_fn() {
        use osm_core::keys::{FLAG_SECONDARY_FN, KEY_FUNCTION};
        let mut r = Router::new(desktop(), Side::Right);
        start_controlling(&mut r);
        let fx = r.on_local(InputEvent::Key {
            keycode: REMAPPED_FN_KEYCODE,
            down: true,
            repeat: false,
            flags: 0x100,
        });
        let [Effect::Send(P::Key(k))] = &fx[..] else {
            panic!("{fx:?}")
        };
        assert_eq!(k.keycode, u32::from(KEY_FUNCTION));
        assert!(k.down);
        assert_eq!(k.flags & FLAG_SECONDARY_FN, FLAG_SECONDARY_FN);
        // A key pressed while Fn is held carries the Fn flag.
        let fx = r.on_local(InputEvent::Key {
            keycode: 0,
            down: true,
            repeat: false,
            flags: 0,
        });
        let [Effect::Send(P::Key(k))] = &fx[..] else {
            panic!("{fx:?}")
        };
        assert_eq!(k.flags & FLAG_SECONDARY_FN, FLAG_SECONDARY_FN);
    }

    #[test]
    fn plain_escape_is_forwarded() {
        let mut r = Router::new(desktop(), Side::Right);
        start_controlling(&mut r);
        let fx = r.on_local(InputEvent::Key {
            keycode: KEY_ESCAPE,
            down: true,
            repeat: false,
            flags: 0,
        });
        assert!(matches!(&fx[..], [Effect::Send(P::Key(_))]));
    }

    #[test]
    fn peer_leave_through_edge_places_cursor_at_our_edge() {
        let mut r = Router::new(desktop(), Side::Right);
        let epoch = start_controlling(&mut r);
        let fx = r.on_peer(P::Leave(v1::Leave {
            epoch,
            y_ratio: 0.5,
            reason: LeaveReason::Edge as i32,
        }));
        assert_eq!(r.role(), Role::Local);
        let [Effect::EndForwarding { at }] = &fx[..] else {
            panic!("{fx:?}")
        };
        assert!(at.x > 990.0 && (at.y - 400.0).abs() < 1e-9);
    }

    #[test]
    fn stale_leave_is_ignored() {
        let mut r = Router::new(desktop(), Side::Right);
        let epoch = start_controlling(&mut r);
        assert!(
            r.on_peer(P::Leave(v1::Leave {
                epoch: epoch + 7,
                y_ratio: 0.5,
                reason: 1
            }))
            .is_empty()
        );
        assert!(matches!(r.role(), Role::Controlling { .. }));
    }

    fn controlled(return_side: v1::Side) -> Router {
        let mut r = Router::new(desktop(), Side::Right);
        let fx = r.on_peer(P::Enter(v1::Enter {
            epoch: 9,
            return_side: return_side as i32,
            y_ratio: 0.5,
        }));
        assert!(matches!(fx[0], Effect::BeginControlled { .. }));
        r
    }

    #[test]
    fn enter_adopts_peer_side_and_places_cursor() {
        let r = controlled(v1::Side::Left);
        assert_eq!(r.role(), Role::Controlled { epoch: 9 });
        assert_eq!(r.peer_side(), Side::Left);
    }

    #[test]
    fn controlled_moves_are_clamped_and_leave_through_return_edge() {
        let mut r = controlled(v1::Side::Left);
        // Moving away from the return edge, and past the bottom: clamped.
        let fx = r.on_peer(P::MouseMove(v1::MouseMove {
            dx: 100,
            dy: 10_000,
        }));
        let [Effect::InjectMove { to, .. }] = &fx[..] else {
            panic!("{fx:?}")
        };
        assert_eq!(to.y, 799.0);
        // Pushing back through the left edge hands control back.
        let fx = r.on_peer(P::MouseMove(v1::MouseMove { dx: -1000, dy: 0 }));
        assert_eq!(r.role(), Role::Local);
        assert!(
            matches!(&fx[0], Effect::Send(P::Leave(l)) if l.reason == LeaveReason::Edge as i32)
        );
        assert!(matches!(fx[1], Effect::EndControlled { handoff: true }));
    }

    #[test]
    fn local_activity_takes_control_back() {
        let mut r = controlled(v1::Side::Left);
        let fx = r.on_local(InputEvent::LocalActivity);
        assert_eq!(r.role(), Role::Local);
        assert!(matches!(fx[0], Effect::EndControlled { handoff: false }));
    }

    #[test]
    fn busy_side_rejects_enter() {
        let mut r = Router::new(desktop(), Side::Right);
        start_controlling(&mut r);
        let fx = r.on_peer(P::Enter(v1::Enter {
            epoch: 1,
            return_side: 1,
            y_ratio: 0.5,
        }));
        assert!(
            matches!(&fx[..], [Effect::Send(P::Leave(l))] if l.reason == LeaveReason::Rejected as i32)
        );
    }

    #[test]
    fn disconnect_recovers_without_sending() {
        let mut r = controlled(v1::Side::Left);
        assert_eq!(
            r.disconnected(),
            vec![Effect::EndControlled { handoff: false }]
        );
        let mut r = Router::new(desktop(), Side::Right);
        start_controlling(&mut r);
        let fx = r.disconnected();
        assert!(matches!(&fx[..], [Effect::EndForwarding { .. }]));
    }

    #[test]
    fn tap_disabled_while_controlling_aborts() {
        let mut r = Router::new(desktop(), Side::Right);
        start_controlling(&mut r);
        let fx = r.on_local(InputEvent::TapDisabled);
        assert_eq!(r.role(), Role::Local);
        assert!(fx.iter().any(|f| matches!(f, Effect::EndForwarding { .. })));
    }

    #[test]
    fn input_ignored_when_not_controlled() {
        let mut r = Router::new(desktop(), Side::Right);
        assert!(
            r.on_peer(P::Key(v1::Key {
                keycode: 0,
                down: true,
                repeat: false,
                flags: 0
            }))
            .is_empty()
        );
        assert!(
            r.on_peer(P::MouseMove(v1::MouseMove { dx: 1, dy: 1 }))
                .is_empty()
        );
    }
}
