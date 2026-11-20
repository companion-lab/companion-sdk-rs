//! companion-plugin-sdk — Rust backend SDK for companion-tauri plugins.
//!
//! A plugin backend is a `cdylib` that implements [`Plugin`] and exports the
//! C ABI via [`export_plugin!`]:
//!
//! ```ignore
//! use companion_plugin_sdk::{Plugin, PluginCtx, export_plugin};
//! use serde_json::{json, Value};
//!
//! struct Hello;
//!
//! impl Plugin for Hello {
//!     fn init(&mut self, _ctx: PluginCtx) -> Result<(), String> { Ok(()) }
//!
//!     fn handle(&mut self, method: &str, payload: Value) -> Result<Value, String> {
//!         match method {
//!             "ping" => Ok(json!({ "pong": true })),
//!             _ => Err(format!("unknown method: {method}")),
//!         }
//!     }
//! }
//!
//! export_plugin!(Hello);
//! ```
//!
//! Build with `crate-type = ["cdylib"]`, then ship the produced
//! `lib<name>.dylib` / `<name>.dll` / `lib<name>.so` next to the plugin's
//! `manifest.json`. The host routes every `plugin_call` to
//! [`Plugin::handle`]; all payloads and results are JSON.
//!
//! Beyond emit/log/dirs, [`PluginCtx::host_call`] reaches host capabilities
//! (ABI 3), each gated by a manifest permission:
//!
//! - `ctx.plugins()` — list installed plugins, enable/disable others
//!   (`"plugins"` permission).
//! - `ctx.window()` — size/position/always-on-top/click-through/follow-spaces
//!   of the companion window (`"window"` permission).
//! - `ctx.app()` — read the resolved Tauri config, app name and version
//!   (`"app"` permission).
//! - `ctx.shortcuts()` — register/unregister global shortcuts
//!   (`"shortcuts"` permission).

mod abi;
mod surfaces;

pub use abi::{HostBridge, PluginVTable, RawBuffer, ABI_VERSION, LOG_ERROR, LOG_INFO, LOG_WARN};
pub use surfaces::{
    AppSurface, LogicalPoint, LogicalSize, PluginSummary, PluginsSurface, ShortcutsSurface,
    WindowAnchor, WindowSurface,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::ffi::c_void;
use std::path::PathBuf;

/// A tool the plugin exposes to the chat model. The host merges these into
/// the tool list the model sees; tool calls arrive as regular `handle`
/// requests, conventionally with method `"tool"` and
/// `{"name": ..., "args": ...}` payloads (see the notes example).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub input_schema: Value,
}

/// Host services available to a plugin. Cheap to clone; keep a copy if the
/// backend needs to emit events or log outside `init`.
#[derive(Clone)]
pub struct PluginCtx {
    bridge: HostBridge,
}

impl std::fmt::Debug for PluginCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The bridge is raw function pointers; identity-only debug output.
        f.debug_struct("PluginCtx").finish_non_exhaustive()
    }
}

// Safety: the host guarantees `HostBridge` callbacks are thread-safe and that
// `user` stays valid for the plugin's lifetime (see module docs on `abi`).
unsafe impl Send for PluginCtx {}
unsafe impl Sync for PluginCtx {}

impl PluginCtx {
    /// Internal constructor used by `export_plugin!` (the macro expands in
    /// the plugin's crate, which cannot see private fields).
    #[doc(hidden)]
    pub fn from_bridge(bridge: HostBridge) -> Self {
        PluginCtx { bridge }
    }

    /// Emitted events are namespaced by the host as `plugin:{id}:{event}`.
    pub fn emit(&self, event: &str, payload: &Value) {
        if self.bridge.user.is_null() {
            return;
        }
        let event = event.as_bytes();
        let payload = payload.to_string().into_bytes();
        (self.bridge.emit_event)(
            self.bridge.user,
            event.as_ptr(),
            event.len(),
            payload.as_ptr(),
            payload.len(),
        );
    }

    pub fn info(&self, msg: &str) {
        self.log(LOG_INFO, msg);
    }

    pub fn warn(&self, msg: &str) {
        self.log(LOG_WARN, msg);
    }

    pub fn error(&self, msg: &str) {
        self.log(LOG_ERROR, msg);
    }

