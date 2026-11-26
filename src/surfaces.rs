//! Typed host-capability surfaces on [`PluginCtx`].
//!
//! Every surface routes through one generic bridge call
//! (`PluginCtx::host_call`): a capability name, a method, and a JSON
//! payload. The host gates each capability behind a manifest permission
//! (`plugins` → `"plugins"`, `window` → `"window"`, `app` → `"app"`,
//! `shortcuts` → `"shortcuts"`, `capabilities` → `"capabilities"`) and
//! errors otherwise, so these methods return `Err` rather than being absent
//! when the permission is missing.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::PluginCtx;

/// Summary of one installed plugin, mirroring the host's `PluginInfo`
/// (camelCase on the wire). Some fields are host-managed bookkeeping; they
/// are kept optional so older hosts can add fields without breaking parse.
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSummary {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub permissions: Vec<String>,
    pub enabled: bool,
    #[serde(default)]
    pub has_frontend: bool,
    #[serde(default)]
    pub has_backend: bool,
    #[serde(default)]
    pub backend_loaded: bool,
    #[serde(default)]
    pub load_error: Option<String>,
    #[serde(default)]
    pub frontend_path: Option<String>,
    #[serde(default)]
    pub dir: String,
    #[serde(default)]
    pub bundled: bool,
    #[serde(default)]
    pub version_check_url: Option<String>,
    #[serde(default)]
    pub update_url: Option<String>,
}

/// Position or size in logical (CSS) pixels.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LogicalPoint {
    pub x: f64,
    pub y: f64,
}

/// Size in logical (CSS) pixels.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LogicalSize {
    pub width: f64,
    pub height: f64,
}

/// Corner snap presets; `center` centers on the current monitor's work area.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WindowAnchor {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Center,
}

/// Control of the companion (main) window. Runtime-only: the user's
/// General-tab preferences win at the next startup. Requires the `"window"`
/// manifest permission. Every method routes to the main window.
#[derive(Debug, Clone, Copy)]
pub struct WindowSurface<'a> {
    ctx: &'a PluginCtx,
}

impl<'a> WindowSurface<'a> {
    /// Logical size of the companion window.
    pub fn get_size(&self) -> Result<LogicalSize, String> {
        self.ctx
            .host_call("window", "get_size", &Value::Null)
            .and_then(parse_into)
    }

    /// Resize the companion window (logical px; clamped by its min size).
    pub fn set_size(&self, size: LogicalSize) -> Result<(), String> {
        self.ctx
            .host_call(
                "window",
                "set_size",
                &serde_json::to_value(size).map_err(|e| e.to_string())?,
            )
            .map(|_| ())
    }

    /// Logical top-left of the companion window (global coordinates).
    pub fn get_position(&self) -> Result<LogicalPoint, String> {
        self.ctx
            .host_call("window", "get_position", &Value::Null)
            .and_then(parse_into)
    }

    /// Move the companion window to absolute logical coordinates.
    pub fn set_position(&self, pos: LogicalPoint) -> Result<(), String> {
        self.ctx
            .host_call(
                "window",
                "set_position",
                &serde_json::to_value(pos).map_err(|e| e.to_string())?,
            )
            .map(|_| ())
    }

    /// Move the companion window to a preset anchor on the current monitor's
    /// work area (taskbar-aware; `center` centers within it).
    pub fn snap_to_anchor(&self, anchor: WindowAnchor) -> Result<(), String> {
        self.ctx
            .host_call("window", "set_position", &json!({ "anchor": anchor }))
            .map(|_| ())
    }

    /// Keep the companion above other windows.
    pub fn set_always_on_top(&self, on: bool) -> Result<(), String> {
        self.ctx
            .host_call("window", "set_always_on_top", &json!(on))
            .map(|_| ())
    }

    /// Temporarily toggle cursor pass-through over transparent pixels.
    /// Unsupported under Wayland (the input-shape exclusion flow there is
    /// frontend-driven).
    pub fn set_click_through(&self, on: bool) -> Result<(), String> {
        self.ctx
            .host_call("window", "set_click_through", &json!(on))
            .map(|_| ())
    }

    /// Toggle the macOS "follow every Space / fullscreen" behavior (no-op
    /// elsewhere; the pref is accepted so callers need no platform branch).
    pub fn set_follow_spaces(&self, join: bool) -> Result<(), String> {
        self.ctx
            .host_call("window", "set_follow_spaces", &json!(join))
            .map(|_| ())
    }
}

