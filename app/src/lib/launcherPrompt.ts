/** 与被控端 launchers::empty_prompt 一致；未知提示符保守地不快捷启动。 */
export function emptyShellPrompt(line: string): boolean {
  const text = line.trim();
  const mark = text.slice(-1);
  if (!mark || !"$#%❯➜>".includes(mark)) return false;
  const prefix = text.slice(0, -1).trim();
  if (/[$#%❯➜>]/u.test(prefix)) return false;
  if (mark === ">") return prefix.startsWith("PS ") || prefix.includes("@") || /^[A-Za-z]:\\/.test(prefix);
  return !prefix || prefix.includes("@") || (prefix.startsWith("[") && prefix.endsWith("]")) || prefix.startsWith("/") || prefix.startsWith("~");
}

/** 本地输入发生后，在收到变化的终端画面前，不能使用旧提示符启动。 */
export function promptGuard() {
  let blocked: string | null = null;
  return {
    input(signature: string) { blocked = signature; },
    ready(signature: string, empty: boolean) {
      if (blocked === signature) return false;
      blocked = null;
      return empty;
    },
  };
}
