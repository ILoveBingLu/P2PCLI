// 把 SVG 渲染成打包要用的图：
// app/src-tauri/installer 里的 dmg 窗口背景（1 倍和 2 倍合成一个 TIFF，Retina 屏也清楚）、NSIS 侧边图和顶栏图（按两倍尺寸出 24 位 BMP），
// 以及 app/src-tauri/icons 里的 Mac 菜单栏模板图、Windows 软件图标 icon.ico。
// 只在 Mac 上跑：用 Chrome 渲染 SVG（CHROME 环境变量指向它的可执行文件），再用系统自带的 sips、tiffutil 转格式。
// 用法：CHROME=/path/to/chrome node scripts/render-art.mjs
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const dir = fileURLToPath(new URL("../app/src-tauri/installer/", import.meta.url));
const icons = fileURLToPath(new URL("../app/src-tauri/icons/", import.meta.url));
const tmp = mkdtempSync(path.join(os.tmpdir(), "p2pcli-art-"));

function render(chrome, svg, width, height, scale, from = dir) {
  const out = path.join(tmp, `${path.basename(svg, ".svg")}-${width}x${height}@${scale}x.png`);
  // 直接打开 SVG 而不是放进 <img>，里面引用的图标才能加载
  execFileSync(chrome, [
    "--headless=new", "--disable-gpu", "--hide-scrollbars", "--allow-file-access-from-files",
    `--user-data-dir=${path.join(tmp, "profile")}`, `--force-device-scale-factor=${scale}`,
    `--window-size=${width},${height}`, "--default-background-color=00000000",
    `--screenshot=${out}`, pathToFileURL(path.join(from, svg)).href,
  ], { stdio: "ignore" });
  return out;
}

/** sips 转出的 BMP 是从上往下存的（高度是负数），翻成常见的从下往上，NSIS 加载更稳 */
function bmp(png, name) {
  const file = png.replace(/\.png$/, ".bmp");
  execFileSync("sips", ["-s", "format", "bmp", png, "--out", file], { stdio: "ignore" });
  const b = readFileSync(file);
  const offset = b.readUInt32LE(10);
  const width = b.readInt32LE(18);
  const height = b.readInt32LE(22);
  if (b.readUInt16LE(28) !== 24) throw new Error(`${name} 不是 24 位色`);
  if (height < 0) {
    const row = Math.ceil((width * 3) / 4) * 4;
    const rows = -height;
    const pixels = Buffer.alloc(row * rows);
    for (let y = 0; y < rows; y++) b.copy(pixels, (rows - 1 - y) * row, offset + y * row, offset + (y + 1) * row);
    pixels.copy(b, offset);
    b.writeInt32LE(rows, 22);
  }
  writeFileSync(path.join(dir, name), b);
}

/** 把各尺寸的 PNG 打成 ico，每个尺寸直接存 PNG（Windows Vista 起都认，NSIS 也认） */
function ico(sizes, pngs, out) {
  const head = Buffer.alloc(6 + 16 * pngs.length);
  head.writeUInt16LE(1, 2);
  head.writeUInt16LE(pngs.length, 4);
  let offset = head.length;
  pngs.forEach((png, i) => {
    const e = 6 + i * 16;
    head[e] = head[e + 1] = sizes[i] % 256; // 256 要写成 0
    head.writeUInt16LE(1, e + 4);
    head.writeUInt16LE(32, e + 6);
    head.writeUInt32LE(png.length, e + 8);
    head.writeUInt32LE(offset, e + 12);
    offset += png.length;
  });
  writeFileSync(out, Buffer.concat([head, ...pngs]));
}

try {
  const chrome = process.env.CHROME;
  if (!chrome) throw new Error("用 CHROME 环境变量指定 Chrome 的可执行文件");
  const dmg = [1, 2].map((scale) => render(chrome, "dmg-background.svg", 660, 440, scale));
  execFileSync("tiffutil", ["-cathidpicheck", ...dmg, "-out", path.join(dir, "dmg-background.tiff")], { stdio: "ignore" });
  bmp(render(chrome, "nsis-sidebar.svg", 164, 314, 2), "nsis-sidebar.bmp");
  bmp(render(chrome, "nsis-header.svg", 150, 57, 2), "nsis-header.bmp");
  // 菜单栏图标只看透明度，颜色由系统按菜单栏深浅来染；48x36 就是 2 倍屏上的 24x18 点
  copyFileSync(render(chrome, "tray.svg", 48, 36, 1, icons), path.join(icons, "tray-template.png"));
  // Windows 软件图标：底板铺满画布；16 和 24 用去掉光芒的小尺寸版
  const sizes = [16, 24, 32, 48, 64, 256];
  const svg = (n) => (n <= 24 ? "windows-icon-small.svg" : "windows-icon.svg");
  ico(sizes, sizes.map((n) => readFileSync(render(chrome, svg(n), n, n, 1, icons))), path.join(icons, "icon.ico"));
  console.log("已更新 dmg-background.tiff、nsis-sidebar.bmp、nsis-header.bmp、tray-template.png、icon.ico");
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
} finally {
  rmSync(tmp, { recursive: true, force: true });
}