    /// Writable directory scoped to this plugin (`{app_data}/plugins-data/{id}/`).
    /// The host creates it before `init`, so storing files here works
    /// immediately. `None` when the host could not provide a path.
    pub fn data_dir(&self) -> Option<PathBuf> {
        read_dir_path(self.bridge.data_dir, self.bridge.user)
    }

    /// Read-only directory of this plugin's package (`manifest.json`,
    /// frontend bundle, backend library, bundled `assets/`).
    pub fn plugin_dir(&self) -> Option<PathBuf> {
        read_dir_path(self.bridge.plugin_dir, self.bridge.user)
    }

    fn log(&self, level: u32, msg: &str) {
        if self.bridge.user.is_null() {
            return;
        }
        let bytes = msg.as_bytes();
        (self.bridge.log)(self.bridge.user, level, bytes.as_ptr(), bytes.len());
    }
}

/// Two-pass path read over a bridge callback: first call sizes the path, the
/// second copies it. Matches the `data_dir`/`plugin_dir` callback contract.
fn read_dir_path(
    get: extern "C" fn(user: *mut c_void, out: *mut u8, out_cap: usize) -> usize,
    user: *mut c_void,
) -> Option<PathBuf> {
    if user.is_null() {
        return None;
    }
    let needed = get(user, std::ptr::null_mut(), 0);
    if needed == 0 {
        return None;
    }
    let mut buf = vec![0u8; needed];
    let written = get(user, buf.as_mut_ptr(), buf.len());
    if written == 0 || written > needed {
        return None;
    }
    buf.truncate(written);
    Some(PathBuf::from(String::from_utf8_lossy(&buf).into_owned()))
}

/// The trait a plugin backend implements.
pub trait Plugin: Send {
    /// Called once, before the first `handle`. Keep `ctx` if you need to emit
    /// events later.
    fn init(&mut self, ctx: PluginCtx) -> Result<(), String> {
        let _ = ctx;
        Ok(())
    }

    /// Handle one request. `method` plus a JSON `payload` in, JSON result out.
    fn handle(&mut self, method: &str, payload: Value) -> Result<Value, String>;

    /// Tools exposed to the chat model. Read at plugin load.
    fn tools(&mut self) -> Vec<ToolDef> {
        Vec::new()
    }

    /// Called before the instance is dropped (app exit or backend disable).
    fn shutdown(&mut self) {}
}

#[doc(hidden)]
/// Stands in for a plugin whose `init` failed, so a broken backend degrades
/// to "every call errors" instead of "host crashed".
pub struct FailedPlugin {
    error: String,
}

impl FailedPlugin {
    pub fn new(error: String) -> Self {
        FailedPlugin { error }
    }
}

impl Plugin for FailedPlugin {
    fn handle(&mut self, _method: &str, _payload: Value) -> Result<Value, String> {
        Err(self.error.clone())
    }
}

