use anyhow::{bail, Context, Result};
use clap::Parser;
use open_why::integration::{IntegrationManifest, IntegrationMode};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

const MAX_MANIFEST_BYTES: u64 = 256 * 1024;

#[derive(Parser)]
#[command(about = "Validate an open-why integration manifest and optionally probe MCP")]
struct Args {
    #[arg(value_name = "MANIFEST")]
    manifest: PathBuf,
    #[arg(long, help = "Launch the declared MCP server and verify its contracts")]
    probe: bool,
}

fn main() {
    if let Err(error) = run() {
        println!(
            "{}",
            json!({
                "contract":"open-why.integration-conformance/v1",
                "status":"error",
                "message":error.to_string()
            })
        );
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = Args::parse();
    let file = File::open(&args.manifest)
        .with_context(|| format!("open manifest {}", args.manifest.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("inspect manifest {}", args.manifest.display()))?;
    if !metadata.is_file() {
        bail!("manifest must be a regular file");
    }
    if metadata.len() > MAX_MANIFEST_BYTES {
        bail!("manifest exceeds 262144 bytes");
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read manifest {}", args.manifest.display()))?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        bail!("manifest exceeds 262144 bytes");
    }
    let manifest: IntegrationManifest = serde_json::from_slice(&bytes).context("parse manifest")?;
    manifest.validate().map_err(anyhow::Error::msg)?;
    if args.probe {
        if manifest.mode != IntegrationMode::McpStdio {
            bail!("--probe is available only for mcp-stdio integrations");
        }
        probe_mcp(&manifest, &args.manifest)?;
    }
    println!(
        "{}",
        json!({
            "contract":"open-why.integration-conformance/v1",
            "status":"ok",
            "integration_id":manifest.integration_id,
            "integration_version":manifest.integration_version,
            "mode":manifest.mode,
            "probed":args.probe
        })
    );
    Ok(())
}

struct McpProcess {
    child: Child,
    responses: Receiver<String>,
}

impl Drop for McpProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl McpProcess {
    fn send(&mut self, value: &Value) -> Result<()> {
        let stdin = self.child.stdin.as_mut().context("MCP stdin unavailable")?;
        serde_json::to_writer(&mut *stdin, value)?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        Ok(())
    }

    fn request(&mut self, value: &Value) -> Result<Value> {
        let expected_id = value
            .get("id")
            .cloned()
            .context("MCP request is missing id")?;
        self.send(value)?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .context("MCP response timed out")?;
            let line = self
                .responses
                .recv_timeout(remaining)
                .context("MCP response timed out")?;
            let response: Value = serde_json::from_str(&line).context("parse MCP response")?;
            match response.get("id") {
                None => continue,
                Some(id) if id == &expected_id => return Ok(response),
                Some(id) => bail!("MCP response id {id} does not match request id {expected_id}"),
            }
        }
    }
}

struct ScratchDir(PathBuf);

impl ScratchDir {
    fn create() -> Result<Self> {
        let root = match std::env::var_os("OPEN_WHY_CONFORMANCE_ROOT") {
            Some(root) => PathBuf::from(root),
            None => std::env::current_dir()?.join("target"),
        };
        fs::create_dir_all(&root)?;
        let path = root
            .canonicalize()?
            .join(format!("open-why-conformance-{}", std::process::id()));
        fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn probe_mcp(manifest: &IntegrationManifest, manifest_path: &Path) -> Result<()> {
    let declaration = manifest.mcp.as_ref().context("missing MCP declaration")?;
    // open-why rejects symlinked database ancestors. On macOS the system temp
    // directory is reached through `/var`, which is commonly a symlink, so keep
    // the probe store under the caller's canonical working directory.
    let scratch = ScratchDir::create()?;
    let mut child = Command::new(&declaration.command)
        .args(&declaration.args)
        .env("OPEN_WHY_DB", scratch.0.join("store.sqlite3"))
        .env(
            "OPEN_WHY_STORE_INSTANCE_ID",
            format!("conformance:{}", std::process::id()),
        )
        .current_dir(manifest_path.parent().unwrap_or_else(|| Path::new(".")))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("launch MCP command `{}`", declaration.command))?;
    let stdout = child.stdout.take().context("MCP stdout unavailable")?;
    let (sender, responses) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let mut process = McpProcess { child, responses };
    let initialized = process.request(&json!({
        "jsonrpc":"2.0",
        "id":1,
        "method":"initialize",
        "params":{
            "protocolVersion":declaration.protocol_version,
            "capabilities":{},
            "clientInfo":{"name":"why-integration-check","version":env!("CARGO_PKG_VERSION")}
        }
    }))?;
    if initialized.get("error").is_some() {
        bail!("MCP initialize returned an error");
    }
    let advertised_protocol = initialized
        .pointer("/result/protocolVersion")
        .and_then(Value::as_str);
    if advertised_protocol != Some(declaration.protocol_version.as_str()) {
        bail!("MCP protocol version does not match manifest");
    }
    process.send(&json!({
        "jsonrpc":"2.0","method":"notifications/initialized","params":{}
    }))?;
    let advertised_contracts: HashSet<_> = initialized
        .pointer("/result/capabilities/experimental/openWhy/contracts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if declaration
        .contracts
        .iter()
        .any(|contract| !advertised_contracts.contains(contract.as_str()))
    {
        bail!("MCP server does not advertise every declared contract");
    }
    let listed = process.request(&json!({
        "jsonrpc":"2.0","id":2,"method":"tools/list","params":{}
    }))?;
    let tools: HashSet<_> = listed
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .collect();
    if manifest
        .capabilities
        .iter()
        .any(|capability| !tools.contains(capability.mcp_tool()))
    {
        bail!("MCP server does not expose every declared capability");
    }
    Ok(())
}
