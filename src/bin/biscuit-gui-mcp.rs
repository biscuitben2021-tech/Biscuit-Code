//! Standalone MCP stdio server exposing the macOS GUI testing driver. Installed
//! into Biscuits via `/install biscuit-gui`, or runnable directly by any MCP
//! client.
fn main() -> anyhow::Result<()> {
    biscuits::gui::serve_stdio()
}
