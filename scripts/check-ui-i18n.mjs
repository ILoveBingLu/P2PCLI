// Checks that every desktop, Android, iOS and HarmonyOS UI string has matching English and Simplified Chinese copy.
// 手机原生版（mobile/android、mobile/ios、mobile/harmony）也用这两份翻译：Kotlin、Swift、ArkTS 里的 t("…")、tr("…") 一起检查。
import { readFileSync, readdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import i18next from "i18next";

const root = fileURLToPath(new URL("..", import.meta.url));
const src = path.join(root, "app/src");
const android = path.join(root, "mobile/android/app/src/main/java");
const ios = path.join(root, "mobile/ios/P2PCLI");
const harmony = path.join(root, "mobile/harmony/entry/src/main/ets");
const skip = new Set(["preview.tsx", "selftest.ts"]);

function walk(dir, out = []) {
  for (const ent of readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, ent.name);
    if (ent.isDirectory()) {
      if (ent.name === "i18n") continue;
      walk(p, out);
    } else if (/\.(ts|tsx|kt|swift|ets)$/.test(ent.name) && !skip.has(ent.name)) out.push(p);
  }
  return out;
}

function unescape(raw, quote) {
  if (quote === "`") return raw.replace(/\\`/g, "`").replace(/\\\$/g, "$").replace(/\\\\/g, "\\");
  return raw.replace(/\\n/g, "\n").replace(/\\"/g, '"').replace(/\\'/g, "'").replace(/\\\\/g, "\\");
}

function keysIn(source) {
  const keys = [];
  const re = /\b(?:t|tr)\(\s*(["'`])((?:\\.|(?!\1)[\s\S])*?)\1/g;
  let match;
  while ((match = re.exec(source))) keys.push(unescape(match[2], match[1]));
  return keys;
}

const backend = [
  "好", "一般", "差", "未知",
  "国内服务器", "国内节点，在国内用连得最快",
  "Cloudflare", "海外节点，适合在国外用，国内访问有时不稳",
  "默认服务器", "安装包内置的牵线服务器",
  "局域网访问", "用于发现同一局域网里的 P2PCLI 设备，并尝试局域网直连。",
  "终端不在空闲提示符，请先完成当前输入或退出正在运行的程序",
  "对方的 P2PCLI 版本较旧，不支持选择目录，请更新对方的 P2PCLI",
  "这是二进制文件，不能在编辑器里打开",
  "文件不是 UTF-8 编码（可能是 GBK 等），暂不支持在编辑器里打开",
  "对方的 P2PCLI 版本较旧，不支持远程编辑，请更新对方的 P2PCLI",
  "内容太大，最多保存 16 MB",
  "打开文件超时",
  "保存超时，文件可能已经写入，请重新打开确认",
  "这个文件不在 Git 的最新提交里",
  "对方的 P2PCLI 版本较旧，不支持这个编辑功能，请更新对方的 P2PCLI",
  "文件太大，不能对比",
  "对方的 P2PCLI 版本较旧，不支持端口转发，请更新对方的 P2PCLI",
  "对方电脑上这个端口没有服务在监听",
  "这是本机，直接在浏览器里打开 localhost 就行，不用转发",
  "这个转发已经停了",
  "地址不对，不打开",
  "对方的 P2PCLI 版本较旧，不支持 Git 面板，请更新对方的 P2PCLI",
  "提交说明不能为空",
  "git 运行超时（可能在等输入密码），请在终端里执行",
  "现在不在任何分支上，没法推送",
  "这个仓库没有配置远程仓库",
  "手机上暂不支持这个功能",
];

const used = new Set(backend);
for (const file of [...walk(src), ...walk(android), ...walk(ios), ...walk(harmony)]) {
  for (const key of keysIn(readFileSync(file, "utf8"))) used.add(key);
}

const en = JSON.parse(readFileSync(path.join(src, "i18n/locales/en.json"), "utf8"));
const zh = JSON.parse(readFileSync(path.join(src, "i18n/locales/zh-CN.json"), "utf8"));
const errors = [];
const placeholders = (text) => [...String(text).matchAll(/\{\{[^}]+\}\}/g)].map((m) => m[0]).sort().join(" ");
const cjkAllowed = new Set(["简体中文"]);

for (const key of used) {
  if (!(key in en)) errors.push(`missing from en.json: ${JSON.stringify(key)}`);
  if (!(key in zh)) errors.push(`missing from zh-CN.json: ${JSON.stringify(key)}`);
  if (key in zh && zh[key] !== key) errors.push(`zh-CN value is not the source string: ${JSON.stringify(key)}`);
  if (key in en && placeholders(key) !== placeholders(en[key])) {
    errors.push(`placeholders differ for ${JSON.stringify(key)} -> ${JSON.stringify(en[key])}`);
  }
  if (key in en && /[\u4e00-\u9fff]/.test(en[key]) && !cjkAllowed.has(key)) {
    errors.push(`English still contains Chinese: ${JSON.stringify(key)} -> ${JSON.stringify(en[key])}`);
  }
}

const i18n = i18next.createInstance();
await i18n.init({
  resources: { en: { translation: en }, "zh-CN": { translation: zh } },
  lng: "en",
  fallbackLng: "en",
  keySeparator: false,
  nsSeparator: false,
  interpolation: { escapeValue: false },
  returnNull: false,
});

const samples = [
  ["连接设置", "Connection"],
  ["牵线服务器", "Rendezvous server"],
  ["已连接 {{name}}", "Connected to {{name}}"],
  ["好", "Good"],
  ["界面语言", "Language"],
  ["跟随系统", "Follow system"],
];
for (const [key, english] of samples) {
  const resolved = i18n.t(key, { lng: "en", name: "studio" });
  const expected = key.includes("{{name}}") ? english.replace("{{name}}", "studio") : english;
  if (resolved !== expected) errors.push(`en ${JSON.stringify(key)} resolved to ${JSON.stringify(resolved)}`);
  if (i18n.t(key, { lng: "zh-CN" }) !== key) errors.push(`zh-CN did not keep ${JSON.stringify(key)}`);
}
if (i18n.t("不存在的句子", { lng: "en" }) !== "不存在的句子") {
  errors.push("missing English keys should fall back to the Chinese source");
}

if (errors.length) {
  console.error(errors.join("\n"));
  process.exit(1);
}
console.log(`ui i18n ok: ${used.size} strings, en and zh-CN`);
