# CLAUDE.md

Working context for this repo. See `README.md` for the actual investigation findings
and plan — this file is about how to work on it, not what it says.

## What this repo is for

Designing and building a Lightyear transport plugin so Bevy/WASM games on the Mugon
platform can move from a private fork of Lightyear 0.19 to a normal, unforked
dependency on current Lightyear (0.30.x). As of 2026-09-17 there's a first prototype
at `crates/lightyear_mugon` that type-checks against real published lightyear 0.30.1
crates for `wasm32-unknown-unknown`, but has not been run against a live Mugon
parentframe or in a browser — see `QUESTIONS.md` for the exact line between "compiles"
and "works," and every judgment call made while building it.

The user (Benedikt, benedikt.strobel@planqc.eu) owns/maintains the Mugon platform
this transport targets, and can change the JS SDK contract if the Rust side needs
something different — it is not an external, frozen dependency.

## How to work on this repo

- The user explicitly asked to be consulted on decisions rather than have me assume.
  All decisions in README.md are currently resolved (see "Decision log" below), but
  this preference stands for any *new* decision point implementation surfaces — check
  in before picking for them, even under an autonomous-mode instruction to bias toward
  not stopping. Standing exception, not a one-time ask.
- When a decision turns out to be harder or more consequential than it looked (e.g.
  the multi-channel investigation below), surface that back to the user with the
  finding before proceeding on your own read of it — don't quietly resolve it and
  mention it only in passing.
- When investigating either Lightyear repo, don't re-clone into this working
  directory — clone into the session scratchpad instead and clean up. This repo's git
  history should stay about the transport crate itself.
- `mugon-sdk-js/` is user-provided ground truth for the JS contract, not something to
  regenerate from the old fork's assumptions. If Rust-side needs diverge from what the
  SDK currently exposes, say so and propose the JS-side change explicitly rather than
  working around a mismatch on the Rust side.
- `mugon-sdk-js/node_modules` and `mugon-sdk-js/dist` are vendor/build output, not
  source to read for understanding the contract — the contract is in `mugon-sdk-js/src`.
