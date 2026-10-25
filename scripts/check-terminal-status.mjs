// Exercise the committed helper only; never build or start the application.
// Run on each host: node scripts/check-terminal-status.mjs
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const windows = process.platform === "win32";
assert(windows || process.platform === "darwin", "Run on Windows or macOS");
const arch = { x64: "x86_64", arm64: "aarch64", ia32: "i686" }[process.arch];
assert(arch, `Unsupported architecture: ${process.arch}`);
const triple = `${arch}-${windows ? "pc-windows-msvc" : "apple-darwin"}`;
const scratch = mkdtempSync(path.join(os.tmpdir(), "p2pcli-status-"));
// Match Tauri's installed filename: psmux rejects renamed server processes.
const program = path.join(scratch, windows ? "psmux.exe" : "tmux");
copyFileSync(path.join(root, "app/src-tauri/binaries", `${windows ? "psmux" : "tmux"}-${triple}${windows ? ".exe" : ""}`), program);
if (!windows) chmodSync(program, 0o755);
const config = path.join(scratch, "empty.conf");
writeFileSync(config, "");
const env = { ...process.env, TERM: "xterm-256color", COLORTERM: "truecolor" };
for (const key of ["TMUX", "PSMUX", "PSMUX_SESSION_NAME"]) delete env[key];
if (windows) env.PSMUX_DATA_DIR = path.join(scratch, "data");
const namespace = path.basename(scratch);
function run(...args) {
  const result = spawnSync(program, ["-L", namespace, "-f", config, ...args], {
    env, cwd: scratch, encoding: "utf8", windowsHide: true, timeout: 15000,
  });
  if (result.error) throw result.error;
  assert.equal(result.status, 0, `${args[0]}: ${result.stderr}`);
  return result.stdout.trim();
}
function hidden() {
  assert.match(run("show-options", "-t", "check", "-v", "status"), /^(off|0)$/);
}
// Use the production format so changing the field order cannot silently break this check.
const source = readFileSync(path.join(root, "crates/core/src/sessions.rs"), "utf8");
const format = JSON.parse(source.match(/const SESSION_FORMAT: &str = ("[^"\r\n]*");/)[1]);
const healthFormat = JSON.parse(source.match(/const HEALTH_FORMAT: &str = ("[^"\r\n]*");/)[1]);
function details() {
  const values = run("list-sessions", "-F", format).split("\t");
  assert(values.length >= 11, `expected 11 fields, got ${values.length}`);
  const [name, created, attached, status, index, window, count, command, cwd, panePid, ...title] = values;
  assert.equal(name, "check");
  assert(Number(created) > 0);
  assert(Number(attached) >= 0);
  assert.match(status, /^(off|0)$/);
  // 认会话里的命令行 AI 要用：会话 shell 的进程号和会话标题
  assert(Number(panePid) > 0, `Invalid pane PID: ${panePid}`);
  assert(!title.join("\t").includes("#{"), "pane_title is not supported");
  return { index: Number(index), window, count: Number(count), command, cwd, panePid: Number(panePid), title: title.join("\t") };
}
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function eventually(check) {
  for (let attempt = 0; ; attempt++) {
    try { return check(); }
    catch (error) { if (attempt === 15) throw error; await delay(200); }
  }
}
try {
  run("new-session", "-d", "-s", "check", "-x", "100", "-y", "30", windows ? "cmd.exe /k" : "/bin/sh");
  run("set-option", "-t", "check", "destroy-unattached", "off");
  run("set-option", "-t", "check", "status", "off");
  hidden();
  const [healthName, backendPid, backendSocket] = run("list-sessions", "-F", healthFormat).split("\t");
  assert.equal(healthName, "check");
  assert(Number(backendPid) > 0, `Invalid backend PID: ${backendPid}`);
  assert(backendSocket && !backendSocket.includes("#{"), `Invalid backend socket: ${backendSocket}`);
  run("has-session", "-t", "check");
  console.log("PASS: read-only health snapshot reports a backend PID and socket for a detached session");
  console.log("Initial:", await eventually(() => {
    const value = details();
    assert(value.command && value.cwd);
    return value;
  }));

  const directory = windows ? process.env.SystemRoot : "/private/tmp";
  run("send-keys", "-t", "check", windows ? `cd /d "${directory}"` : `cd '${directory}'`, "Enter");
  await eventually(() => assert.equal(details().cwd.toLowerCase(), directory.toLowerCase()));
  console.log("PASS: directory updates");

  run("new-window", "-t", "check", "-n", "status-probe", windows ? "powershell.exe -NoLogo -NoProfile" : "/bin/sh");
  console.log("Changed window:", await eventually(() => {
    const value = details();
    assert.equal(value.count, 2);
    assert.equal(value.window, "status-probe");
    if (windows) assert.match(value.command.toLowerCase(), /powershell/);
    return value;
  }));
  hidden();
  run("send-keys", "-t", "check", windows ? "ping.exe -t 127.0.0.1" : "sleep 30", "Enter");
  await eventually(() => assert.match(details().command.toLowerCase(), windows ? /ping/ : /sleep/));
  run("send-keys", "-t", "check", "C-c");
  await eventually(() => assert.match(details().command.toLowerCase(), windows ? /powershell/ : /sh/));
  console.log("PASS: foreground program updates and recovers");
  // 判断 AI 状态要读会话屏幕
  run("send-keys", "-t", "check", windows ? "Write-Output capture-probe-42" : "echo capture-probe-42", "Enter");
  await eventually(() => assert.match(run("capture-pane", "-p", "-t", "check"), /capture-probe-42/));
  console.log("PASS: pane PID, title and screen capture are available for AI detection");
  run("set-option", "-t", "check", "status", "on");
  run("set-option", "-t", "check", "status", "off");
  hidden();
  console.log("PASS: native status stays off across window changes and can be hidden again");
} finally {
  // Only our unique namespace/data directory; never terminate personal sessions.
  run("kill-server");
  console.log(`Test files: ${scratch}`);
}
