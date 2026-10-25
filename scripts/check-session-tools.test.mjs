import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { selectTarget, validatePE, validateMachO, verifyHash, validateUniversal } from "./check-session-tools.mjs";

test("all committed binaries validate offline without compilers or download tools on PATH", () => {
  const script = new URL("check-session-tools.mjs", import.meta.url);
  const result = spawnSync(process.execPath, [fileURLToPath(script), "--all"], {
    env: { ...process.env, PATH: "", HTTP_PROXY: "http://127.0.0.1:1", HTTPS_PROXY: "http://127.0.0.1:1" },
    encoding: "utf8", windowsHide: true,
  });
  assert.equal(result.status, 0, result.stderr || result.error?.message);
});

test("universal binary contains unchanged official slices and rejects corruption", async () => {
  const manifest = JSON.parse(await readFile(new URL("session-tools.json", import.meta.url), "utf8"));
  const bytes = await readFile(new URL("../app/src-tauri/binaries/tmux-universal-apple-darwin", import.meta.url));
  validateUniversal(bytes, manifest.macos.binaries);
  const corrupt = Buffer.from(bytes);
  corrupt[corrupt.length - 1] ^= 1;
  assert.throws(() => validateUniversal(corrupt, manifest.macos.binaries), /SHA-256/);
  const invalid = Buffer.from(bytes);
  invalid.writeUInt32BE(bytes.length, 16);
  assert.throws(() => validateUniversal(invalid, manifest.macos.binaries), /范围/);
});

test("select only the requested OS and CPU, including cross-packaging targets", () => {
  for (const arch of ["x86_64", "aarch64", "i686"])
    assert.deepEqual(selectTarget(`${arch}-pc-windows-msvc`), {
      triple: `${arch}-pc-windows-msvc`, platform: "windows", arch, tool: "psmux", suffix: ".exe",
    });
  for (const arch of ["x86_64", "aarch64", "universal"])
    assert.equal(selectTarget(`${arch}-apple-darwin`).tool, "tmux");
  assert.throws(() => selectTarget("unknown"), /不支持/);
  assert.throws(() => selectTarget("x86_64-unknown-linux-gnu"), /不支持/);
});

test("repository binary corruption fails closed", () => {
  const hash = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
  verifyHash(Buffer.from("abc"), hash);
  assert.throws(() => verifyHash(Buffer.from("abd"), hash), /SHA-256/);
});

test("reject wrong-architecture Windows executables", () => {
  const pe = Buffer.alloc(128);
  pe.write("MZ"); pe.writeUInt32LE(64, 0x3c); pe.writeUInt32LE(0x4550, 64); pe.writeUInt16LE(0x8664, 68);
  validatePE(pe, "x86_64");
  assert.throws(() => validatePE(pe, "aarch64"), /CPU/);
  assert.throws(() => validatePE(Buffer.from("not an executable"), "x86_64"));
});

function macho(minimum, library) {
  const name = Buffer.from(`${library}\0`), size = Math.ceil((24 + name.length) / 8) * 8;
  const bytes = Buffer.alloc(32 + 24 + size);
  bytes.writeUInt32LE(0xfeedfacf, 0); bytes.writeUInt32LE(0x100000c, 4);
  bytes.writeUInt32LE(2, 16); bytes.writeUInt32LE(24 + size, 20);
  bytes.writeUInt32LE(0x32, 32); bytes.writeUInt32LE(24, 36); bytes.writeUInt32LE(1, 40); bytes.writeUInt32LE(minimum << 16, 44);
  bytes.writeUInt32LE(0xc, 56); bytes.writeUInt32LE(size, 60); bytes.writeUInt32LE(24, 64); name.copy(bytes, 80);
  return bytes;
}

test("Mac binaries must support macOS 15 without Homebrew dylibs", () => {
  validateMachO(macho(15, "/usr/lib/libSystem.B.dylib"), "aarch64");
  assert.throws(() => validateMachO(macho(16, "/usr/lib/libSystem.B.dylib"), "aarch64"), /macOS 15/);
  assert.throws(() => validateMachO(macho(15, "/opt/homebrew/lib/libevent.dylib"), "aarch64"), /非系统/);
  assert.throws(() => validateMachO(macho(15, "/usr/lib/libSystem.B.dylib"), "x86_64"), /CPU/);
});

test("Tauri selects one platform's sidecar and license directory", async () => {
  const read = async (file) => JSON.parse(await readFile(new URL(`../app/src-tauri/${file}`, import.meta.url), "utf8"));
  const base = await read("tauri.conf.json");
  assert.equal(base.bundle.externalBin, undefined);
  assert.equal(base.bundle.macOS.minimumSystemVersion, "15.0");
  assert.match(base.build.beforeBuildCommand, /check:session-tools/);
  assert.equal(base.build.beforeDevCommand, "pnpm run dev:web");
  const pkg = JSON.parse(await readFile(new URL("../package.json", import.meta.url), "utf8"));
  assert.equal(pkg.scripts.dev, "node scripts/dev-desktop.mjs");
  for (const [platform, name] of [["windows", "psmux"], ["macos", "tmux"]]) {
    const config = await read(`tauri.${platform}.conf.json`);
    assert.deepEqual(config.bundle.externalBin, [`binaries/${name}`]);
    assert.deepEqual(config.bundle.resources, { [`resources/session-tools/${platform}/`]: "session-tools/" });
  }
});
