mod args;
mod catalog;
mod common;
mod handlers;
mod protocol;

use anyhow::Result;

pub fn serve() -> Result<()> {
    protocol::serve()
}

#[cfg(test)]
mod tests;
