//! Hybrid Logical Clock (HLC).
//!
//! Combines physical time (so ordering roughly tracks wall-clock) with a
//! logical counter (so events created within the same millisecond, or while
//! clocks are skewed, still get a total order). Each device keeps one clock;
//! `tick` stamps a local event, `update` folds in a timestamp seen from a peer.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Hlc {
    /// Physical time component, Unix milliseconds.
    pub wall_ms: i64,
    /// Logical counter, breaks ties within the same `wall_ms`.
    pub counter: u32,
}

impl Hlc {
    pub fn new(wall_ms: i64, counter: u32) -> Self {
        Self { wall_ms, counter }
    }

    /// Stamp a new *local* event given the current physical time.
    ///
    /// If the wall clock advanced past our last `wall_ms`, adopt it and reset
    /// the counter; otherwise keep `wall_ms` and bump the counter (covers
    /// multiple events in the same ms and a non-monotonic wall clock).
    pub fn tick(&mut self, now_ms: i64) -> Hlc {
        if now_ms > self.wall_ms {
            self.wall_ms = now_ms;
            self.counter = 0;
        } else {
            self.counter = self.counter.saturating_add(1);
        }
        *self
    }

    /// Fold in a timestamp received from a peer, then return a fresh local
    /// stamp that is strictly greater than both our previous state and the
    /// remote one — preserving causality across devices.
    pub fn update(&mut self, remote: Hlc, now_ms: i64) -> Hlc {
        let max_wall = now_ms.max(self.wall_ms).max(remote.wall_ms);
        let counter = if max_wall == self.wall_ms && max_wall == remote.wall_ms {
            self.counter.max(remote.counter) + 1
        } else if max_wall == self.wall_ms {
            self.counter + 1
        } else if max_wall == remote.wall_ms {
            remote.counter + 1
        } else {
            0
        };
        self.wall_ms = max_wall;
        self.counter = counter;
        *self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_advances_with_wall_clock() {
        let mut c = Hlc::default();
        let a = c.tick(1000);
        let b = c.tick(1001);
        assert!(b > a);
        assert_eq!(b, Hlc::new(1001, 0));
    }

    #[test]
    fn tick_uses_counter_within_same_ms() {
        let mut c = Hlc::default();
        let a = c.tick(1000);
        let b = c.tick(1000); // same ms
        assert!(b > a);
        assert_eq!(b, Hlc::new(1000, 1));
    }

    #[test]
    fn tick_handles_backwards_clock() {
        let mut c = Hlc::default();
        c.tick(5000);
        let b = c.tick(4000); // clock went backwards
        assert_eq!(b, Hlc::new(5000, 1)); // keeps max wall, bumps counter
    }

    #[test]
    fn update_is_strictly_greater_than_remote() {
        let mut local = Hlc::new(1000, 0);
        let remote = Hlc::new(2000, 5);
        let merged = local.update(remote, 1500);
        assert!(merged > remote);
        assert!(merged > Hlc::new(1000, 0));
    }
}
