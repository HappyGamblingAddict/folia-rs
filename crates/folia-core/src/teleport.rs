//! Per-session teleport request and acknowledgement bookkeeping.
//!
//! In 1.21.11 a `ClientboundTeleportEntity` (play id 70) opens with a VarInt
//! teleport id; the captured join packet uses id `1`, and `join::position_packet`
//! writes the same layout. The client confirms it with a
//! `ServerboundAcceptTeleportation` (play id 0) whose only field is that same
//! VarInt (see [`crate::protocol::parse_accept_teleportation`]).
//!
//! While an id is outstanding the server must not validate the client's
//! movement: the client may still be reporting the position it had before the
//! teleport was applied, and rubber-banding that back would undo the teleport.
//! A stale or unknown acknowledgement is ignored rather than treated as an
//! error, so acks for the login position or for superseded teleports stay safe.

/// First teleport id handed out by the play loop.
///
/// The login position is sent before the play loop starts with id `1000` and is
/// deliberately not tracked here; the old counter was seeded at `1001` and
/// incremented before each send, so `1002` keeps the live packet sequence
/// byte-identical. The ack for `1000` is therefore an unknown id and is ignored.
pub const FIRST_TELEPORT_ID: i32 = 1002;

/// The single teleport a session is currently waiting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TeleportTracker {
    next_id: i32,
    pending: Option<i32>,
}

impl TeleportTracker {
    /// Start issuing ids at `next_id`. Non-positive seeds are mapped to `1` so
    /// a zero/unused id never looks like a real acknowledgement.
    pub fn new(next_id: i32) -> Self {
        Self {
            next_id: next_id.max(1),
            pending: None,
        }
    }

    /// Issue a fresh id and make it pending. Issuing another teleport before
    /// the previous one was acknowledged supersedes it: the late ack for the
    /// older id is then stale and is ignored.
    pub fn issue(&mut self) -> i32 {
        let id = self.next_id;
        self.next_id = if id == i32::MAX { 1 } else { id + 1 };
        self.pending = Some(id);
        id
    }

    /// Clear the pending id when, and only when, `id` matches it. Returns
    /// whether the acknowledgement was accepted.
    pub fn acknowledge(&mut self, id: i32) -> bool {
        if self.pending == Some(id) {
            self.pending = None;
            true
        } else {
            false
        }
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn pending_id(&self) -> Option<i32> {
        self.pending
    }

    pub fn next_id(&self) -> i32 {
        self.next_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_incremented_for_every_issued_teleport() {
        let mut tracker = TeleportTracker::new(FIRST_TELEPORT_ID);
        assert_eq!(tracker.next_id(), FIRST_TELEPORT_ID);
        assert_eq!(tracker.pending_id(), None);
        assert!(!tracker.is_pending());

        assert_eq!(tracker.issue(), FIRST_TELEPORT_ID);
        assert_eq!(tracker.pending_id(), Some(FIRST_TELEPORT_ID));
        assert_eq!(tracker.next_id(), FIRST_TELEPORT_ID + 1);
        assert_eq!(tracker.issue(), FIRST_TELEPORT_ID + 1);
        assert_eq!(tracker.pending_id(), Some(FIRST_TELEPORT_ID + 1));
        assert_eq!(tracker.next_id(), FIRST_TELEPORT_ID + 2);
    }

    #[test]
    fn matching_acknowledgement_clears_only_the_pending_id() {
        let mut tracker = TeleportTracker::new(50);
        let id = tracker.issue();
        assert!(tracker.acknowledge(id));
        assert!(!tracker.is_pending());
        assert_eq!(tracker.pending_id(), None);
        // A second ack for the same id is stale and does nothing.
        assert!(!tracker.acknowledge(id));
        assert!(!tracker.is_pending());
        assert_eq!(tracker.next_id(), 51, "acks never consume an id");
    }

    #[test]
    fn mismatched_acknowledgement_leaves_the_pending_id_untouched() {
        let mut tracker = TeleportTracker::new(50);
        let id = tracker.issue();
        assert!(!tracker.acknowledge(id + 1));
        assert_eq!(tracker.pending_id(), Some(id));
        assert!(!tracker.acknowledge(id - 1));
        assert_eq!(tracker.pending_id(), Some(id));
        assert!(!tracker.acknowledge(-7));
        assert_eq!(tracker.pending_id(), Some(id));
        assert!(tracker.acknowledge(id));
    }

    #[test]
    fn stale_acknowledgement_after_a_new_teleport_is_ignored() {
        let mut tracker = TeleportTracker::new(50);
        let first = tracker.issue();
        let second = tracker.issue();
        assert_ne!(first, second);
        assert!(!tracker.acknowledge(first), "the superseded id is stale");
        assert_eq!(tracker.pending_id(), Some(second));
        assert!(tracker.acknowledge(second));
        assert!(!tracker.is_pending());
    }

    #[test]
    fn unknown_acknowledgement_on_a_fresh_session_is_ignored() {
        let mut tracker = TeleportTracker::new(FIRST_TELEPORT_ID);
        // Real clients ack the login position (id 1000) before any play-loop
        // teleport exists; that ack must be harmless.
        assert!(!tracker.acknowledge(1000));
        assert!(!tracker.is_pending());
        assert_eq!(tracker.issue(), FIRST_TELEPORT_ID);
    }

    #[test]
    fn id_generation_wraps_without_reusing_zero() {
        let mut tracker = TeleportTracker::new(i32::MAX);
        let id = tracker.issue();
        assert_eq!(id, i32::MAX);
        let wrapped = tracker.issue();
        assert_eq!(wrapped, 1);
        assert!(
            !tracker.acknowledge(id),
            "the wrapped id supersedes the old one"
        );
        assert!(tracker.acknowledge(wrapped));
        assert!(!tracker.is_pending());

        let mut zero = TeleportTracker::new(0);
        assert_eq!(zero.next_id(), 1);
        assert_eq!(zero.issue(), 1);
    }
}
