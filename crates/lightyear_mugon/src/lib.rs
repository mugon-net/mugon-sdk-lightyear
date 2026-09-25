//! Lightyear transport for games running on the [Mugon](https://mugon.net) platform.
//!
//! See the repo README for the full design rationale (why this replaces
//! `mugon-net/lightyear`'s fork, the decisions behind each piece here, and what's still
//! open). In short: [`MugonPlugin`] registers this process's one-time JS bridge (see
//! [`bridge`]), watches for the platform to report this process's role (`"server"` or
//! `"client"`, via `mugon.networkmode` — settled at runtime, not compile time, since a single
//! wasm build can run as either), and spawns the matching topology:
//!
//! - **Server**: one [`server::MugonServerEndpoint`] that fans out to one
//!   [`lightyear_link::server::LinkOf`] child per connected client peer.
//! - **Client**: one [`client::MugonClientIo`], the single [`lightyear_link::Link`] to the
//!   platform's one server peer.
//!
//! Both pair with [`lightyear_raw_connection`] (decision #1 — Mugon already vets peer identity
//! before the gameframe sees it, so no connect-token handshake is needed on top), added
//! automatically by [`MugonPlugin`].

mod bindings;
mod bridge;
pub mod client;
mod peer_addr;
pub mod server;

use bevy_app::{App, Plugin, PreUpdate};
use bevy_ecs::prelude::*;
use core::time::Duration;
use lightyear_core::time::Instant;
use lightyear_link::{LinkPlugin, LinkStart, Linked, Unlinked};
use tracing::error;

pub use bridge::{Bridge, NetworkMode};

pub mod prelude {
    pub use crate::client::{MugonClientIo, MugonClientPlugin};
    pub use crate::server::{MugonLinkOfIO, MugonServerEndpoint, MugonServerPlugin};
    pub use crate::{Bridge, MugonPlugin, NetworkMode};
}

/// Installs the Mugon transport: the JS bridge, both client and server transport systems, the
/// paired [`lightyear_raw_connection`] plugins, and the auto-spawn system that starts the
/// right topology once the platform reports this process's role.
///
/// Both client and server transport systems are always installed (regardless of which role
/// this process turns out to have) because Mugon decides the role at runtime, not at compile
/// time — see the module docs. This costs a little unused code in the other role's binary; see
/// `QUESTIONS.md` for whether that should become a compile-time choice instead.
pub struct MugonPlugin;

impl Plugin for MugonPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<LinkPlugin>() {
            app.add_plugins(LinkPlugin);
        }
        app.add_plugins((client::MugonClientPlugin, server::MugonServerPlugin));
        app.add_plugins((
            lightyear_raw_connection::client::RawConnectionPlugin,
            lightyear_raw_connection::server::RawConnectionPlugin,
        ));

        let bridge = Bridge::install();
        app.insert_non_send(bridge);

        app.add_systems(PreUpdate, auto_spawn_role);
        app.add_observer(mark_stale_on_unlinked);
        app.add_systems(PreUpdate, despawn_stale_links);
    }
}

/// How long a Mugon-owned link entity is given, after going [`Unlinked`], before this crate
/// despawns it itself.
///
/// This is a deliberate safety net, not something verified to be *necessary*: every other
/// Lightyear transport crate (`lightyear_crossbeam`, `lightyear_udp`, ...) leaves despawning a
/// disconnected peer entirely to `lightyear_connection`'s own reaction to `Unlinked`/
/// `Disconnected`, and this crate couldn't confirm end-to-end (no running example) that that
/// reaction actually despawns things here too. The grace period exists so this safety net can
/// never race that normal cleanup — see `mark_stale_on_unlinked`/`despawn_stale_links`.
const STALE_LINK_GRACE: Duration = Duration::from_secs(1);

/// Marks when a Mugon-owned link entity became [`Unlinked`], starting the grace period after
/// which [`despawn_stale_links`] removes it if nothing else already did.
#[derive(Component)]
struct StaleLink(Instant);

/// Matches a Mugon-owned link entity (a `MugonClientIo`, or a `MugonLinkOfIO` server child)
/// that hasn't already started its [`StaleLink`] grace period.
type MugonOwnedLinkNotYetStale = (
    Or<(With<client::MugonClientIo>, With<server::MugonLinkOfIO>)>,
    Without<StaleLink>,
);

/// Starts the grace period for any Mugon-owned link entity that just went `Unlinked`.
fn mark_stale_on_unlinked(
    trigger: On<Add, Unlinked>,
    query: Query<Entity, MugonOwnedLinkNotYetStale>,
    mut commands: Commands,
) {
    if query.get(trigger.entity).is_ok() {
        commands
            .entity(trigger.entity)
            .insert(StaleLink(Instant::now()));
    }
}

/// Despawns any [`StaleLink`] entity whose grace period has elapsed.
///
/// If the entity got re-`Linked` before then (e.g. the same Mugon peer reconnecting quickly —
/// recall peer ids are never reused for a *different* peer, per `QUESTIONS.md` #2, so a fresh
/// `Linked` on this exact entity within the grace period is a legitimate fast reconnect, not a
/// stale leftover), the [`StaleLink`] marker is simply removed instead.
fn despawn_stale_links(
    query: Query<(Entity, &StaleLink, Option<&Linked>)>,
    mut commands: Commands,
) {
    let now = Instant::now();
    for (entity, stale, linked) in query.iter() {
        if linked.is_some() {
            commands.entity(entity).remove::<StaleLink>();
            continue;
        }
        if now - stale.0 >= STALE_LINK_GRACE {
            commands.entity(entity).despawn();
        }
    }
}

/// Waits for the platform's settings handshake, then spawns the matching topology exactly
/// once.
///
/// This is a deliberate ergonomics choice (spawn automatically rather than making the host
/// app poll [`Bridge::network_mode`] itself and spawn manually) — flagged in `QUESTIONS.md` as
/// worth confirming, since it's the kind of API-shape decision that's easy to get attached to
/// before the person actually using it has seen the alternative.
fn auto_spawn_role(
    bridge: bevy_ecs::system::NonSend<Bridge>,
    mut spawned: Local<bool>,
    mut commands: Commands,
) {
    if *spawned || !bridge.settings_ready() {
        return;
    }
    match bridge.network_mode() {
        Some(NetworkMode::Server) => {
            let entity = commands.spawn(server::MugonServerEndpoint::new()).id();
            commands.trigger(LinkStart { entity });
        }
        Some(NetworkMode::Client) => {
            let Some(server_id) = bridge.server_id() else {
                // Per protocol.rs / parentframe.ts this should never happen once settings_ready
                // is true for a client (settings always carry mugon.serverid in that case) --
                // wait another frame rather than treating it as fatal.
                return;
            };
            let entity = commands.spawn(client::MugonClientIo::new(server_id)).id();
            commands.trigger(LinkStart { entity });
        }
        None => {
            error!(
                "Mugon settings arrived but mugon.networkmode was missing or unrecognized; \
                 cannot start the transport"
            );
            return;
        }
    }
    *spawned = true;
}
