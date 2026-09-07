use super::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const CHILD_MODE: &str = "OPEN_WHY_ABRUPT_CHILD_MODE";
const CHILD_PATH: &str = "OPEN_WHY_ABRUPT_CHILD_PATH";
const CHILD_RECORD: &str = "OPEN_WHY_ABRUPT_CHILD_RECORD";
const COMMITTED: &str = "committed";
const UNCOMMITTED: &str = "uncommitted";
const SCOPE: &str = "durability-contract";
const PROVIDER: &str = "provider:abrupt-process";

type MutableSnapshot = (f64, i64, i64, Option<String>);

fn mutable_snapshot(path: &Path, id: &str) -> MutableSnapshot {
    Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT effectiveness,times_helpful,accessed_count,updated_at
             FROM decisions WHERE id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap()
}

fn child_path() -> PathBuf {
    PathBuf::from(std::env::var_os(CHILD_PATH).expect("child database path"))
}

fn await_parent_abort() {
    std::io::stdout().flush().unwrap();
    let mut command = [0_u8; 1];
    std::io::stdin().read_exact(&mut command).unwrap();
    assert_eq!(command, [b'X']);
    std::process::abort();
}

#[test]
fn abrupt_process_child() {
    let Ok(mode) = std::env::var(CHILD_MODE) else {
        return;
    };
    let path = child_path();
    match mode.as_str() {
        COMMITTED => {
            let store = Store::open_with_store_instance_id(&path, PROVIDER).unwrap();
            let id = store
                .capture(&capture_decision("Committed before abort"), SCOPE, None)
                .unwrap();
            println!("READY:{id}");
            await_parent_abort();
        }
        UNCOMMITTED => {
            let id = std::env::var(CHILD_RECORD).unwrap();
            let connection = Connection::open(&path).unwrap();
            connection.execute_batch("BEGIN IMMEDIATE").unwrap();
            assert_eq!(
                connection
                    .execute(
                        "UPDATE decisions
                         SET effectiveness=0.01, times_helpful=99, accessed_count=77,
                             updated_at='2099-01-01T00:00:00Z'
                         WHERE id=?1",
                        [&id],
                    )
                    .unwrap(),
                1
            );
            println!("READY");
            await_parent_abort();
        }
        other => panic!("unsupported abrupt-process child mode: {other}"),
    }
}

fn stop_stuck_child(child: &mut Child, message: &str) -> ! {
    let _ = child.kill();
    let _ = child.wait();
    panic!("{message}");
}

fn run_abrupt_child(mode: &str, path: &Path, record_id: Option<&str>) -> String {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "abrupt_process::abrupt_process_child",
            "--nocapture",
        ])
        .env(CHILD_MODE, mode)
        .env(CHILD_PATH, path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(record_id) = record_id {
        command.env(CHILD_RECORD, record_id);
    }
    let mut child = command.spawn().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut observed = 0_usize;
        for _ in 0..128 {
            let mut line = String::new();
            let Ok(bytes) = reader.read_line(&mut line) else {
                break;
            };
            if bytes == 0 {
                break;
            }
            observed += bytes;
            if observed > 64 * 1024 {
                break;
            }
            if line == "READY\n" || line.starts_with("READY:") {
                let _ = sender.send(Some(line));
                return;
            }
        }
        let _ = sender.send(None);
    });
    let ready = match receiver.recv_timeout(Duration::from_secs(10)) {
        Ok(Some(line)) => line,
        Ok(None) => stop_stuck_child(&mut child, "child did not return a readiness message"),
        Err(_) => stop_stuck_child(&mut child, "child did not become ready within 10 seconds"),
    };
    child.stdin.take().unwrap().write_all(b"X").unwrap();
    let status = child.wait().unwrap();
    assert!(!status.success(), "abrupt child exited successfully");
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert!(
            status.signal().is_some(),
            "abrupt child was not terminated by a signal"
        );
    }
    ready.trim_end().to_owned()
}

#[test]
fn committed_capture_and_evidence_survive_abrupt_process_termination() {
    let dir = temp_dir("committed-abrupt-process");
    let path = dir.join("store.db");
    let ready = run_abrupt_child(COMMITTED, &path, None);
    let id = ready.strip_prefix("READY:").unwrap();

    let reopened = Store::open(&path).unwrap();
    let record = reopened.get_record(id).unwrap().unwrap();
    assert_eq!(record.id, id);
    assert_eq!(record.scope, SCOPE);
    assert_eq!(record.title, "Committed before abort");
    assert_eq!(record.content, "body for Committed before abort");
    assert_eq!(record.kind, "decision");
    assert_eq!(record.source, "cycle-test");
    assert_eq!(record.importance, 0.5);
    let evidence = identity(&reopened, id, SCOPE);
    assert_eq!(evidence.record_id, id);
    assert_eq!(evidence.scope, SCOPE);
    assert_eq!(evidence.store_instance_id, PROVIDER);
    assert_eq!(evidence.contract, EVIDENCE_IDENTITY_CONTRACT);
    assert_eq!(evidence.record_digest_contract, RECORD_DIGEST_CONTRACT);
    assert_eq!(evidence.record_digest.len(), 64);
    assert!(evidence
        .record_digest
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(
        reopened.store_identity().unwrap().store_instance_id,
        PROVIDER
    );

    drop(reopened);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn uncommitted_multi_column_mutation_is_absent_after_abrupt_termination() {
    let dir = temp_dir("uncommitted-abrupt-process");
    let path = dir.join("store.db");
    let store = Store::open_with_store_instance_id(&path, PROVIDER).unwrap();
    let id = store
        .capture(&capture_decision("Original before abort"), SCOPE, None)
        .unwrap();
    drop(store);
    let before = full_capture_snapshot(&path);
    let mutable_before = mutable_snapshot(&path, &id);

    assert_eq!(run_abrupt_child(UNCOMMITTED, &path, Some(&id)), "READY");

    let reopened = Store::open(&path).unwrap();
    assert_eq!(
        reopened.store_identity().unwrap().store_instance_id,
        PROVIDER
    );
    assert_eq!(
        reopened.get_record(&id).unwrap().unwrap().title,
        "Original before abort"
    );
    drop(reopened);
    assert_eq!(full_capture_snapshot(&path), before);
    assert_eq!(mutable_snapshot(&path, &id), mutable_before);

    std::fs::remove_dir_all(dir).unwrap();
}
