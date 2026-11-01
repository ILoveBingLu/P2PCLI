// Exercise the bundled multiplexer only; do not build or launch P2PCLI.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const windows = process.platform === "win32";
assert(windows || process.platform === "darwin");
const arch = { x64: "x86_64", arm64: "aarch64", ia32: "i686" }[process.arch];
const triple = `${arch}-${windows ? "pc-windows-msvc" : "apple-darwin"}`;
const scratch = mkdtempSync(path.join(os.tmpdir(), "p2pcli-agent-input-"));
const program = path.join(scratch, windows ? "psmux.exe" : "tmux");
copyFileSync(path.join(root, "app/src-tauri/binaries", `${windows ? "psmux" : "tmux"}-${triple}${windows ? ".exe" : ""}`), program);
if (!windows) chmodSync(program, 0o755);
const config = path.join(scratch, "empty.conf");
const capture = path.join(scratch, "received.json");
writeFileSync(config, "");
const env = { ...process.env, TERM: "xterm-256color", P2PCLI_INPUT_CAPTURE: capture };
for (const name of ["TMUX", "PSMUX", "PSMUX_SESSION_NAME"]) delete env[name];
if (windows) env.PSMUX_DATA_DIR = path.join(scratch, "data");
const namespace = path.basename(scratch);
function run(...args) {
  const result = spawnSync(program, ["-L", namespace, "-f", config, ...args], {
    cwd: scratch, env, encoding: "utf8", windowsHide: true, timeout: 15000,
  });
  if (result.error) throw result.error;
  assert.equal(result.status, 0, `${args[0]}: ${result.stderr}`);
  return result.stdout.replace(/[\r\n]+$/, "");
}
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function eventually(check) {
  for (let attempt = 0; ; attempt++) {
    try { return check(); }
    catch (error) { if (attempt >= 30) throw error; await wait(200); }
  }
}
const source = readFileSync(path.join(root, "crates/core/src/sessions.rs"), "utf8");
const format = JSON.parse(source.match(/const AGENT_INPUT_FORMAT: &str = ("[^"\r\n]*");/)[1]);
const fixture = path.join(root, "scripts/fixtures/agent-input.mjs");
try {
  run("new-session", "-d", "-s", "check", "-x", "100", "-y", "30", `"${process.execPath}" "${fixture}"`);
  run("set-option", "-t", "check", "destroy-unattached", "off");
  run("set-option", "-t", "check", "status", "off");
  const pane = await eventually(() => {
    const fields = run("display-message", "-p", "-t", "check", format).split("\t");
    assert.equal(fields.length, 9);
    assert(Number(fields[0]) > 0);
    assert.equal(fields[3], "0");
    if (!windows) assert.equal(fields[4], "1");
    assert.match(fields[6].toLowerCase(), /node/);
    assert.match(fields[7], /^(0|off)$/);
    const target = `check:${fields[1]}.${fields[2]}`;
    const screen = run("capture-pane", "-p", "-t", target);
    assert.equal(screen.split(/\r?\n/)[Number(fields[5])].trim(), ">");
    return target;
  });
  const otherCapture = path.join(scratch, "other.json");
  run("new-session", "-d", "-s", "other", "-x", "100", "-y", "30", `"${process.execPath}" "${fixture}" "${otherCapture}"`);
  await eventually(() => assert.match(run("capture-pane", "-p", "-t", "other"), />/));
  const text = "onboarding \u8fde\u63a5\u534f\u4f5c\nliteral $(not-a-command) --flag";
  if (windows) run("send-paste", "-t", pane, Buffer.from(text).toString("base64"));
  else {
    run("set-buffer", "-b", "onboarding-check", "--", text);
    run("paste-buffer", "-p", "-d", "-b", "onboarding-check", "-t", pane);
  }
  await wait(80);
  run("send-keys", "-t", pane, "Enter");
  await eventually(() => {
    const received = JSON.parse(readFileSync(capture, "utf8"));
    assert(received.startsWith("\x1b[200~"), JSON.stringify(received));
    assert(received.endsWith("\x1b[201~\r"), JSON.stringify(received));
    assert.equal(received.slice(6, -7).replaceAll("\r", "\n"), text);
  });
  assert(!existsSync(otherCapture), "Input leaked to the other session");
  console.log("PASS: bracketed input, Unicode, session isolation, exact pane target, and separate Enter");
  console.log(`Diagnostics: ${scratch}`);
} finally {
  // This namespace and data directory belong only to this test.
  run("kill-server");
}
