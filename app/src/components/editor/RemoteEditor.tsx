import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type MouseEvent, type ReactNode } from "react";
import {
  ArrowUp, ChevronDown, ChevronRight, CircleAlert, Copy, File, FilePlus, Files, Folder, FolderOpen, FolderPlus, GitBranch, GitCompare,
  House, LoaderCircle, Pencil, RefreshCw, Save, Search, TextSearch, Trash2, TriangleAlert, X, Zap, ZapOff,
} from "lucide-react";
import { api, CONFLICT_PREFIX, type EntryListing, type FileList, type FileMeta, type GitStatus } from "../../api";
import { Button } from "@/components/ui/button";
import {
  AlertDialog, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { toast } from "@/components/ui/toast";
import { useContextMenu, type MenuAction } from "../ContextMenu";
import { writeClipboardText } from "@/lib/clipboard";
import { cn } from "@/lib/utils";
import { t, tr } from "@/i18n";
import { languageFor, monaco } from "./monaco";
import { baseName, dirName, gitMarks, isUnder, joinPath, movePath, type GitKind } from "./paths";
import { NameDialog, QuickOpen, SearchPanel, type SearchTarget } from "./EditorPanels";
import { LspManager, type LspStatus } from "./lsp";

type Eol = "LF" | "CRLF";

interface OpenFile {
  path: string;
  name: string;
  state: "loading" | "ready" | "error";
  error?: string;
  meta?: FileMeta;
  dirty: boolean;
  saving: boolean;
  /** 保存时发现文件在对方电脑上被改过 */
  conflict: boolean;
  /** 还没保存，但已经发现对方电脑上的文件变了（有未保存修改时不自动替换） */
  remoteChanged: boolean;
  /** 对方电脑上已经没有这个文件了（被删或被移走） */
  gone: boolean;
  saveError?: string;
  eol: Eol;
  /** 文件里 LF 和 CRLF 混着用，保存时会统一成 eol */
  mixed: boolean;
}

interface Loaded {
  model: monaco.editor.ITextModel;
  /** 和对方电脑上一致时的版本号；当前版本号和它不同就是有未保存的修改 */
  saved: number;
  /** 上次核对对方电脑上文件有没有变的时间 */
  checkedAt: number;
}

/** 跳转定义、查引用时预先读出来的文件：还没开标签，打开时直接用 */
interface Peeked { model: monaco.editor.ITextModel; meta: FileMeta; eol: Eol; mixed: boolean }
/** 预读的文件最多留这么多个 */
const MAX_PEEKED = 60;

interface DirState { listing?: EntryListing; loading: boolean; error?: string }

type Row =
  | { kind: "entry"; path: string; name: string; dir: boolean; depth: number }
  | { kind: "note"; key: string; text: string; depth: number; error?: boolean };

interface Confirm { title: string; body: string; confirm: string; onConfirm: () => void; extra?: { label: string; onPress: () => void } }
type Naming = { kind: "file" | "dir"; dir: string } | { kind: "rename"; path: string };
interface Diff { path: string; title: string; original: monaco.editor.ITextModel }

function detectEol(text: string): { eol: Eol; mixed: boolean } {
  const crlf = text.match(/\r\n/g)?.length ?? 0;
  const lf = (text.match(/\n/g)?.length ?? 0) - crlf;
  return { eol: crlf > lf ? "CRLF" : "LF", mixed: crlf > 0 && lf > 0 };
}

const EOL = (eol: Eol) => eol === "CRLF" ? monaco.editor.EndOfLineSequence.CRLF : monaco.editor.EndOfLineSequence.LF;
/** 没改动的文件切回来时，隔这么久以上才去对方电脑上核对一次 */
const RECHECK_MS = 2000;
/** 编辑器开着时，每隔这么久看一眼对方电脑上的文件、目录和 Git 状态有没有变 */
const POLL_MS = 3000;
/** 不在 Git 仓库里时，隔这么久才再问一次 */
const GIT_RETRY_MS = 30_000;
const GIT_LETTER: Record<GitKind, string> = { modified: "M", added: "A", untracked: "U", deleted: "D", conflict: "!" };
let modelSeq = 0;
const modelUri = (kind: string, name: string) => monaco.Uri.from({ scheme: "p2pcli", authority: `${kind}${++modelSeq}`, path: "/" + name });

const editorOptions = (): monaco.editor.IEditorOptions & monaco.editor.IGlobalEditorOptions => ({
  theme: "p2pcli-dark",
  automaticLayout: true,
  fontFamily: getComputedStyle(document.documentElement).getPropertyValue("--font-mono").trim() || "monospace",
  fontSize: 13,
  scrollBeyondLastLine: false,
  // 编辑器只占半个窗口，缩略图太占地方
  minimap: { enabled: false },
  // 中文标点会被当成“容易混淆的字符”框起来，写中文注释时满屏都是框
  unicodeHighlight: { ambiguousCharacters: false },
});

/**
 * 远程编辑器：左边是对方电脑上的文件树和搜索，右边是 Monaco。编辑全在本机内存里，打开、保存时各走一次网络，
 * 网络再慢也不影响打字。保存时核对打开时的哈希，文件被终端里的 AI 等改过就提示冲突，不悄悄盖掉；
 * 开着时每隔几秒看一眼对方电脑上的文件、目录和 Git 状态，没改过的文件自动换成最新内容。
 */
export default function RemoteEditor({ connId, root: requestedRoot, rootSeq, active, onDirty, onClose }: {
  /** 断开时为空：保留打开的文件和未保存的修改，重新连上后接着用 */
  connId: number | null;
  root: string;
  /** 外面（目录选择、工具栏）要求换根目录时加一 */
  rootSeq: number;
  active: boolean;
  onDirty: (count: number) => void;
  onClose: () => void;
}) {
  const online = connId != null;
  const [root, setRoot] = useState<string | null>(null);
  const [dirs, setDirs] = useState<Record<string, DirState>>({});
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set());
  const [rootError, setRootError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [side, setSide] = useState<"files" | "search">("files");
  const [files, setFiles] = useState<OpenFile[]>([]);
  const [activePath, setActivePath] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<Confirm | null>(null);
  const [naming, setNaming] = useState<Naming | null>(null);
  const [quickOpen, setQuickOpen] = useState(false);
  const [fileCache, setFileCache] = useState<{ list: FileList; at: number } | null>(null);
  const [git, setGit] = useState<GitStatus | null>(null);
  const [diff, setDiff] = useState<Diff | null>(null);
  const menu = useContextMenu();

  const host = useRef<HTMLDivElement>(null);
  const diffHost = useRef<HTMLDivElement>(null);
  const editor = useRef<monaco.editor.IStandaloneCodeEditor | null>(null);
  const diffEditor = useRef<monaco.editor.IStandaloneDiffEditor | null>(null);
  const loaded = useRef(new Map<string, Loaded>());
  const peeked = useRef(new Map<string, Peeked>());
  const lsp = useRef<LspManager | null>(null);
  const [, setLspTick] = useState(0);
  const viewStates = useRef(new Map<string, monaco.editor.ICodeEditorViewState | null>());
  /** 从搜索结果打开时要跳到的位置，文件读出来以后再跳 */
  const pendingReveal = useRef(new Map<string, SearchTarget>());
  /** 从右键菜单要求和 Git 版本对比的文件，读出来以后再开对比 */
  const pendingDiff = useRef<string | null>(null);
  const filesRef = useRef(files);
  filesRef.current = files;
  const connRef = useRef(connId);
  connRef.current = connId;
  const activeRef = useRef(activePath);
  activeRef.current = activePath;
  const treeRef = useRef({ root, dirs, expanded });
  treeRef.current = { root, dirs, expanded };
  const diffRef = useRef(diff);
  diffRef.current = diff;
  const gitRef = useRef(git);
  gitRef.current = git;

  const update = (path: string, patch: Partial<OpenFile>) =>
    setFiles((list) => list.map((f) => f.path === path ? { ...f, ...patch } : f));

  // ---------- 文件树 ----------
  const loadDir = async (path: string) => {
    const conn = connRef.current;
    if (conn == null) return null;
    setDirs((d) => ({ ...d, [path]: { ...d[path], loading: true, error: undefined } }));
    try {
      const listing = await api.listEntries(conn, path);
      setDirs((d) => ({ ...d, [path]: { listing, loading: false } }));
      return listing;
    } catch (e) {
      setDirs((d) => ({ ...d, [path]: { ...d[path], loading: false, error: tr(String(e)) } }));
      return null;
    }
  };

  const openRoot = async (path: string) => {
    const conn = connRef.current;
    if (conn == null) return;
    setRootError(null);
    try {
      const listing = await api.listEntries(conn, path);
      setRoot(listing.path);
      setExpanded(new Set());
      setSelected(null);
      setGit(null);
      gitCheckedAt.current = 0;
      setDirs({ [listing.path]: { listing, loading: false } });
    } catch (e) {
      setRootError(tr(String(e)));
    }
  };

  useEffect(() => { void openRoot(requestedRoot); }, [rootSeq]); // eslint-disable-line react-hooks/exhaustive-deps
  // 断开后重新连上：根目录还没读出来就再读一次
  useEffect(() => { if (online && root == null) void openRoot(requestedRoot); }, [online]); // eslint-disable-line react-hooks/exhaustive-deps

  const refreshTree = () => {
    if (root == null) return;
    for (const path of [root, ...expanded]) void loadDir(path);
    setFileCache(null);
    gitCheckedAt.current = 0;
    void pollGit();
  };

  const expand = (path: string, open: boolean) => {
    setExpanded((s) => {
      if (s.has(path) === open) return s;
      const next = new Set(s);
      if (open) next.add(path); else next.delete(path);
      return next;
    });
    if (open && !treeRef.current.dirs[path]?.listing) void loadDir(path);
  };

  const rows = useMemo(() => {
    const out: Row[] = [];
    const walk = (path: string, depth: number) => {
      const listing = dirs[path]?.listing;
      if (!listing) return;
      for (const entry of listing.entries) {
        const full = joinPath(path, entry.name);
        out.push({ kind: "entry", path: full, name: entry.name, dir: entry.dir, depth });
        if (!entry.dir || !expanded.has(full)) continue;
        const child = dirs[full];
        if (child?.error) out.push({ kind: "note", key: full + ":error", text: child.error, depth: depth + 1, error: true });
        else if (!child?.listing) out.push({ kind: "note", key: full + ":loading", text: t("读取中…"), depth: depth + 1 });
        else if (child.listing.entries.length === 0) out.push({ kind: "note", key: full + ":empty", text: t("空目录"), depth: depth + 1 });
        else walk(full, depth + 1);
      }
      if (listing.truncated) out.push({ kind: "note", key: path + ":more", text: t("只列出了前 {{count}} 项", { count: listing.entries.length }), depth });
    };
    if (root != null) walk(root, 0);
    return out;
  }, [dirs, expanded, root]);

  const marks = useMemo(() => gitMarks(git, root), [git, root]);

  // ---------- 编辑器 ----------
  useEffect(() => {
    const instance = monaco.editor.create(host.current!, { ...editorOptions(), model: null });
    instance.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => {
      if (activeRef.current) void save(activeRef.current);
    });
    // Monaco 拿到焦点时按键不会冒泡到外面，快捷键在这里也注册一份
    instance.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyP, () => setQuickOpen(true));
    instance.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyMod.Shift | monaco.KeyCode.KeyF, () => setSide("search"));
    editor.current = instance;
    const manager = new LspManager({
      connId: () => connRef.current,
      root: () => treeRef.current.root,
      loadModel: (path) => loadModel(path),
      pathOf: (uri) => {
        const key = uri.toString();
        for (const [path, entry] of [...loaded.current, ...peeked.current]) if (entry.model.uri.toString() === key) return path;
        return null;
      },
      open: (path, range) => {
        setSelected(path);
        void openFile(path, { path, line: range.startLineNumber, col: range.startColumn, len: range.startLineNumber === range.endLineNumber ? range.endColumn - range.startColumn : 0 });
      },
      changed: () => setLspTick((n) => n + 1),
    });
    lsp.current = manager;
    const models = loaded.current;
    const peeks = peeked.current;
    return () => {
      manager.dispose();
      lsp.current = null;
      instance.dispose();
      diffEditor.current?.dispose();
      diffRef.current?.original.dispose();
      for (const { model } of models.values()) model.dispose();
      for (const { model } of peeks.values()) model.dispose();
      models.clear();
      peeks.clear();
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // 连接断开或重连：语言服务跟着连接走，换了连接要重新开
  const lspConn = useRef(connId);
  useEffect(() => {
    if (lspConn.current === connId) return;
    lspConn.current = connId;
    lsp.current?.reset();
  }, [connId]);

  const activeFile = files.find((f) => f.path === activePath) ?? null;

  const reveal = (instance: monaco.editor.ICodeEditor, target: SearchTarget) => {
    const range = new monaco.Range(target.line, target.col, target.line, target.col + target.len);
    instance.setSelection(range);
    // 编辑器刚从隐藏变成显示时还没量好宽度，马上滚动会横着滚过头；等下一帧排好版再滚
    requestAnimationFrame(() => {
      instance.layout();
      instance.revealRangeInCenterIfOutsideViewport(range, monaco.editor.ScrollType.Immediate);
      instance.focus();
    });
  };

  // 切换文件：记住上一个文件的光标和滚动位置，换上新文件的
  const shownPath = useRef<string | null>(null);
  useEffect(() => {
    const instance = editor.current;
    if (!instance) return;
    const model = activeFile?.state === "ready" ? loaded.current.get(activeFile.path)?.model ?? null : null;
    if (instance.getModel() !== model) {
      if (shownPath.current) viewStates.current.set(shownPath.current, instance.saveViewState());
      instance.setModel(model);
      shownPath.current = model ? activeFile!.path : null;
      if (model) {
        const view = viewStates.current.get(activeFile!.path);
        if (view) instance.restoreViewState(view);
        const target = pendingReveal.current.get(activeFile!.path);
        pendingReveal.current.delete(activeFile!.path);
        if (target) reveal(instance, target);
        else if (active) instance.focus();
      }
    }
    instance.updateOptions({ readOnly: !online || !activeFile || activeFile.state !== "ready" || !!activeFile.meta?.readonly });
    diffEditor.current?.getModifiedEditor().updateOptions({ readOnly: !online || !!activeFile?.meta?.readonly });
  }, [activeFile, online, active]);

  // 换到别的文件就关掉对比
  useEffect(() => { if (diff && diff.path !== activePath) closeDiff(); }, [activePath]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => { onDirty(files.filter((f) => f.dirty).length); }, [files]); // eslint-disable-line react-hooks/exhaustive-deps

  const openFile = async (path: string, target?: SearchTarget) => {
    if (target) pendingReveal.current.set(path, target);
    const existing = filesRef.current.find((f) => f.path === path);
    if (existing) {
      // 已经开着又正显示着：模型不会换，直接跳
      if (target && activeRef.current === path && existing.state === "ready" && editor.current && !diffRef.current) {
        pendingReveal.current.delete(path);
        reveal(editor.current, target);
      }
      setActivePath(path);
      void recheck(path);
      return;
    }
    setActivePath(path);
    const name = baseName(path);
    setFiles((list) => [...list, { path, name, state: "loading", dirty: false, saving: false, conflict: false, remoteChanged: false, gone: false, eol: "LF", mixed: false }]);
    const conn = connRef.current;
    const early = peeked.current.get(path);
    if (conn == null && !early) return update(path, { state: "error", error: t("连接已断开") });
    try {
      let file: Peeked;
      if (early) {
        // 跳转时已经读过了：直接用，对方电脑上有没有变交给定时核对
        peeked.current.delete(path);
        file = early;
      } else {
        const { meta, text } = await api.readFile(conn!, path);
        // 等待期间标签被关掉了
        if (!filesRef.current.some((f) => f.path === path)) return;
        const { eol, mixed } = detectEol(text);
        const model = monaco.editor.createModel(text, languageFor(name), modelUri("f", name));
        model.setEOL(EOL(eol));
        file = { model, meta, eol, mixed };
      }
      const { model, meta, eol, mixed } = file;
      const entry: Loaded = { model, saved: model.getAlternativeVersionId(), checkedAt: early ? 0 : Date.now() };
      loaded.current.set(path, entry);
      model.onDidChangeContent(() => {
        // 改名以后路径会变，按模型找回现在的路径
        const current = [...loaded.current].find(([, l]) => l === entry)?.[0];
        if (!current) return;
        const dirty = model.getAlternativeVersionId() !== entry.saved;
        if (filesRef.current.find((f) => f.path === current)?.dirty !== dirty) update(current, { dirty });
      });
      update(path, { state: "ready", meta, eol, mixed });
      lsp.current?.attach(path, model);
      if (pendingDiff.current === path) {
        pendingDiff.current = null;
        void compareWithHead(path);
      }
    } catch (e) {
      pendingReveal.current.delete(path);
      update(path, { state: "error", error: tr(String(e)) });
    }
  };

  /** 跳转定义、查引用要看别的文件：已经开着的直接用，没开的先读出来，不开标签 */
  const loadModel = async (path: string) => {
    const known = loaded.current.get(path) ?? peeked.current.get(path);
    if (known) return known.model;
    const conn = connRef.current;
    if (conn == null) return null;
    const { meta, text } = await api.readFile(conn, path);
    const again = loaded.current.get(path) ?? peeked.current.get(path);
    if (again) return again.model;
    const { eol, mixed } = detectEol(text);
    const model = monaco.editor.createModel(text, languageFor(baseName(path)), modelUri("p", baseName(path)));
    model.setEOL(EOL(eol));
    peeked.current.set(path, { model, meta, eol, mixed });
    if (peeked.current.size > MAX_PEEKED) {
      const [oldest, entry] = peeked.current.entries().next().value!;
      peeked.current.delete(oldest);
      entry.model.dispose();
    }
    return model;
  };

  /** 用对方电脑上的最新内容替换；放进撤销记录，按撤销还能回到替换前 */
  const replaceWith = (path: string, meta: FileMeta, text: string) => {
    const entry = loaded.current.get(path);
    if (!entry) return;
    const { eol, mixed } = detectEol(text);
    const { model } = entry;
    model.pushStackElement();
    model.setEOL(EOL(eol));
    model.pushEditOperations([], [{ range: model.getFullModelRange(), text }], () => null);
    model.pushStackElement();
    entry.saved = model.getAlternativeVersionId();
    entry.checkedAt = Date.now();
    update(path, { meta, eol, mixed, dirty: false, conflict: false, remoteChanged: false, gone: false, saveError: undefined });
  };

  /** 读对方电脑上的最新内容：没改过的直接换上；有未保存修改的只标出来，由用户决定 */
  const pullRemote = async (path: string) => {
    const conn = connRef.current;
    const entry = loaded.current.get(path);
    if (conn == null || !entry) return;
    entry.checkedAt = Date.now();
    const { meta, text } = await api.readFile(conn, path);
    const now = filesRef.current.find((f) => f.path === path);
    if (!now || now.saving) return;
    if (now.meta?.hash === meta.hash) {
      // 只是修改时间变了（比如被 touch），内容没变：记下新的时间，下次不用再读
      if (now.meta.mtime !== meta.mtime || now.gone) update(path, { meta, gone: false });
      return;
    }
    if (now.dirty) update(path, { remoteChanged: true, gone: false });
    else replaceWith(path, meta, text);
  };

  /** 切回某个文件时看一眼；太频繁就跳过 */
  const recheck = async (path: string) => {
    const entry = loaded.current.get(path);
    const file = filesRef.current.find((f) => f.path === path);
    if (!entry || !file || file.state !== "ready" || file.saving || connRef.current == null) return;
    if (Date.now() - entry.checkedAt < RECHECK_MS) return;
    await pullRemote(path).catch(() => {});
  };

  // ---------- 定时核对对方电脑上的变化 ----------
  const gitCheckedAt = useRef(0);
  const pollGit = async () => {
    const conn = connRef.current;
    const currentRoot = treeRef.current.root;
    if (conn == null || currentRoot == null) return;
    // 不在仓库里时不用每次都问
    if (!gitRef.current && gitCheckedAt.current && Date.now() - gitCheckedAt.current < GIT_RETRY_MS) return;
    gitCheckedAt.current = Date.now();
    const status = await api.gitStatus(conn, currentRoot).catch(() => null);
    if (treeRef.current.root !== currentRoot) return;
    setGit((old) => JSON.stringify(old) === JSON.stringify(status) ? old : status);
  };

  const pollFiles = async () => {
    const conn = connRef.current;
    const ready = filesRef.current.filter((f) => f.state === "ready" && f.meta && !f.saving);
    if (conn == null || ready.length === 0) return;
    const stats = await api.statFiles(conn, ready.map((f) => f.path));
    await Promise.all(ready.map(async (file, i) => {
      const stat = stats[i];
      const now = filesRef.current.find((f) => f.path === file.path);
      if (!now?.meta || now.saving) return;
      if (!stat || stat.dir) {
        if (!now.gone) update(file.path, { gone: true });
        return;
      }
      if (!now.gone && stat.size === now.meta.size && stat.mtime === now.meta.mtime) return;
      // 有未保存修改并且已经提示过的，不用再读
      if (now.dirty && now.remoteChanged) return;
      await pullRemote(file.path).catch(() => {});
    }));
  };

  const pollTree = async () => {
    const conn = connRef.current;
    const { root: currentRoot, expanded: open } = treeRef.current;
    if (conn == null || currentRoot == null) return;
    // 展开得太多时只看前 30 个，免得每轮发一大串请求
    const paths = [currentRoot, ...[...open].slice(0, 30)];
    const results = await Promise.all(paths.map((path) => api.listEntries(conn, path).then((l) => [path, l] as const, () => [path, null] as const)));
    if (treeRef.current.root !== currentRoot) return;
    const missing = results.filter(([path, listing]) => !listing && path !== currentRoot).map(([path]) => path);
    setDirs((d) => {
      let next = d;
      for (const [path, listing] of results) {
        if (!listing || JSON.stringify(d[path]?.listing) === JSON.stringify(listing)) continue;
        if (next === d) next = { ...d };
        next[path] = { listing, loading: false };
      }
      return next;
    });
    // 展开着的目录被删了：收起来
    if (missing.length) setExpanded((s) => new Set([...s].filter((p) => !missing.some((m) => isUnder(p, m)))));
  };

  const polling = useRef(false);
  useEffect(() => {
    if (!active || !online) return;
    const tick = async () => {
      if (polling.current || document.hidden) return;
      polling.current = true;
      try { await Promise.allSettled([pollFiles(), pollTree(), pollGit()]); }
      finally { polling.current = false; }
    };
    void tick();
    const timer = window.setInterval(() => void tick(), POLL_MS);
    return () => window.clearInterval(timer);
  }, [active, online, root]); // eslint-disable-line react-hooks/exhaustive-deps

  // ---------- 保存 ----------
  const save = async (path: string, force = false) => {
    const file = filesRef.current.find((f) => f.path === path);
    const entry = loaded.current.get(path);
    const conn = connRef.current;
    if (!file || !entry || file.state !== "ready" || !file.meta || file.saving || conn == null || file.meta.readonly) return;
    if (!file.dirty && !force && !file.mixed) return;
    const version = entry.model.getAlternativeVersionId();
    const text = entry.model.getValue();
    update(path, { saving: true, saveError: undefined });
    try {
      const meta = await api.writeFile(conn, path, text, file.meta.bom, file.meta.hash, force);
      entry.saved = version;
      entry.checkedAt = Date.now();
      lsp.current?.saved(entry.model);
      update(path, { meta, saving: false, conflict: false, remoteChanged: false, gone: false, mixed: false, dirty: entry.model.getAlternativeVersionId() !== version });
      if (file.gone) void loadDir(dirName(path));
    } catch (e) {
      const message = String(e);
      if (message.includes(CONFLICT_PREFIX)) update(path, { saving: false, conflict: true });
      else update(path, { saving: false, saveError: tr(message) });
    }
  };

  const reload = async (path: string) => {
    const conn = connRef.current;
    if (conn == null) return;
    try {
      const { meta, text } = await api.readFile(conn, path);
      replaceWith(path, meta, text);
      closeDiff();
    } catch (e) {
      update(path, { saveError: tr(String(e)) });
    }
  };

  const closeFile = (path: string, force = false) => {
    const file = filesRef.current.find((f) => f.path === path);
    if (file?.dirty && !force) {
      setConfirm({
        title: t("“{{name}}”有未保存的修改", { name: file.name }),
        body: t("不保存就关闭，这些修改会丢失。"),
        confirm: t("不保存，直接关闭"),
        onConfirm: () => closeFile(path, true),
        extra: online && !file.meta?.readonly ? {
          label: t("保存并关闭"),
          onPress: () => void save(path).then(() => {
            if (!filesRef.current.find((f) => f.path === path)?.dirty) closeFile(path, true);
          }),
        } : undefined,
      });
      return;
    }
    if (diffRef.current?.path === path) closeDiff();
    const entry = loaded.current.get(path);
    if (shownPath.current === path) {
      editor.current?.setModel(null);
      shownPath.current = null;
    }
    if (entry) lsp.current?.detach(entry.model);
    entry?.model.dispose();
    loaded.current.delete(path);
    viewStates.current.delete(path);
    const rest = filesRef.current.filter((f) => f.path !== path);
    setFiles(rest);
    if (activeRef.current === path) {
      const index = filesRef.current.findIndex((f) => f.path === path);
      setActivePath(rest[Math.min(index, rest.length - 1)]?.path ?? null);
    }
  };

  // ---------- 对比 ----------
  useEffect(() => {
    if (!diff) return;
    const entry = loaded.current.get(diff.path);
    if (!entry || !diffHost.current) return;
    if (!diffEditor.current) {
      diffEditor.current = monaco.editor.createDiffEditor(diffHost.current, {
        ...editorOptions(),
        originalEditable: false,
        renderSideBySide: true,
        // 窗口窄时自动改成上下合在一起显示
        useInlineViewWhenSpaceIsLimited: true,
      });
      diffEditor.current.getModifiedEditor().addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => {
        if (activeRef.current) void save(activeRef.current);
      });
    }
    diffEditor.current.setModel({ original: diff.original, modified: entry.model });
    diffEditor.current.getModifiedEditor().updateOptions({ readOnly: !online || !!entry && !!filesRef.current.find((f) => f.path === diff.path)?.meta?.readonly });
  }, [diff]); // eslint-disable-line react-hooks/exhaustive-deps

  const closeDiff = () => {
    const current = diffRef.current;
    if (!current) return;
    diffEditor.current?.setModel(null);
    current.original.dispose();
    setDiff(null);
  };

  const showDiff = (path: string, title: string, text: string) => {
    closeDiff();
    const original = monaco.editor.createModel(text, languageFor(baseName(path)), modelUri("d", baseName(path)));
    setActivePath(path);
    setDiff({ path, title, original });
  };

  const compareWithHead = async (path: string) => {
    const conn = connRef.current;
    if (conn == null) return;
    if (!loaded.current.has(path)) {
      pendingDiff.current = path;
      return void openFile(path);
    }
    try {
      const { text } = await api.readFile(conn, path, true);
      showDiff(path, t("左边是 Git 最新提交里的版本，右边是现在的内容（可以接着编辑）"), text);
    } catch (e) {
      toast.error(tr(String(e)));
    }
  };

  const compareWithRemote = async (path: string) => {
    const conn = connRef.current;
    if (conn == null) return;
    try {
      const { text } = await api.readFile(conn, path);
      showDiff(path, t("左边是对方电脑上现在的版本，右边是你的修改（可以接着编辑）"), text);
    } catch (e) {
      toast.error(tr(String(e)));
    }
  };

  // ---------- 新建、重命名、删除 ----------
  /** 改名或移动以后，打开着的文件跟着换路径 */
  const moveOpenFiles = (from: string, to: string) => {
    const moved = filesRef.current.filter((f) => isUnder(f.path, from));
    if (!moved.length) return;
    for (const f of moved) {
      const next = movePath(f.path, from, to);
      const entry = loaded.current.get(f.path);
      if (entry) {
        lsp.current?.detach(entry.model);
        loaded.current.delete(f.path);
        loaded.current.set(next, entry);
        monaco.editor.setModelLanguage(entry.model, languageFor(baseName(next)));
        lsp.current?.attach(next, entry.model);
      }
      if (viewStates.current.has(f.path)) {
        viewStates.current.set(next, viewStates.current.get(f.path)!);
        viewStates.current.delete(f.path);
      }
      if (shownPath.current === f.path) shownPath.current = next;
    }
    setFiles((list) => list.map((f) => isUnder(f.path, from) ? { ...f, path: movePath(f.path, from, to), name: baseName(movePath(f.path, from, to)), meta: f.meta && { ...f.meta, path: movePath(f.path, from, to) } } : f));
    if (activeRef.current && isUnder(activeRef.current, from)) setActivePath(movePath(activeRef.current, from, to));
    if (diffRef.current && isUnder(diffRef.current.path, from)) setDiff({ ...diffRef.current, path: movePath(diffRef.current.path, from, to) });
  };

  const afterChange = (dir: string) => {
    setFileCache(null);
    void loadDir(dir);
    if (root != null && dir !== root) expand(dir, true);
    gitCheckedAt.current = 0;
    void pollGit();
  };

  const submitName = async (name: string) => {
    const conn = connRef.current;
    if (conn == null || !naming) throw new Error(t("连接已断开"));
    if (naming.kind === "rename") {
      const to = joinPath(dirName(naming.path), name);
      if (to === naming.path) return;
      await api.renamePath(conn, naming.path, to);
      moveOpenFiles(naming.path, to);
      setExpanded((s) => new Set([...s].map((p) => isUnder(p, naming.path) ? movePath(p, naming.path, to) : p)));
      if (selected && isUnder(selected, naming.path)) setSelected(movePath(selected, naming.path, to));
      afterChange(dirName(naming.path));
      return;
    }
    const path = joinPath(naming.dir, name);
    if (naming.kind === "file") {
      await api.createFile(conn, path);
      afterChange(naming.dir);
      setSelected(path);
      void openFile(path);
    } else {
      await api.createDir(conn, path);
      afterChange(naming.dir);
      setSelected(path);
    }
  };

  const askDelete = (path: string, dir: boolean) => {
    setConfirm({
      title: t("把“{{name}}”移到废纸篓？", { name: baseName(path) }),
      body: dir
        ? t("这个文件夹和里面的所有内容会移到对方电脑的废纸篓（回收站），需要时可以从那里找回。")
        : t("会移到对方电脑的废纸篓（回收站），需要时可以从那里找回。"),
      confirm: t("移到废纸篓"),
      onConfirm: () => void (async () => {
        const conn = connRef.current;
        if (conn == null) return;
        try {
          await api.deletePath(conn, path);
        } catch (e) {
          toast.error(tr(String(e)));
          return;
        }
        // 没改过的直接关掉；有未保存修改的留着，标成已删除，保存会重新创建
        for (const f of filesRef.current.filter((f) => isUnder(f.path, path))) {
          if (f.dirty) update(f.path, { gone: true });
          else closeFile(f.path, true);
        }
        setExpanded((s) => new Set([...s].filter((p) => !isUnder(p, path))));
        if (selected && isUnder(selected, path)) setSelected(null);
        afterChange(dirName(path));
      })(),
    });
  };

  /** 新建放在哪：选中的是目录就放里面，是文件就放它旁边，都没选就放根目录 */
  const targetDir = () => {
    if (root == null) return null;
    if (!selected) return root;
    const row = rows.find((r) => r.kind === "entry" && r.path === selected);
    return row?.kind === "entry" && row.dir ? row.path : dirName(selected);
  };

  const openMenu = (e: MouseEvent, row: Extract<Row, { kind: "entry" }> | null) => {
    if (root == null || !online) return;
    if (row) setSelected(row.path);
    const dir = row ? (row.dir ? row.path : dirName(row.path)) : root;
    const kind = row ? marks.files.get(row.path) : undefined;
    const actions: MenuAction[] = [
      { id: "new-file", label: t("新建文件"), icon: <FilePlus />, onAction: () => setNaming({ kind: "file", dir }) },
      { id: "new-dir", label: t("新建文件夹"), icon: <FolderPlus />, onAction: () => setNaming({ kind: "dir", dir }) },
    ];
    if (row && !row.dir && (kind === "modified" || kind === "conflict")) {
      actions.push({ id: "diff", label: t("和 Git 版本对比"), icon: <GitCompare />, separated: true, onAction: () => void compareWithHead(row.path) });
    }
    if (row) {
      actions.push(
        { id: "rename", label: t("重命名"), icon: <Pencil />, separated: !(row && !row.dir && (kind === "modified" || kind === "conflict")), onAction: () => setNaming({ kind: "rename", path: row.path }) },
        { id: "delete", label: t("移到废纸篓"), icon: <Trash2 />, danger: true, onAction: () => askDelete(row.path, row.dir) },
      );
      if (row.dir) actions.push({ id: "root", label: t("以这个文件夹为根目录"), icon: <FolderOpen />, separated: true, onAction: () => void openRoot(row.path) });
    }
    actions.push({
      id: "copy", label: t("复制路径"), icon: <Copy />, separated: !row?.dir,
      onAction: () => void writeClipboardText(row?.path ?? root).catch((err) => toast.error(String(err))),
    });
    menu.open(e, actions);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    const mod = e.metaKey || e.ctrlKey;
    if (mod && !e.shiftKey && e.key.toLowerCase() === "p") { e.preventDefault(); setQuickOpen(true); }
    else if (mod && e.shiftKey && e.key.toLowerCase() === "f") { e.preventDefault(); setSide("search"); }
  };

  const parent = root != null ? dirs[root]?.listing?.parent ?? null : null;
  const dirtyCount = files.filter((f) => f.dirty).length;
  const activeKind = activeFile ? marks.files.get(activeFile.path) : undefined;
  const changedElsewhere = activeFile && (activeFile.conflict || activeFile.remoteChanged) && !activeFile.gone;
  const activeModel = activeFile?.state === "ready" ? loaded.current.get(activeFile.path)?.model : undefined;
  const lspStatus = activeFile && activeModel && !activeModel.isDisposed() ? lsp.current?.status(activeFile.path, activeModel.getLanguageId()) ?? null : null;

  return (
    <section className="remote-editor" aria-label={t("代码编辑器")} onKeyDown={onKeyDown}>
      <header className="remote-editor-head">
        <Button variant="ghost" size="icon-xs" aria-label={t("上一级")} title={t("上一级")} isDisabled={!online || !parent}
          onPress={() => parent && void openRoot(parent)}><ArrowUp /></Button>
        <Button variant="ghost" size="icon-xs" aria-label={t("用户目录")} title={t("用户目录")} isDisabled={!online}
          onPress={() => void openRoot("")}><House /></Button>
        <span className="remote-editor-root" title={root ?? ""}><bdi>{root ?? requestedRoot}</bdi></span>
        <Button variant="ghost" size="icon-xs" aria-label={t("按名字打开文件")} title={t("按名字打开文件（⌘/Ctrl+P）")} isDisabled={!online || root == null}
          onPress={() => setQuickOpen(true)}><Search /></Button>
        <Button variant="ghost" size="icon-xs" aria-label={t("刷新文件列表")} title={t("刷新文件列表")} isDisabled={!online || root == null}
          onPress={refreshTree}><RefreshCw /></Button>
        <Button variant="ghost" size="icon-xs" aria-label={t("关闭编辑器")} title={t("关闭编辑器")} onPress={() => {
          if (dirtyCount) {
            setConfirm({
              title: t("有 {{count}} 个文件未保存", { count: dirtyCount }),
              body: t("关闭编辑器会丢失这些修改。"),
              confirm: t("不保存，关闭编辑器"),
              onConfirm: onClose,
            });
          } else onClose();
        }}><X /></Button>
      </header>
      {!online && <div className="remote-editor-banner" role="status">
        <CircleAlert />{t("连接已断开。重新连接这台设备后才能读取和保存，未保存的修改会一直保留。")}
      </div>}
      <div className="remote-editor-body">
        <div className="remote-editor-side">
          <div className="remote-editor-side-tabs" role="tablist">
            <button type="button" role="tab" aria-selected={side === "files"} className={cn(side === "files" && "is-active")}
              onClick={() => setSide("files")} title={t("文件")}><Files />{t("文件")}</button>
            <button type="button" role="tab" aria-selected={side === "search"} className={cn(side === "search" && "is-active")}
              onClick={() => setSide("search")} title={t("在文件中搜索（⌘/Ctrl+Shift+F）")}><TextSearch />{t("搜索")}</button>
            <span className="grow" />
            {side === "files" && <>
              <Button variant="ghost" size="icon-xs" aria-label={t("新建文件")} title={t("新建文件")} isDisabled={!online || root == null}
                onPress={() => { const dir = targetDir(); if (dir) setNaming({ kind: "file", dir }); }}><FilePlus /></Button>
              <Button variant="ghost" size="icon-xs" aria-label={t("新建文件夹")} title={t("新建文件夹")} isDisabled={!online || root == null}
                onPress={() => { const dir = targetDir(); if (dir) setNaming({ kind: "dir", dir }); }}><FolderPlus /></Button>
            </>}
          </div>
          <nav className="remote-editor-tree" aria-label={t("文件")} hidden={side !== "files"}
            onContextMenu={(e) => { if (e.target === e.currentTarget) openMenu(e, null); }}>
            {rootError && <div className="remote-editor-note is-error">{rootError}</div>}
            {root == null && !rootError && <div className="remote-editor-note">{t("读取中…")}</div>}
            {rows.map((row) => {
              if (row.kind === "note") {
                return <div key={row.key} className={cn("remote-editor-note", row.error && "is-error")} style={{ paddingLeft: 10 + row.depth * 12 }}>{row.text}</div>;
              }
              const kind = marks.files.get(row.path);
              const changedInside = row.dir && marks.dirs.has(row.path);
              return <button key={row.path} type="button" title={row.path}
                className={cn("remote-editor-node", row.path === activePath && "is-active", row.path === selected && "is-selected",
                  row.name.startsWith(".") && "is-hidden", kind && `git-${kind}`, changedInside && "git-inside")}
                style={{ paddingLeft: 6 + row.depth * 12 }}
                onContextMenu={(e) => openMenu(e, row)}
                onClick={() => { setSelected(row.path); if (row.dir) expand(row.path, !expanded.has(row.path)); else void openFile(row.path); }}>
                {row.dir
                  ? <>{expanded.has(row.path) ? <ChevronDown className="chevron" /> : <ChevronRight className="chevron" />}
                    {expanded.has(row.path) ? <FolderOpen /> : <Folder />}</>
                  : <><span className="chevron" /><File /></>}
                <span className="truncate">{row.name}</span>
                {kind && <span className="remote-editor-git-letter">{GIT_LETTER[kind]}</span>}
                {!kind && changedInside && <span className="remote-editor-git-dot" aria-hidden="true" />}
              </button>;
            })}
          </nav>
          {side === "search" && <SearchPanel connId={connId} root={root} onOpen={(target) => void openFile(target.path, target)} />}
        </div>
        <div className="remote-editor-main">
          {files.length > 0 && <div className="remote-editor-tabs" role="tablist" aria-label={t("打开的文件")}>
            {files.map((f) => {
              const kind = marks.files.get(f.path);
              return (
                <div key={f.path} role="tab" aria-selected={f.path === activePath} title={f.path}
                  className={cn("remote-editor-tab", f.path === activePath && "is-active", kind && `git-${kind}`, f.gone && "is-gone")}
                  onClick={() => setActivePath(f.path)}
                  onAuxClick={(e) => { if (e.button === 1) closeFile(f.path); }}>
                  <span className="truncate">{f.name}</span>
                  <button type="button" className="remote-editor-tab-close" data-dirty={f.dirty || undefined}
                    aria-label={t("关闭 {{name}}", { name: f.name })} onClick={(e) => { e.stopPropagation(); closeFile(f.path); }}>
                    <span className="dot" aria-hidden="true" /><X />
                  </button>
                </div>
              );
            })}
          </div>}
          {changedElsewhere && <div className="remote-editor-banner is-warning" role="alert">
            <TriangleAlert />
            <span>{activeFile.conflict
              ? t("这个文件在对方电脑上已被修改（可能是终端里的程序或 AI 改的）。")
              : t("对方电脑上的这个文件刚被修改了，而你这里还有未保存的修改。")}</span>
            <Button size="xs" variant="outline" onPress={() => void compareWithRemote(activeFile.path)}><GitCompare />{t("对比")}</Button>
            <Button size="xs" variant="outline" onPress={() => void reload(activeFile.path)}>{t("用对方的版本")}</Button>
            <Button size="xs" variant="destructive" onPress={() => void save(activeFile.path, true)}>{t("用我的覆盖")}</Button>
          </div>}
          {activeFile?.gone && <div className="remote-editor-banner is-warning" role="alert">
            <TriangleAlert />
            <span>{t("对方电脑上已经没有这个文件了（被删除或移走）。保存会在原来的位置重新创建它。")}</span>
            {activeFile.state === "ready" && !activeFile.meta?.readonly && online &&
              <Button size="xs" variant="outline" onPress={() => void save(activeFile.path, true)}>{t("重新创建")}</Button>}
            <Button size="xs" variant="outline" onPress={() => closeFile(activeFile.path)}>{t("关闭")}</Button>
          </div>}
          {activeFile?.saveError && <div className="remote-editor-banner is-error" role="alert">
            <CircleAlert /><span>{t("保存失败：{{error}}", { error: activeFile.saveError })}</span>
          </div>}
          {diff && <div className="remote-editor-banner is-diff" role="status">
            <GitCompare /><span>{diff.title}</span>
            <Button size="xs" variant="outline" onPress={closeDiff}>{t("关闭对比")}</Button>
          </div>}
          <div className="remote-editor-surface">
            <div ref={host} className="remote-editor-monaco" hidden={activeFile?.state !== "ready" || !!diff} />
            <div ref={diffHost} className="remote-editor-monaco" hidden={!diff} />
            {!activeFile && <Placeholder>{t("在左边选择一个文件打开，或按 ⌘/Ctrl+P 按名字找文件。编辑在本机进行，⌘/Ctrl+S 保存到对方电脑。")}</Placeholder>}
            {activeFile?.state === "loading" && <Placeholder><LoaderCircle className="animate-spin" />{t("正在打开…")}</Placeholder>}
            {activeFile?.state === "error" && <Placeholder error>{activeFile.error}</Placeholder>}
          </div>
          {(activeFile?.state === "ready" || git) && <footer className="remote-editor-status">
            {git?.branch && <span className="remote-editor-branch" title={t("Git 分支")}><GitBranch />{git.branch}</span>}
            {activeFile?.state === "ready" && <>
              <span>{languageLabel(activeFile.name)}</span>
              <span>{activeFile.meta?.bom ? "UTF-8 BOM" : "UTF-8"}</span>
              <span title={activeFile.mixed ? t("文件里混用了 LF 和 CRLF，保存时会统一成 {{eol}}", { eol: activeFile.eol }) : undefined}>
                {activeFile.eol}{activeFile.mixed && " ⚠"}
              </span>
              {activeFile.meta?.readonly && <span>{t("只读")}</span>}
              {lspStatus && <LspBadge status={lspStatus} onRetry={() => lsp.current?.retry(lspStatus.family)} />}
            </>}
            <span className="grow" />
            {activeFile?.state === "ready" && <>
              {(activeKind === "modified" || activeKind === "conflict") && !diff &&
                <Button size="xs" variant="ghost" title={t("和 Git 最新提交里的版本对比")} onPress={() => void compareWithHead(activeFile.path)}>
                  <GitCompare />{t("对比")}
                </Button>}
              <span>{activeFile.saving ? t("正在保存…") : activeFile.dirty ? t("未保存") : t("已保存")}</span>
              <Button size="xs" variant="ghost" isDisabled={!online || !activeFile.dirty || activeFile.saving || !!activeFile.meta?.readonly}
                title={t("保存（⌘/Ctrl+S）")} onPress={() => void save(activeFile.path)}><Save />{t("保存")}</Button>
            </>}
          </footer>}
        </div>
      </div>
      {quickOpen && root != null && <QuickOpen connId={connId} root={root} cache={fileCache} onCache={setFileCache}
        onOpen={(path) => { setSelected(path); void openFile(path); }} onClose={() => { setQuickOpen(false); editor.current?.focus(); }} />}
      {naming && <NameDialog
        title={naming.kind === "rename" ? t("重命名") : naming.kind === "file" ? t("新建文件") : t("新建文件夹")}
        description={naming.kind === "rename" ? naming.path : t("放在 {{dir}}", { dir: naming.dir })}
        initial={naming.kind === "rename" ? baseName(naming.path) : ""}
        confirm={naming.kind === "rename" ? t("重命名") : t("新建")}
        onSubmit={submitName}
        onClose={() => setNaming(null)} />}
      {menu.element}
      {confirm && <AlertDialog open>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{confirm.title}</AlertDialogTitle>
            <AlertDialogDescription>{confirm.body}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <Button variant="outline" onPress={() => setConfirm(null)}>{t("取消")}</Button>
            {confirm.extra && <Button onPress={() => { setConfirm(null); confirm.extra!.onPress(); }}>{confirm.extra.label}</Button>}
            <Button variant="destructive" onPress={() => { setConfirm(null); confirm.onConfirm(); }}>{confirm.confirm}</Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>}
    </section>
  );
}

