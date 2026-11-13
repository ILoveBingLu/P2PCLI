import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { mobileArtifacts, prepareRelease, updateArtifacts } from "./app-release.mjs";

const version = "0.1.5";
const options = { version, tag: `v${version}`, repository: "ILoveBingLu/P2PCLI", notes: "Linux release\n\"notes\"" };

async function fixture(t) {
  const dir = await mkdtemp(path.join(os.tmpdir(), "p2pcli-release-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const files = [`P2PCLI_${version}_aarch64.dmg`, `P2PCLI_${version}_x64.dmg`,
    ...Object.values(updateArtifacts(version)).flatMap((file) => [file, `${file}.sig`]), ...mobileArtifacts(version)];
  for (const file of files) await writeFile(path.join(dir, file), `fixture:${file}`);
  return dir;
}

test("all seven architectures are covered without mixing Linux package formats", () => {
  const artifacts = updateArtifacts(version);
  assert.equal(Object.keys(artifacts).length, 11);
  assert.equal(new Set(Object.values(artifacts)).size, 11);
  for (const [arch, fileArch] of [["x86_64", "x64"], ["aarch64", "arm64"]]) {
    assert.equal(artifacts[`linux-${arch}`], undefined);
    for (const [kind, ext] of [["appimage", "AppImage"], ["deb", "deb"], ["rpm", "rpm"]])
      assert.equal(artifacts[`linux-${arch}-${kind}`], `P2PCLI_${version}_${fileArch}.${ext}`);
  }
  assert.equal(artifacts["windows-i686"], `P2PCLI_${version}_x86-setup.exe`);
  assert.equal(artifacts["darwin-aarch64"], `P2PCLI_${version}_aarch64.app.tar.gz`);
});

test("release manifest embeds signatures and hashes every published artifact", async (t) => {
  const dir = await fixture(t);
  const result = await prepareRelease({ ...options, dir, date: "2026-11-01T00:00:00Z" });
  assert.equal(result.notes, options.notes);
  assert.deepEqual(JSON.parse(await readFile(path.join(dir, "latest.json"), "utf8")), result);
  for (const [platform, file] of Object.entries(updateArtifacts(version))) {
    assert.equal(result.platforms[platform].signature, `fixture:${file}.sig`);
    assert.equal(new URL(result.platforms[platform].url).pathname, `/ILoveBingLu/P2PCLI/releases/download/v${version}/${file}`);
  }
  const lines = (await readFile(path.join(dir, "SHA256SUMS"), "utf8")).trim().split("\n");
  assert.equal(lines.length, 27);
  for (const file of mobileArtifacts(version)) assert.ok(lines.some((line) => line.endsWith(`  ${file}`)));
  for (const line of lines) {
    const [hash, file] = line.split("  ");
    assert.equal(hash, createHash("sha256").update(await readFile(path.join(dir, file))).digest("hex"));
  }
});

test("a missing or empty phone package prevents publication", async (t) => {
  const dir = await fixture(t);
  const [apk, ipa] = mobileArtifacts(version);
  await rm(path.join(dir, apk));
  await assert.rejects(prepareRelease({ ...options, dir }), /ENOENT/);
  await writeFile(path.join(dir, apk), "apk");
  await writeFile(path.join(dir, ipa), "");
  await assert.rejects(prepareRelease({ ...options, dir }), /Missing or empty release artifact/);
  await assert.rejects(readFile(path.join(dir, "latest.json")), /ENOENT/);
});

test("a missing Linux package or signature prevents publication", async (t) => {
  const dir = await fixture(t);
  const file = path.join(dir, `P2PCLI_${version}_arm64.rpm.sig`);
  await rm(file);
  await assert.rejects(prepareRelease({ ...options, dir }), /ENOENT/);
  await writeFile(file, " \n");
  await assert.rejects(prepareRelease({ ...options, dir }), /Empty signature/);
  await writeFile(file, "signature");
  await rm(path.join(dir, `P2PCLI_${version}_x64.deb`));
  await assert.rejects(prepareRelease({ ...options, dir }), /ENOENT/);
  await assert.rejects(readFile(path.join(dir, "latest.json")), /ENOENT/);
});

test("release metadata rejects an unrelated tag or repository", async () => {
  await assert.rejects(prepareRelease({ ...options, dir: ".", tag: "v0.1.4" }), /tag/);
  await assert.rejects(prepareRelease({ ...options, dir: ".", repository: "../other/path" }), /repository/);
});
