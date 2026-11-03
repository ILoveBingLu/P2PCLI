// 编辑器的几个附属界面：起名对话框、全项目搜索、按名字快速打开
import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { CaseSensitive, ChevronDown, ChevronRight, CircleX, File, Info, LoaderCircle, Regex, Search, TriangleAlert, WholeWord } from "lucide-react";
import { api, type FileList, type SearchResult } from "../../api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { cn } from "@/lib/utils";
import { t, tr } from "@/i18n";
import { baseName, fuzzyScore, joinPath, nameProblem, relativeTo, sepOf } from "./paths";
import type { FileProblems, FoundSymbol, LspManager } from "./lsp";

/** 新建文件、新建文件夹、重命名时输入名字 */
export function NameDialog({ title, description, initial, confirm, onSubmit, onClose }: {
  title: string;
  description: string;
  initial: string;
  confirm: string;
  /** 抛出的错误显示在对话框里，成功就关掉 */
  onSubmit: (name: string) => Promise<void>;
  onClose: () => void;
}) {
  const [name, setName] = useState(initial);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    // 重命名时先选中不带扩展名的部分，和系统的文件管理器一样
    const el = input.current;
    if (!el) return;
    el.focus();
    const dot = initial.lastIndexOf(".");
    el.setSelectionRange(0, dot > 0 ? dot : initial.length);
  }, [initial]);
  const submit = async () => {
    const problem = nameProblem(name);
    if (problem) return setError(problem);
    setBusy(true);
    setError(null);
    try {
      await onSubmit(name.trim());
      onClose();
    } catch (e) {
      setError(tr(String(e)));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription className="break-all">{description}</DialogDescription>
        </DialogHeader>
        <Input ref={input} value={name} aria-label={title} onChange={(e) => { setName(e.target.value); setError(null); }}
          onKeyDown={(e) => { if (e.key === "Enter" && !e.nativeEvent.isComposing) { e.preventDefault(); void submit(); } }} />
        {error && <p role="alert" className="text-sm text-danger wrap-break-word">{error}</p>}
        <DialogFooter>
          <Button variant="outline" onPress={onClose}>{t("取消")}</Button>
          <Button isDisabled={busy} onPress={() => void submit()}>{busy && <LoaderCircle className="size-4 animate-spin" />}{confirm}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export interface SearchTarget { path: string; line: number; col: number; len: number }

/** 在对方电脑上搜整个目录的文件内容；回车才搜，免得每打一个字都跑一遍 */
export function SearchPanel({ connId, root, onOpen }: {
  connId: number | null;
  root: string | null;
  onOpen: (target: SearchTarget) => void;
}) {
  const [query, setQuery] = useState("");
  const [options, setOptions] = useState({ caseSensitive: false, wholeWord: false, regex: false });
  const [result, setResult] = useState<SearchResult | null>(null);
  const [searched, setSearched] = useState<{ query: string; root: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());
  const request = useRef(0);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => { input.current?.focus(); input.current?.select(); }, []);

  const run = async (next = options) => {
    if (!query || connId == null || root == null) return;
    const id = ++request.current;
    setBusy(true);
    setError(null);
    try {
      const found = await api.searchFiles(connId, root, query, next);
      if (id !== request.current) return;
      setResult(found);
      setSearched({ query, root });
      setCollapsed(new Set());
    } catch (e) {
      if (id === request.current) setError(tr(String(e)));
    } finally {
      if (id === request.current) setBusy(false);
    }
  };

  const toggle = (key: keyof typeof options) => {
    const next = { ...options, [key]: !options[key] };
    setOptions(next);
    if (searched) void run(next);
  };

  const count = result?.files.reduce((n, f) => n + f.matches.length, 0) ?? 0;
  return (
    <div className="remote-editor-search">
      <div className="remote-editor-search-box">
        <Input ref={input} value={query} placeholder={t("搜索，回车开始")} aria-label={t("在文件中搜索")}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter" && !e.nativeEvent.isComposing) { e.preventDefault(); void run(); } }} />
        <div className="remote-editor-search-options">
          <Toggle on={options.caseSensitive} label={t("区分大小写")} onPress={() => toggle("caseSensitive")}><CaseSensitive /></Toggle>
          <Toggle on={options.wholeWord} label={t("全字匹配")} onPress={() => toggle("wholeWord")}><WholeWord /></Toggle>
          <Toggle on={options.regex} label={t("正则表达式")} onPress={() => toggle("regex")}><Regex /></Toggle>
          <Button size="icon-xs" variant="ghost" aria-label={t("搜索")} title={t("搜索")} isDisabled={!query || connId == null || busy} onPress={() => void run()}>
            {busy ? <LoaderCircle className="animate-spin" /> : <Search />}
          </Button>
        </div>
      </div>
      {error && <div className="remote-editor-note is-error">{error}</div>}
      {result && !error && <div className="remote-editor-note">
        {count === 0 ? t("没有找到") : t("{{files}} 个文件中有 {{count}} 处", { files: result.files.length, count })}
        {result.truncated && " · " + t("结果太多或搜索超时，只显示了一部分")}
      </div>}
      {!result && !error && <div className="remote-editor-note">{t("搜索对方电脑上这个目录里的文件内容，会跳过 .gitignore 里的文件、二进制文件和超过 2 MB 的文件。")}</div>}
      <div className="remote-editor-search-results">
        {result?.files.map((file) => {
          const open = !collapsed.has(file.path);
          const rel = searched ? relativeTo(file.path, searched.root) : file.path;
          return <div key={file.path}>
            <button type="button" className="remote-editor-search-file" title={file.path}
              onClick={() => setCollapsed((s) => { const n = new Set(s); if (open) n.add(file.path); else n.delete(file.path); return n; })}>
              {open ? <ChevronDown className="chevron" /> : <ChevronRight className="chevron" />}<File />
              <span className="truncate">{baseName(file.path)}</span>
              <span className="remote-editor-search-dir truncate">{rel.includes("/") ? rel.slice(0, rel.lastIndexOf("/")) : ""}</span>
              <span className="remote-editor-search-count">{file.matches.length}</span>
            </button>
            {open && file.matches.map((m) => (
              <button key={`${m.line}:${m.col}`} type="button" className="remote-editor-search-match"
                title={t("第 {{line}} 行", { line: m.line })}
                onClick={() => onOpen({ path: file.path, line: m.line, col: m.col, len: m.len })}>
                <Highlight text={m.preview} start={m.preview_col} len={m.len} />
              </button>
            ))}
          </div>;
        })}
      </div>
    </div>
  );
}

function Toggle({ on, label, onPress, children }: { on: boolean; label: string; onPress: () => void; children: ReactNode }) {
  return <Button size="icon-xs" variant="ghost" aria-label={label} aria-pressed={on} title={label}
    className={cn("remote-editor-toggle", on && "is-on")} onPress={onPress}>{children}</Button>;
}

/** 预览里把匹配的那段标出来；位置按 UTF-16 算，和 JS 字符串的下标一致 */
function Highlight({ text, start, len }: { text: string; start: number; len: number }) {
  return <span className="truncate">{text.slice(0, start)}<mark>{text.slice(start, start + len)}</mark>{text.slice(start + len)}</span>;
}

/** ⌘P：按名字找文件。文件列表在打开时从对方电脑上拿一次，之后在本机筛选，打字不用等网络 */
export function QuickOpen({ connId, root, cache, onCache, onOpen, onClose }: {
  connId: number | null;
  root: string;
  cache: { list: FileList; at: number } | null;
  onCache: (cache: { list: FileList; at: number }) => void;
  onOpen: (path: string) => void;
  onClose: () => void;
}) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const fresh = cache && cache.list.root === root && Date.now() - cache.at < 30_000;
  const list = cache?.list.root === root ? cache.list : null;
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (fresh || connId == null) return;
    let stop = false;
    api.listFiles(connId, root)
      .then((next) => { if (!stop) onCache({ list: next, at: Date.now() }); })
      .catch((e) => { if (!stop) setError(tr(String(e))); });
    return () => { stop = true; };
  }, [root, connId]); // eslint-disable-line react-hooks/exhaustive-deps

  const matches = useMemo(() => {
    if (!list) return [];
    if (!query.trim()) return list.files.slice(0, 50);
    const scored: { path: string; score: number }[] = [];
    for (const path of list.files) {
      const score = fuzzyScore(query, path);
      if (score != null) scored.push({ path, score });
    }
    return scored.sort((a, b) => b.score - a.score).slice(0, 50).map((s) => s.path);
  }, [list, query]);

  useEffect(() => {
    listRef.current?.querySelector<HTMLElement>(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const open = (rel: string) => {
    onOpen(joinPath(root, rel.split("/").join(sepOf(root))));
    onClose();
  };
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "ArrowDown") { e.preventDefault(); setActive((i) => Math.min(i + 1, matches.length - 1)); }
    else if (e.key === "ArrowUp") { e.preventDefault(); setActive((i) => Math.max(i - 1, 0)); }
    else if (e.key === "Enter" && !e.nativeEvent.isComposing) { e.preventDefault(); if (matches[active]) open(matches[active]); }
    else if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); onClose(); }
  };

  return (
    <div className="remote-editor-quickopen-backdrop" onPointerDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div className="remote-editor-quickopen" role="dialog" aria-label={t("按名字打开文件")}>
        <Input autoFocus value={query} placeholder={t("输入文件名")} aria-label={t("输入文件名")}
          onChange={(e) => { setQuery(e.target.value); setActive(0); }} onKeyDown={onKeyDown} />
        <div ref={listRef} className="remote-editor-quickopen-list" role="listbox">
          {error && <div className="remote-editor-note is-error">{error}</div>}
          {!list && !error && <div className="remote-editor-note"><LoaderCircle className="inline size-3 animate-spin" /> {t("正在读取文件列表…")}</div>}
          {list && matches.length === 0 && <div className="remote-editor-note">{t("没有匹配的文件")}</div>}
          {matches.map((rel, i) => (
            <button key={rel} type="button" role="option" aria-selected={i === active} data-index={i}
              className="remote-editor-quickopen-item" onMouseMove={() => setActive(i)} onClick={() => open(rel)}>
              <File /><span className="truncate">{baseName(rel)}</span>
              <span className="remote-editor-search-dir truncate">{rel.includes("/") ? rel.slice(0, rel.lastIndexOf("/")) : ""}</span>
            </button>
          ))}
          {list?.truncated && <div className="remote-editor-note">{t("文件太多，只读取了一部分")}</div>}
        </div>
      </div>
    </div>
  );
}