/// The installed companion plugins. Requires the `"plugins"` manifest
/// permission. Cross-tenant guardrail: the host refuses to enable/disable
/// the *calling* plugin at runtime (a disable would strand the caller
/// mid-call; an enable can re-enter `init` while the host holds its state
/// lock).
#[derive(Debug, Clone, Copy)]
pub struct PluginsSurface<'a> {
    ctx: &'a PluginCtx,
}

impl<'a> PluginsSurface<'a> {
    /// All discovered plugins with their enabled state and manifest info.
    pub fn list(&self) -> Result<Vec<PluginSummary>, String> {
        self.ctx
            .host_call("plugins", "list", &Value::Null)
            .and_then(parse_into)
    }

    /// One plugin's summary; `None` for an unknown id (the calling plugin's
    /// own info: `ctx.plugins().info(None)`).
    pub fn info(&self, id: Option<&str>) -> Result<Option<PluginSummary>, String> {
        let payload = match id {
            Some(id) => json!({ "id": id }),
            None => Value::Null,
        };
        self.ctx
            .host_call("plugins", "info", &payload)
            .and_then(parse_into)
    }

    /// Enable or disable another plugin. Refused for the calling plugin.
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), String> {
        self.ctx
            .host_call(
                "plugins",
                "set_enabled",
                &json!({ "id": id, "enabled": enabled }),
            )
            .map(|_| ())
    }

    /// Is this plugin enabled (independent of whether its backend loaded)?
    pub fn is_enabled(&self, id: &str) -> Result<bool, String> {
        self.ctx
            .host_call("plugins", "is_enabled", &json!({ "id": id }))
            .and_then(parse_into)
    }
}

/// Read access to the host app. Requires the `"app"` manifest permission.
#[derive(Debug, Clone, Copy)]
pub struct AppSurface<'a> {
    ctx: &'a PluginCtx,
}

impl<'a> AppSurface<'a> {
    /// The host's resolved Tauri config (`tauri.conf.json` after
    /// platform/build merges) as JSON. Read-only — the config is baked into
    /// the binary at build time; runtime-mutable behavior (windows,
    /// shortcuts) lives on the other surfaces.
    pub fn config(&self) -> Result<Value, String> {
        self.ctx.host_call("app", "config", &Value::Null)
    }

    /// The app's display name (`productName`).
    pub fn name(&self) -> Result<String, String> {
        self.ctx
            .host_call("app", "name", &Value::Null)
            .and_then(parse_into)
    }

    /// The app's version string.
    pub fn version(&self) -> Result<String, String> {
        self.ctx
            .host_call("app", "version", &Value::Null)
            .and_then(parse_into)
    }
}

/// Global shortcuts registered by this backend. Requires the `"shortcuts"`
/// manifest permission. Presses are delivered as host events named
/// `plugin:{id}:shortcut` (see `PluginCtx::emit` docs for namespacing) with
/// a `{"shortcut": "…"}` payload — to all webviews, not back into the
/// backend, so a plugin reacts either from its frontend or by having its
/// frontend call into the backend.
#[derive(Debug, Clone, Copy)]
pub struct ShortcutsSurface<'a> {
    ctx: &'a PluginCtx,
}

impl<'a> ShortcutsSurface<'a> {
    /// Register a global shortcut (`"CmdOrCtrl+Shift+K"` style). Pressing it
    /// emits `plugin:{id}:shortcut`. An id already taken by another
    /// registration (app or another plugin) fails.
    pub fn register(&self, shortcut: &str) -> Result<(), String> {
        self.ctx
            .host_call("shortcuts", "register", &json!({ "shortcut": shortcut }))
            .map(|_| ())
    }

    /// Unregister a previously registered shortcut.
    pub fn unregister(&self, shortcut: &str) -> Result<(), String> {
        self.ctx
            .host_call("shortcuts", "unregister", &json!({ "shortcut": shortcut }))
            .map(|_| ())
    }

    /// Is this shortcut currently registered (by anyone)?
    pub fn is_registered(&self, shortcut: &str) -> Result<bool, String> {
        self.ctx
            .host_call(
                "shortcuts",
                "is_registered",
                &json!({ "shortcut": shortcut }),
            )
            .and_then(parse_into)
    }
}

/// Plugin-to-plugin capabilities. Publish a named capability for other
/// plugins' backends, call one another plugin published, or list what is
/// available. Requires the `"capabilities"` manifest permission.
///
/// Names are namespaced by this plugin's id (`"<plugin-id>.<name>"` — the
/// host enforces the prefix, so a plugin cannot spoof another's capability).
/// A publishing backend answers calls as
/// `handle("capability_call", {"capability", "method", "payload"})`.
#[derive(Debug, Clone, Copy)]
pub struct CapabilitiesSurface<'a> {
    ctx: &'a PluginCtx,
}