- Building needs the MinGW-w64 toolchain on `PATH`, not just `rust-toolchain.toml`'s
  GNU channel selection — see README's **Building** section and `QUESTIONS.md` §7 for
  why (no Windows SDK on this machine → MSVC-target builds can't link). Don't
  "helpfully" switch this back to MSVC or delete `rust-toolchain.toml` without checking
  whether the SDK gap has actually been fixed first.
- No test runner is wired up yet for this crate (it's wasm-only; `cargo test` compiles
  a `.wasm` binary but can't execute it without `wasm-bindgen-test` + a JS runtime).
  Don't report a change as "tested" based on `cargo check`/`cargo test` output alone —
  say what was actually verified, same as `QUESTIONS.md` does.

## Key facts worth not re-deriving

- Fork: `mugon-net/lightyear`, branch `0.19.0` (NOT `0.19.0-mugon`, which is a stale
  ancestor of it). Custom transport: `lightyear/src/transport/mugon/{mod,common,client,server}.rs`.
- Upstream: `cBournhonesque/lightyear`, now workspace-split under `crates/`, currently
  0.30.x, `lightyear_link` published on crates.io.
- The fork's `window.registerCallbacks`/`sendFromMugonSocket`/`closeMugonSocket` JS
  contract (u64 ids, direct synchronous calls) is **obsolete**. The real current
  contract is `mugon-sdk-js/src/gameframe.ts`'s `window.setupParentFrameCommunication`/
  `sendDataMessage`/`disconnect`/`getSettingValue`, using string peer ids and
  postMessage/MessageChannel under the hood, plus a `Channel` enum (reliable/unreliable
  × ordered/unordered) on every message.
- New Lightyear's core transport abstraction is `lightyear_link::Link` (byte-queue ECS
  component) + `Endpoint`/`LinkOf` for fan-out. Reference implementations to imitate:
  `crates/io/crossbeam` (simplest) and `crates/io/udp` (`lib.rs` + `endpoint.rs`,
  closest structural analogue for the server/multi-peer side).
- No more hand-rolled async/task-polling machinery is needed — Bevy's own
  `PreUpdate`/`PostUpdate` schedule replaces the old fork's tokio-channel polling loops.

## Decision log

All five items are resolved (see README.md "Decisions" for full reasoning per item —
this is just the summary, don't let it drift out of sync with README):

- Topology: star (one server/host, many clients).
- Dependency mode: crates.io releases, not a git dependency.
- Connection layer: `lightyear_raw_connection` (not `lightyear_netcode`) — Mugon
  already vets peer identity before the gameframe sees it, so a connect-token
  handshake on top would be redundant.
- Peer-id representation: Mugon's string ids get a **sequential** synthetic
  `SocketAddr` surrogate (not a hash), tracked in a bimap on the endpoint component.
  Required because `lightyear_raw_connection`'s `PeerId::Raw` variant only accepts a
  `SocketAddr`, and `lightyear_core::id::PeerId` has no string/opaque variant.
- Mugon `Channel` vs. Lightyear's own reliability layer: **investigated, not just
  decided** — Lightyear multiplexes multiple app-channels/reliability-modes into one
  shared byte packet per `Link` before it reaches transport (`PacketBuilder` batches
  into one `SendPayload`; exactly one `Link` per connection entity in
  `lightyear_transport::plugin`). External per-channel routing across Mugon's 4
  channels isn't achievable without patching `lightyear_transport` itself — i.e.
  re-forking. So the real decision was narrower than originally scoped: use
  `Channel::UnreliableUnordered` as the single channel for Lightyear's byte stream
  (matches the UDP-shaped pipe Lightyear is designed for; avoids double reliability
  and head-of-line blocking that a reliable-ordered channel would add). Confirmed with
  the user that Mugon's `UnreliableUnordered` is WebRTC-datagram-like and
  message-framed (whole message or nothing, no truncation) — the delivery model
  Lightyear already assumes.
  - Side finding worth remembering when writing `bindings.rs`: `sendDataMessage`
    transfers (detaches) the `ArrayBuffer` it's given. Copy `Link::send` payloads into
    a fresh `Uint8Array` before sending — don't transfer a view into Lightyear's
    pooled buffers.
  - If a future need for genuinely different delivery guarantees per logical channel
    comes up, that requires patching `lightyear_transport`'s packet builder, not
    something to attempt inside this crate — flag it back to the user rather than
    trying to route around it silently.

## Open items from building the prototype — all answered 2026-09-17

These came up while implementing, after the five decisions above were already settled.
All six were put to the user as a batch (per their ask) and are now resolved — full
reasoning and what changed for each lives in `QUESTIONS.md`, not here:

1. Client does get an explicit `"con"` for the server peer → implemented: `client.rs`
   now waits for it (`poll_connect` + `Bridge::take_connect`) instead of assuming
   `Linked` on `LinkStart`. Residual gap (not asked for, not built): full
   reconnect-after-disconnect would also need `auto_spawn_role` to spawn a *new*
   `MugonClientIo`, which it still doesn't (its one-shot guard was kept, per #4).
2. Peer ids are never reused for a different peer → confirmed the existing
   idempotent-relink handling in `server.rs` was already correct; no change needed.
3. Despawn-on-disconnect was unverified → added an explicit safety net:
   `StaleLink`/`mark_stale_on_unlinked`/`despawn_stale_links` in `lib.rs`, a
   grace-period despawn (`STALE_LINK_GRACE`, 1s) that only fires if
   `lightyear_connection`'s own cleanup didn't already remove the entity, and stands
   down if the entity gets re-`Linked` within the window (relies on #2's guarantee).
4. Keep auto-spawn — no change.
5. No compile-time client/server feature split — no change.
6. Wait for the `Server`→`Endpoint` rename to ship in a release before updating
   `server.rs` — no change now, still a live action item for that future bump.

If you're picking further work back up and something here turns out to matter
differently than assumed, say so and point at the specific item rather than silently
choosing differently — same standing preference as above.
