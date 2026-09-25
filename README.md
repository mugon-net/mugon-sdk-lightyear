# lightyear-mugon-transport

Plan and implementation for a Lightyear transport that lets Bevy/WASM
games run on the [Mugon](https://mugon.net) platform, targeting current/upstream
Lightyear instead of the private fork this project replaces.

## Implementation status

A first prototype of `crates/lightyear_mugon` exists and type-checks cleanly against the real
published `lightyear_link`/`lightyear_connection`/`lightyear_raw_connection` 0.30.1 APIs for
`wasm32-unknown-unknown`. It has **not** been run against a live Mugon parentframe, in a
browser, or otherwise end-to-end — see **What's actually been verified** in `QUESTIONS.md` for
the precise line between "compiles" and "works," and the rest of that file for every judgment
call made along the way that's worth your review. See **Building** below for the local
toolchain setup this needed on Windows.

## Background

[`mugon-net/lightyear`](https://github.com/mugon-net/lightyear) is a fork of
[`cBournhonesque/lightyear`](https://github.com/cBournhonesque/lightyear) pinned at
**v0.19.0**. It exists purely to add a custom `mugon` transport
(`lightyear/src/transport/mugon/{mod,common,client,server}.rs`, ~730 lines including
wiring). A fork was required because, at 0.19, transports were internal
`enum_dispatch` variants (`ClientTransportEnum`, `ServerTransportEnum`) defined inside
the `lightyear` crate itself — adding one meant patching core files
(`client/io/{config,transport}.rs`, `server/io/{config,transport}.rs`,
`transport/mod.rs`, plus two `netcode` files for an unrelated WASM `SystemTime` panic
that upstream has since fixed with the `web_time` crate).

Upstream has since gone through a major workspace rework (now at **v0.30.1**,
published on crates.io): the monolithic `lightyear` crate split into ~30 crates under
`crates/`, and transports are no longer special — they are ordinary external Bevy
plugins built against a small, stable, transport-agnostic core
(`lightyear_link`, `lightyear_connection`, etc.). **No fork is needed for this
project anymore.** This repo is where the replacement gets designed and built.

Relevant commits/branches at time of writing (2026-09-17):
- Fork: `mugon-net/lightyear@0.19.0` branch, HEAD `3c938ceb` (the mugon transport
  itself lands in commit range starting from `de6ad570`, the pristine 0.19.0 tag it
  diverged from). Note: `0.19.0-mugon` is a **stale ancestor** branch — use `0.19.0`.
- Upstream: `cBournhonesque/lightyear@main`, HEAD `1759ae9` at investigation time,
  workspace version `0.30.0`; `lightyear_link` published as `0.30.1` on crates.io.

## What changed upstream that matters here

The new core abstraction is `lightyear_link::Link`: a plain ECS component with a
`recv`/`send` byte-payload queue (`VecDeque<Bytes>`), an `MTU`, and lifecycle marker
components (`Linking` / `Linked` / `Unlinked`). A transport plugin's whole job is:

- On `LinkStart` (an entity event): stand up whatever the transport needs, then insert
  `Linked`.
- Every frame, in `LinkReceiveSystems::BufferToLink` (`PreUpdate`): move bytes from the
  real backend into `Link::recv`.
- Every frame, in `LinkSystems::Send` (`PostUpdate`): drain `Link::send` and hand bytes
  to the real backend.
- On `Unlink`: tear the backend down.

For a transport with many simultaneous peers (our case — one host, many clients), the
`lightyear_link::endpoint` module adds `Endpoint` (a relationship-target component
owned by the listening/hosting entity) and `LinkOf { endpoint }` (put on each per-peer
child entity, alongside its own `Link`). New peers are handled by spawning a new
`LinkOf` + `Link` child entity; `EndpointLinkPlugin` handles conditioner inheritance and
despawning all children when the endpoint unlinks.

This means the whole thing is just a Bevy plugin crate with no special build-time
integration. Confirmed by reading two shipped reference transports:
- `crates/io/crossbeam` — simplest possible transport (in-process channels), single
  peer, no fan-out. Good template for the send/receive system shape.
- `crates/io/udp` (`lib.rs` + `endpoint.rs`) — closest structural analogue to Mugon:
  `UdpEndpoint` owns the "socket", keeps a `HashMap<PeerKey, LinkOfStatus>`, and spawns
  a `LinkOf` child the first time it sees a new peer. Good template for the
  server/endpoint shape.

Built-in transports that need JS interop (`websocket`, `webtransport`) go through an
external crate ecosystem called **Aeronet** rather than hand-written `wasm-bindgen`
externs, so there is no in-tree example of a raw JS-bridge transport to copy from
directly — Mugon's transport will hand-write its `wasm-bindgen` bindings the way the
old fork did, just targeting `Link`/`Endpoint` instead of the old
`Transport`/`PacketSender`/`PacketReceiver` traits.

One important simplification the new model gives us for free: **the old fork's async
machinery (tokio `mpsc`/`oneshot` channels, `tokio::select!`, `wasm_bindgen_futures::
spawn_local` polling loops, `yield_to_browser`/`YIELD_DELAY`) goes away entirely.**
Bevy's own per-frame schedule (`PreUpdate`/`PostUpdate`) *is* the polling loop; a JS
callback just needs to push into a plain queue (e.g. `Rc<RefCell<VecDeque<_>>>` — WASM
is single-threaded so no `Arc<Mutex<_>>`/channel is needed), and the `Send` system can
call the JS send function directly and synchronously.

## The Mugon JS bridge: old fork vs. current SDK

The old fork's Rust code assumed three `window`-scoped externs it called directly:
`registerCallbacks(own_id: u64, onConnect, onMessage, onDisconnected)`,
`sendFromMugonSocket(to_id: u64, bytes) -> bool`, `closeMugonSocket(id: u64)`. Peer
identity was a `u64` that client/server code round-tripped through a fake IPv6
`SocketAddr` (`socket_addr_to_id`/`id_to_socket_addr` in `common.rs`) purely to reuse
`SocketAddr`-shaped lightyear APIs. The client and server files actually disagreed on
`on_new_message`'s signature (`Option<Vec<u8>>` vs `Uint8Array` with no close path) —
this was never a working, coherent contract, just an in-progress prototype.

**The current, real contract lives in `@mugon/sdk` (vendored at `./mugon-sdk-js`,
copied in by the user from the live source, v0.0.1).** It is a materially different
and more capable design:

- **Peer IDs are strings**, not `u64`. No more address-encoding hack needed for
  identity.
- Communication with the platform is **not** direct synchronous `window.*` calls into
  a global relay — it's `postMessage`/`MessageChannel` based. The game's wasm runs
  inside a sandboxed `<iframe>` ("gameframe"); the embedding page ("parentframe", the
  actual Mugon platform code) hands the gameframe one end of a `MessageChannel` on
  load and bridges it to a `ParentframeTransport` (backed by a dev-CLI local relay or
  the real production transport — the parentframe doesn't care which).
- `mugon-sdk-js/src/gameframe.ts` is what actually runs inside the same JS realm as
  our wasm build and is what our `wasm-bindgen` externs must target (it self-installs
  onto `window` via `injectClientApiIntoWindowObject()`):
  - `window.setupParentFrameCommunication(onReadyMsg?, onConnectMsg?, onDisconnectMsg?, onDataMsg?, onSettingsMsg?)`
    — registers callbacks once; internally waits for the parentframe's `{type:"init"}`
    postMessage carrying the `MessagePort`, then replies `{type:"rdy", protocol}`.
  - `window.sendDataMessage(id: string, data: Uint8Array, channel?: Channel)` —
    `channel` defaults to `ReliableOrdered`.
  - `window.disconnect(id: string)` — tells the parentframe to drop a peer.
  - `window.getSettingValue(key)` — reads `"mugon.networkmode"` (`"server" | "client"`),
    `"mugon.serverid"`, `"mugon.clientid"` once the `"set"` message has been received.
    This is how a single build knows whether it's the host or a client, and what its
    own/the server's id is — there's no separate server vs. client SDK entry point.
  - `window.startCanvasManager()` — unrelated to networking (canvas resize), ignore.
- **`Channel` is a first-class part of the wire protocol**: every send/receive carries
  one of `ReliableOrdered | ReliableUnordered | UnreliableOrdered | UnreliableUnordered`
  (`mugon-sdk-js/src/protocol.ts`). One data channel per combination exists per peer↔
  server pair already, at the platform level.
- Message shapes (`protocol.ts`): `con`/`dc` (connect/disconnect, carry `clientid`),
  `msg` (carries `fromclientid`/`toclientid`, `channel`, `data: Uint8Array`), `set`
  (settings dict), `rdy` (protocol handshake/version check).
- Both "server" and "client" roles run the *same* gameframe code; role and identity
  come from settings, not from which functions exist.

**Practical conclusion: do not try to reuse the old fork's JS-facing Rust code.**
Rewrite the bindings from scratch against `mugon-sdk-js`. The user has confirmed the
JS SDK itself can still be adjusted if the Rust side needs something different from it
— it's not frozen.

## Feature-parity checklist (from the old fork)

What the replacement needs to do, functionally, regardless of implementation details:

- [ ] Client: connect to exactly one server peer, given during setup.
- [ ] Server: accept an open-ended number of client peers, each identified separately.
- [ ] Bidirectional byte-payload send/receive per peer, respecting Lightyear's `Link`
      buffer model (`RecvPayload`/`SendPayload`, MTU).
- [ ] Surface connect/disconnect lifecycle per peer (`Linked`/`Unlinked`, and for the
      server, per-`LinkOf`-child spawn/despawn) — this is strictly easier to get right
      than the fork's manual `HashMap<SocketAddr, Sender>` bookkeeping, because
      `Endpoint`/`LinkOf` do the fan-out bookkeeping for us.
- [ ] WASM-only (both client and, per the SDK, server — Mugon's "server" is also a
      gameframe instance, not a native process). Confirm this still holds for how you
      intend to run the host — see **Open decisions** below.
- [ ] No leaked `wasm_bindgen::Closure`s across reconnects (the old fork `.forget()`d
      all three closures per connect with a `// TODO` acknowledging the leak; store
      them on the component instead and let `Unlink`/despawn drop them).

## Decisions

All open items are now resolved. Recorded here with the reasoning, since the
reasoning is what future changes need to stay consistent with.

### 1. Connection/auth layer: `lightyear_raw_connection` ✅

Chosen over `lightyear_netcode`. Rationale: Mugon already hands the gameframe a
platform-vetted `clientid`/`serverid` per session before the game ever sees it —
there's no anonymous/unauthenticated path into a gameframe. A netcode connect-token
handshake on top would just be theater over auth Mugon already did, plus an extra
token-issuing side channel to build and keep in sync. `raw_connection` makes
`Linked` on a `LinkOf` child imply `Connected` directly, with no handshake.

### 2. Peer identity representation: synthetic `SocketAddr`, assigned sequentially ✅

Follows directly from #1. `lightyear_raw_connection`'s server side
(`RawConnectionPlugin::on_link_of_linked`) reads a `PeerAddr` component off each
`LinkOf` child and builds `RemoteId(PeerId::Raw(peer_addr.0))` — so `PeerId::Raw`
*requires* a `SocketAddr`, and `lightyear_core::id::PeerId` has no string/opaque-id
variant. Since Mugon's peer ids are strings, the transport still needs a stable
surrogate address per peer, purely to satisfy this type.

Do this with a **sequential counter, not a hash of the string** (e.g.
`SocketAddr::new(127.0.0.1, next_port)`, or a fake-IPv6 counter like the old fork
used if more than ~64K concurrent peers per process is ever plausible). A bimap
(`HashMap<String, SocketAddr>` + `HashMap<SocketAddr, String>`) on the `MugonEndpoint`
component maps between Mugon's real id and the surrogate, assigned the first time a
`"con"` message names a new peer and torn down on `"dc"`. Sequential beats hashing:
no collisions to reason about, and the addresses stay meaningful in logs/debugging
(`127.0.0.1:1`, `:2`, `:3` in connection order) instead of opaque hash output.

### 3. Mugon's per-message `Channel` vs. Lightyear's own channel/reliability system ✅

This was investigated properly rather than designed on paper, because the answer
changes what's even possible. Finding, not preference:

**Lightyear multiplexes multiple app-level channels of different reliability modes
into one shared byte packet before the bytes ever reach `Link`.** Verified directly:
`lightyear_transport::plugin` queries connection entities as
`Query<(Entity, &mut Link, &mut Transport), ...>` — exactly **one `Link` per
connection**, no per-channel or per-reliability-class fan-out. Its `PacketBuilder`
batches messages from multiple `ChannelId`s (any mix of `ChannelMode`) into a single
`SendPayload` up to MTU (`crates/transport/transport/src/packet/packet_builder.rs`).
By the time a payload hits `Link::send`, which app channel(s) and reliability
mode(s) it contains is no longer visible from outside.

**Consequence: true per-app-channel routing to Mugon's 4 channels is not something an
external transport crate can do.** Doing it would require patching
`lightyear_transport`'s packet builder itself — i.e. re-forking, exactly what this
migration exists to get away from. So this was never really a "design it now vs.
later" choice; the multi-link design floated in the first draft of this doc wasn't
achievable from outside `lightyear_transport` at all, at least not in the current
architecture. (Flagging since this reverses the original ask to "design it now" —
better to say so than let the doc quietly imply it was built.)

**The real, narrower decision was: which single Mugon channel carries Lightyear's one
byte stream.** Lightyear's own crate docs say it "builds up on top of lightyear-io,
to add packet fragmentation, channels, and reliability" — it's designed to run over a
UDP-shaped pipe (lossy, unordered, no built-in reliability) and supply its own
fragmentation/ordering/reliability/acks on top, exactly as it does over plain UDP.
Running that over a *reliable-ordered* channel adds a second reliability layer for
free and, worse, head-of-line blocking: one dropped-then-retransmitted "reliable"
packet at the Mugon layer stalls delivery of every message behind it, including ones
Lightyear's own channels marked unreliable/unordered and didn't want blocked.

**Decision: the transport uses `Channel.UnreliableUnordered` for the single Link's
byte stream.** Confirmed with the user that Mugon's `UnreliableUnordered` maps to a
WebRTC-datagram-like channel (unordered/unreliable data channel semantics) and is
message-framed — a message either arrives whole or not at all, never truncated or
merged with another. That's exactly the delivery model Lightyear already assumes
from UDP, so this is a straight swap-in, not a new assumption.

The other three Mugon channels (`ReliableOrdered`, `ReliableUnordered`,
`UnreliableOrdered`) are unused by the data path. They stay available for something
orthogonal to Lightyear's own message pipeline if a need shows up later (e.g.
transport-internal signaling) — not planned now, just not precluded.

**Implementation note surfaced by this investigation, worth carrying into the code:**
`gameframe.ts`'s `sendDataMessage` does `port.postMessage(msg, [data.buffer])` — it
*transfers* (detaches) the underlying `ArrayBuffer`. A `Bytes`/`SendPayload` popped
from `Link::send` may be a view into a shared/pooled allocation (`PacketBuilder` uses
a `BufferPool`). Copy into a fresh `Uint8Array` per send rather than transferring a
view over Lightyear's own buffer pool — detaching a buffer the pool still thinks it
owns would be a nasty, hard-to-reproduce bug.

### 4. Dependency mode: crates.io releases ✅

`lightyear_link` etc. confirmed published, latest `0.30.1`. No need for a git
dependency.

### 5. Topology: star ✅

One server/host peer, many client peers. Matches `Endpoint`/`LinkOf` directly (host =
`Endpoint`, one `LinkOf` child per client) and is what the SDK models (`mode: "server"
| "client"`, a single `serverid` setting on the client side).

## Crate shape (as built)

```
crates/lightyear_mugon/
  Cargo.toml            # lightyear_link, lightyear_connection, lightyear_raw_connection,
                         # aeronet_io, wasm-bindgen, js-sys, bytes, tracing, thiserror
  src/
    lib.rs               # MugonPlugin: installs Bridge, both client/server transport
                          # plugins, both lightyear_raw_connection plugins,
                          # auto_spawn_role, and the StaleLink despawn safety net
                          # (mark_stale_on_unlinked / despawn_stale_links)
    bindings.rs           # wasm-bindgen externs matching mugon-sdk-js/src/gameframe.ts
                          # 1:1 (setupParentFrameCommunication, sendDataMessage,
                          # disconnect, getSettingValue); Channel enum (0-3)
    bridge.rs             # Bridge: owns the 5 long-lived Closures + the Rc<RefCell<_>>
                          # queues they feed (connects/disconnects/recv), plus the
                          # settings/handshake accessors. NonSend resource (holds
                          # Rc/Closure, not Send/Sync) — see its module docs for why
                          # setupParentFrameCommunication is called exactly once.
                          # take_connect() lets a consumer claim one specific queued
                          # "con" without disturbing entries meant for someone else.
    peer_addr.rs           # PeerAddrTable: sequential String<->SocketAddr bimap
                          # (decision #2) — has its own unit tests (reviewed by hand;
                          # not yet executed, see QUESTIONS.md)
    client.rs             # MugonClientIo (+ RawClient): single Link to the one server
                          # peer, settings-derived server_id, no fan-out. Waits for an
                          # explicit "con" naming the server before marking Linked
                          # (poll_connect) — see QUESTIONS.md #1.
    server.rs             # MugonServerEndpoint (+ RawServer): Server + LinkOf child per
                          # connected peer, PeerAddrTable-backed routing, spawns on
                          # "con" / unlinks on "dc"
```

Send/receive systems mirror `crates/io/crossbeam/src/lib.rs`'s shape: `receive` drains
`Bridge`'s queue into `Link::recv` inside `LinkReceiveSystems::BufferToLink`; `send` drains
`Link::send`, copies each payload into a fresh `Uint8Array` (per the implementation note in
decision #3 — never hand `postMessage` a view into Lightyear's buffer pool), and calls
`Bridge::send` (→ `window.sendDataMessage` with `Channel::UnreliableUnordered`) inside
`LinkSystems::Send`. On the server side, routing is via `MugonServerEndpoint`'s
`HashMap<String, Entity>` rather than iterating a `Server`/`Endpoint` relationship collection
directly — see `server.rs`.

Every judgment call baked into this — the client-side con-handling, reconnection/despawn
semantics, the auto-spawn API choice, and more — is written up in **`QUESTIONS.md`**, along
with what has and hasn't actually been verified.

## Building

This repo pins the `stable-x86_64-pc-windows-gnu` Rust toolchain via `rust-toolchain.toml`
(see `QUESTIONS.md` §7 for why — short version: this dev machine's Visual Studio install was
missing the Windows SDK, so MSVC-target builds couldn't link). Building also needs a MinGW-w64
`bin/` directory on `PATH` for the linker itself; this repo used the one `winget install
BrechtSanders.WinLibs.POSIX.UCRT` installs:

```sh
export PATH="/c/Users/<you>/AppData/Local/Microsoft/WinGet/Packages/BrechtSanders.WinLibs.POSIX.UCRT_Microsoft.Winget.Source_8wekyb3d8bbwe/mingw64/bin:$PATH"
cargo check -p lightyear_mugon --target wasm32-unknown-unknown
```

`.cargo/config.toml` also sets `--cfg getrandom_backend="wasm_js"`, required by `getrandom`
0.3/0.4 (transitive, via `rand`) on `wasm32-unknown-unknown` — same requirement the old fork's
`Cargo.toml` noted for its own `getrandom` pin.

## Repo layout

- `mugon-sdk-js/` — vendored copy of the current `@mugon/sdk` JS/TS source (user-
  provided, not fetched by me). This is the authoritative wire contract; treat the old
  fork's Rust code as historical reference only, not as a spec.
- `crates/lightyear_mugon/` — the transport crate itself; see **Crate shape** above.
- `QUESTIONS.md` — every judgment call and open question from building the prototype,
  collected for review rather than asked one at a time.

See `CLAUDE.md` for durable working context/conventions for future sessions on this
repo.
