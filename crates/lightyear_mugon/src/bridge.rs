//! Shared state between the Mugon JS callbacks and Bevy's per-frame systems.
//!
//! `Bridge` is registered exactly once, at [`crate::MugonPlugin::build`] time, via
//! [`Bridge::install`]. It calls `window.setupParentFrameCommunication` once (see
//! `bindings.rs` for why more than once is unsafe) and owns the five [`Closure`]s for the
//! rest of the process's lifetime, forwarding everything they receive into a plain
//! `Rc<RefCell<_>>` queue that ordinary Bevy systems drain every frame.
//!
//! `Bridge` is not `Send`/`Sync` (it holds `Rc`/`Closure`, which wrap JS values), so it must
//! be installed with [`bevy_app::App::insert_non_send_resource`] and read with
//! `NonSend`/`NonSendMut`, not `Res`/`ResMut`. This is standard practice for wasm-bindgen
//! state in Bevy; `wasm32-unknown-unknown` browser builds are effectively single-threaded
//! anyway.

use crate::bindings;
use bytes::Bytes;
use js_sys::{Reflect, Uint8Array};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkMode {
    Server,
    Client,
}

#[derive(Default)]
pub(crate) struct BridgeState {
    /// Set once the `"set"` (settings) message has arrived. Before this, `getSettingValue`
    /// returns `None` for everything, per `gameframe.ts`.
    pub(crate) settings_ready: bool,
    /// Set once the `"rdy"` (protocol handshake ack) message has arrived. This confirms the
    /// parentframe accepted our protocol version — see `parentframe.ts`'s `sendReady`, sent
    /// ~250ms after settings.
    pub(crate) handshake_complete: bool,
    /// Peer ids that connected (`"con"`), oldest first.
    pub(crate) connects: VecDeque<String>,
    /// Peer ids that disconnected (`"dc"`), oldest first.
    pub(crate) disconnects: VecDeque<String>,
    /// `(from_peer_id, payload)` pairs received (`"msg"`), oldest first.
    pub(crate) recv: VecDeque<(String, Bytes)>,
}

/// Owns the live JS-side registration for the whole process. See module docs.
pub struct Bridge {
    pub(crate) state: Rc<RefCell<BridgeState>>,
    // Kept alive for as long as `Bridge` exists (i.e. for the app's lifetime) so JS can keep
    // calling into them. Never `.forget()`-leaked: dropping `Bridge` (which normally only
    // happens when the whole App/World goes away) drops these too.
    _on_ready: Closure<dyn FnMut(JsValue)>,
    _on_connect: Closure<dyn FnMut(JsValue)>,
    _on_disconnect: Closure<dyn FnMut(JsValue)>,
    _on_data: Closure<dyn FnMut(JsValue)>,
    _on_settings: Closure<dyn FnMut(JsValue)>,
}

fn get_string_field(obj: &JsValue, key: &str) -> Option<String> {
    Reflect::get(obj, &JsValue::from_str(key)).ok()?.as_string()
}

fn get_bytes_field(obj: &JsValue, key: &str) -> Option<Bytes> {
    let value = Reflect::get(obj, &JsValue::from_str(key)).ok()?;
    let array: Uint8Array = value.dyn_into().ok()?;
    Some(Bytes::from(array.to_vec()))
}

impl Bridge {
    /// Registers this process's Mugon callbacks. Must be called exactly once — see
    /// `bindings::setup_parent_frame_communication`.
    pub(crate) fn install() -> Self {
        let state = Rc::new(RefCell::new(BridgeState::default()));

        let on_ready = {
            let state = state.clone();
            Closure::wrap(Box::new(move |_msg: JsValue| {
                state.borrow_mut().handshake_complete = true;
            }) as Box<dyn FnMut(JsValue)>)
        };

        let on_connect = {
            let state = state.clone();
            Closure::wrap(Box::new(move |msg: JsValue| {
                if let Some(id) = get_string_field(&msg, "clientid") {
                    state.borrow_mut().connects.push_back(id);
                }
            }) as Box<dyn FnMut(JsValue)>)
        };

        let on_disconnect = {
            let state = state.clone();
            Closure::wrap(Box::new(move |msg: JsValue| {
                if let Some(id) = get_string_field(&msg, "clientid") {
                    state.borrow_mut().disconnects.push_back(id);
                }
            }) as Box<dyn FnMut(JsValue)>)
        };

        let on_data = {
            let state = state.clone();
            Closure::wrap(Box::new(move |msg: JsValue| {
                let from = get_string_field(&msg, "fromclientid");
                let data = get_bytes_field(&msg, "data");
                if let (Some(from), Some(data)) = (from, data) {
                    state.borrow_mut().recv.push_back((from, data));
                }
            }) as Box<dyn FnMut(JsValue)>)
        };

        let on_settings = {
            let state = state.clone();
            Closure::wrap(Box::new(move |_msg: JsValue| {
                state.borrow_mut().settings_ready = true;
            }) as Box<dyn FnMut(JsValue)>)
        };

        bindings::setup_parent_frame_communication(
            on_ready.as_ref().unchecked_ref(),
            on_connect.as_ref().unchecked_ref(),
            on_disconnect.as_ref().unchecked_ref(),
            on_data.as_ref().unchecked_ref(),
            on_settings.as_ref().unchecked_ref(),
        );

        Self {
            state,
            _on_ready: on_ready,
            _on_connect: on_connect,
            _on_disconnect: on_disconnect,
            _on_data: on_data,
            _on_settings: on_settings,
        }
    }

