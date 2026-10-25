// 编命令行工具 p2pcli，放到 Tauri 打包要的位置 app/src-tauri/binaries/p2pcli-<目标>，随安装包一起发。
// 打包时 Tauri 会给出目标（TAURI_ENV_TARGET_TRIPLE）和是不是调试版；直接跑就按本机编调试版。
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const binaries = path.join(root, "app/src-tauri/binaries");

function run(cmd, args) {
  const r = spawnSync(cmd, args, { cwd: root, stdio: ["ignore", "inherit", "inherit"], windowsHide: true });
  if (r.status !== 0) throw new Error(`${cmd} ${args.join(" ")} 失败`);
}

function hostTriple() {
  const r = spawnSync("rustc", ["-vV"], { encoding: "utf8" });
  const line = (r.stdout ?? "").split("\n").find((l) => l.startsWith("host: "));
  if (!line) throw new Error("读不到 Rust 的本机目标，装好 Rust 了吗？");
  return line.slice(6).trim();
}

/** 编一个目标的 p2pcli，返回编出来的文件 */
function build(triple, release) {
  run("cargo", ["build", "-p", "p2pcli-cli", "--target", triple, ...(release ? ["--release"] : [])]);
  const exe = triple.includes("windows") ? "p2pcli.exe" : "p2pcli";
  return path.join(root, "target", triple, release ? "release" : "debug", exe);
}

/** release 不传时：在 Tauri 打包里跑（有 TAURI_ENV_PLATFORM）就看是不是调试版，直接跑就编调试版 */
export function main({ release } = {}) {
  const triple = process.env.TAURI_ENV_TARGET_TRIPLE || hostTriple();
  release ??= process.env.TAURI_ENV_PLATFORM ? process.env.TAURI_ENV_DEBUG !== "true" : false;
  console.log(`编命令行工具：${triple}，${release ? "正式版" : "调试版"}`);
  mkdirSync(binaries, { recursive: true });
  const suffix = triple.includes("windows") ? ".exe" : "";
  const out = path.join(binaries, `p2pcli-${triple}${suffix}`);
  if (triple === "universal-apple-darwin") {
    // 双架构包：两个架构各编一份再合起来
    const parts = ["aarch64-apple-darwin", "x86_64-apple-darwin"].map((t) => build(t, release));
    run("lipo", ["-create", "-output", out, ...parts]);
  } else {
    copyFileSync(build(triple, release), out);
  }
  console.log(`命令行工具已放到 ${path.relative(root, out)}`);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main();
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
