// 终端输出里的文件路径：编译器报错（src/main.rs:42:5、main.ts(12,3)）、Python 回溯（File "x.py", line 3）、
// AI 说的“改了 app/src/App.tsx”。只认像路径的：带行号，或者带目录分隔符，免得把 1.2.3、foo.bar 都当成链接。

export interface TerminalPathLink {
  /** 原样的路径，相对路径由用的地方按终端当前目录算 */
  path: string;
  line?: number;
  col?: number;
}

export interface FoundLink extends TerminalPathLink {
  /** 在这一行文字里的起止下标（end 不含） */
  start: number;
  end: number;
}

// 开头不能紧跟在路径字符后面（免得从网址、单词中间开始）；路径 = 可选的根（C:\ ./ ../ /）+ 若干目录 + 带扩展名的文件名；
// 后面可选 :行:列、(行,列)、", line 行"
const PATH = /(?<![\w./\\~:-])((?:[A-Za-z]:[\\/]|\.{1,2}[\\/]|\/)?(?:[\w.@+-]+[\\/])*[\w@+-][\w.@+-]*\.[A-Za-z][\w]{0,9})(?::(\d+)(?::(\d+))?|\((\d+)(?:,\s*(\d+))?\)|", line (\d+))?/g;

export function findPathLinks(text: string): FoundLink[] {
  const out: FoundLink[] = [];
  for (const m of text.matchAll(PATH)) {
    const path = m[1];
    const line = Number(m[2] ?? m[4] ?? m[6]) || undefined;
    const col = Number(m[3] ?? m[5]) || undefined;
    // 网址里的一段（https://a.com/x.js）不算
    if (text.slice(Math.max(0, m.index! - 3), m.index!).endsWith("//")) continue;
    if (!line && !/[\\/]/.test(path)) continue;
    // Python 回溯：路径在引号里，引号不算链接的一部分
    const end = m[6] ? m.index! + path.length : m.index! + m[0].length;
    out.push({ path, line, col, start: m.index!, end });
  }
  return out;
}
