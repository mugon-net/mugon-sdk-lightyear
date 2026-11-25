# mugon-sdk-lightyear

[Lightyear](https://github.com/cBournhonesque/lightyear) networking for Bevy games that run on
the [mugon.net](https://mugon.net) platform. The `mugon-sdk-lightyear` crate is a Lightyear transport
on top of [`mugon-sdk-bevy`](https://github.com/mugon-net/mugon-sdk-bevy): the platform decides
whether a process is the **server** (host) or a **client** and carries the traffic between players,
and Lightyear does replication, prediction and interpolation on top.

| `mugon-sdk-lightyear` | Lightyear | Bevy | `mugon-sdk-bevy` |
| --- | --- | --- | --- |
| 0.1 | 0.31 | 0.20 | 0.1 |

## Getting started

The `bevy` template of the [mugon CLI](https://github.com/mugon-net/mugon-cli) sets up a project
for `mugon-sdk-bevy`; add this crate to it:

```toml
[dependencies]
bevy = "0.20"
lightyear = "0.31"
mugon-sdk-lightyear = "0.1"

[target.wasm32-unknown-unknown.dependencies]
getrandom = { version = "0.3", features = ["wasm_js"] }
```

```toml
# .cargo/config.toml
[target.wasm32-unknown-unknown]
rustflags = ["--cfg", "getrandom_backend=\"wasm_js\""]
```

```rust
use bevy::prelude::*;
use mugon_sdk_lightyear::prelude::*;

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, MugonLightyearPlugin))
        .add_systems(Update, on_players)
        .run();
}

fn on_players(mut connected: MessageReader<PlayerConnected>) {
    for player in connected.read() {
        info!("player connected: {:?}", player.peer_id);
    }
}
```

`MugonLightyearPlugin` installs `mugon_sdk_bevy::MugonPlugin`, Lightyear's client and server
plugin groups and the transport, and spawns the topology that matches the platform-assigned role
on the first frame. The wasm has to be started from inside the callback of `Mugon.start`, which
the `bevy` template already does.

## How it works

- **Topology.** One host (the Lightyear server) and any number of clients, matching the
  platform's model. The server gets one link per connected player; a client has a single link to the
  server.
- **No handshake.** The platform has already vetted every player's identity, so the transport pairs
  with `lightyear_raw_connection` instead of Lightyear's connect-token flow.
- **Player identity.** The platform identifies players by string ids. Lightyear wants socket
  addresses, so each id is mapped to a synthetic address, assigned sequentially per session.
- **Channel.** All traffic uses the platform's `UnreliableUnordered` channel. Lightyear brings its
  own reliability layer; stacking it on a reliable channel would only add head-of-line blocking.
- **Frame loop.** Incoming payloads are moved into Lightyear's links in `PreUpdate`, and outgoing
  ones are sent in `PostUpdate`. There is no async runtime.

## Repository layout

- `crates/mugon-sdk-lightyear/`: the transport crate.

## Building

```sh
cargo check --target wasm32-unknown-unknown
```

The repository's `.cargo/config.toml` sets the `getrandom` backend that `wasm32-unknown-unknown`
needs. The `mugon-sdk-bevy` dependency is resolved by path inside the mugon monorepo and from
crates.io once published.

## License

[MIT](LICENSE-MIT)

## Links

- Repository and issues: <https://github.com/mugon-net/mugon-sdk-lightyear>
- Bevy SDK: <https://github.com/mugon-net/mugon-sdk-bevy>
- JavaScript SDK: <https://github.com/mugon-net/mugon-sdk-js>
- CLI: <https://github.com/mugon-net/mugon-cli>
- Platform: <https://mugon.net>
