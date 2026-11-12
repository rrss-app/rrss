//! Tauri MCP Bridge — **development only**.
//!
//! Enabled with Cargo feature `mcp-debug` and only registered under `debug_assertions`
//! (typical `cargo run` / `dev` profile). Release builds never attach the plugin, even if
//! the feature is accidentally enabled.
//!
//! The bridge exposes WebSocket control (default port **9223**) and can invoke IPC commands
//! and execute JS in the webview. Bind address is locked to loopback — not `0.0.0.0`.

/// WebSocket bind host (loopback only).
#[cfg(all(feature = "mcp-debug", debug_assertions))]
pub const MCP_DEBUG_WS_HOST: &str = "127.0.0.1";

/// First port tried by the plugin; scans up to 100 ports on `MCP_DEBUG_WS_HOST` if busy.
#[cfg(all(feature = "mcp-debug", debug_assertions))]
pub const MCP_DEBUG_WS_BASE_PORT: u16 = 9223;

#[cfg(all(feature = "mcp-debug", debug_assertions))]
pub fn attach<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    use tauri_plugin_mcp_bridge::Builder as McpBridgeBuilder;

    builder.plugin(
        McpBridgeBuilder::new()
            .bind_address(MCP_DEBUG_WS_HOST)
            .base_port(MCP_DEBUG_WS_BASE_PORT)
            .build(),
    )
}

#[cfg(not(all(feature = "mcp-debug", debug_assertions)))]
pub fn attach<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder
}
