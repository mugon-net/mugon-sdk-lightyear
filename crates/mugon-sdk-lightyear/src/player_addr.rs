//! Synthetic `SocketAddr` surrogates for Mugon's string player ids.
//!
//! `lightyear_raw_connection`'s `PeerId::Raw` variant
//! (and `RawConnectionPlugin`'s `Query<(&LinkOf, &PeerAddr)>`) require a `SocketAddr` as the
//! player's identity. Mugon player ids are strings, so this module assigns each one a stable,
//! sequential loopback address for as long as it stays connected, purely to satisfy that
//! type — no real socket exists.
//!
//! Sequential, not hashed: addresses stay meaningful in logs (`127.0.0.1:1`, `:2`, ... in
//! connection order) and there's no collision case to reason about.

use core::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::collections::HashMap;

/// Bidirectional mapping between Mugon player ids and their synthetic `SocketAddr`.
#[derive(Default)]
pub(crate) struct PlayerAddrTable {
    next_port: u16,
    by_id: HashMap<String, SocketAddr>,
    by_addr: HashMap<SocketAddr, String>,
}

impl PlayerAddrTable {
    /// Assigns a fresh synthetic address for `player_id` if it doesn't have one yet, and returns
    /// it either way (idempotent — a duplicate `"player-connected"` for an already-known player is not an
    /// error, see `server.rs`).
    pub(crate) fn assign(&mut self, player_id: &str) -> SocketAddr {
        if let Some(addr) = self.by_id.get(player_id) {
            return *addr;
        }
        self.next_port = self.next_port.wrapping_add(1).max(1);
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), self.next_port);
        self.by_id.insert(player_id.to_string(), addr);
        self.by_addr.insert(addr, player_id.to_string());
        addr
    }

    /// Removes and returns the address for `player_id`, if it was known.
    pub(crate) fn remove_by_id(&mut self, player_id: &str) -> Option<SocketAddr> {
        let addr = self.by_id.remove(player_id)?;
        self.by_addr.remove(&addr);
        Some(addr)
    }

    /// Looks up the player id for a synthetic address, e.g. to route an outgoing payload back
    /// to `Bridge::send`.
    pub(crate) fn player_id(&self, addr: SocketAddr) -> Option<&str> {
        self.by_addr.get(&addr).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assign_is_sequential_and_idempotent() {
        let mut table = PlayerAddrTable::default();
        let a1 = table.assign("alice");
        let a2 = table.assign("bob");
        assert_ne!(a1, a2);
        assert_eq!(table.assign("alice"), a1, "re-assigning is idempotent");
        assert_eq!(table.player_id(a1), Some("alice"));
        assert_eq!(table.player_id(a2), Some("bob"));
    }

    #[test]
    fn remove_clears_both_directions() {
        let mut table = PlayerAddrTable::default();
        let addr = table.assign("alice");
        assert_eq!(table.remove_by_id("alice"), Some(addr));
        assert_eq!(table.player_id(addr), None);
        assert_eq!(table.remove_by_id("alice"), None, "already removed");
    }
}
