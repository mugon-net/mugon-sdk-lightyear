//! Lightyear transport for games running on the [Mugon](https://mugon.net) platform.
//!
//! [`MugonLightyearPlugin`] wraps Lightyear's `ClientPlugins`/`ServerPlugins` together
//! with this crate's own transport (built on `mugon_sdk_bevy::MugonPlugin`'s JS bridge to
//! `mugon-sdk-js` — see that crate for the bridge's own design), reads this process's role
//! (`"server"` or `"client"`, from the `MugonContext` the platform hands the game — settled at
//! runtime, not compile time, since a single wasm build can run as either), spawns the matching
//! topology, and surfaces player connect/disconnect as plain Bevy events
//! ([`PlayerConnected`]/[`PlayerDisconnected`]) so game code doesn't need to know anything about
//! Lightyear's connection-lifecycle components to spawn/despawn per-player entities:
//!
//! - **Server**: one [`server::MugonServerEndpoint`] that fans out to one
//!   [`lightyear_link::server::LinkOf`] child per connected client player.
//! - **Client**: one [`client::MugonClientIo`], the single [`lightyear_link::Link`] to the
//!   platform's one server player.
//!
//! Both pair with [`lightyear_raw_connection`] (Mugon already vets player identity
//! before the gameframe sees it, so no connect-token handshake is needed on top), added
//! automatically by [`MugonLightyearPlugin`].
//!
//! Both the client and the server transport are always installed: the platform decides the role
//! at runtime, and a single wasm build can run as either.

pub mod client;
mod player_addr;
pub mod server;

use bevy_app::{App, Plugin, PreUpdate};
use bevy_ecs::message::MessageWriter;
use bevy_ecs::prelude::*;
use core::time::Duration;
use lightyear::prelude::*;
use lightyear_core::time::Instant;
use lightyear_link::{LinkPlugin, LinkStart, Linked, Unlinked};

pub use mugon_sdk_bevy::{Bridge, NetworkRole, Player};
pub use server::MugonPlayer;

pub mod prelude {
    pub use crate::client::{MugonClientIo, MugonLightyearClientPlugin};
    pub use crate::server::{
        MugonLightyearServerPlugin, MugonLinkOfIO, MugonPlayer, MugonServerEndpoint,
    };
    pub use crate::{
        Bridge, MugonLightyearPlugin, NetworkRole, Player, PlayerConnected, PlayerDisconnected,
    };
}

/// Fired when a player's Lightyear connection entity gets [`Connected`].
///
/// On the server this fires once per connected client (the `LinkOf` child entity); on the
/// client it fires once, for its single connection to the server. `peer_id` is Lightyear's own
/// identity for the link (a [`PeerId::Raw`] wrapping the synthetic `SocketAddr` `player_addr`
/// assigns per platform player id) — sufficient for `ControlledBy`/`NetworkTarget::Single` and
/// friends; the platform's own string player id isn't exposed outside this crate today
/// (`player_addr` is private), which is fine since gameplay code only needs Lightyear-level
/// identity.
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerConnected {
    pub entity: Entity,
    pub peer_id: PeerId,
}

/// Fired when a player's Lightyear connection entity gets [`Disconnected`]. See
/// [`PlayerConnected`] for what `peer_id` is.
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerDisconnected {
    pub entity: Entity,
    pub peer_id: PeerId,
}

/// Installs the Mugon transport: the JS bridge (via `mugon_sdk_bevy::MugonPlugin`), Lightyear's
/// facade plugin groups, both client and server transport systems, the paired
/// [`lightyear_raw_connection`] plugins, and the auto-spawn system that starts the right
/// topology once the platform reports this process's role.
///
/// Both client and server transport systems are always installed (regardless of which role
/// this process turns out to have) because Mugon decides the role at runtime, not at compile
/// time — see the module docs. This costs a little unused code in the other role's binary.
///
/// If the app also needs `mugon_sdk_bevy::MugonPlugin` directly (e.g. for `touch-controls`),
/// add it *before* this plugin, or not at all — adding it afterwards panics (duplicate plugin).
pub struct MugonLightyearPlugin;

