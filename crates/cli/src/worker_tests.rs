use super::*;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("p2pcli-worker-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn test_worker(dir: &Path, program: String) -> Worker {
    Worker {
        config: WorkerConfig {
            coordinator: None,
            project_id: "project".into(),
            credential_file: dir.join("credential.json"),
            cwd: dir.to_owned(),
            program,
            poll_interval_secs: 1,
        },
        cwd: dir.to_owned(),
        target: "local".into(),
        auth: SessionAuth {
            session_id: "session".into(),
            token: "secret".into(),
        },
        data_dir: dir.to_owned(),
        paths: WorkerPaths::new(dir).unwrap(),
    }
}

#[cfg(unix)]
fn mock_program(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join(format!(
        "mock-{}.sh",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
    let mut mode = std::fs::metadata(&path).unwrap().permissions();
    mode.set_mode(0o700);
    std::fs::set_permissions(&path, mode).unwrap();
    path
}

#[cfg(unix)]
#[tokio::test]
async fn real_child_uses_stdin_and_official_json_result() {
    let dir = TempDir::new();
    let program = mock_program(
        dir.path(),
        r#"printf '%s\n' "$@" > args.txt
cat > prompt.txt
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"mock completed","permission_denials":[]}'"#,
    );
    let worker = test_worker(dir.path(), program.to_string_lossy().into_owned());
    let journal = worker.paths.attempts.join("test.json");
    let output = run_process(&worker, &journal, "literal $(touch should-not-run)")
        .await
        .unwrap();
    assert!(output.status.success());
    let result = parse_result(&output).unwrap();
    assert_eq!(result.summary, "mock completed");
    assert!(!result.failed);
    assert!(!dir.path().join("should-not-run").exists());
    assert_eq!(
        std::fs::read(journal.with_extension("stdout.log")).unwrap(),
        output.stdout.bytes
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("args.txt")).unwrap(),
        "-p\nComplete the managed task provided on stdin.\n--output-format\njson\n--permission-mode\ndefault\n--permission-prompts\nnone\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("prompt.txt")).unwrap(),
        "literal $(touch should-not-run)"
    );
}

#[tokio::test]
async fn started_recovery_becomes_unknown_without_rerun() {
    let dir = TempDir::new();
    let worker = test_worker(dir.path(), "must-never-be-spawned".into());
    let paths = &worker.paths;
    let path = paths.attempts.join("attempt.json");
    persist(
        &path,
        &AttemptRecord {
            task_id: "task".into(),
            attempt_id: Some("attempt".into()),
            version: 4,
            status: AttemptStatus::Started,
            result: None,
            pending: None,
            updated_at: 1,
            last_error: None,
            reported: false,
        },
    )
    .unwrap();
    worker.recover_started().await.unwrap();
    let record: AttemptRecord = read_json(&path).unwrap();
    assert_eq!(record.status, AttemptStatus::Unknown);
    assert!(
        record
            .last_error
            .as_deref()
            .unwrap()
            .contains("will not be rerun")
    );
    let recovered = std::fs::read(&path).unwrap();
    worker.recover_started().await.unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), recovered);
    assert!(paths.results.read_dir().unwrap().next().is_none());
}

#[tokio::test]
async fn output_capture_is_bounded_and_drains_input() {
    let dir = TempDir::new();
    let path = dir.path().join("bounded.log");
    let input = vec![b'x'; 8192];
    let captured = capture_output(&input[..], path.clone(), 128).await.unwrap();
    assert!(captured.truncated);
    assert_eq!(captured.bytes, vec![b'x'; 128]);
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 128);
    check_private(&path).unwrap();
}

#[test]
fn result_request_preserves_cas_version_and_operation_id() {
    let request = CollabRequest {
        request_id: "stable-result".into(),
        project_id: Some("project".into()),
        auth: Some(SessionAuth {
            session_id: "session".into(),
            token: "secret".into(),
        }),
        action: Action::TaskResult {
            task_id: "task".into(),
            attempt_id: "attempt".into(),
            result: failed_result("result"),
            expected_version: 12,
        },
    };
    let json = serde_json::to_value(request).unwrap();
    assert_eq!(json["request_id"], "stable-result");
    assert_eq!(json["action"]["expected_version"], 12);
}

#[test]
fn screen_text_is_not_accepted_as_result() {
    #[cfg(unix)]
    let status = {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(0)
    };
    #[cfg(windows)]
    let status = {
        use std::os::windows::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(0)
    };
    let result = parse_result(&ProcessOutcome {
        status,
        stdout: CapturedOutput {
            bytes: b"looks successful".to_vec(),
            truncated: false,
        },
        stderr: CapturedOutput {
            bytes: Vec::new(),
            truncated: false,
        },
        cancelled: false,
    });
    assert!(result.is_err());
}
