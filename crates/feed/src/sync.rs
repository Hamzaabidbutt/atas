//! Order book synchronisation.
//!
//! A websocket depth stream is useless on its own: it sends *changes*, and a
//! client that starts applying them to an empty book produces depth that looks
//! plausible and is wrong. Every venue therefore specifies a handshake — open
//! the stream, buffer, fetch a REST snapshot, discard the buffered updates the
//! snapshot already contains, and verify the remainder joins on exactly.
//!
//! That handshake is a state machine with several ways to be subtly wrong, and
//! all of them fail silently: the book simply drifts. It is kept here as a
//! pure type with no I/O so it can be tested exhaustively, because the
//! alternative is discovering the bug from a mis-drawn ladder weeks later.

use serde::{Deserialize, Serialize};

/// What the caller should do with an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncAction {
    /// Hold it until the snapshot arrives.
    Buffer,
    /// Apply it to the book now.
    Apply,
    /// The snapshot already includes it. Drop it.
    Discard,
    /// A gap was detected. Discard the book and start the handshake again.
    Resync,
}

/// Where the handshake has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncState {
    /// Buffering updates while waiting for the REST snapshot.
    AwaitingSnapshot,
    /// Snapshot applied and updates joining on cleanly.
    Live,
    /// A gap was detected; nothing may be applied until a fresh snapshot.
    Gapped,
}

/// Drives a venue's depth handshake.
///
/// `T` is the venue's own delta payload; the synchroniser only ever looks at
/// the sequence range, never the contents.
#[derive(Debug, Clone)]
pub struct DepthSynchroniser<T> {
    state: SyncState,
    /// Final sequence id of the last update applied.
    last_applied: u64,
    buffer: Vec<(u64, u64, T)>,
    /// Cap on buffered updates, so a snapshot that never arrives cannot
    /// consume memory without bound.
    buffer_limit: usize,
    resyncs: u64,
}

impl<T> Default for DepthSynchroniser<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> DepthSynchroniser<T> {
    /// A synchroniser waiting for its first snapshot.
    pub fn new() -> Self {
        Self {
            state: SyncState::AwaitingSnapshot,
            last_applied: 0,
            buffer: Vec::new(),
            buffer_limit: 4_096,
            resyncs: 0,
        }
    }

    /// Set how many updates may be buffered before the handshake is abandoned.
    pub fn with_buffer_limit(mut self, limit: usize) -> Self {
        assert!(limit > 0, "buffer limit must be positive");
        self.buffer_limit = limit;
        self
    }

    /// Current state.
    pub fn state(&self) -> SyncState {
        self.state
    }

    /// How many updates are buffered.
    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }

    /// Final sequence id of the last applied update.
    pub fn last_applied(&self) -> u64 {
        self.last_applied
    }

    /// How many times a gap has forced a resync. Persistently non-zero means
    /// the connection is dropping updates.
    pub fn resyncs(&self) -> u64 {
        self.resyncs
    }

    /// Offer an update covering sequence ids `[first_id, final_id]`.
    ///
    /// While awaiting a snapshot the payload is buffered and
    /// [`SyncAction::Buffer`] returned. Once live, an update that joins on
    /// exactly returns [`SyncAction::Apply`]; one the book already contains
    /// returns [`SyncAction::Discard`]; a gap returns [`SyncAction::Resync`]
    /// and moves to [`SyncState::Gapped`].
    pub fn on_delta(&mut self, first_id: u64, final_id: u64, payload: T) -> (SyncAction, Option<T>) {
        match self.state {
            SyncState::AwaitingSnapshot => {
                if self.buffer.len() >= self.buffer_limit {
                    // The snapshot is not coming. Start over rather than
                    // buffering for ever.
                    self.buffer.clear();
                    self.resyncs += 1;
                    return (SyncAction::Resync, None);
                }
                self.buffer.push((first_id, final_id, payload));
                (SyncAction::Buffer, None)
            }
            SyncState::Gapped => (SyncAction::Resync, None),
            SyncState::Live => {
                // Entirely behind the book: a duplicate from stream overlap.
                if final_id <= self.last_applied {
                    return (SyncAction::Discard, None);
                }
                // Must begin at or before the next id the book expects.
                // A later start means updates were dropped in between.
                if first_id > self.last_applied + 1 {
                    self.state = SyncState::Gapped;
                    self.resyncs += 1;
                    return (SyncAction::Resync, None);
                }
                self.last_applied = final_id;
                (SyncAction::Apply, Some(payload))
            }
        }
    }

    /// Apply a snapshot at `last_update_id`, returning the buffered updates
    /// that must now be applied, in order.
    ///
    /// Returns `Err` when the buffer cannot bridge to the snapshot — the
    /// snapshot is older than everything buffered, so the caller must fetch a
    /// newer one rather than apply a book with a hole in it.
    pub fn on_snapshot(&mut self, last_update_id: u64) -> Result<Vec<T>, SyncError> {
        let buffered = std::mem::take(&mut self.buffer);

        // Drop everything the snapshot already contains.
        let mut pending: Vec<(u64, u64, T)> = buffered
            .into_iter()
            .filter(|(_, final_id, _)| *final_id > last_update_id)
            .collect();
        pending.sort_by_key(|(first, _, _)| *first);

        // The first surviving update must cover the id right after the
        // snapshot, or there is a hole between the two.
        if let Some((first_id, _, _)) = pending.first() {
            if *first_id > last_update_id + 1 {
                self.state = SyncState::AwaitingSnapshot;
                self.resyncs += 1;
                return Err(SyncError::SnapshotTooOld {
                    snapshot: last_update_id,
                    first_buffered: *first_id,
                });
            }
        }

        self.last_applied = last_update_id;
        self.state = SyncState::Live;

        let mut out = Vec::with_capacity(pending.len());
        for (_, final_id, payload) in pending {
            self.last_applied = self.last_applied.max(final_id);
            out.push(payload);
        }
        Ok(out)
    }

    /// Begin the handshake again, discarding buffered state.
    ///
    /// Call after a reconnect, or after acting on [`SyncAction::Resync`].
    pub fn restart(&mut self) {
        self.state = SyncState::AwaitingSnapshot;
        self.last_applied = 0;
        self.buffer.clear();
    }
}

