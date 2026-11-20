//! C ABI shared between the companion plugin host (`src-tauri`) and plugins
//! built with this SDK. Everything that crosses a `dylib` boundary is plain
//! `#[repr(C)]` data plus UTF-8 JSON buffers — no Rust types with layout that
//! rustc could change.
//!
//! Safety contract (upheld by the host):
//! - `HostBridge` callbacks are thread-safe and the `user` pointer stays valid
//!   for the whole life of the plugin instance.
//! - Buffers returned by the plugin are freed by calling the plugin's own
//!   `companion_plugin_free_buffer` symbol, keeping allocators matched.
//! - The host checks `companion_plugin_abi_version` before calling anything
//!   else and refuses to load a plugin built against a different ABI.

use std::ffi::c_void;

/// Bump whenever the vtable below changes shape or semantics. Plugins built
/// against a different version are rejected at load time.
///
/// v2: added `data_dir` / `plugin_dir` path callbacks to [`HostBridge`]
/// (writable per-plugin data dir; read-only package dir for assets).
///
/// v3: added `host_call` / `free_bridge_buffer` callbacks to [`HostBridge`] —
/// the backend can invoke host capabilities (plugin management, window
/// control, app config, global shortcuts) gated by manifest permissions.
pub const ABI_VERSION: u32 = 3;

/// A byte buffer handed across the FFI boundary. Ownership transfers: the
/// receiver must free it through the original owner's `free_buffer`.
#[repr(C)]
pub struct RawBuffer {
    pub ptr: *mut u8,
    pub len: usize,
    pub cap: usize,
}

impl RawBuffer {
    pub const NULL: RawBuffer = RawBuffer {
        ptr: std::ptr::null_mut(),
        len: 0,
        cap: 0,
    };

    pub fn from_vec(mut v: Vec<u8>) -> Self {
        let ptr = v.as_mut_ptr();
        let len = v.len();
        let cap = v.capacity();
        std::mem::forget(v);
        RawBuffer { ptr, len, cap }
    }

    pub const fn is_null(&self) -> bool {
        self.ptr.is_null()
    }
}

/// Log levels understood by the host's `log` callback.
pub const LOG_INFO: u32 = 0;
pub const LOG_WARN: u32 = 1;
pub const LOG_ERROR: u32 = 2;

/// Function pointers the host hands to a plugin so it can call back into the
/// app (emit events to windows, write to the host log). Raw pointers keep this
/// `#[repr(C)]`; the host guarantees thread safety of the callbacks.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HostBridge {
    pub user: *mut c_void,
    /// Emit a Tauri event named `event` with a JSON `payload` to all windows.
    pub emit_event: extern "C" fn(
        user: *mut c_void,
        event: *const u8,
        event_len: usize,
        payload: *const u8,
        payload_len: usize,
    ),
    pub log: extern "C" fn(user: *mut c_void, level: u32, msg: *const u8, msg_len: usize),
    /// v2: Write the plugin's writable data directory (UTF-8 path, no NUL)
    /// into `out`. Returns the number of bytes the path needs. When `out` is
    /// null or `out_cap` is smaller than that, nothing is copied — call again
    /// with a big enough buffer. Returns 0 if the host has no data dir.
    pub data_dir: extern "C" fn(user: *mut c_void, out: *mut u8, out_cap: usize) -> usize,
    /// v2: Same protocol as `data_dir`, but for the read-only plugin package
    /// directory (holds `manifest.json`, the frontend bundle, the backend
    /// library, and any bundled `assets/`).
    pub plugin_dir: extern "C" fn(user: *mut c_void, out: *mut u8, out_cap: usize) -> usize,
    /// v3: Invoke a host capability method. `capability` and `method` are
    /// UTF-8 (no NUL), `payload` is a UTF-8 JSON document (null/empty = no
    /// args). The host returns a JSON envelope
    /// `{"ok":true,"value":...}` / `{"ok":false,"error":"..."}` allocated by
    /// the host — free it with [`HostBridge::free_bridge_buffer`]. The host
    /// gates every capability behind the manifest's `permissions` list and
    /// rejects unknown capability/method names with an error envelope.
    pub host_call: extern "C" fn(
        user: *mut c_void,
        capability: *const u8,
        capability_len: usize,
        method: *const u8,
        method_len: usize,
        payload: *const u8,
        payload_len: usize,
    ) -> RawBuffer,
    /// v3: Free a `RawBuffer` the host previously returned from
    /// `host_call`. The plugin must not free it through its own
    /// `companion_plugin_free_buffer` — the allocation belongs to the host.
    pub free_bridge_buffer: extern "C" fn(user: *mut c_void, buffer: RawBuffer),
}

/// The plugin side of the ABI, produced by `export_plugin!`.
#[repr(C)]
pub struct PluginVTable {
    pub abi_version: extern "C" fn() -> u32,
    /// Build the plugin instance, run `init`, return an opaque handle.
    pub create: extern "C" fn(bridge: HostBridge) -> *mut c_void,
    /// Handle one request. Returns a JSON envelope:
    /// `{"ok":true,"value":...}` or `{"ok":false,"error":"..."}`.
    pub handle: extern "C" fn(
        instance: *mut c_void,
        method: *const u8,
        method_len: usize,
        payload: *const u8,
        payload_len: usize,
    ) -> RawBuffer,
    /// JSON array of `ToolDef`s this plugin exposes to the chat model.
    pub tools: extern "C" fn(instance: *mut c_void) -> RawBuffer,
    /// Drop the instance. Host must not use the instance afterwards.
    pub destroy: extern "C" fn(instance: *mut c_void),
    /// Free a buffer the plugin previously returned.
    pub free_buffer: extern "C" fn(buffer: RawBuffer),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abi_version_is_3() {
        assert_eq!(ABI_VERSION, 3);
    }

    #[test]
    fn log_levels_are_distinct_and_ordered() {
        assert_eq!(LOG_INFO, 0);
        assert_eq!(LOG_WARN, 1);
        assert_eq!(LOG_ERROR, 2);
    }

    #[test]
    fn null_buffer_is_null() {
        assert!(RawBuffer::NULL.is_null());
        assert_eq!(RawBuffer::NULL.len, 0);
        assert_eq!(RawBuffer::NULL.cap, 0);
    }

    #[test]
    fn from_vec_preserves_contents_and_can_be_reclaimed() {
        // Ownership transfers to the buffer; the receiver frees it through
        // the plugin's free_buffer, which rebuilds the Vec exactly the way
        // done here. Round-tripping proves ptr/len/cap were captured
        // faithfully and the layout is what the FFI side expects.
        let v = b"hello world".to_vec();
        let buf = RawBuffer::from_vec(v);
        assert!(!buf.is_null());
        assert_eq!(buf.len, 11);
        assert!(buf.cap >= 11);
        let reclaimed = unsafe { Vec::from_raw_parts(buf.ptr, buf.len, buf.cap) };
        assert_eq!(reclaimed, b"hello world");
    }
}
