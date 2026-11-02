//! Server (host) side of the Mugon transport: one [`MugonServerEndpoint`] fanning out to one
//! [`LinkOf`] child per connected client player, mirroring `lightyear_udp`'s `UdpEndpoint`.

use crate::player_addr::PlayerAddrTable;
use aeronet_io::connection::PeerAddr;
use bevy_app::{App, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::prelude::*;
use lightyear_core::time::Instant;
use lightyear_link::server::{LinkOf, Server, ServerLinkPlugin};
use lightyear_link::{
    recv_payload_from_bytes, Link, LinkReceiveSystems, LinkStart, LinkSystems, Linked, Unlink,
    UnlinkReason,
};
use lightyear_raw_connection::server::RawServer;
use mugon_sdk_bevy::{Bridge, Channel, Player};
use std::collections::HashMap;
use tracing::debug;

/// Server-side Mugon transport component. Insert on the entity that should act as the host,
/// spawn it, and trigger [`LinkStart`] on it (see `MugonLightyearPlugin` for the auto-spawn path
/// this crate uses in the common case).
///
/// Requires [`RawServer`]: a `LinkOf` child reaching `Linked` implies `Connected`
/// with no separate handshake.
///
/// Note: `lightyear_link` 0.30.1 (the published version this crate depends on)
/// names this relationship-target component `Server`, with child links pointing back via
/// `LinkOf { server: Entity }`. Unreleased upstream `main` has since renamed this to
/// `Endpoint`/`LinkOf { endpoint: Entity }` (transport-neutral naming, since a P2P player can also
/// own child links without being a "server"). This crate's naming follows once that lands in a
/// release.
#[derive(Component, Default)]
#[require(Server, RawServer = RawServer)]
pub struct MugonServerEndpoint {
    addrs: PlayerAddrTable,
    entities: HashMap<String, Entity>,
}

impl MugonServerEndpoint {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Marker for child link entities owned by a [`MugonServerEndpoint`], distinguishing this
/// transport's [`LinkOf`] children if another transport is ever attached to the same endpoint.
#[derive(Component)]
pub struct MugonLinkOfIO;

/// The platform player behind a [`MugonLinkOfIO`] link entity, with the display name the
/// platform gave it. Read it from the entity of a [`PlayerConnected`](crate::PlayerConnected)
/// event to learn who just joined.
#[derive(Component, Clone, Debug)]
pub struct MugonPlayer(pub Player);

pub struct MugonLightyearServerPlugin;

impl Plugin for MugonLightyearServerPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<ServerLinkPlugin>() {
            app.add_plugins(ServerLinkPlugin);
        }
        app.add_observer(link);
        app.add_observer(disconnect_player_on_local_unlink);
        app.add_systems(
            PreUpdate,
            (spawn_and_despawn_players, receive)
                .chain()
                .in_set(LinkReceiveSystems::BufferToLink),
        );
        app.add_systems(PostUpdate, send.in_set(LinkSystems::Send));
    }
}

fn link(
    trigger: On<LinkStart>,
    query: Query<(), With<MugonServerEndpoint>>,
    mut commands: Commands,
) {
    if query.get(trigger.entity).is_ok() {
        commands.entity(trigger.entity).insert(Linked);
    }
}

/// Spawns a [`LinkOf`] child for each newly-connected player and unlinks the child for each
/// disconnected one.
///
/// Assumes exactly one [`MugonServerEndpoint`] per process, matching Mugon's star topology
/// (one host, many clients).
///
/// A repeated `"player-connected"` for a player id we already have an entity for is treated as idempotent
/// (existing entity is simply marked `Linked` again), not a duplicate spawn — this crate
/// doesn't assume the platform never re-announces a still-connected player.
fn spawn_and_despawn_players(
    bridge: bevy_ecs::system::NonSend<Bridge>,
    mut endpoints: Query<(Entity, &mut MugonServerEndpoint), With<Linked>>,
    mut commands: Commands,
) {
    let Ok((endpoint_entity, mut endpoint)) = endpoints.single_mut() else {
        return;
    };

    while let Some(player) = bridge.pop_connect() {
        let player_id = player.id.clone();
        if let Some(&entity) = endpoint.entities.get(&player_id) {
            debug!(%player_id, "duplicate Mugon \"player-connected\" for an already-tracked player; re-linking");
            commands.entity(entity).insert(Linked);
            continue;
        }
        let addr = endpoint.addrs.assign(&player_id);
        let entity = commands
            .spawn((
                LinkOf {
                    server: endpoint_entity,
                },
                Link::default(),
                Linked,
                PeerAddr(addr),
                MugonLinkOfIO,
                MugonPlayer(player),
            ))
            .id();
        endpoint.entities.insert(player_id, entity);
    }

    while let Some(player_id) = bridge.pop_disconnect() {
        endpoint.addrs.remove_by_id(&player_id);
        if let Some(entity) = endpoint.entities.remove(&player_id) {
            commands.trigger(Unlink {
                entity,
                reason: UnlinkReason::ByPeer("player disconnected".into()),
            });
        }
    }
}

/// Calls `window.disconnect` for a player whenever *this side* unlinks the connection for a
/// reason other than the player already having reported its own disconnect (`UnlinkReason::
/// ByPeer`, which `spawn_and_despawn_players` sets from Mugon's `"player-disconnected"` event) — e.g. the
/// connection layer or the app closing the link locally. Avoids redundantly telling the
/// platform to drop a player that already told us it's gone.
fn disconnect_player_on_local_unlink(
    trigger: On<Unlink>,
    bridge: bevy_ecs::system::NonSend<Bridge>,
    links: Query<&PeerAddr, With<MugonLinkOfIO>>,
    endpoint: Query<&MugonServerEndpoint>,
) {
    if matches!(trigger.reason, UnlinkReason::ByPeer(_)) {
        return;
    }
    let Ok(endpoint) = endpoint.single() else {
        return;
    };
    let Ok(addr) = links.get(trigger.entity) else {
        return;
    };
    if let Some(player_id) = endpoint.addrs.player_id(addr.0) {
        bridge.disconnect_player(player_id);
    }
}

fn receive(
    bridge: bevy_ecs::system::NonSend<Bridge>,
    endpoint: Query<&MugonServerEndpoint>,
    mut links: Query<&mut Link, With<MugonLinkOfIO>>,
) {
    let Ok(endpoint) = endpoint.single() else {
        return;
    };
    while let Some((from, data)) = bridge.pop_recv() {
        let Some(&entity) = endpoint.entities.get(&from) else {
            debug!(from_player = %from, "data from an unknown Mugon player; dropping");
            continue;
        };
        if let Ok(mut link) = links.get_mut(entity) {
            link.recv
                .push(recv_payload_from_bytes(data), Instant::now());
        }
    }
}

fn send(
    bridge: bevy_ecs::system::NonSend<Bridge>,
    mut links: Query<(&mut Link, &PeerAddr), With<MugonLinkOfIO>>,
    endpoint: Query<&MugonServerEndpoint>,
) {
    let Ok(endpoint) = endpoint.single() else {
        return;
    };
    for (mut link, addr) in links.iter_mut() {
        let Some(player_id) = endpoint.addrs.player_id(addr.0) else {
            continue;
        };
        for payload in link.send.drain() {
            bridge.send(player_id, payload.as_ref(), Channel::UnreliableUnordered);
        }
    }
}