impl<'a> CapabilitiesSurface<'a> {
    /// Publish a capability under `"<plugin-id>.<name>"`. Call this from
    /// `init`: init runs on every plugin (re)enable, so the registration is
    /// refreshed whenever the backend loads.
    pub fn provide(&self, name: &str) -> Result<(), String> {
        self.ctx
            .host_call("capabilities", "provide", &json!({ "name": name }))
            .map(|_| ())
    }

    /// Call a method on a capability another plugin's backend published. The
    /// target receives `handle("capability_call", {"capability": name,
    /// "method": method, "payload": payload})`.
    ///
    /// No cycles: calling into a plugin that is itself mid-`handle` blocks
    /// on that backend's mutex (calls are serialized per backend) until the
    /// host's lock valve fails the call.
    pub fn call(&self, name: &str, method: &str, payload: &Value) -> Result<Value, String> {
        self.ctx.host_call(
            "capabilities",
            "call",
            &json!({ "name": name, "method": method, "payload": payload }),
        )
    }

    /// Every backend-published capability: capability name → owning plugin id.
    pub fn list(&self) -> Result<BTreeMap<String, String>, String> {
        self.ctx
            .host_call("capabilities", "list", &Value::Null)
            .and_then(parse_into)
    }
}

impl PluginCtx {
    /// Generic host capability call — the transport under every typed
    /// surface. Prefer the surfaces; this raw form only exists for
    /// capabilities an SDK version predates.
    ///
    /// `capability`/`method` are host-defined (`"plugins"`, `"window"`,
    /// `"app"`, `"shortcuts"`); `payload` is JSON args (`Value::Null` for
    /// none). The host enforces manifest permissions per capability and
    /// returns `Err` for unknown capability/method names.
    pub fn host_call(
        &self,
        capability: &str,
        method: &str,
        payload: &Value,
    ) -> Result<Value, String> {
        self.host_call_inner(capability.as_bytes(), method.as_bytes(), payload)
    }

    /// The typed surfaces. Each is permission-gated by the host; a missing
    /// permission surfaces as `Err` on the first call, not on construction.
    pub fn window(&self) -> WindowSurface<'_> {
        WindowSurface { ctx: self }
    }

    pub fn plugins(&self) -> PluginsSurface<'_> {
        PluginsSurface { ctx: self }
    }

    pub fn app(&self) -> AppSurface<'_> {
        AppSurface { ctx: self }
    }

    pub fn shortcuts(&self) -> ShortcutsSurface<'_> {
        ShortcutsSurface { ctx: self }
    }

    /// Plugin-to-plugin capabilities (see [`CapabilitiesSurface`]).
    pub fn capabilities(&self) -> CapabilitiesSurface<'_> {
        CapabilitiesSurface { ctx: self }
    }

    fn host_call_inner(
        &self,
        capability: &[u8],
        method: &[u8],
        payload: &Value,
    ) -> Result<Value, String> {
        if self.bridge.user.is_null() {
            // No host bridge (unit tests, or a plugin built against an SDK
            // newer than its host): capability calls cannot work.
            return Err("host does not support capability calls".to_string());
        }
        let payload_bytes = payload.to_string().into_bytes();
        let buffer = (self.bridge.host_call)(
            self.bridge.user,
            capability.as_ptr(),
            capability.len(),
            method.as_ptr(),
            method.len(),
            payload_bytes.as_ptr(),
            payload_bytes.len(),
        );
        if buffer.is_null() {
            return Err("host returned a null envelope".to_string());
        }
        // Copy out before freeing: the bytes die with the buffer.
        let bytes = unsafe { std::slice::from_raw_parts(buffer.ptr, buffer.len) }.to_vec();
        (self.bridge.free_bridge_buffer)(self.bridge.user, buffer);
        let envelope: Value = serde_json::from_slice(&bytes)
            .map_err(|e| format!("host returned invalid JSON envelope: {e}"))?;
        if envelope.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            Ok(envelope.get("value").cloned().unwrap_or(Value::Null))
        } else {
            Err(envelope
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown host error")
                .to_string())
        }
    }
}

/// Parse a successful `host_call` result into a typed value.
fn parse_into<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|e| format!("unexpected host response shape: {e}"))
}
