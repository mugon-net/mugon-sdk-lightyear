# Questions from implementing the prototype

Collected while building `crates/lightyear_mugon` so you have one place to review them after
being away, per your ask. Answered on 2026-09-17 — see each item for the resolution and what
changed as a result. None of this has been run against a live Mugon parentframe (dev-CLI relay
or production) or in a browser; see **What's actually been verified** at the end before treating
any of this as working software.

## 1. Does the client ever receive an explicit `"con"` for the server peer? — RESOLVED: yes

**Answer: yes, the client does get `"con"` events**, not just settings + `"rdy"`. Implementation
changed accordingly: `client.rs`'s `LinkStart` observer now only marks the entity `Linking`
(previously it marked `Linked` immediately, assuming presence was purely implied by settings —
that assumption was wrong). A new `poll_connect` system finishes the transition to `Linked` once
a `"con"` naming this client's `server_id` actually arrives, via a new `Bridge::take_connect`
that removes a specific matching entry without disturbing the queue's other entries (needed
because a `"con"` for the server can arrive before the client's `MugonClientIo` entity even
exists — see the method's doc comment for why a plain FIFO pop isn't safe there).

**Residual gap, not asked for and not implemented:** this only fixes the *first* connection's
sequencing. Full reconnect support (the client noticing a *new* `"con"` after having already
gone through `Linked` → `Unlinked` once) would additionally require `auto_spawn_role` to spawn a
new `MugonClientIo` again after the first one — but `auto_spawn_role` still only ever spawns
once per process (its `Local<bool>` guard latches permanently, per decision #4 below, which was
kept as-is). If you need actual reconnect-after-disconnect behavior, that's a separate,
not-yet-designed piece of work.

## 2. Reconnection / peer-id reuse semantics on the server side — RESOLVED: ids are never reused

