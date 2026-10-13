//! Control lease state machine (report §6.1–6.2).
//!
//! One `SessionCore` per OS interactive session. It is driven by a single
//! actor; the input hook never calls into it directly but reads the
//! `ForwardMode` snapshot the driver publishes from `Action::SetForwarding`.
//!
//! Timers and transport live in the driver. When a prepare times out or a
//! lease TTL expires, the driver feeds the matching event back in.

use osm_model::{DeviceId, LeaseEpoch};

/// What the input hook does with local physical events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForwardMode {
    /// Deliver to the local OS.
    Local,
    /// Suppress locally but send nowhere: the previous target is releasing
    /// and the next one is not committed yet.
    Hold,
    /// Suppress locally and forward under this lease.
    Remote { target: DeviceId, epoch: LeaseEpoch },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lease {
    pub peer: DeviceId,
    pub epoch: LeaseEpoch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparePhase {
    /// LEASE_PREPARE sent, waiting for the target's reservation.
    AwaitingReady,
    /// Reservation held; RELEASE sent to the previous target, waiting for
    /// RELEASED. We must not commit on a timer alone (§6.2).
    AwaitingRelease,
    /// LEASE_COMMIT sent, waiting for ACTIVE.
    AwaitingActive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlState {
    Local,
    Paused,
    Preparing {
        next: Lease,
        previous: Option<Lease>,
        phase: PreparePhase,
    },
    Sending(Lease),
    Receiving(Lease),
    /// Releasing app-held input state; the driver reports `CleanupDone`.
    Recovering,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Edge crossing, hotkey or QuickJump resolved to a target.
    SwitchIntent {
        target: DeviceId,
    },
    PrepareReady {
        target: DeviceId,
        epoch: LeaseEpoch,
    },
    PrepareRejected {
        target: DeviceId,
        epoch: LeaseEpoch,
    },
    PrepareTimedOut {
        epoch: LeaseEpoch,
    },
    PreviousReleased {
        peer: DeviceId,
        epoch: LeaseEpoch,
    },
    CommitActive {
        target: DeviceId,
        epoch: LeaseEpoch,
    },
    CommitFailed {
        target: DeviceId,
        epoch: LeaseEpoch,
    },
    InboundLease {
        owner: DeviceId,
        epoch: LeaseEpoch,
    },
    /// The inbound lease TTL ran out without a valid heartbeat.
    LeaseExpired {
        epoch: LeaseEpoch,
    },
    /// Local physical input while being controlled.
    LocalReclaim,
    Panic,
    ConnectionLost {
        peer: DeviceId,
    },
    CleanupDone,
    Pause,
    Resume,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RejectReason {
    Busy,
    Paused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    SetForwarding(ForwardMode),
    SendPrepare(Lease),
    SendCancelReservation(Lease),
    SendReleaseAll(Lease),
    SendCommit(Lease),
    AcceptLease(Lease),
    RejectLease {
        owner: DeviceId,
        epoch: LeaseEpoch,
        reason: RejectReason,
    },
    /// Release keys/buttons this app injected under the lease, and only those.
    ReleaseOwnedInput(Lease),
    NotifyReclaimed(Lease),
}

#[derive(Debug)]
pub struct SessionCore {
    state: ControlState,
    last_epoch: LeaseEpoch,
}

impl Default for SessionCore {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionCore {
    pub fn new() -> Self {
        Self {
            state: ControlState::Local,
            last_epoch: LeaseEpoch::default(),
        }
    }

    pub fn state(&self) -> ControlState {
        self.state
    }

    fn allocate(&mut self, peer: DeviceId) -> Lease {
        self.last_epoch = self.last_epoch.next();
        Lease {
            peer,
            epoch: self.last_epoch,
        }
    }

    pub fn handle(&mut self, event: Event) -> Vec<Action> {
        use Action as A;
        use ControlState as S;
        use Event as E;

        let (next, actions) = match (self.state, event) {
            // Panic always wins, whatever else is going on.
            (S::Sending(lease), E::Panic) => (
                S::Recovering,
                vec![
                    A::SetForwarding(ForwardMode::Local),
                    A::SendReleaseAll(lease),
                ],
            ),
            (S::Preparing { next, previous, .. }, E::Panic) => {
                let mut v = vec![
                    A::SetForwarding(ForwardMode::Local),
                    A::SendCancelReservation(next),
                ];
                v.extend(previous.map(A::SendReleaseAll));
                (S::Recovering, v)
            }
            (S::Receiving(lease), E::Panic | E::LocalReclaim) => (
                S::Recovering,
                vec![A::ReleaseOwnedInput(lease), A::NotifyReclaimed(lease)],
            ),

            (S::Local, E::SwitchIntent { target }) => {
                let next = self.allocate(target);
                (
                    S::Preparing {
                        next,
                        previous: None,
                        phase: PreparePhase::AwaitingReady,
                    },
                    vec![A::SendPrepare(next)],
                )
            }
            (S::Sending(current), E::SwitchIntent { target }) if current.peer != target => {
                let next = self.allocate(target);
                (
                    S::Preparing {
                        next,
                        previous: Some(current),
                        phase: PreparePhase::AwaitingReady,
                    },
                    vec![A::SendPrepare(next)],
                )
            }

            (
                S::Preparing {
                    next,
                    previous,
                    phase: PreparePhase::AwaitingReady,
                },
                E::PrepareReady { target, epoch },
            ) if next.peer == target && next.epoch == epoch => match previous {
                Some(prev) => (
                    S::Preparing {
                        next,
                        previous,
                        phase: PreparePhase::AwaitingRelease,
                    },
                    vec![A::SetForwarding(ForwardMode::Hold), A::SendReleaseAll(prev)],
                ),
                None => (
                    S::Preparing {
                        next,
                        previous,
                        phase: PreparePhase::AwaitingActive,
                    },
                    vec![A::SendCommit(next)],
                ),
            },

            (
                S::Preparing {
                    next,
                    previous: Some(prev),
                    phase: PreparePhase::AwaitingRelease,
                },
                E::PreviousReleased { peer, epoch },
            ) if prev.peer == peer && prev.epoch == epoch => (
                S::Preparing {
                    next,
                    previous: None,
                    phase: PreparePhase::AwaitingActive,
                },
                vec![A::SendCommit(next)],
            ),

            (
                S::Preparing {
                    next,
                    phase: PreparePhase::AwaitingActive,
                    ..
                },
                E::CommitActive { target, epoch },
            ) if next.peer == target && next.epoch == epoch => (
                S::Sending(next),
                vec![A::SetForwarding(ForwardMode::Remote { target, epoch })],
            ),

            // The new target refused before the old one released anything:
            // stay where we were (§6.2).
            (
                S::Preparing {
                    next,
                    previous,
                    phase: PreparePhase::AwaitingReady,
                },
                E::PrepareRejected { target, epoch },
            ) if next.peer == target && next.epoch == epoch => back_out(previous),
            (
                S::Preparing {
                    next,
                    previous,
                    phase: PreparePhase::AwaitingReady,
                },
                E::PrepareTimedOut { epoch },
            ) if next.epoch == epoch => {
                let (state, mut v) = back_out(previous);
                v.insert(0, A::SendCancelReservation(next));
                (state, v)
            }

            // Old target is gone or unconfirmed and the new one failed: never
            // feed two targets, go home.
            (S::Preparing { next, .. }, E::CommitFailed { target, epoch })
                if next.peer == target && next.epoch == epoch =>
            {
                (S::Recovering, vec![A::SetForwarding(ForwardMode::Local)])
            }
            (
                S::Preparing {
                    next,
                    phase: PreparePhase::AwaitingRelease | PreparePhase::AwaitingActive,
                    ..
                },
                E::PrepareTimedOut { epoch },
            ) if next.epoch == epoch => (
                S::Recovering,
                vec![
                    A::SetForwarding(ForwardMode::Local),
                    A::SendCancelReservation(next),
                ],
            ),

            (S::Local, E::InboundLease { owner, epoch }) => {
                let lease = Lease { peer: owner, epoch };
                (S::Receiving(lease), vec![A::AcceptLease(lease)])
            }
            (S::Paused, E::InboundLease { owner, epoch }) => (
                S::Paused,
                vec![A::RejectLease {
                    owner,
                    epoch,
                    reason: RejectReason::Paused,
                }],
            ),
            (state, E::InboundLease { owner, epoch }) => (
                state,
                vec![A::RejectLease {
                    owner,
                    epoch,
                    reason: RejectReason::Busy,
                }],
            ),

            (S::Receiving(lease), E::LeaseExpired { epoch }) if lease.epoch == epoch => {
                (S::Recovering, vec![A::ReleaseOwnedInput(lease)])
            }
            (S::Receiving(lease), E::ConnectionLost { peer }) if lease.peer == peer => {
                (S::Recovering, vec![A::ReleaseOwnedInput(lease)])
            }
            (S::Sending(lease), E::ConnectionLost { peer }) if lease.peer == peer => {
                (S::Recovering, vec![A::SetForwarding(ForwardMode::Local)])
            }
            (S::Preparing { next, previous, .. }, E::ConnectionLost { peer })
                if next.peer == peer || previous.is_some_and(|p| p.peer == peer) =>
            {
                (
                    S::Recovering,
                    vec![
                        A::SetForwarding(ForwardMode::Local),
                        A::SendCancelReservation(next),
                    ],
                )
            }

            (S::Recovering, E::CleanupDone) => (S::Local, vec![]),
            (S::Local, E::Pause) => (S::Paused, vec![]),
            (S::Paused, E::Resume) => (S::Local, vec![]),

            // Stale epochs, duplicates and events that do not apply in the
            // current state are dropped. They must never resurrect a lease.
            (state, _) => (state, vec![]),
        };
        self.state = next;
        actions
    }
}

fn back_out(previous: Option<Lease>) -> (ControlState, Vec<Action>) {
    match previous {
        Some(prev) => (ControlState::Sending(prev), vec![]),
        None => (
            ControlState::Local,
            vec![Action::SetForwarding(ForwardMode::Local)],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(n: u8) -> DeviceId {
        DeviceId([n; 32])
    }

    fn sending_to(core: &mut SessionCore, target: DeviceId) -> Lease {
        let a = core.handle(Event::SwitchIntent { target });
        let Action::SendPrepare(lease) = a[0] else {
            panic!("expected prepare, got {a:?}")
        };
        core.handle(Event::PrepareReady {
            target,
            epoch: lease.epoch,
        });
        core.handle(Event::CommitActive {
            target,
            epoch: lease.epoch,
        });
        assert_eq!(core.state(), ControlState::Sending(lease));
        lease
    }

    #[test]
    fn local_to_sending_commits_after_ready() {
        let mut core = SessionCore::new();
        let b = dev(2);
        let a = core.handle(Event::SwitchIntent { target: b });
        let Action::SendPrepare(lease) = a[0] else {
            unreachable!()
        };
        let a = core.handle(Event::PrepareReady {
            target: b,
            epoch: lease.epoch,
        });
        assert_eq!(a, vec![Action::SendCommit(lease)]);
        let a = core.handle(Event::CommitActive {
            target: b,
            epoch: lease.epoch,
        });
        assert_eq!(
            a,
            vec![Action::SetForwarding(ForwardMode::Remote {
                target: b,
                epoch: lease.epoch
            })]
        );
    }

    #[test]
    fn a_b_c_handoff_waits_for_release_before_commit() {
        let mut core = SessionCore::new();
        let (b, c) = (dev(2), dev(3));
        let old = sending_to(&mut core, b);

        let a = core.handle(Event::SwitchIntent { target: c });
        let Action::SendPrepare(new) = a[0] else {
            unreachable!()
        };
        assert!(new.epoch > old.epoch);

        let a = core.handle(Event::PrepareReady {
            target: c,
            epoch: new.epoch,
        });
        assert_eq!(
            a,
            vec![
                Action::SetForwarding(ForwardMode::Hold),
                Action::SendReleaseAll(old)
            ]
        );

        // A commit must not happen on anything but B's RELEASED.
        assert!(
            core.handle(Event::CommitActive {
                target: c,
                epoch: new.epoch
            })
            .is_empty()
        );

        let a = core.handle(Event::PreviousReleased {
            peer: b,
            epoch: old.epoch,
        });
        assert_eq!(a, vec![Action::SendCommit(new)]);
        core.handle(Event::CommitActive {
            target: c,
            epoch: new.epoch,
        });
        assert_eq!(core.state(), ControlState::Sending(new));
    }

    #[test]
    fn rejected_prepare_keeps_previous_target() {
        let mut core = SessionCore::new();
        let (b, c) = (dev(2), dev(3));
        let old = sending_to(&mut core, b);
        let a = core.handle(Event::SwitchIntent { target: c });
        let Action::SendPrepare(new) = a[0] else {
            unreachable!()
        };
        core.handle(Event::PrepareRejected {
            target: c,
            epoch: new.epoch,
        });
        assert_eq!(core.state(), ControlState::Sending(old));
    }

    #[test]
    fn commit_failure_after_release_goes_home() {
        let mut core = SessionCore::new();
        let (b, c) = (dev(2), dev(3));
        let old = sending_to(&mut core, b);
        let a = core.handle(Event::SwitchIntent { target: c });
        let Action::SendPrepare(new) = a[0] else {
            unreachable!()
        };
        core.handle(Event::PrepareReady {
            target: c,
            epoch: new.epoch,
        });
        core.handle(Event::PreviousReleased {
            peer: b,
            epoch: old.epoch,
        });
        let a = core.handle(Event::CommitFailed {
            target: c,
            epoch: new.epoch,
        });
        assert_eq!(a, vec![Action::SetForwarding(ForwardMode::Local)]);
        assert_eq!(core.state(), ControlState::Recovering);
        core.handle(Event::CleanupDone);
        assert_eq!(core.state(), ControlState::Local);
    }

    #[test]
    fn busy_target_rejects_second_owner() {
        let mut core = SessionCore::new();
        let epoch = LeaseEpoch(1);
        core.handle(Event::InboundLease {
            owner: dev(1),
            epoch,
        });
        let a = core.handle(Event::InboundLease {
            owner: dev(9),
            epoch,
        });
        assert_eq!(
            a,
            vec![Action::RejectLease {
                owner: dev(9),
                epoch,
                reason: RejectReason::Busy
            }]
        );
    }

    #[test]
    fn stale_expiry_does_not_release_current_lease() {
        let mut core = SessionCore::new();
        let current = Lease {
            peer: dev(1),
            epoch: LeaseEpoch(5),
        };
        core.handle(Event::InboundLease {
            owner: current.peer,
            epoch: current.epoch,
        });
        assert!(
            core.handle(Event::LeaseExpired {
                epoch: LeaseEpoch(4)
            })
            .is_empty()
        );
        assert_eq!(core.state(), ControlState::Receiving(current));
        let a = core.handle(Event::LeaseExpired {
            epoch: current.epoch,
        });
        assert_eq!(a, vec![Action::ReleaseOwnedInput(current)]);
    }

    #[test]
    fn panic_while_sending_restores_local_first() {
        let mut core = SessionCore::new();
        let lease = sending_to(&mut core, dev(2));
        let a = core.handle(Event::Panic);
        assert_eq!(a[0], Action::SetForwarding(ForwardMode::Local));
        assert!(a.contains(&Action::SendReleaseAll(lease)));
    }

    #[test]
    fn local_reclaim_releases_only_owned_input() {
        let mut core = SessionCore::new();
        let lease = Lease {
            peer: dev(1),
            epoch: LeaseEpoch(1),
        };
        core.handle(Event::InboundLease {
            owner: lease.peer,
            epoch: lease.epoch,
        });
        let a = core.handle(Event::LocalReclaim);
        assert_eq!(
            a,
            vec![
                Action::ReleaseOwnedInput(lease),
                Action::NotifyReclaimed(lease)
            ]
        );
    }
}
