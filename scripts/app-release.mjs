import { createHash } from "node:crypto";
import { readFile, stat, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

export function updateArtifacts(version) {
  const prefix = `P2PCLI_${version}_`;
  const entries = {
    "darwin-aarch64": `${prefix}aarch64.app.tar.gz`,
    "darwin-x86_64": `${prefix}x64.app.tar.gz`,
    "windows-x86_64": `${prefix}x64-setup.exe`,
    "windows-aarch64": `${prefix}arm64-setup.exe`,
    "windows-i686": `${prefix}x86-setup.exe`,
  };
  // Updater 2.14 selects OS-arch-installer first. Never send an AppImage to a DEB/RPM installation.
  for (const [arch, filenameArch] of [["x86_64", "x64"], ["aarch64", "arm64"]]) {
    for (const [kind, extension] of [["appimage", "AppImage"], ["deb", "deb"], ["rpm", "rpm"]])
      entries[`linux-${arch}-${kind}`] = `${prefix}${filenameArch}.${extension}`;
  }
  return entries;
}

/** 手机安装包：不走在线更新，只进发布页和 SHA256SUMS。可选：没配签名密钥、打包失败时这次就不带 */
export function mobileArtifacts(version) {
  return [`P2PCLI_${version}_android-arm64.apk`, `P2PCLI_${version}_ios-unsigned.ipa`];
}

export async function prepareRelease({ dir, version, tag, repository, notes = "", date = new Date().toISOString() }) {
  if (!/^\d+\.\d+\.\d+(?:-[\w.-]+)?$/.test(version) || tag !== `v${version}`)
    throw new Error("Release tag must match the configured version");
  if (!/^[\w.-]+\/[\w.-]+$/.test(repository)) throw new Error("Invalid GitHub repository");
  const artifacts = updateArtifacts(version);
  const files = [`P2PCLI_${version}_aarch64.dmg`, `P2PCLI_${version}_x64.dmg`,
    ...Object.values(artifacts).flatMap((file) => [file, `${file}.sig`])];
  for (const file of mobileArtifacts(version)) {
    const info = await stat(path.join(dir, file)).catch(() => null);
    if (info?.isFile() && info.size) files.push(file);
  }
  for (const file of files) {
    const info = await stat(path.join(dir, file));
    if (!info.isFile() || !info.size) throw new Error(`Missing or empty release artifact: ${file}`);
  }
  const platforms = {};
  for (const [platform, file] of Object.entries(artifacts)) {
    const signature = (await readFile(path.join(dir, `${file}.sig`), "utf8")).trim();
    if (!signature) throw new Error(`Empty signature: ${file}`);
    platforms[platform] = { url: `https://github.com/${repository}/releases/download/${tag}/${file}`, signature };
  }
  const manifest = { version, notes, pub_date: date, platforms };
  await writeFile(path.join(dir, "latest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
  const checksums = [];
  for (const file of [...files, "latest.json"].sort()) {
    const hash = createHash("sha256").update(await readFile(path.join(dir, file))).digest("hex");
    checksums.push(`${hash}  ${file}`);
  }
  await writeFile(path.join(dir, "SHA256SUMS"), `${checksums.join("\n")}\n`);
  return manifest;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  const config = JSON.parse(await readFile(new URL("../app/src-tauri/tauri.conf.json", import.meta.url), "utf8"));
  await prepareRelease({ dir: "dist", version: config.version, tag: process.env.GITHUB_REF_NAME,
    repository: process.env.GITHUB_REPOSITORY, notes: process.env.RELEASE_NOTES });
}