    /// Whether `getSettingValue` will return real data yet (the `"set"` message has arrived).
    pub fn settings_ready(&self) -> bool {
        self.state.borrow().settings_ready
    }

    /// Whether the protocol handshake completed (the `"rdy"` message has arrived).
    pub fn handshake_complete(&self) -> bool {
        self.state.borrow().handshake_complete
    }

    /// `getSettingValue("mugon.networkmode")`, parsed. `None` before settings arrive, or if the
    /// platform ever sends a value this crate doesn't recognize (treated as a protocol
    /// mismatch, not silently ignored).
    pub fn network_mode(&self) -> Option<NetworkMode> {
        match bindings::get_setting_value("mugon.networkmode")?.as_str() {
            "server" => Some(NetworkMode::Server),
            "client" => Some(NetworkMode::Client),
            _ => None,
        }
    }

    /// `getSettingValue("mugon.serverid")`.
    pub fn server_id(&self) -> Option<String> {
        bindings::get_setting_value("mugon.serverid")
    }

    /// `getSettingValue("mugon.clientid")`.
    pub fn client_id(&self) -> Option<String> {
        bindings::get_setting_value("mugon.clientid")
    }

    /// Pops the next queued `"con"` peer id, if any.
    ///
    /// Only safe for a consumer that wants *every* `"con"` regardless of which peer it names
    /// (the server side: every connecting peer is relevant). A consumer that's only looking for
    /// one specific id (the client side, looking for its one server peer) must use
    /// [`take_connect`](Self::take_connect) instead — draining entries that don't match would
    /// discard them for whoever they actually belonged to.
    pub(crate) fn pop_connect(&self) -> Option<String> {
        self.state.borrow_mut().connects.pop_front()
    }

    /// Removes and returns whether a queued `"con"` for exactly `peer_id` was present, leaving
    /// every other queued entry untouched.
    ///
    /// Needed because a `"con"` can arrive before the entity that's waiting for it exists yet —
    /// e.g. the client's one server peer can connect before this process's settings handshake
    /// (and therefore its `MugonClientIo` entity) exists at all. A plain FIFO pop would either
    /// miss it (if something else pops first) or wrongly discard an unrelated entry while
    /// searching for it.
    pub(crate) fn take_connect(&self, peer_id: &str) -> bool {
        let mut state = self.state.borrow_mut();
        if let Some(pos) = state.connects.iter().position(|id| id == peer_id) {
            state.connects.remove(pos);
            true
        } else {
            false
        }
    }

    /// Pops the next queued `"dc"` peer id, if any.
    pub(crate) fn pop_disconnect(&self) -> Option<String> {
        self.state.borrow_mut().disconnects.pop_front()
    }

    /// Pops the next queued `(from_peer_id, payload)`, if any.
    pub(crate) fn pop_recv(&self) -> Option<(String, Bytes)> {
        self.state.borrow_mut().recv.pop_front()
    }

    /// Sends `data` to `peer_id` on the fixed data channel (see decision #3 in the repo
    /// README: always `UnreliableUnordered`). Copies `data` into a fresh `Uint8Array` first —
    /// `sendDataMessage` transfers the buffer it's given, so it must never be a view into
    /// something else still owns (e.g. Lightyear's packet buffer pool).
    pub(crate) fn send(&self, peer_id: &str, data: &[u8]) {
        let array = Uint8Array::new_with_length(data.len() as u32);
        array.copy_from(data);
        bindings::send_data_message(
            peer_id,
            &array,
            bindings::Channel::UnreliableUnordered as u8,
        );
    }

    /// `window.disconnect(peer_id)`.
    pub(crate) fn disconnect(&self, peer_id: &str) {
        bindings::disconnect(peer_id);
    }
}