impl Plugin for MugonLightyearPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<mugon_sdk_bevy::MugonPlugin>() {
            app.add_plugins(mugon_sdk_bevy::MugonPlugin);
        }

        // Facade plugin groups first, this crate's own transport plugins second -- confirmed
        // live (Milestone 1 checkpoint) that the other order panics.
        // `server::ServerPlugins`'s `PluginGroupBuilder` adds
        // `lightyear_link::server::ServerLinkPlugin` unconditionally, with no
        // `is_plugin_added` guard of its own (that check only happens once, when a plugin
        // *group* is finished and applied to the `App`). `MugonLightyearServerPlugin` also wants
        // `ServerLinkPlugin`, but guards its own add with `is_plugin_added`. Whichever side runs
        // first "wins" the unguarded slot; the second side's guard (if it has one) then
        // correctly no-ops. So `ServerPlugins`/`ClientPlugins` must go first, this crate's
        // transport plugins second -- the reverse order panics at startup with "plugin was
        // already added in application". Do not additionally enable the `lightyear` facade
        // crate's `raw_connection`/`netcode` cargo features in this crate's Cargo.toml -- either
        // one would make `ClientPlugins`/`ServerPlugins` try to add `RawConnectionPlugin` a
        // second time (this crate adds it unconditionally below, with no guard at all), hitting
        // the same failure mode.
        app.add_plugins((
            lightyear::prelude::client::ClientPlugins::default(),
            lightyear::prelude::server::ServerPlugins::default(),
        ));

        if !app.is_plugin_added::<LinkPlugin>() {
            app.add_plugins(LinkPlugin);
        }
        app.add_plugins((
            client::MugonLightyearClientPlugin,
            server::MugonLightyearServerPlugin,
        ));
        app.add_plugins((
            lightyear_raw_connection::client::RawConnectionPlugin,
            lightyear_raw_connection::server::RawConnectionPlugin,
        ));

        // `lightyear_prediction::RollbackPlugin` (always installed by `ClientPlugins`'s default
        // `prediction` feature) schedules `reset_input_rollback_tracker` unconditionally in
        // `PreUpdate`, and that system takes a hard `ResMut<LastConfirmedInput>` -- but
        // `LastConfirmedInput` is only initialized when a `PredictionManager` resource is
        // inserted (`PredictionPlugin`'s `On<Insert, PredictionManager>` observer), which is
        // opt-in and no Mugon game had opted into yet. Without it, that system panics with
        // "Resource does not exist" on every tick, even with zero predicted entities. The actual
        // prediction/rollback pipeline still stays dormant per-topology (`should_run` requires
        // `NetworkTopology::Client`/`P2P`) and per-component (`PredictionTarget`), so inserting
        // this unconditionally on both roles is safe -- it only prevents this crash.
        app.insert_resource(PredictionManager::default());

        app.add_message::<PlayerConnected>();
        app.add_message::<PlayerDisconnected>();
        app.add_observer(on_connected);
        app.add_observer(on_disconnected);

        app.add_systems(PreUpdate, auto_spawn_role);
        app.add_observer(mark_stale_on_unlinked);
        app.add_systems(PreUpdate, despawn_stale_links);
    }
}

/// Opts every connected link into replication and emits [`PlayerConnected`].
///
/// Neither `ReplicationSender` (server-side, per connected client link) nor
/// `ReplicationReceiver` (the client's own connection entity) are inserted automatically by
/// Lightyear's replicon backend -- a link that lacks them stays connected (messages still flow)
/// but never replicates entities. Every Mugon game wants replication on for every player, so this
/// is handled here once rather than in each game.
fn on_connected(
    trigger: On<Add, Connected>,
    remote_ids: Query<&RemoteId>,
    is_client_of: Query<(), With<lightyear::prelude::server::ClientOf>>,
    is_client: Query<(), With<Client>>,
    mut events: MessageWriter<PlayerConnected>,
    mut commands: Commands,
) {
    if is_client_of.get(trigger.entity).is_ok() {
        commands.entity(trigger.entity).insert(ReplicationSender);
    }
    if is_client.get(trigger.entity).is_ok() {
        commands.entity(trigger.entity).insert(ReplicationReceiver);
    }
    if let Ok(remote_id) = remote_ids.get(trigger.entity) {
        events.write(PlayerConnected {
            entity: trigger.entity,
            peer_id: remote_id.0,
        });
    }
}

fn on_disconnected(
    trigger: On<Add, Disconnected>,
    remote_ids: Query<&RemoteId>,
    mut events: MessageWriter<PlayerDisconnected>,
) {
    if let Ok(remote_id) = remote_ids.get(trigger.entity) {
        events.write(PlayerDisconnected {
            entity: trigger.entity,
            peer_id: remote_id.0,
        });
    }
}

/// How long a Mugon-owned link entity is given, after going [`Unlinked`], before this crate
/// despawns it itself.
///
/// This is a deliberate safety net, not something verified to be *necessary*: every other
/// Lightyear transport crate (`lightyear_crossbeam`, `lightyear_udp`, ...) leaves despawning a
/// disconnected player entirely to `lightyear_connection`'s own reaction to `Unlinked`/
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
/// If the entity got re-`Linked` before then (e.g. the same Mugon player reconnecting quickly —
/// recall player ids are never reused for a *different* player, so a fresh
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

/// Spawns the topology matching this process's role exactly once, on the first frame.
///
/// Spawning automatically, rather than making the app read [`Bridge::network_role`] itself and
/// spawn manually, keeps the role-dependent setup out of the game. An app that wants to control
/// the topology itself can read the role from the bridge instead.
fn auto_spawn_role(
    bridge: bevy_ecs::system::NonSend<Bridge>,
    mut spawned: Local<bool>,
    mut commands: Commands,
) {
    if *spawned {
        return;
    }
    match bridge.network_role() {
        NetworkRole::Server => {
            let entity = commands.spawn(server::MugonServerEndpoint::new()).id();
            commands.trigger(LinkStart { entity });
        }
        NetworkRole::Client => {
            let entity = commands
                .spawn(client::MugonClientIo::new(bridge.server_player().id.clone()))
                .id();
            commands.trigger(LinkStart { entity });
        }
    }
    *spawned = true;
}