**Answer: peer ids are never reused for a different peer.** This confirms `server.rs`'s existing
idempotent-relink handling (a repeated `"con"` for a tracked id re-links the existing entity
rather than spawning a duplicate) was already correct — no code change needed here. This also
means a `"con"` for the same id arriving again *while that entity is still in its post-disconnect
grace period* (see #3) is safely understood as the same peer reconnecting quickly, not a
different peer colliding with stale state — `despawn_stale_links` (new, see #3) relies on exactly
this guarantee when it cancels a pending despawn on re-`Linked`.

## 3. Who despawns a disconnected peer's entity? — RESOLVED: added an explicit safety net

**Answer: add an explicit despawn as a safety net.** Implemented in `lib.rs` rather than
duplicated per-role: `mark_stale_on_unlinked` (an observer on `On<Add, Unlinked>`) inserts a new
internal `StaleLink(Instant)` marker on any Mugon-owned link entity (`MugonClientIo` or
`MugonLinkOfIO`) the moment it goes `Unlinked`; `despawn_stale_links` (a `PreUpdate` system)
despawns any `StaleLink` entity whose grace period (`STALE_LINK_GRACE`, currently 1 second) has
elapsed. The grace period exists so this safety net can't race `lightyear_connection`'s own
normal reaction to `Unlinked`/`Disconnected` — it only ever fires if that reaction *didn't*
already remove the entity. If an entity gets re-`Linked` within the grace period (see #2), the
`StaleLink` marker is just removed instead of despawning.

Still not verified end-to-end (no running example) that `lightyear_connection`'s own path
despawns things in the normal case — this safety net means it no longer matters whether it does,
but if you want to actually confirm/fix the upstream behavior rather than just work around it,
that's still open.

## 4. Auto-spawn vs. manual spawn API — RESOLVED: keep auto-spawn

**Answer: keep the current auto-spawn behavior.** No code change. `MugonPlugin` continues to
watch for the platform's settings handshake and spawn `MugonServerEndpoint`/`MugonClientIo`
itself the moment `mugon.networkmode` is known.

## 5. Compile-time client/server split? — RESOLVED: no feature split

**Answer: no feature split.** No code change. `MugonPlugin` continues to install both
`MugonClientPlugin` and `MugonServerPlugin` unconditionally, regardless of which role a given
process ends up playing.

## 6. `lightyear_link` upstream rename (`Server`/`LinkOf{server}` → `Endpoint`/`LinkOf{endpoint}`) — RESOLVED: wait for release

**Answer: wait until the rename ships in an actual release**, then do the mechanical update. No
action taken now; this section is otherwise unchanged from the original writeup below.

This crate is built against the **published** `lightyear_link` 0.30.1 (per decision #4:
crates.io releases, not git), which names the server-side fan-out relationship
`lightyear_link::server::{Server, LinkOf, ServerLinkPlugin}` with `LinkOf { server: Entity }`.

While researching the design (before writing any code), I'd read `cBournhonesque/lightyear`'s
unreleased `main` branch, which has already renamed this to
`lightyear_link::endpoint::{Endpoint, LinkOf, EndpointLinkPlugin}` with
`LinkOf { endpoint: Entity }` — more accurate naming, since a P2P peer can own child links
without being a "server". The two names are otherwise structurally identical.

Action item for whenever this crate's `lightyear_link` dependency gets bumped past whatever
release ships that rename: `server.rs` needs a mechanical find-and-replace
(`Server`→`Endpoint`, `.server`→`.endpoint`, `ServerLinkPlugin`→`EndpointLinkPlugin`). Not
urgent, just don't be surprised by it — I'm noting it here so it's a known, planned update
rather than a mystery breakage later.

## 7. Windows dev-environment notes (not a design decision — recording so you don't hit the same wall)

This machine had Visual Studio 2022 Community's C++ toolset but no Windows SDK installed, so
MSVC-target Rust builds couldn't link anything with a build script or proc-macro (`kernel32.lib`
not found — i.e. basically every nontrivial crate). `vs_installer.exe modify --add
Microsoft.VisualStudio.Component.Windows11SDK.22621 --quiet` refused with exit code 5007
("should be run elevated from the beginning"), and I didn't want to chase interactive UAC
elevation from an automated session.

What I did instead:
- Installed a portable MinGW-w64 toolchain via `winget install BrechtSanders.WinLibs.POSIX.UCRT`
  (user-scope, no elevation needed).
- Added `rust-toolchain.toml` at the repo root pinning `stable-x86_64-pc-windows-gnu` — so
  *this repo* builds with the GNU toolchain regardless of your global rustup default.
- Left your global rustup default on MSVC, just updated (`rustup update stable`, needed anyway
  since `lightyear_link` 0.30.1 requires rustc ≥1.95 and this machine had 1.90 — now 1.98.1).
- Also ran into Git for Windows' own `usr/bin/link.exe` (a POSIX `link` command) shadowing
  MSVC's real linker on `PATH` — irrelevant now that this repo uses the GNU toolchain, but
  worth knowing about if you ever go back to MSVC here.

**Building this repo currently requires MinGW's `bin/` directory on `PATH`** (not just the
right rustup toolchain) — the linker itself isn't picked up automatically:
```
export PATH="/c/Users/<you>/AppData/Local/Microsoft/WinGet/Packages/BrechtSanders.WinLibs.POSIX.UCRT_Microsoft.Winget.Source_8wekyb3d8bbwe/mingw64/bin:$PATH"
```
If you'd rather use MSVC long-term (e.g. to match other native Rust work you do), installing
the Windows 11 SDK component through the Visual Studio Installer's GUI (Individual Components
tab — the `--quiet` flag is what needed elevation, the interactive installer shouldn't) would
let you delete `rust-toolchain.toml` and drop the `PATH` requirement.

## What's actually been verified

- `cargo check --target wasm32-unknown-unknown` and `cargo clippy --target wasm32-unknown-unknown`
  both pass clean (zero warnings), including after implementing the changes from items 1 and 3
  above — against the real published `lightyear_link`/`lightyear_connection`/
  `lightyear_raw_connection` 0.30.1 APIs. This confirms the component/plugin/system wiring
  type-checks and the trait bounds (`NonSend<Bridge>`, the `#[require(...)]` chains, the new
  `On<Add, Unlinked>` observer, etc.) are all satisfied. It does **not** confirm the runtime
  ordering/timing reasoning behind `take_connect` or the `StaleLink` grace period is correct —
  that's exactly the kind of thing that only shows up when actually run.
- `cargo test --target wasm32-unknown-unknown --lib` **compiles** a test binary successfully,
  but I could not **execute** it — a `.wasm` file isn't a native Windows executable, and running
  it needs a `wasm-bindgen-test` harness + a JS runtime (Node, which is present on this
  machine, or a browser), neither of which is wired up yet. `peer_addr.rs`'s unit tests also
  use plain `#[test]`, which doesn't work correctly under `wasm32-unknown-unknown` at all
  without switching to `#[wasm_bindgen_test]` — I reviewed that logic by hand instead of
  relying on the tests actually running. Recommend either wiring up `wasm-bindgen-test` +
  `wasm-pack test --node`, or splitting pure-logic modules like `peer_addr.rs` into a
  target-agnostic sub-crate that can run under plain `cargo test` on the host.
- **Nothing has been run against an actual Mugon parentframe** (dev-CLI relay or production),
  in a browser, or otherwise end-to-end. There is no example/demo yet. Everything above is
  "the code compiles and the types line up," not "this works."
