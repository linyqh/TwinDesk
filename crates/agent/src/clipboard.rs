//! Clipboard hand-off on screen switches.
//!
//! The Mac the cursor leaves offers its clipboard if it changed since the
//! last hand-off; the other Mac fetches it once over a bulk connection and
//! writes it locally, so the next Cmd+V there pastes it. Nothing here runs on
//! the engine thread except the change-count snapshot at connect:
//! pasteboard calls go to the pasteboard server and may stall, so offers are
//! prepared on a worker and contents move on bulk workers.
//!
//! Change counts are only ever compared with counts from the same Mac.

use std::io::Write;
use std::sync::Mutex;

use osm_platform_macos::Pasteboard;
use osm_protocol::v1;
use osm_transfer::TransferError;
use osm_transfer::clipboard::{self, Representation};

#[derive(Debug)]
pub(crate) struct Clipboard {
    board: Pasteboard,
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    /// Change count already handed off, written by us, or present at
    /// connect. Only newer contents are offered, so a received clipboard is
    /// never echoed back.
    synced: Option<i64>,
    served: Option<Served>,
    /// Bumped by each incoming offer and by disconnect; only the newest
    /// fetch may write.
    incoming: u64,
}

#[derive(Debug)]
struct Served {
    offer_id: u64,
    token: Vec<u8>,
    change_count: i64,
}

/// Snapshot taken when a fetch starts. The fetched contents are written only
/// if nothing else touched the local clipboard in the meantime.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Ticket {
    incoming: u64,
    change_count: i64,
}

impl Clipboard {
    #[cfg(test)]
    pub(crate) fn board(&self) -> &Pasteboard {
        &self.board
    }

    pub(crate) fn new(board: Pasteboard) -> Self {
        Self {
            board,
            state: Mutex::default(),
        }
    }

    /// Contents copied before the connection existed are not pushed.
    pub(crate) fn connected(&self) {
        let mut g = self.state.lock().unwrap();
        g.synced = Some(self.board.change_count());
        g.served = None;
        g.incoming += 1;
    }

    pub(crate) fn disconnected(&self) {
        let mut g = self.state.lock().unwrap();
        g.served = None;
        g.incoming += 1;
    }

    /// Returns an offer if the clipboard changed since the last hand-off and
    /// holds something shareable. Replaces any earlier unclaimed offer.
    pub(crate) fn offer(
        &self,
        offer_id: u64,
        token: Vec<u8>,
        bulk_port: u16,
    ) -> Option<v1::ClipboardOffer> {
        let mut g = self.state.lock().unwrap();
        let count = self.board.change_count();
        if g.synced.is_none_or(|s| s == count) {
            return None;
        }
        g.synced = Some(count);
        g.served = None;
        if clipboard::shareable_types(&self.board.types()).is_empty() {
            return None;
        }
        g.served = Some(Served {
            offer_id,
            token: token.clone(),
            change_count: count,
        });
        Some(v1::ClipboardOffer {
            offer_id,
            token,
            bulk_port: bulk_port.into(),
        })
    }

    /// Answers one fetch for the current offer. Each offer is served once.
    pub(crate) fn serve(
        &self,
        fetch: &v1::ClipboardFetch,
        w: &mut impl Write,
    ) -> Result<(), TransferError> {
        let reps = {
            let mut g = self.state.lock().unwrap();
            let count = match g.served.take() {
                Some(s)
                    if s.offer_id == fetch.offer_id
                        && crate::transfers::constant_time_eq(&s.token, &fetch.token) =>
                {
                    s.change_count
                }
                other => {
                    g.served = other;
                    osm_transfer::send::refuse(w, &TransferError::NotFound);
                    return Err(TransferError::NotFound);
                }
            };
            self.read(count)
        };
        match reps {
            Ok(reps) => clipboard::serve(w, &reps),
            Err(e) => {
                osm_transfer::send::refuse(w, &e);
                Err(e)
            }
        }
    }

    /// Reads every shareable representation of the generation `count`,
    /// refusing if the user copied something else after the offer.
    fn read(&self, count: i64) -> Result<Vec<Representation>, TransferError> {
        let changed = || TransferError::SourceChanged("剪贴板".into());
        if self.board.change_count() != count {
            return Err(changed());
        }
        let reps = clipboard::shareable_types(&self.board.types())
            .into_iter()
            .filter_map(|ty| self.board.data(&ty).map(|data| Representation { ty, data }))
            .collect();
        if self.board.change_count() != count {
            return Err(changed());
        }
        clipboard::within_limits(reps)
    }

    /// Called in offer order, so a later offer always supersedes an earlier
    /// fetch still in flight.
    pub(crate) fn next_incoming(&self) -> u64 {
        let mut g = self.state.lock().unwrap();
        g.incoming += 1;
        g.incoming
    }