/// Export the plugin ABI. Accepts any expression producing a value
/// implementing [`Plugin`]:
///
/// ```ignore
/// export_plugin!(Notes);
/// // or with a constructor:
/// export_plugin!(Notes::new());
/// ```
#[macro_export]
macro_rules! export_plugin {
    ($create:expr) => {
        #[no_mangle]
        pub extern "C" fn companion_plugin_abi_version() -> u32 {
            $crate::ABI_VERSION
        }

        #[no_mangle]
        pub extern "C" fn companion_plugin_create(
            bridge: $crate::HostBridge,
        ) -> *mut ::core::ffi::c_void {
            let ctx = $crate::PluginCtx::from_bridge(bridge);
            let boxed: Box<dyn $crate::Plugin> = match ::std::panic::catch_unwind(
                ::std::panic::AssertUnwindSafe(|| $create),
            ) {
                Ok(mut plugin) => match plugin.init(ctx) {
                    Ok(()) => Box::new(plugin),
                    Err(e) => Box::new($crate::FailedPlugin::new(e)),
                },
                Err(_) => Box::new($crate::FailedPlugin::new(
                    "plugin panicked during construction".to_string(),
                )),
            };
            // Double-box: the trait object is a fat pointer, but the FFI
            // boundary only carries a thin `*mut c_void`. Wrapping it in
            // `Box<Box<dyn Plugin>>` keeps the vtable intact.
            Box::into_raw(Box::new(boxed)) as *mut ::core::ffi::c_void
        }

        #[no_mangle]
        pub extern "C" fn companion_plugin_handle(
            instance: *mut ::core::ffi::c_void,
            method: *const u8,
            method_len: usize,
            payload: *const u8,
            payload_len: usize,
        ) -> $crate::RawBuffer {
            if instance.is_null() {
                let err = ::serde_json::json!({ "ok": false, "error": "null instance" });
                return $crate::RawBuffer::from_vec(err.to_string().into_bytes());
            }
            let plugin =
                unsafe { &mut *(instance as *mut Box<dyn $crate::Plugin>) };
            let method = if method.is_null() {
                String::new()
            } else {
                let bytes = unsafe { ::core::slice::from_raw_parts(method, method_len) };
                String::from_utf8_lossy(bytes).into_owned()
            };
            let parsed = if payload.is_null() || payload_len == 0 {
                Ok(::serde_json::Value::Null)
            } else {
                let bytes = unsafe { ::core::slice::from_raw_parts(payload, payload_len) };
                ::serde_json::from_slice(bytes)
                    .map_err(|e| format!("invalid JSON payload: {e}"))
            };
            let envelope = match parsed {
                Err(e) => ::serde_json::json!({ "ok": false, "error": e }),
                Ok(payload) => match ::std::panic::catch_unwind(
                    ::std::panic::AssertUnwindSafe(|| plugin.handle(&method, payload)),
                ) {
                    Ok(Ok(value)) => ::serde_json::json!({ "ok": true, "value": value }),
                    Ok(Err(e)) => ::serde_json::json!({ "ok": false, "error": e }),
                    Err(_) => ::serde_json::json!({
                        "ok": false,
                        "error": "plugin panicked while handling a request"
                    }),
                },
            };
            $crate::RawBuffer::from_vec(envelope.to_string().into_bytes())
        }

        #[no_mangle]
        pub extern "C" fn companion_plugin_tools(
            instance: *mut ::core::ffi::c_void,
        ) -> $crate::RawBuffer {
            let tools = if instance.is_null() {
                Vec::new()
            } else {
                let plugin =
                    unsafe { &mut *(instance as *mut Box<dyn $crate::Plugin>) };
                match ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
                    plugin.tools()
                })) {
                    Ok(tools) => tools,
                    Err(_) => Vec::new(),
                }
            };
            let json = ::serde_json::to_vec(&tools).unwrap_or_default();
            $crate::RawBuffer::from_vec(json)
        }

        #[no_mangle]
        pub extern "C" fn companion_plugin_destroy(instance: *mut ::core::ffi::c_void) {
            if instance.is_null() {
                return;
            }
            let plugin = instance as *mut Box<dyn $crate::Plugin>;
            unsafe {
                let mut boxed = Box::from_raw(plugin);
                let _ = ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(
                    || boxed.shutdown(),
                ));
                drop(boxed);
            }
        }

        #[no_mangle]
        pub extern "C" fn companion_plugin_free_buffer(buffer: $crate::RawBuffer) {
            if buffer.ptr.is_null() {
                return;
            }
            unsafe {
                drop(::std::vec::Vec::from_raw_parts(
                    buffer.ptr,
                    buffer.len,
                    buffer.cap,
                ));
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    // -- bridge fixtures -----------------------------------------------------
    //
    // Callbacks are plain extern "C" fns (the bridge can't capture closures),
    // so the fixtures carry their state in statics. Tests are effectively
    // single-threaded around each call site.

    static EMITTED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
    static LOGGED: Mutex<Vec<(u32, String)>> = Mutex::new(Vec::new());

    extern "C" fn rec_emit(
        _user: *mut c_void,
        event: *const u8,
        event_len: usize,
        payload: *const u8,
        payload_len: usize,
    ) {
        let event =
            unsafe { String::from_utf8_lossy(std::slice::from_raw_parts(event, event_len)) };
        let payload =
            unsafe { String::from_utf8_lossy(std::slice::from_raw_parts(payload, payload_len)) };
        EMITTED
            .lock()
            .unwrap()
            .push((event.into_owned(), payload.into_owned()));
    }

    extern "C" fn rec_log(_user: *mut c_void, level: u32, msg: *const u8, msg_len: usize) {
        let msg = unsafe { String::from_utf8_lossy(std::slice::from_raw_parts(msg, msg_len)) };
        LOGGED.lock().unwrap().push((level, msg.into_owned()));
    }

    extern "C" fn nop_emit(
        _user: *mut c_void,
        _event: *const u8,
        _event_len: usize,
        _payload: *const u8,
        _payload_len: usize,
    ) {
    }

    extern "C" fn nop_log(_user: *mut c_void, _level: u32, _msg: *const u8, _msg_len: usize) {}

    extern "C" fn nop_dir(_user: *mut c_void, _out: *mut u8, _out_cap: usize) -> usize {
        0
    }

    extern "C" fn nop_host_call(
        _user: *mut c_void,
        _capability: *const u8,
        _capability_len: usize,
        _method: *const u8,
        _method_len: usize,
        _payload: *const u8,
        _payload_len: usize,
    ) -> RawBuffer {
        RawBuffer::NULL
    }

    extern "C" fn nop_free_bridge_buffer(_user: *mut c_void, _buffer: RawBuffer) {}

    fn bridge_with(user: *mut c_void, emit_and_log: bool) -> HostBridge {
        if emit_and_log {
            HostBridge {
                user,
                emit_event: rec_emit,
                log: rec_log,
                data_dir: nop_dir,
                plugin_dir: nop_dir,
                host_call: nop_host_call,
                free_bridge_buffer: nop_free_bridge_buffer,
            }
        } else {
            HostBridge {
                user,
                emit_event: nop_emit,
                log: nop_log,
                data_dir: nop_dir,
                plugin_dir: nop_dir,
                host_call: nop_host_call,
                free_bridge_buffer: nop_free_bridge_buffer,
            }
        }
    }

    /// Non-null `user` pointer for bridge fixtures. The fake callbacks never
    /// dereference it; a dangling-but-aligned pointer avoids inventing a
    /// fake address (clippy::manual_dangling_ptr).
    fn live_user() -> *mut c_void {
        std::ptr::dangling_mut::<c_void>()
    }

    // -- ToolDef --------------------------------------------------------------

    #[test]
    fn tool_def_serializes_all_fields() {
        let def = ToolDef {
            name: "save_note".into(),
            description: "Save a note".into(),
            input_schema: json!({"type": "object"}),
        };
        let parsed: ToolDef = serde_json::from_str(
            r#"{"name":"save_note","description":"Save a note","input_schema":{"type":"object"}}"#,
        )
        .unwrap();
        assert_eq!(def.name, parsed.name);
        assert_eq!(def.description, parsed.description);
        assert_eq!(def.input_schema, parsed.input_schema);
        let wire = serde_json::to_string(&parsed).unwrap();
        assert_eq!(
            wire,
            r#"{"name":"save_note","description":"Save a note","input_schema":{"type":"object"}}"#
        );
    }

    #[test]
    fn tool_def_input_schema_defaults_to_null_when_omitted() {
        // Older/looser plugin manifests may omit the schema; the wire format
        // must still round-trip (null schema, not a missing field).
        let def: ToolDef = serde_json::from_str(r#"{"name":"n","description":"d"}"#).unwrap();
        assert_eq!(def.input_schema, Value::Null);
        assert_eq!(
            serde_json::to_string(&def).unwrap(),
            r#"{"name":"n","description":"d","input_schema":null}"#
        );
    }

    // -- FailedPlugin ----------------------------------------------------------

    #[test]
    fn failed_plugin_errors_every_call_with_the_init_error() {
        let mut p = FailedPlugin::new("init exploded".to_string());
        assert_eq!(
            p.handle("ping", json!({})),
            Err("init exploded".to_string())
        );
        // The error is cloned per call: a second call still carries it.
        assert_eq!(
            p.handle("other", Value::Null),
            Err("init exploded".to_string())
        );
    }

    #[test]
    fn failed_plugin_uses_default_trait_methods() {
        let mut p = FailedPlugin::new("nope".to_string());
        assert!(p
            .init(PluginCtx::from_bridge(bridge_with(
                std::ptr::null_mut(),
                false
            )))
            .is_ok());
        assert!(p.tools().is_empty());
        p.shutdown(); // default no-op
    }

    // -- PluginCtx logging / events --------------------------------------------

    #[test]
    fn ctx_log_forwards_with_the_right_level() {
        LOGGED.lock().unwrap().clear();
        let ctx = PluginCtx::from_bridge(bridge_with(live_user(), true));
        ctx.info("starting");
        ctx.warn("hmm");
        ctx.error("bad");
        let logged = LOGGED.lock().unwrap().clone();
        assert_eq!(
            logged,
            vec![
                (LOG_INFO, "starting".to_string()),
                (LOG_WARN, "hmm".to_string()),
                (LOG_ERROR, "bad".to_string()),
            ]
        );
    }

    #[test]
    fn ctx_emit_serializes_the_payload_json() {
        EMITTED.lock().unwrap().clear();
        let ctx = PluginCtx::from_bridge(bridge_with(live_user(), true));
        ctx.emit("state", &json!({ "ok": true }));
        let emitted = EMITTED.lock().unwrap().clone();
        assert_eq!(
            emitted,
            vec![("state".to_string(), r#"{"ok":true}"#.to_string())]
        );
    }

    #[test]
    fn ctx_calls_with_null_user_are_silent_noops() {
        // Plugins built against hosts that never handed a bridge must not
        // crash on log/emit.
        let ctx = PluginCtx::from_bridge(bridge_with(std::ptr::null_mut(), true));
        ctx.info("ignored");
        ctx.emit("ignored", &json!(null));
        assert!(LOGGED.lock().unwrap().is_empty());
        assert!(EMITTED.lock().unwrap().is_empty());
    }

    // -- read_dir_path: the two-pass dir protocol --------------------------------

    /// Per-test fixture state carried through the `user` pointer, so the
    /// tests share no statics and stay parallel-safe.
    struct DirFixture {
        bytes: Vec<u8>,
        /// Force the copy call to return this count (after writing exactly
        /// that many bytes), simulating a short write.
        copy_ret: Option<usize>,
    }

    extern "C" fn dir_callback(user: *mut c_void, out: *mut u8, out_cap: usize) -> usize {
        let fx = unsafe { &*(user as *const DirFixture) };
        if out.is_null() {
            return fx.bytes.len(); // sizing pass
        }
        if let Some(ret) = fx.copy_ret {
            if ret > 0 && ret <= fx.bytes.len() && ret <= out_cap {
                unsafe { std::ptr::copy_nonoverlapping(fx.bytes.as_ptr(), out, ret) };
            }
            return ret;
        }
        if out_cap < fx.bytes.len() {
            return fx.bytes.len(); // too small: nothing copied, per contract
        }
        unsafe { std::ptr::copy_nonoverlapping(fx.bytes.as_ptr(), out, fx.bytes.len()) };
        fx.bytes.len()
    }

    /// Run read_dir_path against a fresh fixture, reclaimed after the call
    /// (read_dir_path never frees `user` — the host owns it).
    fn read_fixture(bytes: &[u8], copy_ret: Option<usize>) -> Option<PathBuf> {
        let fx = Box::into_raw(Box::new(DirFixture {
            bytes: bytes.to_vec(),
            copy_ret,
        }));
        let result = read_dir_path(dir_callback, fx as *mut c_void);
        unsafe { drop(Box::from_raw(fx)) };
        result
    }

    #[test]
    fn read_dir_path_null_user_short_circuits() {
        assert_eq!(read_dir_path(dir_callback, std::ptr::null_mut()), None);
    }

    #[test]
    fn read_dir_path_zero_needed_means_no_dir() {
        assert_eq!(read_fixture(b"", None), None);
    }

    #[test]
    fn read_dir_path_happy_path_round_trip() {
        assert_eq!(
            read_fixture(b"/tmp/plugins-data/demo", None),
            Some(PathBuf::from("/tmp/plugins-data/demo"))
        );
    }

    #[test]
    fn read_dir_path_loses_invalid_utf8_characters_only() {
        // Hosts hand back UTF-8, but a mangled path degrades instead of
        // failing the load.
        assert_eq!(
            read_fixture(&[0xFF, 0xFE], None),
            Some(PathBuf::from("\u{FFFD}\u{FFFD}"))
        );
    }

    #[test]
    fn read_dir_path_rejects_a_zero_second_call() {
        assert_eq!(read_fixture(b"/tmp/x", Some(0)), None);
    }

    #[test]
    fn read_dir_path_rejects_a_second_call_beyond_the_sized_length() {
        assert_eq!(read_fixture(b"/tmp/x", Some(7)), None); // needed = 6
    }

    #[test]
    fn read_dir_path_truncates_to_what_was_written() {
        // written < needed is accepted (not a contract violation): the path
        // is whatever the host actually wrote.
        assert_eq!(
            read_fixture(b"/tmp/plugins", Some(4)),
            Some(PathBuf::from("/tmp"))
        );
    }

    // -- host_call: the generic capability transport -----------------------------

    /// Per-test fixture state carried through the `user` pointer (shared
    /// statics would race across parallel tests): records each request, the
    /// buffers freed, and answers with a canned response.
    struct HostCallFixture {
        calls: Mutex<Vec<(String, String, String)>>,
        freed: Mutex<Vec<usize>>,
        /// `Null` answers with a null buffer; otherwise an ok envelope.
        response: Value,
    }

    extern "C" fn rec_host_call(
        user: *mut c_void,
        capability: *const u8,
        capability_len: usize,
        method: *const u8,
        method_len: usize,
        payload: *const u8,
        payload_len: usize,
    ) -> RawBuffer {
        let fx = unsafe { &*(user as *const HostCallFixture) };
        let capability = unsafe {
            String::from_utf8_lossy(std::slice::from_raw_parts(capability, capability_len))
        };
        let method =
            unsafe { String::from_utf8_lossy(std::slice::from_raw_parts(method, method_len)) };
        let payload = if payload.is_null() || payload_len == 0 {
            "null".to_string()
        } else {
            unsafe { String::from_utf8_lossy(std::slice::from_raw_parts(payload, payload_len)) }
                .into_owned()
        };
        fx.calls
            .lock()
            .unwrap()
            .push((capability.into_owned(), method.into_owned(), payload));
        match &fx.response {
            Value::Null => RawBuffer::NULL, // simulates a host without a response
            value => RawBuffer::from_vec(
                json!({ "ok": true, "value": value })
                    .to_string()
                    .into_bytes(),
            ),
        }
    }

    extern "C" fn rec_free_bridge_buffer(user: *mut c_void, buffer: RawBuffer) {
        if buffer.is_null() {
            return;
        }
        unsafe {
            drop(std::vec::Vec::from_raw_parts(
                buffer.ptr, buffer.len, buffer.cap,
            ));
        }
        // The fixture outlives every call the test makes (reclaimed only
        // after the closure returns), so recording is safe here.
        if !user.is_null() {
            let fx = unsafe { &*(user as *const HostCallFixture) };
            fx.freed.lock().unwrap().push(buffer.len);
        }
    }

    /// Run `f` against a PluginCtx whose host_call is `call_fn`, backed by a
    /// fresh leaked fixture answering with `response` (rec_host_call reads
    /// it; other callbacks may ignore it). Reclaims the fixture afterwards
    /// and returns (result, recorded calls, freed buffer lens).
    fn with_host_call<R>(
        call_fn: extern "C" fn(
            *mut c_void,
            *const u8,
            usize,
            *const u8,
            usize,
            *const u8,
            usize,
        ) -> RawBuffer,
        response: Value,
        f: impl FnOnce(&PluginCtx) -> R,
    ) -> (R, Vec<(String, String, String)>, Vec<usize>) {
        let fx = Box::into_raw(Box::new(HostCallFixture {
            calls: Mutex::new(Vec::new()),
            freed: Mutex::new(Vec::new()),
            response,
        }));
        let bridge = HostBridge {
            user: fx as *mut c_void,
            emit_event: nop_emit,
            log: nop_log,
            data_dir: nop_dir,
            plugin_dir: nop_dir,
            host_call: call_fn,
            free_bridge_buffer: rec_free_bridge_buffer,
        };
        let ctx = PluginCtx::from_bridge(bridge);
        let result = f(&ctx);
        // Snapshot the records while the fixture is still alive, then
        // reclaim it.
        let calls = unsafe { (*fx).calls.lock().unwrap().clone() };
        let freed = unsafe { (*fx).freed.lock().unwrap().clone() };
        unsafe { drop(Box::from_raw(fx)) };
        (result, calls, freed)
    }

    #[test]
    fn host_call_routes_the_request_and_parses_the_value() {
        let (list, calls, freed) = with_host_call(
            rec_host_call,
            json!([{
                "id": "alarms",
                "name": "Alarms",
                "version": "0.3.1",
                "enabled": true
            }]),
            |ctx| ctx.plugins().list().unwrap(),
        );
        assert_eq!(
            list,
            vec![PluginSummary {
                id: "alarms".into(),
                name: "Alarms".into(),
                version: "0.3.1".into(),
                description: String::new(),
                permissions: Vec::new(),
                enabled: true,
                has_frontend: false,
                has_backend: false,
                backend_loaded: false,
                load_error: None,
                frontend_path: None,
                dir: String::new(),
                bundled: false,
                version_check_url: None,
                update_url: None,
            }]
        );
        assert_eq!(
            calls,
            vec![(
                "plugins".to_string(),
                "list".to_string(),
                "null".to_string()
            )]
        );
        // The host-allocated envelope must be freed through the host's
        // free function, exactly once.
        assert_eq!(freed.len(), 1);
    }

    #[test]
    fn host_call_surfaces_forward_capability_method_and_args() {
        let (_, calls, _) = with_host_call(rec_host_call, Value::Null, |ctx| {
            // A null-buffer response reads as a transport failure, so each
            // call errors; only the request side is asserted here.
            let _ = ctx.window().snap_to_anchor(WindowAnchor::BottomRight);
            let _ = ctx.window().set_always_on_top(false);
            let _ = ctx.plugins().set_enabled("mcp", true);
            let _ = ctx.plugins().info(Some("chat"));
            let _ = ctx.shortcuts().register("CmdOrCtrl+Shift+K");
            let _ = ctx.app().version();
        });

        assert_eq!(
            calls,
            vec![
                (
                    "window".into(),
                    "set_position".into(),
                    r#"{"anchor":"bottom-right"}"#.into()
                ),
                ("window".into(), "set_always_on_top".into(), "false".into()),
                (
                    "plugins".into(),
                    "set_enabled".into(),
                    r#"{"enabled":true,"id":"mcp"}"#.into()
                ),
                ("plugins".into(), "info".into(), r#"{"id":"chat"}"#.into()),
                (
                    "shortcuts".into(),
                    "register".into(),
                    r#"{"shortcut":"CmdOrCtrl+Shift+K"}"#.into()
                ),
                ("app".into(), "version".into(), "null".into()),
            ]
        );
    }

    #[test]
    fn host_call_error_envelope_becomes_an_err() {
        extern "C" fn err_host_call(
            _user: *mut c_void,
            _capability: *const u8,
            _capability_len: usize,
            _method: *const u8,
            _method_len: usize,
            _payload: *const u8,
            _payload_len: usize,
        ) -> RawBuffer {
            RawBuffer::from_vec(
                json!({ "ok": false, "error": "permission \"window\" not granted" })
                    .to_string()
                    .into_bytes(),
            )
        }
        // The error path is the same transport: an error envelope, not a
        // panic. The fixture (real, dereferenceable) only feeds the free
        // bookkeeping here.
        let (err, _, _) = with_host_call(err_host_call, Value::Null, |ctx| ctx.window().get_size());
        assert_eq!(err, Err("permission \"window\" not granted".to_string()));
    }

    #[test]
    fn host_call_null_user_and_null_buffer_are_errors() {
        // No host bridge at all: capability calls cannot work.
        let ctx = PluginCtx::from_bridge(bridge_with(std::ptr::null_mut(), true));
        assert_eq!(
            ctx.host_call("plugins", "list", &json!(null)),
            Err("host does not support capability calls".to_string())
        );
        // Host present but answers with a null buffer.
        let (err, _, _) = with_host_call(rec_host_call, Value::Null, |ctx| ctx.app().config());
        assert_eq!(err, Err("host returned a null envelope".to_string()));
    }

    // -- export_plugin!: the full ABI a plugin binary exports ---------------------

    static MODE: AtomicUsize = AtomicUsize::new(0);
    const MODE_OK: usize = 0;
    const MODE_INIT_FAIL: usize = 1;
    static SHUTDOWN_COUNT: AtomicUsize = AtomicUsize::new(0);

    struct MacroTestPlugin;

    impl Plugin for MacroTestPlugin {
        fn init(&mut self, _ctx: PluginCtx) -> Result<(), String> {
            if MODE.load(Ordering::SeqCst) == MODE_INIT_FAIL {
                Err("init exploded".to_string())
            } else {
                Ok(())
            }
        }

        fn handle(&mut self, method: &str, payload: Value) -> Result<Value, String> {
            match method {
                "echo" => Ok(payload),
                "err" => Err("handled error".to_string()),
                "panic" => panic!("boom"),
                _ => Err(format!("unknown method: {method}")),
            }
        }

        fn tools(&mut self) -> Vec<ToolDef> {
            vec![ToolDef {
                name: "t1".into(),
                description: "d1".into(),
                input_schema: json!({}),
            }]
        }

        fn shutdown(&mut self) {
            SHUTDOWN_COUNT.fetch_add(1, Ordering::SeqCst);
        }
    }

    crate::export_plugin!(MacroTestPlugin);

    /// Parse the envelope returned by companion_plugin_handle, freeing the
    /// buffer through the plugin's own free_buffer (allocator-matched).
    fn envelope(instance: *mut c_void, method: &[u8], payload: &[u8]) -> Value {
        let buf = companion_plugin_handle(
            instance,
            method.as_ptr(),
            method.len(),
            payload.as_ptr(),
            payload.len(),
        );
        assert!(!buf.is_null(), "handle always returns a buffer");
        let parsed: Value =
            serde_json::from_slice(unsafe { std::slice::from_raw_parts(buf.ptr, buf.len) })
                .expect("envelope is valid JSON");
        companion_plugin_free_buffer(buf);
        parsed
    }

    /// One sequential test: the macro-generated symbols are process-global,
    /// so all lifecycle behaviors run in a fixed order to avoid races on the
    /// MODE/SHUTDOWN statics.
    #[test]
    fn export_plugin_wires_the_full_abi() {
        // abi_version first — the host checks it before anything else.
        assert_eq!(companion_plugin_abi_version(), ABI_VERSION);

        // A failed init degrades to a FailedPlugin instead of crashing load.
        MODE.store(MODE_INIT_FAIL, Ordering::SeqCst);
        let broken = companion_plugin_create(bridge_with(live_user(), false));
        assert_eq!(
            envelope(broken, b"ping", b"{}"),
            json!({ "ok": false, "error": "init exploded" })
        );
        companion_plugin_destroy(broken);
        assert_eq!(SHUTDOWN_COUNT.load(Ordering::SeqCst), 0); // FailedPlugin: default no-op

        // A healthy instance handles every envelope branch.
        MODE.store(MODE_OK, Ordering::SeqCst);
        let instance = companion_plugin_create(bridge_with(live_user(), false));
        assert!(!instance.is_null());

        assert_eq!(
            envelope(instance, b"echo", br#"{"n":1}"#),
            json!({ "ok": true, "value": { "n": 1 } })
        );
        // Null/empty payload parses as JSON null.
        assert_eq!(
            envelope(instance, b"echo", &[0u8; 0]),
            json!({ "ok": true, "value": null })
        );
        assert_eq!(
            envelope(instance, b"err", b"{}"),
            json!({ "ok": false, "error": "handled error" })
        );
        assert_eq!(
            envelope(instance, b"panic", b"{}"),
            json!({ "ok": false, "error": "plugin panicked while handling a request" })
        );
        assert_eq!(
            envelope(instance, b"nope", b"{}"),
            json!({ "ok": false, "error": "unknown method: nope" })
        );
        // Malformed payload JSON surfaces as an error envelope, not a panic.
        let bad = envelope(instance, b"echo", b"{oops");
        assert_eq!(bad["ok"], false);
        assert!(bad["error"]
            .as_str()
            .unwrap()
            .starts_with("invalid JSON payload:"));

        // tools() serializes the ToolDef list.
        let buf = companion_plugin_tools(instance);
        let tools: Vec<Value> =
            serde_json::from_slice(unsafe { std::slice::from_raw_parts(buf.ptr, buf.len) })
                .unwrap();
        companion_plugin_free_buffer(buf);
        assert_eq!(
            tools,
            vec![json!({"name":"t1","description":"d1","input_schema":{}})]
        );

        // Destroy calls shutdown exactly once; double destroy would be a
        // host bug (the pointer is dangling), so only single destroy here.
        companion_plugin_destroy(instance);
        assert_eq!(SHUTDOWN_COUNT.load(Ordering::SeqCst), 1);

        // Null-instance guards stay panic-free for every entry point.
        assert_eq!(
            envelope(std::ptr::null_mut(), b"echo", b"{}"),
            json!({ "ok": false, "error": "null instance" })
        );
        let tools = companion_plugin_tools(std::ptr::null_mut());
        assert_eq!(
            serde_json::from_slice::<Vec<Value>>(unsafe {
                std::slice::from_raw_parts(tools.ptr, tools.len)
            })
            .unwrap(),
            Vec::<Value>::new()
        );
        companion_plugin_free_buffer(tools);
        companion_plugin_destroy(std::ptr::null_mut()); // no-op

        // free_buffer on the null buffer is a no-op.
        companion_plugin_free_buffer(RawBuffer::NULL);
    }
}
