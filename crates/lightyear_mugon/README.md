# lightyear_mugon

A [Lightyear](https://github.com/cBournhonesque/lightyear) transport for Bevy games running on
the [mugon.net](https://mugon.net) platform (built on
[`mugon-sdk-bevy`](https://crates.io/crates/mugon-sdk-bevy)).

The platform decides at runtime whether a process is the **server** (host) or a **client**, and
carries all traffic between peers. Peer identity is already vetted by the platform, so this
crate pairs the transport with `lightyear_raw_connection` (no connect-token handshake).

## Usage

```rust
use bevy::prelude::*;
use lightyear_mugon::prelude::*;

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, MugonLightyearPlugin))
        .add_systems(Update, on_peers)
        .run();
}

fn on_peers(mut connected: MessageReader<PeerConnected>) {
    for peer in connected.read() {
        info!("peer connected: {:?}", peer.peer_id);
    }
}
```

`MugonLightyearPlugin` installs `mugon_sdk_bevy::MugonPlugin`, Lightyear's client and server
plugin groups, and this crate's transport, and spawns the topology matching the platform-assigned
role on the first frame. The wasm has to be started from inside the callback of `Mugon.start`
(see `mugon-sdk-bevy`).

All traffic uses the platform's `UnreliableUnordered` channel: Lightyear brings its own
reliability layer, so layering it over a reliable channel would only add head-of-line blocking.

## Status

Compiles against Lightyear 0.30.1 for `wasm32-unknown-unknown`. It has not yet been run
end-to-end in a browser; see `QUESTIONS.md` in the repository for what is and isn't verified.

## License

Licensed under either of MIT or Apache-2.0, at your option.
