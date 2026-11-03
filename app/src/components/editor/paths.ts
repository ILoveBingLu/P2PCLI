// 对方电脑上的路径：Windows 用反斜杠，别的用斜杠；都按对方的写法拼，不在本机这边转换。
import type { GitStatus } from "../../api";
import { t } from "@/i18n";

export function sepOf(path: string) {
  return path.includes("\\") && !path.includes("/") ? "\\" : "/";
}

export function baseName(path: string) {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

export function joinPath(base: string, name: string) {
  const sep = sepOf(base);
  return base.endsWith(sep) ? base + name : base + sep + name;
}

/** 上一级目录；已经是根目录时原样返回 */
export function dirName(path: string) {
  const sep = sepOf(path);
  const trimmed = path.endsWith(sep) && path.length > 1 ? path.slice(0, -1) : path;
  const index = trimmed.lastIndexOf(sep);
  if (index < 0) return path;
  // /a → /；C:\a → C:\
  return index === 0 || trimmed[index - 1] === ":" ? trimmed.slice(0, index + 1) : trimmed.slice(0, index);
}

/** path 是 dir 本身或在 dir 里面 */
export function isUnder(path: string, dir: string) {
  if (path === dir) return true;
  const sep = sepOf(dir);
  return path.startsWith(dir.endsWith(sep) ? dir : dir + sep);
}

/** 改名以后，原来在 from 里面的路径换成 to 开头 */
export function movePath(path: string, from: string, to: string) {
  return path === from ? to : to + path.slice(from.length);
}

/** 相对 base 的路径，分隔符统一成 /，显示用 */
export function relativeTo(path: string, base: string) {
  if (!isUnder(path, base) || path === base) return path;
  const sep = sepOf(base);
  return path.slice(base.endsWith(sep) ? base.length : base.length + 1).split(sep).join("/");
}

/** 文件名里不能有分隔符，也不能是 . 和 ..；返回错误说明，没问题时返回空 */
export function nameProblem(name: string): string | null {
  const trimmed = name.trim();
  if (!trimmed) return t("名字不能为空");
  if (/[\\/]/.test(trimmed)) return t("名字里不能有 / 或 \\");
  if (trimmed === "." || trimmed === "..") return t("名字不能是 . 或 ..");
  if (/[\u0000-\u001f]/.test(trimmed)) return t("名字里不能有控制字符");
  return null;
}

export type GitKind = "modified" | "added" | "untracked" | "deleted" | "conflict";

export interface GitMarks {
  /** 文件树路径 → 这个文件的改动 */
  files: Map<string, GitKind>;
  /** 里面有改动的目录 */
  dirs: Set<string>;
}

function kindOf(status: string): GitKind {
  if (status === "??") return "untracked";
  if (status.includes("U") || status === "AA" || status === "DD") return "conflict";
  if (status.includes("D")) return "deleted";
  if (status[0] === "A" || status[0] === "R" || status[0] === "C") return "added";
  return "modified";
}

/** 把 git status 的结果换算成文件树里的路径：entries 相对仓库根，root 相对仓库根在 prefix */
export function gitMarks(status: GitStatus | null, root: string | null): GitMarks {
  const marks: GitMarks = { files: new Map(), dirs: new Set() };
  if (!status || root == null) return marks;
  const sep = sepOf(root);
  for (const entry of status.entries) {
    if (!entry.path.startsWith(status.prefix)) continue;
    // 未跟踪的整个目录，git 会写成 "dir/"
    const rel = entry.path.slice(status.prefix.length).replace(/\/$/, "");
    if (!rel) continue;
    const path = joinPath(root, rel.split("/").join(sep));
    marks.files.set(path, kindOf(entry.status));
    for (let dir = dirName(path); isUnder(dir, root) && dir !== root; dir = dirName(dir)) marks.dirs.add(dir);
  }
  return marks;
}

/** 按名字快速打开：字母按顺序出现就算匹配，文件名里连着出现的、靠前的排前面 */
export function fuzzyScore(query: string, path: string): number | null {
  const q = query.toLowerCase().replace(/\s+/g, "");
  if (!q) return 0;
  const target = path.toLowerCase();
  const nameStart = target.lastIndexOf("/") + 1;
  let score = 0;
  let last = -1;
  for (const ch of q) {
    const index = target.indexOf(ch, last + 1);
    if (index < 0) return null;
    // 连续的字母加分，落在文件名里加分，跳得越远扣分越多
    score += index === last + 1 ? 6 : 1;
    if (index >= nameStart) score += 3;
    score -= Math.min(index - last - 1, 10) * 0.2;
    last = index;
  }
  const name = target.slice(nameStart);
  if (name.startsWith(q)) score += 20;
  else if (name.includes(q)) score += 10;
  return score - path.length * 0.01;
}
