//! `wasm-bindgen` externs matching `mugon-sdk-js/src/gameframe.ts` 1:1.
//!
//! These are the only functions the gameframe SDK installs onto `window`
//! (see `gameframe.ts::injectClientApiIntoWindowObject`). Everything else in
//! this crate is built on top of this narrow surface.

use js_sys::Function;
use wasm_bindgen::prelude::*;

/// Mirrors `protocol.ts`'s `Channel` enum. Values must match the JS side exactly —
/// they are shared wire constants, not just a Rust-side convenience.
///
/// Only [`Channel::UnreliableUnordered`] is used today (decision #3 in the repo README: it's
/// the only channel Lightyear's own byte stream sends on). The others are kept here, spelled
/// out in full, so the mapping to `protocol.ts` stays complete and obvious even though this
/// crate is currently a one-channel consumer.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Channel {
    ReliableOrdered = 0,
    ReliableUnordered = 1,
    UnreliableOrdered = 2,
    UnreliableUnordered = 3,
}

#[wasm_bindgen]
extern "C" {
    /// `window.setupParentFrameCommunication(onReadyMsg?, onConnectMsg?, onDisconnectMsg?, onDataMsg?, onSettingsMsg?)`
    ///
    /// Must be called exactly once for the lifetime of the wasm module: `gameframe.ts` stores
    /// the callbacks and adds a `window` message listener as module-level state, not per-call
    /// state, so calling this more than once stacks redundant listeners and re-processes the
    /// `"init"` handshake. See `Bridge::install`.
    #[wasm_bindgen(js_namespace = window, js_name = setupParentFrameCommunication)]
    pub(crate) fn setup_parent_frame_communication(
        on_ready: &Function,
        on_connect: &Function,
        on_disconnect: &Function,
        on_data: &Function,
        on_settings: &Function,
    );

    /// `window.sendDataMessage(id, data, channel)`.
    ///
    /// Note from the JS side (`gameframe.ts`): `port.postMessage(msg, [data.buffer])`
    /// *transfers* (detaches) `data`'s backing `ArrayBuffer`. Callers must pass a `Uint8Array`
    /// that owns its buffer outright (e.g. a fresh copy), never a view into a buffer something
    /// else (such as Lightyear's packet buffer pool) still expects to read from afterwards.
    #[wasm_bindgen(js_namespace = window, js_name = sendDataMessage)]
    pub(crate) fn send_data_message(id: &str, data: &js_sys::Uint8Array, channel: u8);

    /// `window.disconnect(id)`.
    #[wasm_bindgen(js_namespace = window, js_name = disconnect)]
    pub(crate) fn disconnect(id: &str);

    /// `window.getSettingValue(key)`.
    ///
    /// Returns `None` until the `"set"` message has been received (see `gameframe.ts`) —
    /// i.e. before `Bridge`'s `on_settings` callback has fired at least once.
    #[wasm_bindgen(js_namespace = window, js_name = getSettingValue)]
    pub(crate) fn get_setting_value(key: &str) -> Option<String>;
}