/** 找不到语言服务时告诉用户在对方电脑上装什么 */
function installHint(family: string) {
  switch (family) {
    case "rust": return t("在对方电脑上运行 rustup component add rust-analyzer，或者在 VS Code、Cursor 里装 rust-analyzer 插件。");
    case "go": return t("在对方电脑上运行 go install golang.org/x/tools/gopls@latest。");
    case "python": return t("在对方电脑上运行 pip install basedpyright（或 npm i -g pyright）。");
    case "typescript": return t("在项目里装 typescript 7 或更新的版本（自带语言服务）；用更早的版本时，在对方电脑上运行 npm i -g typescript-language-server。");
    case "c": return t("在对方电脑上装 clangd（Mac 装了 Xcode 命令行工具就有）。");
    case "swift": return t("在对方电脑上装 Xcode 或 Swift 工具链，里面带 sourcekit-lsp。");
    case "ruby": return t("在对方电脑上运行 gem install ruby-lsp。");
    case "php": return t("在对方电脑上运行 npm i -g intelephense。");
    case "shell": return t("在对方电脑上运行 npm i -g bash-language-server。");
    default: return t("在对方电脑上装好这种语言的语言服务（LSP）。");
  }
}

/** 状态栏里语言服务的状态：启动中、索引中、可用、出错、没装 */
function LspBadge({ status, onRetry }: { status: LspStatus; onRetry: () => void }) {
  const full = status.server ?? "";
  // 状态栏地方小：typescript-language-server 显示成 typescript，完整名字在提示里
  const name = full.replace(/[-_]?(language-?server|langserver|lsp)$/i, "") || full;
  if (status.state === "starting") {
    return <span className="remote-editor-lsp" title={t("正在对方电脑上启动语言服务…")}><LoaderCircle className="animate-spin" /><span className="remote-editor-lsp-name">{t("启动语言服务…")}</span></span>;
  }
  if (status.state === "missing") {
    return <button type="button" className="remote-editor-lsp is-off" onClick={onRetry}
      title={t("对方电脑上没有找到这种语言的语言服务，所以没有补全和跳转。") + "\n" + installHint(status.family) + "\n" + t("装好后点这里重试。")}>
      <ZapOff /><span className="remote-editor-lsp-name">{t("无补全")}</span>
    </button>;
  }
  if (status.state === "error") {
    return <button type="button" className="remote-editor-lsp is-error" onClick={onRetry}
      title={t("{{server}} 停止了，点这里重新启动。", { server: full || t("语言服务") }) + (status.detail ? "\n" + status.detail : "")}>
      <TriangleAlert /><span className="remote-editor-lsp-name">{t("{{server}} 已停止", { server: name || t("语言服务") })}</span>
    </button>;
  }
  const title = t("补全、跳转、报错由对方电脑上的 {{server}} 提供", { server: full }) + (status.root ? "\n" + t("项目目录：{{root}}", { root: status.root }) : "");
  return <span className="remote-editor-lsp" title={status.detail ? title + "\n" + status.detail : title}>
    {status.state === "busy" ? <LoaderCircle className="animate-spin" /> : <Zap />}
    <span className="remote-editor-lsp-name">{name}</span>
    {status.state === "busy" && status.detail && <span className="remote-editor-lsp-detail">{status.detail}</span>}
  </span>;
}

function Placeholder({ children, error }: { children: ReactNode; error?: boolean }) {
  return <div className={cn("remote-editor-placeholder", error && "is-error")}>{children}</div>;
}

function languageLabel(name: string) {
  const id = languageFor(name);
  return monaco.languages.getLanguages().find((l) => l.id === id)?.aliases?.[0] ?? id;
}
