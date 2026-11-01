//! Client side of the Mugon transport: exactly one [`Link`], to the platform's single server
//! player named at construction time.

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
use mugon_sdk_bevy::{Bridge, Channel};

/// A single link entity always uses this synthetic address for its one player (the server).
/// Safe to hardcode (rather than going through `PlayerAddrTable`, which the server side needs to
/// disambiguate *many* players): a client entity has exactly one player for its whole lifetime, and
/// a fresh entity is spawned per (re)connection, so there's no cross-entity collision to avoid.
const SERVER_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 1);

/// Client-side Mugon transport component.
///
/// Requires [`RawClient`]: `Linked` on this entity implies `Connected` with no
/// separate handshake, since Mugon already vetted this player before the gameframe saw it.
#[derive(Component)]
#[require(Link, RawClient = RawClient)]
pub struct MugonClientIo {
    server_player_id: String,
}

impl MugonClientIo {
    /// `server_player_id` is the platform-assigned id of the (single) server player, i.e.
    /// `Bridge::server_player().id`.
    pub fn new(server_player_id: impl Into<String>) -> Self {
        Self {
            server_player_id: server_player_id.into(),
        }
    }
}

pub struct MugonLightyearClientPlugin;

impl Plugin for MugonLightyearClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(link);
        app.add_observer(disconnect_server_on_local_unlink);
        app.add_systems(
            PreUpdate,
            (complete_link, poll_disconnect, receive)
                .chain()
                .in_set(LinkReceiveSystems::BufferToLink),
        );
        app.add_systems(PostUpdate, send.in_set(LinkSystems::Send));
    }
}

/// Marks the client entity `Linking` (attempting to connect). The platform only hands the game
/// its context once the network is up, and a client's `ctx.initialPlayers` always contains its server
/// (`mugon-sdk-js`'s `Parentframe` guarantees it), so `complete_link` can finish the
/// transition to `Linked` on the next frame without waiting for anything.
fn link(trigger: On<LinkStart>, query: Query<(), With<MugonClientIo>>, mut commands: Commands) {
    if query.get(trigger.entity).is_ok() {
        commands.entity(trigger.entity).insert(Linking);
    }
}

type LinkingMugonClient = (With<MugonClientIo>, With<Linking>, Without<Linked>);

/// Finishes linking a client entity that is `Linking`.
///
/// Inserts [`LocalAddr`], not [`PeerAddr`](aeronet_io::connection::PeerAddr): despite the name
/// suggesting "this entity's peer", `lightyear_raw_connection::client::RawConnectionPlugin`'s
/// `Linked` → `Connected` observer (`on_linked`) reads `&LocalAddr` to derive a fallback
/// `LocalId` and only ever defaults `RemoteId` to the generic `PeerId::Server` — it never reads
/// `PeerAddr` at all. Inserting `PeerAddr` here (matching `server.rs`'s pattern, which *is*
/// read by the server-side observer) left the query's `&LocalAddr` unsatisfied, so `Connected`
/// was never added and `PlayerConnected` never fired for the client.
fn complete_link(
    query: Query<Entity, LinkingMugonClient>,
    mut commands: Commands,
) {
    for entity in query.iter() {
        commands
            .entity(entity)
            .insert((Linked, LocalAddr(SERVER_ADDR)));
    }
}

/// Calls `window.disconnect` for the server player whenever *this side* unlinks for a reason
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
        bridge.disconnect_player(&client.server_player_id);
    }
}

/// Unlinks if the platform reports the server player disconnected.
fn poll_disconnect(
    bridge: bevy_ecs::system::NonSend<Bridge>,
    query: Query<(Entity, &MugonClientIo)>,
    mut commands: Commands,
) {
    while let Some(player_id) = bridge.pop_disconnect() {
        for (entity, client) in query.iter() {
            if client.server_player_id == player_id {
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
        if from == client.server_player_id {
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
        bridge.send(
            &client.server_player_id,
            payload.as_ref(),
            Channel::UnreliableUnordered,
        );
    }
}