/** 语言服务报的所有问题：没打开的文件也在里面，点一条跳过去 */
export function ProblemsPanel({ problems, root, onOpen }: {
  problems: FileProblems[];
  root: string | null;
  onOpen: (target: SearchTarget) => void;
}) {
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());
  if (!problems.length) {
    return <div className="remote-editor-search">
      <div className="remote-editor-note">{t("没有发现问题。打开代码文件后，语言服务会在这里列出整个项目的错误和警告。")}</div>
    </div>;
  }
  return (
    <div className="remote-editor-search">
      <div className="remote-editor-search-results">
        {problems.map((file) => {
          const open = !collapsed.has(file.path);
          const rel = root ? relativeTo(file.path, root) : file.path;
          return <div key={file.path}>
            <button type="button" className="remote-editor-search-file" title={file.path}
              onClick={() => setCollapsed((c) => { const n = new Set(c); if (open) n.add(file.path); else n.delete(file.path); return n; })}>
              {open ? <ChevronDown className="chevron" /> : <ChevronRight className="chevron" />}<File />
              <span className="truncate">{baseName(file.path)}</span>
              <span className="remote-editor-search-dir truncate">{rel.includes("/") ? rel.slice(0, rel.lastIndexOf("/")) : ""}</span>
              <span className="remote-editor-search-count">{file.problems.length}</span>
            </button>
            {open && file.problems.map((p, i) => (
              <button key={i} type="button" className={cn("remote-editor-problem", `is-${p.severity}`)}
                title={[p.message, p.source && `${p.source}${p.code ? `(${p.code})` : ""}`].filter(Boolean).join("\n")}
                onClick={() => onOpen({ path: file.path, line: p.line, col: p.col, len: 0 })}>
                {p.severity === "error" ? <CircleX /> : p.severity === "warning" ? <TriangleAlert /> : <Info />}
                <span className="truncate">{p.message.split("\n")[0]}</span>
                <span className="remote-editor-search-dir">{p.line}:{p.col}</span>
              </button>
            ))}
          </div>;
        })}
      </div>
    </div>
  );
}

