// Packaging-time preparation only. Installed apps never download or install tools.
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { chmod, copyFile, mkdir, mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const manifest = JSON.parse(await readFile(new URL("session-tools.json", import.meta.url), "utf8"));
const cache = path.join(root, ".cache/session-tools");
const binaries = path.join(root, "app/src-tauri/binaries");
const resources = path.join(root, "app/src-tauri/resources/session-tools");
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
const recipe = sha256(Buffer.concat([await readFile(fileURLToPath(import.meta.url)), Buffer.from(JSON.stringify(manifest))]));

export function selectTarget(triple) {
  const windows = /^(x86_64|aarch64|i686)-pc-windows-msvc$/.exec(triple);
  if (windows) return { triple, platform: "windows", arch: windows[1], tool: "psmux", suffix: ".exe" };
  const mac = /^(x86_64|aarch64|universal)-apple-darwin$/.exec(triple);
  if (mac) return { triple, platform: "macos", arch: mac[1], tool: "tmux", suffix: "" };
  throw new Error(`暂不支持此桌面打包目标：${triple}。不会混入其他平台的会话组件。`);
}

export function verifyHash(bytes, expected) {
  if (sha256(bytes) !== expected) throw new Error("会话组件 SHA-256 校验失败，拒绝使用下载内容。");
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
  if (minimum == null || minimum > (11 << 16)) throw new Error("tmux 的最低系统版本高于 macOS 11");
  return { minimum, libraries };
}

function run(exe, args, options = {}) {
  const result = spawnSync(exe, args, { cwd: root, windowsHide: true, timeout: 600_000, ...options });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${exe} 失败：${result.stderr?.toString().trim() || result.status}`);
  return result.stdout;
}

async function download(asset) {
  await mkdir(cache, { recursive: true });
  const file = path.join(cache, `${asset.sha256}-${path.basename(new URL(asset.url).pathname)}`);
  try { verifyHash(await readFile(file), asset.sha256); return file; } catch (e) {
    if (e.code !== "ENOENT") throw e; // A corrupt cache must never be trusted.
  }
  const temp = `${file}.${process.pid}.download`;
  console.log(`准备会话组件：${path.basename(new URL(asset.url).pathname)}`);
  try {
    run(process.platform === "win32" ? "curl.exe" : "/usr/bin/curl", [
      "--fail", "--location", "--silent", "--show-error", "--retry", "2", "--connect-timeout", "20",
      "--max-time", "180", "--output", temp, asset.url,
    ], { stdio: "inherit" });
    verifyHash(await readFile(temp), asset.sha256);
    await rename(temp, file);
  } finally { await rm(temp, { force: true }); }
  return file;
}

function member(archive, name) {
  return run("tar", ["-xOf", archive, name], { maxBuffer: 32 * 1024 * 1024 });
}

async function writeChanged(file, bytes, executable = false) {
  await mkdir(path.dirname(file), { recursive: true });
  let same = false;
  try { same = (await readFile(file)).equals(Buffer.from(bytes)); } catch (e) { if (e.code !== "ENOENT") throw e; }
  if (!same) await writeFile(file, bytes);
  if (executable && process.platform !== "win32") await chmod(file, 0o755);
}

async function licenses(platform, archives) {
  const dir = path.join(resources, platform);
  if (platform === "windows") {
    await writeChanged(path.join(dir, "LICENSE.psmux"), member(archives.psmux, "LICENSE"));
  } else {
    for (const [name, source] of Object.entries(manifest.macos.sources))
      await writeChanged(path.join(dir, `LICENSE.${name}`), member(archives[name], `${source.directory}/${source.license}`));
  }
  await writeChanged(path.join(dir, "versions.json"), JSON.stringify(manifest[platform], null, 2) + "\n");
}

async function prepareWindows(target, downloadOnly) {
  const archive = await download(manifest.windows.archives[target.arch]);
  if (downloadOnly) return;
  const bytes = member(archive, "psmux.exe");
  validatePE(bytes, target.arch);
  await writeChanged(path.join(binaries, `psmux-${target.triple}.exe`), bytes, true);
  await licenses("windows", { psmux: archive });
  console.log(`已准备 psmux ${manifest.windows.version} / ${target.arch}（只包含当前目标架构）。`);
}

const shellQuote = (value) => `'${value.replaceAll("'", "'\\''")}'`;

async function buildMacArch(arch, archives) {
  const triple = `${arch}-apple-darwin`;
  const output = path.join(binaries, `tmux-${triple}`);
  const stamp = path.join(cache, `${triple}.json`);
  try {
    const saved = JSON.parse(await readFile(stamp, "utf8")), bytes = await readFile(output);
    if (saved.recipe === recipe && saved.sha256 === sha256(bytes)) {
      validateMachO(bytes, arch);
      await chmod(output, 0o755);
      console.log(`复用 tmux 缓存：${arch}`);
      return output;
    }
  } catch (e) { if (e.code !== "ENOENT") console.log(`重新准备 tmux：${e.message}`); }
  const work = await mkdtemp(path.join(os.tmpdir(), "p2pcli-tmux-"));
  try {
    for (const archive of Object.values(archives)) run("/usr/bin/tar", ["-xzf", archive, "-C", work]);
    const prefix = path.join(work, "prefix");
    const appleArch = arch === "aarch64" ? "arm64" : "x86_64";
    const sdk = run("/usr/bin/xcrun", ["--sdk", "macosx", "--show-sdk-path"]).toString().trim();
    const flags = `-O2 -arch ${appleArch} -mmacosx-version-min=11.0 -isysroot ${shellQuote(sdk)}`;
    const env = {
      ...process.env, PATH: "/usr/bin:/bin:/usr/sbin:/sbin", CC: "/usr/bin/clang", CXX: "/usr/bin/clang++",
      AR: "/usr/bin/ar", RANLIB: "/usr/bin/ranlib", YACC: "/usr/bin/yacc", PKG_CONFIG: "/usr/bin/false",
      MACOSX_DEPLOYMENT_TARGET: "11.0", SDKROOT: sdk,
      // Prevent configure from detecting newer weak-linked APIs merely because
      // the build Mac is newer than the deployment target.
      CFLAGS: flags, CXXFLAGS: flags, CPPFLAGS: "", LDFLAGS: `${flags} -Wl,-no_weak_imports`, LIBS: "",
      PKG_CONFIG_PATH: "", PKG_CONFIG_LIBDIR: "", CPATH: "", LIBRARY_PATH: "",
    };
    const jobs = String(Math.max(1, Math.min(os.cpus().length, 8)));
    const host = `${arch === "aarch64" ? "aarch64" : "x86_64"}-apple-darwin`;
    const buildHost = `${process.arch === "arm64" ? "aarch64" : "x86_64"}-apple-darwin`;
    const configure = (dir, args, buildEnv = env) => run("/bin/sh", ["./configure", `--prefix=${prefix}`, `--build=${buildHost}`, `--host=${host}`, ...args], { cwd: dir, env: buildEnv, stdio: "inherit" });
    const make = (dir, args, buildEnv = env) => run("/usr/bin/make", ["-j", jobs, ...args], { cwd: dir, env: buildEnv, stdio: "inherit" });
    console.log(`准备 macOS 11+ tmux ${manifest.macos.version} / ${arch}（首次准备会编译辅助组件）。`);
    const eventDir = path.join(work, manifest.macos.sources.libevent.directory);
    configure(eventDir, ["--disable-shared", "--enable-static", "--disable-openssl", "--disable-samples", "--disable-libevent-regress"]);
    make(eventDir, []);
    make(eventDir, ["install"]); // Private temporary prefix, never /usr/local or Homebrew.
    const unicodeDir = path.join(work, manifest.macos.sources.utf8proc.directory);
    make(unicodeDir, ["libutf8proc.a"]);
    await mkdir(path.join(prefix, "include"), { recursive: true });
    await copyFile(path.join(unicodeDir, "utf8proc.h"), path.join(prefix, "include/utf8proc.h"));
    await copyFile(path.join(unicodeDir, "libutf8proc.a"), path.join(prefix, "lib/libutf8proc.a"));
    const tmuxDir = path.join(work, manifest.macos.sources.tmux.directory);
    const tmuxEnv = { ...env, CPPFLAGS: `-I${shellQuote(path.join(prefix, "include"))}`, LDFLAGS: `${env.LDFLAGS} -L${shellQuote(path.join(prefix, "lib"))}` };
    configure(tmuxDir, ["--enable-utf8proc", "--disable-jemalloc", "--disable-systemd", "--disable-utempter", "--with-TERM=screen-256color"], tmuxEnv);
    make(tmuxDir, [], tmuxEnv);
    const built = path.join(tmuxDir, "tmux");
    validateMachO(await readFile(built), arch);
    run("/usr/bin/strip", ["-x", built]);
    run("/usr/bin/codesign", ["--force", "--sign", "-", built]);
    const bytes = await readFile(built);
    validateMachO(bytes, arch);
    await writeChanged(output, bytes, true);
    await writeChanged(stamp, JSON.stringify({ recipe, sha256: sha256(bytes) }));
    return output;
  } finally {
    // Only the unique directory created by mkdtemp above is removed.
    await rm(work, { recursive: true, force: true });
  }
}

async function prepareMac(target, downloadOnly) {
  if (!downloadOnly && process.platform !== "darwin") throw new Error("macOS 辅助组件需在 Mac 打包机上准备；Windows 不会下载或构建 Mac 组件。可用 --download-only 单独缓存源码。");
  const archives = {};
  for (const [name, source] of Object.entries(manifest.macos.sources)) archives[name] = await download(source);
  if (downloadOnly) return;
  const arches = target.arch === "universal" ? ["aarch64", "x86_64"] : [target.arch];
  const outputs = [];
  for (const arch of arches) outputs.push(await buildMacArch(arch, archives));
  if (target.arch === "universal") {
    const fat = path.join(binaries, "tmux-universal-apple-darwin");
    run("/usr/bin/lipo", ["-create", ...outputs, "-output", fat]);
    run("/usr/bin/codesign", ["--force", "--sign", "-", fat]);
    await chmod(fat, 0o755);
  }
  await licenses("macos", archives);
  console.log(`已准备 tmux ${manifest.macos.version} / ${target.arch}，非系统依赖已静态链接。`);
}

export async function main(args = process.argv.slice(2)) {
  let triple = process.env.TAURI_ENV_TARGET_TRIPLE || process.env.CARGO_BUILD_TARGET;
  let plan = false, downloadOnly = false;
  for (let i = 0; i < args.length; i++) {
    if (args[i] === "--target") { triple = args[++i]; if (!triple) throw new Error("--target 缺少值"); }
    else if (args[i].startsWith("--target=")) triple = args[i].slice(9);
    else if (args[i] === "--print-plan") plan = true;
    else if (args[i] === "--download-only") downloadOnly = true;
    else throw new Error(`未知参数：${args[i]}`);
  }
  triple ||= run("rustc", ["--print", "host-tuple"]).toString().trim();
  const target = selectTarget(triple);
  if (plan) { console.log(JSON.stringify(target, null, 2)); return; }
  if (target.platform === "windows") await prepareWindows(target, downloadOnly);
  else await prepareMac(target, downloadOnly);
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href)
  main().catch((error) => { console.error(error.message); process.exitCode = 1; });
