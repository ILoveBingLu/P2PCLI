// Offline validation of binaries committed to Git. Never downloads or compiles.
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { chmod, readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const manifest = JSON.parse(await readFile(new URL("session-tools.json", import.meta.url), "utf8"));
const binaries = path.join(root, "app/src-tauri/binaries");
const resources = path.join(root, "app/src-tauri/resources/session-tools");
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

export function selectTarget(triple) {
  const windows = /^(x86_64|aarch64|i686)-pc-windows-msvc$/.exec(triple);
  if (windows) return { triple, platform: "windows", arch: windows[1], tool: "psmux", suffix: ".exe" };
  const mac = /^(x86_64|aarch64|universal)-apple-darwin$/.exec(triple);
  if (mac) return { triple, platform: "macos", arch: mac[1], tool: "tmux", suffix: "" };
  throw new Error(`暂不支持此桌面打包目标：${triple}。不会混入其他平台的会话组件。`);
}

export function verifyHash(bytes, expected) {
  if (sha256(bytes) !== expected) throw new Error("会话组件 SHA-256 校验失败，请恢复仓库中的原始文件。");
}

export function validatePE(bytes, arch) {
  if (bytes.length < 64 || bytes.toString("ascii", 0, 2) !== "MZ") throw new Error("不是 Windows 可执行文件");
  const pe = bytes.readUInt32LE(0x3c);
  if (pe + 6 > bytes.length || bytes.readUInt32LE(pe) !== 0x4550) throw new Error("PE 文件头不合法");
  if (bytes.readUInt16LE(pe + 4) !== { x86_64: 0x8664, aarch64: 0xaa64, i686: 0x14c }[arch])
    throw new Error("psmux 的 CPU 架构与打包目标不匹配");
}

export function validateMachO(bytes, arch) {
  if (bytes.length < 32 || bytes.readUInt32LE(0) !== 0xfeedfacf) throw new Error("不是 64 位 Mach-O 文件");
  if (bytes.readUInt32LE(4) !== { x86_64: 0x1000007, aarch64: 0x100000c }[arch])
    throw new Error("tmux 的 CPU 架构与打包目标不匹配");
  let offset = 32, minimum;
  const libraries = [];
  for (let i = 0; i < bytes.readUInt32LE(16); i++) {
    if (offset + 8 > bytes.length) throw new Error("Mach-O load command 越界");
    const cmd = bytes.readUInt32LE(offset), size = bytes.readUInt32LE(offset + 4);
    if (size < 8 || offset + size > bytes.length) throw new Error("Mach-O load command 不合法");
    if (cmd === 0x32 || cmd === 0x24) minimum = bytes.readUInt32LE(offset + (cmd === 0x32 ? 12 : 8));
    if ([0xc, 0x80000018, 0x8000001f, 0x80000023, 0x20].includes(cmd)) {
      const start = offset + bytes.readUInt32LE(offset + 8);
      const name = bytes.toString("utf8", start, offset + size).split("\0")[0];
      if (!name.startsWith("/usr/lib/") && !name.startsWith("/System/Library/Frameworks/"))
        throw new Error(`tmux 仍依赖非系统动态库：${name}`);
      libraries.push(name);
    }
    offset += size;
  }
  if (minimum == null || minimum > (15 << 16)) throw new Error("tmux 的最低系统版本高于 macOS 15");
  return { minimum, libraries };
}

export function validateUniversal(bytes, slices) {
  if (bytes.length < 48 || bytes.readUInt32BE(0) !== 0xcafebabe || bytes.readUInt32BE(4) !== 2)
    throw new Error("不是双架构 Mach-O universal 文件");
  const seen = new Set();
  const ranges = [];
  for (let i = 0; i < 2; i++) {
    const entry = 8 + i * 20, cpu = bytes.readUInt32BE(entry);
    const arch = { [0x100000c]: "aarch64", [0x1000007]: "x86_64" }[cpu];
    const offset = bytes.readUInt32BE(entry + 8), size = bytes.readUInt32BE(entry + 12);
    const alignment = bytes.readUInt32BE(entry + 16);
    if (!arch || seen.has(arch) || offset < 48 || offset + size > bytes.length || alignment > 30 || offset % (2 ** alignment))
      throw new Error("universal 架构或切片范围不合法");
    if (ranges.some(([start, end]) => offset < end && offset + size > start))
      throw new Error("universal 切片重叠");
    ranges.push([offset, offset + size]);
    seen.add(arch);
    const slice = bytes.subarray(offset, offset + size);
    validateMachO(slice, arch);
    verifyHash(slice, slices[`${arch}-apple-darwin`].sha256);
  }
}

async function readRequired(file) {
  try { return await readFile(file); }
  catch (error) {
    if (error.code !== "ENOENT") throw error;
    throw new Error(`仓库会话组件缺失，请从 Git 恢复文件：${file}`);
  }
}

export async function checkTarget(triple) {
  const target = selectTarget(triple), spec = manifest[target.platform];
  const entry = spec.binaries[triple], file = path.join(binaries, entry.file);
  const bytes = await readRequired(file);
  if (bytes.length !== entry.size) throw new Error(`组件大小不匹配，请恢复仓库文件：${file}`);
  verifyHash(bytes, entry.sha256);
  if (target.platform === "windows") validatePE(bytes, target.arch);
  else if (target.arch === "universal") {
    validateUniversal(bytes, spec.binaries);
    // Tauri also builds each thin target before assembling a universal app.
    for (const slice of entry.slices) await checkTarget(slice);
  } else validateMachO(bytes, target.arch);
  const licenseFiles = target.platform === "windows" ? ["LICENSE.psmux"] :
    ["COPYING.tmux", "COPYING.ncurses", "COPYRIGHT.musl", "LICENSE.libevent", "LICENSE.utf8proc"];
  for (const name of licenseFiles) {
    if (!(await readRequired(path.join(resources, target.platform, name))).length)
      throw new Error(`组件许可证为空：${name}`);
  }
  const provenance = JSON.parse(await readRequired(path.join(resources, target.platform, "versions.json")));
  if (JSON.stringify(provenance) !== JSON.stringify(spec)) throw new Error("组件版本记录与清单不一致");
  if (target.platform === "macos" && process.platform === "darwin") await chmod(file, 0o755);
  return target;
}

export async function main(args = process.argv.slice(2)) {
  let triple = process.env.TAURI_ENV_TARGET_TRIPLE || process.env.CARGO_BUILD_TARGET;
  let plan = false, all = false;
  for (let i = 0; i < args.length; i++) {
    if (args[i] === "--target") { triple = args[++i]; if (!triple) throw new Error("--target 缺少值"); }
    else if (args[i].startsWith("--target=")) triple = args[i].slice(9);
    else if (args[i] === "--print-plan") plan = true;
    else if (args[i] === "--all") all = true;
    else throw new Error(`未知参数：${args[i]}`);
  }
  if (!all && !triple) {
    // Match Cargo's host even when Node is running under CPU emulation.
    const host = spawnSync("rustc", ["--print", "host-tuple"], { encoding: "utf8", windowsHide: true, timeout: 10000 });
    if (host.error || host.status !== 0) throw new Error("无法读取 Rust 主机目标，请安装 Rust 或用 --target 指定校验目标");
    triple = host.stdout.trim();
  }
  const targets = all ? Object.values(manifest).flatMap((spec) => Object.keys(spec.binaries || {})) : [triple];
  if (plan) { console.log(JSON.stringify(targets.map(selectTarget), null, 2)); return; }
  for (const target of targets) await checkTarget(target);
  console.log(`本地会话组件校验通过：${targets.join(", ")}（无需下载或编译）。`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href)
  main().catch((error) => { console.error(error.message); process.exitCode = 1; });
