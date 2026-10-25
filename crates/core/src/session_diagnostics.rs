//! Read-only evidence collected after a session disappears. Never log PTY data or shell arguments.
use std::time::Duration;
use tokio::process::Command;

pub async fn capture(namespace: &str, last_backends: &[String]) {
    #[cfg(not(windows))]
    for backend in last_backends {
        if let Some((pid, socket)) = backend.split_once('\t') {
            let metadata = tokio::fs::symlink_metadata(socket).await;
            tracing::warn!(%namespace, %pid, %socket, exists = metadata.is_ok(),
                error = ?metadata.err(), "会话消失时的 socket 状态");
        }
    }
    #[cfg(windows)]
    let _ = last_backends;

    #[cfg(windows)]
    let mut command = {
        let mut cmd = Command::new("tasklist.exe");
        cmd.args(["/FI", "IMAGENAME eq psmux.exe", "/FO", "CSV", "/NH"]);
        cmd.creation_flags(0x08000000);
        cmd
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut cmd = Command::new("/bin/ps");
        cmd.args(["-axo", "pid=,ppid=,comm="]);
        cmd
    };
    command.kill_on_drop(true);
    match tokio::time::timeout(Duration::from_secs(5), command.output()).await {
        Ok(Ok(out)) => {
            let processes: Vec<String> = String::from_utf8_lossy(&out.stdout).lines()
                .filter(|line| line.to_lowercase().contains(if cfg!(windows) { "psmux" } else { "tmux" }))
                .take(64).map(|line| line.chars().take(512).collect()).collect();
            tracing::warn!(%namespace, status = %out.status, ?processes, "会话组件进程快照（仅 PID、父进程和程序名）");
        }
        other => tracing::warn!(%namespace, error = ?other, "无法采集会话组件进程快照"),
    }
    #[cfg(target_os = "macos")]
    capture_crashes(namespace).await;
}

#[cfg(target_os = "macos")]
async fn capture_crashes(namespace: &str) {
    use tokio::io::AsyncReadExt;
    let Some(home) = std::env::home_dir() else { return };
    let directory = home.join("Library/Logs/DiagnosticReports");
    let mut entries = match tokio::fs::read_dir(&directory).await {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!(%namespace, %error, "无法读取 tmux 系统崩溃报告目录");
            return;
        }
    };
    let mut reports = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if !entry.file_name().to_string_lossy().starts_with("tmux") { continue; }
        let Ok(metadata) = entry.metadata().await else { continue };
        let Ok(modified) = metadata.modified() else { continue };
        if metadata.is_file() && modified.elapsed().unwrap_or_default() < Duration::from_secs(3 * 86400) {
            reports.push((modified, path));
        }
    }
    reports.sort_by(|a, b| b.0.cmp(&a.0));
    tracing::warn!(%namespace, recent_reports = reports.len(), "最近三天 tmux 崩溃报告索引（需按 PID 和时间关联，不能直接视为本次原因）");
    for (modified, path) in reports.into_iter().take(5) {
        tracing::warn!(%namespace, report = %path.display(), ?modified, "tmux 系统崩溃报告");
        let Ok(file) = tokio::fs::File::open(&path).await else { continue };
        let mut bytes = Vec::new();
        if file.take(2 * 1024 * 1024).read_to_end(&mut bytes).await.is_err() { continue; }
        // .ips is a JSON header followed by a JSON body. Select only process/termination evidence.
        for value in serde_json::Deserializer::from_slice(&bytes).into_iter::<serde_json::Value>().flatten() {
            let mut summary = serde_json::Map::new();
            for field in ["timestamp", "captureTime", "procName", "pid", "exception", "termination", "faultingThread"] {
                if let Some(value) = value.get(field) { summary.insert(field.into(), value.clone()); }
            }
            if !summary.is_empty() {
                tracing::warn!(%namespace, report = %path.display(), evidence = %serde_json::Value::Object(summary), "tmux 崩溃原因摘要");
            }
        }
    }
}
