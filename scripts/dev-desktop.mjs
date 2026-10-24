// Prepare before starting Tauri, not in its asynchronous Vite hook: an
// already-running dev server must not race Cargo's sidecar validation.
import { createRequire } from "node:module";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { main as prepare } from "./prepare-session-tools.mjs";

const args = process.argv.slice(2);
let target;
for (let i = 0; i < args.length && args[i] !== "--"; i++) {
  if (args[i] === "--target" || args[i] === "-t") target = args[++i];
  else if (args[i].startsWith("--target=")) target = args[i].slice(9);
}
try {
  if (!args.includes("--help") && !args.includes("-h")) await prepare(target ? ["--target", target] : []);
  const cli = createRequire(import.meta.url).resolve("@tauri-apps/cli/tauri.js");
  const child = spawn(process.execPath, [cli, "dev", ...args], {
    cwd: fileURLToPath(new URL("../", import.meta.url)), stdio: "inherit", windowsHide: true,
  });
  for (const signal of ["SIGINT", "SIGTERM"]) process.on(signal, () => child.kill(signal));
  child.on("error", (error) => { console.error(error.message); process.exitCode = 1; });
  child.on("exit", (code) => { process.exitCode = code ?? 1; });
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