/// Why a snapshot could not be reconciled with the buffered updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SyncError {
    /// The snapshot predates the buffered updates, leaving a hole between
    /// them. Fetch a newer snapshot.
    #[error("snapshot at {snapshot} is older than the first buffered update at {first_buffered}")]
    SnapshotTooOld {
        /// Sequence id the snapshot was taken at.
        snapshot: u64,
        /// First sequence id still buffered.
        first_buffered: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Payloads are just labels; the synchroniser never looks inside them.
    fn sync() -> DepthSynchroniser<&'static str> {
        DepthSynchroniser::new()
    }

    #[test]
    fn buffers_until_the_snapshot_arrives() {
        let mut s = sync();
        assert_eq!(s.state(), SyncState::AwaitingSnapshot);

        assert_eq!(s.on_delta(10, 12, "a").0, SyncAction::Buffer);
        assert_eq!(s.on_delta(13, 15, "b").0, SyncAction::Buffer);
        assert_eq!(s.buffered(), 2);
    }

    #[test]
    fn a_snapshot_drops_what_it_already_contains() {
        let mut s = sync();
        s.on_delta(1, 5, "old");
        s.on_delta(6, 10, "boundary");
        s.on_delta(11, 15, "new");

        // Snapshot at 10 already contains everything up to and including 10.
        let apply = s.on_snapshot(10).unwrap();
        assert_eq!(apply, vec!["new"], "only updates past the snapshot apply");
        assert_eq!(s.state(), SyncState::Live);
        assert_eq!(s.last_applied(), 15);
        assert_eq!(s.buffered(), 0);
    }

    #[test]
    fn a_snapshot_mid_update_still_applies_that_update() {
        // The update straddling the snapshot must be applied: the snapshot
        // contains only part of its range.
        let mut s = sync();
        s.on_delta(6, 12, "straddles");
        let apply = s.on_snapshot(10).unwrap();
        assert_eq!(apply, vec!["straddles"]);
        assert_eq!(s.last_applied(), 12);
    }

    #[test]
    fn buffered_updates_are_applied_in_sequence_order() {
        let mut s = sync();
        // Delivered out of order, as a reordering proxy could produce.
        s.on_delta(21, 25, "third");
        s.on_delta(11, 15, "first");
        s.on_delta(16, 20, "second");

        let apply = s.on_snapshot(10).unwrap();
        assert_eq!(apply, vec!["first", "second", "third"]);
    }

    #[test]
    fn a_snapshot_older_than_the_buffer_is_rejected() {
        // Applying it would leave a hole between the snapshot and the first
        // buffered update, and the book would be silently wrong.
        let mut s = sync();
        s.on_delta(100, 110, "later");

        let err = s.on_snapshot(50).unwrap_err();
        assert_eq!(
            err,
            SyncError::SnapshotTooOld {
                snapshot: 50,
                first_buffered: 100
            }
        );
        assert_eq!(s.state(), SyncState::AwaitingSnapshot, "must retry");
        assert_eq!(s.resyncs(), 1);
    }

    #[test]
    fn an_empty_buffer_accepts_any_snapshot() {
        let mut s = sync();
        assert!(s.on_snapshot(999).unwrap().is_empty());
        assert_eq!(s.state(), SyncState::Live);
        assert_eq!(s.last_applied(), 999);
    }

    #[test]
    fn live_updates_that_join_on_are_applied() {
        let mut s = sync();
        s.on_snapshot(100).unwrap();

        let (action, payload) = s.on_delta(101, 105, "next");
        assert_eq!(action, SyncAction::Apply);
        assert_eq!(payload, Some("next"));
        assert_eq!(s.last_applied(), 105);

        // Contiguous continuation.
        assert_eq!(s.on_delta(106, 110, "more").0, SyncAction::Apply);
        assert_eq!(s.last_applied(), 110);
    }

    #[test]
    fn an_overlapping_update_is_applied_not_dropped() {
        // Venues may resend a range that starts before the book's position;
        // as long as it reaches past it, it carries new information.
        let mut s = sync();
        s.on_snapshot(100).unwrap();
        assert_eq!(s.on_delta(98, 104, "overlaps").0, SyncAction::Apply);
        assert_eq!(s.last_applied(), 104);
    }

    #[test]
    fn a_duplicate_update_is_discarded_without_a_resync() {
        // REST and websocket overlap at startup routinely. This is normal
        // traffic, not a gap, and must not tear the book down.
        let mut s = sync();
        s.on_snapshot(100).unwrap();
        s.on_delta(101, 110, "applied");

        assert_eq!(s.on_delta(101, 110, "again").0, SyncAction::Discard);
        assert_eq!(s.on_delta(95, 99, "ancient").0, SyncAction::Discard);
        assert_eq!(s.state(), SyncState::Live, "duplicates are not gaps");
        assert_eq!(s.resyncs(), 0);
        assert_eq!(s.last_applied(), 110);
    }

    #[test]
    fn a_gap_forces_a_resync_and_latches() {
        let mut s = sync();
        s.on_snapshot(100).unwrap();
        s.on_delta(101, 105, "ok");

        // 107 skips 106: updates were lost.
        assert_eq!(s.on_delta(107, 110, "gap").0, SyncAction::Resync);
        assert_eq!(s.state(), SyncState::Gapped);
        assert_eq!(s.resyncs(), 1);

        // Nothing may be applied until a fresh snapshot, even a perfect one.
        assert_eq!(s.on_delta(106, 108, "would fit").0, SyncAction::Resync);
        assert_eq!(s.resyncs(), 1, "the gap is only counted once");
    }

    #[test]
    fn restart_returns_to_the_handshake() {
        let mut s = sync();
        s.on_snapshot(100).unwrap();
        s.on_delta(107, 110, "gap");
        assert_eq!(s.state(), SyncState::Gapped);

        s.restart();
        assert_eq!(s.state(), SyncState::AwaitingSnapshot);
        assert_eq!(s.last_applied(), 0);
        assert_eq!(s.on_delta(1, 2, "fresh").0, SyncAction::Buffer);
    }

    #[test]
    fn a_snapshot_that_never_arrives_does_not_leak() {
        let mut s = DepthSynchroniser::<&str>::new().with_buffer_limit(4);
        for _ in 0..4 {
            assert_eq!(s.on_delta(1, 2, "x").0, SyncAction::Buffer);
        }
        // The fifth gives up rather than buffering without bound.
        assert_eq!(s.on_delta(1, 2, "x").0, SyncAction::Resync);
        assert_eq!(s.buffered(), 0);
        assert_eq!(s.resyncs(), 1);
    }

    #[test]
    fn a_full_handshake_then_a_gap_then_recovery() {
        let mut s = sync();

        // Buffer while the snapshot request is in flight.
        s.on_delta(98, 100, "stale");
        s.on_delta(101, 103, "bridge");
        let apply = s.on_snapshot(100).unwrap();
        assert_eq!(apply, vec!["bridge"]);
        assert_eq!(s.state(), SyncState::Live);

        // Run cleanly.
        assert_eq!(s.on_delta(104, 106, "a").0, SyncAction::Apply);

        // Lose updates.
        assert_eq!(s.on_delta(200, 210, "jump").0, SyncAction::Resync);

        // Recover with a fresh handshake.
        s.restart();
        s.on_delta(215, 218, "buffered");
        let apply = s.on_snapshot(214).unwrap();
        assert_eq!(apply, vec!["buffered"]);
        assert_eq!(s.state(), SyncState::Live);
        assert_eq!(s.last_applied(), 218);
    }
}