    /// Taken on the fetch worker before any data moves.
    pub(crate) fn ticket(&self, incoming: u64) -> Ticket {
        Ticket {
            incoming,
            change_count: self.board.change_count(),
        }
    }

    /// Writes fetched contents unless the fetch was superseded, the link
    /// dropped, or the user copied something locally meanwhile.
    pub(crate) fn apply(&self, ticket: Ticket, reps: Vec<Representation>) -> Result<(), String> {
        let mut g = self.state.lock().unwrap();
        if g.incoming != ticket.incoming {
            return Err("superseded".into());
        }
        if self.board.change_count() != ticket.change_count {
            return Err("local clipboard changed".into());
        }
        let items: Vec<_> = reps.into_iter().map(|r| (r.ty, r.data)).collect();
        let count = self.board.write(&items)?;
        g.synced = Some(count);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use osm_transfer::clipboard::{HTML, TEXT};

    fn text(s: &str) -> Vec<(String, Vec<u8>)> {
        vec![(TEXT.to_string(), s.as_bytes().to_vec())]
    }

    fn serve_bytes(c: &Clipboard, offer: &v1::ClipboardOffer) -> Result<Vec<u8>, TransferError> {
        let mut wire = Vec::new();
        c.serve(
            &v1::ClipboardFetch {
                offer_id: offer.offer_id,
                token: offer.token.clone(),
            },
            &mut wire,
        )
        .map(|()| wire)
    }

    #[test]
    fn offers_only_contents_copied_since_the_last_hand_off() {
        let c = Clipboard::new(Pasteboard::with_unique_name());
        c.board.write(&text("before connect")).unwrap();
        assert!(c.offer(1, vec![1; 16], 9).is_none(), "not connected yet");
        c.connected();
        assert!(c.offer(1, vec![1; 16], 9).is_none());
        c.board.write(&text("copied")).unwrap();
        let offer = c.offer(2, vec![2; 16], 9).unwrap();
        assert_eq!((offer.offer_id, offer.bulk_port), (2, 9));
        assert!(c.offer(3, vec![3; 16], 9).is_none(), "already handed off");
    }

    #[test]
    fn serves_once_and_refuses_bad_tokens_and_changed_contents() {
        let c = Clipboard::new(Pasteboard::with_unique_name());
        c.connected();
        c.board
            .write(&[
                (HTML.into(), b"<b>x</b>".to_vec()),
                (TEXT.into(), b"x".to_vec()),
            ])
            .unwrap();
        let offer = c.offer(5, vec![5; 16], 9).unwrap();
        let mut forged = offer.clone();
        forged.token = vec![6; 16];
        assert!(matches!(
            serve_bytes(&c, &forged),
            Err(TransferError::NotFound)
        ));
        let wire = serve_bytes(&c, &offer).unwrap();
        let reps = clipboard::receive(&mut &wire[..]).unwrap();
        assert_eq!(reps.len(), 2);
        assert_eq!(reps[0].ty, HTML);
        assert!(serve_bytes(&c, &offer).is_err(), "served twice");

        c.board.write(&text("one")).unwrap();
        let offer = c.offer(7, vec![7; 16], 9).unwrap();
        c.board.write(&text("two")).unwrap();
        assert!(matches!(
            serve_bytes(&c, &offer),
            Err(TransferError::SourceChanged(_))
        ));
    }

    #[test]
    fn apply_writes_without_echo_unless_superseded_or_locally_changed() {
        let c = Clipboard::new(Pasteboard::with_unique_name());
        c.connected();
        let reps = |s: &str| {
            vec![Representation {
                ty: TEXT.into(),
                data: s.as_bytes().to_vec(),
            }]
        };

        let ticket = c.ticket(c.next_incoming());
        c.apply(ticket, reps("remote")).unwrap();
        assert_eq!(c.board.data(TEXT).unwrap(), b"remote");
        assert!(c.offer(1, vec![1; 16], 9).is_none(), "would echo back");

        let ticket = c.ticket(c.next_incoming());
        c.board.write(&text("local copy")).unwrap();
        assert!(c.apply(ticket, reps("late")).is_err());
        assert_eq!(c.board.data(TEXT).unwrap(), b"local copy");

        let old = c.ticket(c.next_incoming());
        let _new = c.ticket(c.next_incoming());
        assert!(c.apply(old, reps("stale")).is_err());
        let ticket = c.ticket(c.next_incoming());
        c.disconnected();
        assert!(c.apply(ticket, reps("after disconnect")).is_err());
        assert_eq!(c.board.data(TEXT).unwrap(), b"local copy");
    }
}