/** LSP 的符号种类，显示成一两个字母 */
const SYMBOL_LETTERS: Record<number, string> = {
  2: "M", 3: "N", 4: "P", 5: "C", 6: "m", 7: "p", 8: "f", 9: "c", 10: "E", 11: "I", 12: "ƒ", 13: "v", 14: "K", 22: "e", 23: "S", 26: "T",
};

/** ⌘T：在整个项目里按名字找函数、类型；问的是对方电脑上的语言服务，打字停一下再问 */
export function SymbolSearch({ manager, root, onOpen, onClose }: {
  manager: LspManager | null;
  root: string | null;
  onOpen: (target: SearchTarget) => void;
  onClose: () => void;
}) {
  const [query, setQuery] = useState("");
  const [found, setFound] = useState<FoundSymbol[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [active, setActive] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);
  const available = !!manager?.canSearchSymbols();

  useEffect(() => {
    if (!manager || !available || !query.trim()) { setFound(null); return; }
    let cancelled = false;
    const listeners: (() => void)[] = [];
    const token = {
      get isCancellationRequested() { return cancelled; },
      onCancellationRequested(listener: () => void) { listeners.push(listener); return { dispose() {} }; },
    };
    const timer = window.setTimeout(() => {
      setBusy(true);
      manager.symbols(query.trim(), token).then((list) => {
        if (!cancelled) { setFound(list); setActive(0); }
      }).finally(() => { if (!cancelled) setBusy(false); });
    }, 200);
    return () => { cancelled = true; window.clearTimeout(timer); listeners.forEach((l) => l()); };
  }, [query, manager, available]);

  useEffect(() => {
    listRef.current?.querySelector<HTMLElement>(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const items = found ?? [];
  const open = (item: FoundSymbol) => {
    onOpen({ path: item.path, line: item.line, col: item.col, len: item.name.length });
    onClose();
  };
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "ArrowDown") { e.preventDefault(); setActive((i) => Math.min(i + 1, items.length - 1)); }
    else if (e.key === "ArrowUp") { e.preventDefault(); setActive((i) => Math.max(i - 1, 0)); }
    else if (e.key === "Enter" && !e.nativeEvent.isComposing) { e.preventDefault(); if (items[active]) open(items[active]); }
    else if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); onClose(); }
  };

  return (
    <div className="remote-editor-quickopen-backdrop" onPointerDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div className="remote-editor-quickopen" role="dialog" aria-label={t("按名字找符号")}>
        <Input autoFocus value={query} placeholder={t("输入函数、类型的名字")} aria-label={t("按名字找符号")}
          onChange={(e) => setQuery(e.target.value)} onKeyDown={onKeyDown} />
        <div ref={listRef} className="remote-editor-quickopen-list" role="listbox">
          {!available && <div className="remote-editor-note">{t("先打开一个代码文件，等语言服务启动后才能按名字找符号。")}</div>}
          {available && busy && !found && <div className="remote-editor-note"><LoaderCircle className="inline size-3 animate-spin" /> {t("正在查找…")}</div>}
          {available && found && !items.length && <div className="remote-editor-note">{t("没有匹配的符号")}</div>}
          {items.map((item, i) => {
            const rel = root ? relativeTo(item.path, root) : item.path;
            return (
              <button key={`${item.path}:${item.line}:${item.col}:${item.name}`} type="button" role="option" aria-selected={i === active} data-index={i}
                className="remote-editor-quickopen-item" onMouseMove={() => setActive(i)} onClick={() => open(item)} title={`${item.path}:${item.line}`}>
                <span className="remote-editor-symbol-kind">{SYMBOL_LETTERS[item.kind] ?? "·"}</span>
                <span className="truncate">{item.name}</span>
                <span className="remote-editor-search-dir truncate">{[item.container, `${rel}:${item.line}`].filter(Boolean).join(" · ")}</span>
              </button>
            );
          })}
        </div>
      </div>
    </div>
  );
}
