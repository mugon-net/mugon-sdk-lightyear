//! Client side of the Mugon transport: exactly one [`Link`], to the platform's single server
//! peer named at construction time.

use crate::bridge::Bridge;
use aeronet_io::connection::LocalAddr;
use bevy_app::{App, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::prelude::*;
use core::net::{IpAddr, Ipv4Addr, SocketAddr};
use lightyear_core::time::Instant;
use lightyear_link::{
    recv_payload_from_bytes, Link, LinkReceiveSystems, LinkStart, LinkSystems, Linked, Linking,
    Unlink, UnlinkReason,
};
use lightyear_raw_connection::client::RawClient;

/// A single link entity always uses this synthetic address for its one peer (the server).
/// Safe to hardcode (rather than going through `PeerAddrTable`, which the server side needs to
/// disambiguate *many* peers): a client entity has exactly one peer for its whole lifetime, and
/// a fresh entity is spawned per (re)connection, so there's no cross-entity collision to avoid.
const SERVER_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 1);

/// Client-side Mugon transport component.
///
/// Requires [`RawClient`] (decision #1): `Linked` on this entity implies `Connected` with no
/// separate handshake, since Mugon already vetted this peer before the gameframe saw it.
#[derive(Component)]
#[require(Link, RawClient = RawClient)]
pub struct MugonClientIo {
    server_id: String,
}

impl MugonClientIo {
    /// `server_id` is the platform-assigned id of the (single) server peer, i.e.
    /// `Bridge::server_id()` once settings have arrived.
    pub fn new(server_id: impl Into<String>) -> Self {
        Self {
            server_id: server_id.into(),
        }
    }
}

pub struct MugonClientPlugin;

impl Plugin for MugonClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(link);
        app.add_observer(disconnect_server_on_local_unlink);
        app.add_systems(
            PreUpdate,
            (poll_connect, poll_disconnect, receive)
                .chain()
                .in_set(LinkReceiveSystems::BufferToLink),
        );
        app.add_systems(PostUpdate, send.in_set(LinkSystems::Send));
    }
}

/// Marks the client entity `Linking` (attempting to connect) — confirmed with the platform:
/// the client *does* receive an explicit `"con"` naming its server peer (not just settings +
/// `"rdy"`, which this crate previously assumed was sufficient on its own). `poll_connect`
/// finishes the transition to `Linked` once that `"con"` actually arrives.
fn link(trigger: On<LinkStart>, query: Query<(), With<MugonClientIo>>, mut commands: Commands) {
    if query.get(trigger.entity).is_ok() {
        commands.entity(trigger.entity).insert(Linking);
    }
}

/// Finishes linking once the platform's `"con"` for this client's server peer arrives.
///
/// Uses [`Bridge::take_connect`] rather than [`Bridge::pop_connect`]: a `"con"` for the server
/// can arrive before this entity even exists (the client's settings handshake — and therefore
/// its `MugonClientIo` spawn — isn't guaranteed to happen first), so this must look for its own
/// id specifically rather than draining the queue in FIFO order and risking discarding an
/// unrelated entry while searching.
///
/// Inserts [`LocalAddr`], not [`PeerAddr`](aeronet_io::connection::PeerAddr): despite the name
/// suggesting "this entity's peer", `lightyear_raw_connection::client::RawConnectionPlugin`'s
/// `Linked` → `Connected` observer (`on_linked`) reads `&LocalAddr` to derive a fallback
/// `LocalId` and only ever defaults `RemoteId` to the generic `PeerId::Server` — it never reads
/// `PeerAddr` at all. Inserting `PeerAddr` here (matching `server.rs`'s pattern, which *is*
/// read by the server-side observer) left the query's `&LocalAddr` unsatisfied, so `Connected`
/// was never added and `PeerConnected` never fired for the client.
fn poll_connect(
    bridge: bevy_ecs::system::NonSend<Bridge>,
    query: Query<(Entity, &MugonClientIo), Without<Linked>>,
    mut commands: Commands,
) {
    for (entity, client) in query.iter() {
        if bridge.take_connect(&client.server_id) {
            commands
                .entity(entity)
                .insert((Linked, LocalAddr(SERVER_ADDR)));
        }
    }
}

/// Calls `window.disconnect` for the server peer whenever *this side* unlinks for a reason
/// other than the platform already having reported the server's own disconnect (see the
/// matching comment in `server.rs`).
fn disconnect_server_on_local_unlink(
    trigger: On<Unlink>,
    bridge: bevy_ecs::system::NonSend<Bridge>,
    query: Query<&MugonClientIo>,
) {
    if matches!(trigger.reason, UnlinkReason::ByPeer(_)) {
        return;
    }
    if let Ok(client) = query.get(trigger.entity) {
        bridge.disconnect(&client.server_id);
    }
}

/// Unlinks if the platform reports the server peer disconnected.
fn poll_disconnect(
    bridge: bevy_ecs::system::NonSend<Bridge>,
    query: Query<(Entity, &MugonClientIo)>,
    mut commands: Commands,
) {
    while let Some(peer_id) = bridge.pop_disconnect() {
        for (entity, client) in query.iter() {
            if client.server_id == peer_id {
                commands.trigger(Unlink {
                    entity,
                    reason: UnlinkReason::ByPeer("server disconnected".into()),
                });
            }
        }
    }
}

fn receive(
    bridge: bevy_ecs::system::NonSend<Bridge>,
    mut query: Query<(&mut Link, &MugonClientIo), With<Linked>>,
) {
    let Ok((mut link, client)) = query.single_mut() else {
        return;
    };
    while let Some((from, data)) = bridge.pop_recv() {
        if from == client.server_id {
            link.recv
                .push(recv_payload_from_bytes(data), Instant::now());
        }
    }
}

fn send(
    bridge: bevy_ecs::system::NonSend<Bridge>,
    mut query: Query<(&mut Link, &MugonClientIo), With<Linked>>,
) {
    let Ok((mut link, client)) = query.single_mut() else {
        return;
    };
    for payload in link.send.drain() {
        bridge.send(&client.server_id, payload.as_ref());
    }
}
