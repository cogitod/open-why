use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

static SERIAL: AtomicU64 = AtomicU64::new(0);

pub struct Sandbox(pub PathBuf);

impl Sandbox {
    pub fn new() -> Self {
        let path = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "open-why-onboarding-{}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    pub fn isolate(&self, command: &mut Command) {
        command
            .env("HOME", &self.0)
            .env_remove("OPEN_WHY_DB")
            .env_remove("OPEN_WHY_STORE_INSTANCE_ID")
            .env_remove("OPEN_WHY_EMBED_MODEL_PATH")
            .env_remove("OPEN_WHY_EMBED_URL")
            .env_remove("OPEN_WHY_EMBED_API_KEY")
            .env_remove("OPEN_WHY_DEBUG_RANK")
            .env("OPEN_WHY_AUTO_FETCH", "0")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_why"));
        self.isolate(&mut command);
        command
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

pub fn failure(output: &Output, expected: &str) {
    assert!(!output.status.success());
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(text.contains(expected), "expected {expected:?} in {text:?}");
}

pub fn setup(sandbox: &Sandbox, path: &Path, client: &str) -> Output {
    let output = sandbox
        .command()
        .args(["setup", "--db"])
        .arg(path)
        .args(["--client", client])
        .output()
        .unwrap();
    success(&output);
    output
}

pub fn parse_config(bytes: &[u8], client: &str) -> Value {
    match client {
        "codex" => {
            let config: toml::Value = toml::from_str(std::str::from_utf8(bytes).unwrap()).unwrap();
            serde_json::to_value(&config["mcp_servers"]["open-why"]).unwrap()
        }
        "claude-code" => {
            serde_json::from_slice::<Value>(bytes).unwrap()["mcpServers"]["open-why"].clone()
        }
        _ => serde_json::from_slice(bytes).unwrap(),
    }
}

pub fn snapshot(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(path: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                out.insert(path.clone(), Vec::new());
                visit(&path, out);
            } else {
                out.insert(path.clone(), std::fs::read(path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(path, &mut out);
    out
}

pub struct Server {
    child: Child,
    stdin: ChildStdin,
    responses: Receiver<Value>,
    sequence: u64,
}

impl Server {
    pub fn spawn(sandbox: &Sandbox, entry: &Value) -> Self {
        let mut command = Command::new(entry["command"].as_str().unwrap());
        sandbox.isolate(&mut command);
        for argument in entry["args"].as_array().unwrap() {
            command.arg(argument.as_str().unwrap());
        }
        for (key, value) in entry["env"].as_object().unwrap() {
            command.env(key, value.as_str().unwrap());
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut stderr, &mut std::io::sink());
        });
        let (tx, responses) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let Ok(value) = serde_json::from_str(&line) else {
                    break;
                };
                if tx.send(value).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            responses,
            sequence: 0,
        }
    }

    pub fn notify(&mut self, method: &str) {
        writeln!(self.stdin, "{}", json!({"jsonrpc":"2.0","method":method})).unwrap();
        self.stdin.flush().unwrap();
    }

    pub fn request(&mut self, method: &str, params: Value) -> Value {
        self.sequence += 1;
        writeln!(
            self.stdin,
            "{}",
            json!({"jsonrpc":"2.0","id":self.sequence,"method":method,"params":params})
        )
        .unwrap();
        self.stdin.flush().unwrap();
        let response = self
            .responses
            .recv_timeout(Duration::from_secs(10))
            .expect("MCP response within 10 seconds");
        assert_eq!(response["id"], self.sequence);
        assert!(response.get("error").is_none(), "{response}");
        response
    }

    pub fn tool(&mut self, name: &str, arguments: Value) -> Value {
        let response = self.request("tools/call", json!({"name":name,"arguments":arguments}));
        assert_eq!(response["result"]["isError"], false, "{response}");
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
