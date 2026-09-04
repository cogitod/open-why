mod args;
mod catalog;
mod common;
mod handlers;
mod protocol;

use anyhow::Result;

pub fn serve() -> Result<()> {
    protocol::serve()
}

/// Long-lived MCP server, independent of any one client's session. Meant to run under a
/// supervisor (e.g. launchd); plain `why serve` connects to it when present.
pub fn serve_daemon() -> Result<()> {
    protocol::serve_daemon()
}

#[cfg(test)]
mod tests;
