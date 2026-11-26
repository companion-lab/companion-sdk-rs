# companion-plugin-sdk

Rust backend SDK for [companion-tauri](https://github.com/companion-lab) plugins.

A plugin backend is a `cdylib` that implements the [`Plugin`] trait and exports
the C ABI through the `export_plugin!` macro. The companion host loads the
library, checks the ABI version, and routes every `plugin_call` from the
webviews to your `handle` method. All payloads and results are JSON —
everything that crosses the `dylib` boundary is plain `#[repr(C)]` data plus
UTF-8 JSON buffers.

## Quick start

```rust
use companion_plugin_sdk::{export_plugin, Plugin, PluginCtx};
use serde_json::{json, Value};

struct Hello;

impl Plugin for Hello {
    fn init(&mut self, _ctx: PluginCtx) -> Result<(), String> { Ok(()) }

    fn handle(&mut self, method: &str, payload: Value) -> Result<Value, String> {
        match method {
            "ping" => Ok(json!({ "pong": true })),
            _ => Err(format!("unknown method: {method}")),
        }
    }
}

export_plugin!(Hello);
```

`Cargo.toml`:

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
companion-plugin-sdk = "0.1"
serde_json = "1"
```

Ship the produced `lib<name>.dylib` / `<name>.dll` / `lib<name>.so` next to the
plugin's `manifest.json`:

```json
{
  "id": "notes",
  "name": "Notes",
  "version": "0.1.0",
  "description": "Example plugin.",
  "abi": 3,
  "permissions": ["storage", "events"],
  "backend": { "library": "libnotes_backend.dylib" }
}
```

Drop the package into the app's `plugins/` directory (or import it through
**Settings → Plugins → Import plugin…**) and the host discovers it on the next
launch.

## What you get

- **`Plugin` trait** — `init`, `handle`, `tools`, `shutdown`. Tools are merged
  into the chat model's tool list; tool calls arrive as regular `handle`
  requests (conventionally method `"tool"` with `{"name": ..., "args": ...}`).
- **`PluginCtx`** — host services:
  - `data_dir()` — writable per-plugin directory
    (`{app_data}/plugins-data/{id}/`), created before `init`.
  - `plugin_dir()` — read-only package directory (manifest, bundles,
    bundled `assets/`).
  - `emit(event, payload)` — events delivered to every webview, namespaced
    as `plugin:{id}:{event}`.
  - `info` / `warn` / `error` — routed to the host console.
- **`ToolDef`** — tool definition merged into the chat tool list.
- **`export_plugin!`** — exports the C ABI entry points
  (`companion_plugin_abi_version` / `_create` / `_handle` / `_tools` /
  `_destroy` / `_free_buffer`).

## Controlling the host app (ABI 3)

`PluginCtx::host_call` reaches host capabilities; the host enforces one
manifest permission per capability and errors on unknown names:

| Surface | Permission | What it does |
| --- | --- | --- |
| `ctx.plugins()` | `"plugins"` | `list()` all installed plugins with enabled state; `info(…)`; `set_enabled(id, bool)`; `is_enabled(id)` — the calling plugin cannot toggle itself |
| `ctx.window()` | `"window"` | size/position (absolute or corner `snap_to_anchor`), `set_always_on_top`, `set_click_through`, `set_follow_spaces` of the companion window — runtime-only, user prefs win at next startup |
| `ctx.app()` | `"app"` | `config()` — the resolved `tauri.conf.json` as JSON (read-only); `name()`; `version()` |
| `ctx.shortcuts()` | `"shortcuts"` | `register`/`unregister`/`is_registered` global shortcuts; a press emits `plugin:{id}:shortcut` with `{"shortcut": …}` to every webview |
| `ctx.capabilities()` | `"capabilities"` | plugin-to-plugin capabilities — see below |

```rust
impl Plugin for Notes {
    fn init(&mut self, ctx: PluginCtx) -> Result<(), String> {
        if let Ok(cfg) = ctx.app().config() {
            ctx.info(&format!("host app config has {} keys", cfg.as_object().map_or(0, |m| m.len())));
        }
        let _ = ctx.shortcuts().register("CmdOrCtrl+Shift+N");
        Ok(())
    }

    fn handle(&mut self, method: &str, payload: Value) -> Result<Value, String> {
        match method {
            "enabled-plugins" => {
                let list = self.ctx.plugins().list()?;
                Ok(serde_json::to_value(list).unwrap())
            }
            _ => Err(format!("unknown method: {method}")),
        }
    }
}
```

Notes:

- A missing permission surfaces as `Err("permission \"…\" not granted")` on
  the first call, not on construction.
- `ctx.plugins()` calls during backend *startup* (`init` at app boot) return
  an error — the plugin host is not ready until discovery finishes. Window,
  app, and shortcuts calls work from `init` on.
- Window changes are runtime-only: the user's General-tab preferences win at
  the next startup.
- Shortcut presses are delivered to webviews (`plugin:{id}:shortcut`), not
  back into the backend; a plugin reacts from its frontend, which can call
  the backend.

### Plugin-to-plugin capabilities

`ctx.capabilities()` lets backends share functionality directly. Publish a
named capability in `init` (init runs on every plugin enable, so the
registration is refreshed whenever the backend loads); other backends call
it through the host:

```rust
impl Plugin for Encryptor {
    fn init(&mut self, ctx: PluginCtx) -> Result<(), String> {
        // Names are namespaced by this plugin's id — the host enforces the
        // "encryptor." prefix, so a plugin cannot spoof another's capability.
        ctx.capabilities().provide("encryptor.seal")?;
        Ok(())
    }

    fn handle(&mut self, method: &str, payload: Value) -> Result<Value, String> {
        match method {
            // A capability consumer arrives here with the capability,
            // method and payload it was called with.
            "capability_call" => {
                let cap = payload["capability"].as_str().unwrap_or_default();
                let m = payload["method"].as_str().unwrap_or_default();
                match (cap, m) {
                    ("encryptor.seal", "seal") => { /* ... */ }
                    _ => Err(format!("unknown capability method: {cap}/{m}")),
                }
            }
            _ => Err(format!("unknown method: {method}")),
        }
    }
}

// Somewhere else, another backend calls it:
let sealed = ctx.capabilities().call("encryptor.seal", "seal", &json!({ "text": "hi" }))?;
```

Notes:

- Requires the `"capabilities"` manifest permission on **both** sides
  (publisher and caller).
- `provide` and `list` work during startup `init`; `call` needs the plugin
  host to be ready and errors until discovery finishes.
- No capability-call cycles: calling into a plugin that is itself
  mid-`handle` blocks on that backend's mutex (calls are serialized per
  backend) until the host's lock valve fails the call.
- Disabling or uninstalling a plugin drops its published capabilities; its
  init re-provides them on the next enable.
- Frontends consume capabilities too: `ctx.capabilities.consume(name)` in
  the webview sees frontend-published providers, and
  `ctx.capabilities.callCapability(name, method, payload)` reaches
  backend-published ones through the host (see the JS SDK README).

## Concurrency & lifetime

`handle` calls are serialized by the host (one call at a time per backend), so
`&mut self` is safe. Calls run on a blocking-pool thread — a slow call delays
only its own plugin. Backends are loaded eagerly for enabled plugins and never
unloaded while the app runs; a `shutdown` is best-effort at process teardown.
If `init` fails, the host keeps the plugin alive as a `FailedPlugin` that
errors every call instead of crashing.

## ABI versioning

`ABI_VERSION` (currently **3** — v2 added the `data_dir`/`plugin_dir` path
callbacks, v3 added the `host_call` capability bridge) is checked by the host
before anything else; plugins built against a different ABI are rejected at
load time. Bump the crate version whenever `ABI_VERSION` changes and rebuild
your plugin against the matching release.

## License

MIT
