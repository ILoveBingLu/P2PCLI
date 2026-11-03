import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  ArrowUp, ChevronDown, ChevronRight, CircleAlert, File, Folder, FolderOpen, House, LoaderCircle, RefreshCw, Save, TriangleAlert, X,
} from "lucide-react";
import { api, CONFLICT_PREFIX, type EntryListing, type FileMeta } from "../../api";
import { Button } from "@/components/ui/button";
import {
  AlertDialog, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { cn } from "@/lib/utils";
import { t, tr } from "@/i18n";
import { languageFor, monaco } from "./monaco";

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

interface DirState { listing?: EntryListing; loading: boolean; error?: string }

type Row =
  | { kind: "entry"; path: string; name: string; dir: boolean; depth: number }
  | { kind: "note"; key: string; text: string; depth: number; error?: boolean };

interface Confirm { title: string; body: string; confirm: string; onConfirm: () => void; extra?: { label: string; onPress: () => void } }

export function baseName(path: string) {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

/** 路径用对方电脑的分隔符拼：只有反斜杠的是 Windows 路径 */
export function joinPath(base: string, name: string) {
  const sep = base.includes("\\") && !base.includes("/") ? "\\" : "/";
  return base.endsWith(sep) ? base + name : base + sep + name;
}

function detectEol(text: string): { eol: Eol; mixed: boolean } {
  const crlf = text.match(/\r\n/g)?.length ?? 0;
  const lf = (text.match(/\n/g)?.length ?? 0) - crlf;
  return { eol: crlf > lf ? "CRLF" : "LF", mixed: crlf > 0 && lf > 0 };
}

const EOL = (eol: Eol) => eol === "CRLF" ? monaco.editor.EndOfLineSequence.CRLF : monaco.editor.EndOfLineSequence.LF;
/** 没改动的文件切回来时，隔这么久以上才去对方电脑上核对一次 */
const RECHECK_MS = 2000;
let modelSeq = 0;

/**
 * 远程编辑器：左边是对方电脑上的文件树，右边是 Monaco。编辑全在本机内存里，打开、保存时各走一次网络，
 * 网络再慢也不影响打字。保存时核对打开时的哈希，文件被终端里的 AI 等改过就提示冲突，不悄悄盖掉。
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
  const [files, setFiles] = useState<OpenFile[]>([]);
  const [activePath, setActivePath] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<Confirm | null>(null);

  const host = useRef<HTMLDivElement>(null);
  const editor = useRef<monaco.editor.IStandaloneCodeEditor | null>(null);
  const loaded = useRef(new Map<string, Loaded>());
  const viewStates = useRef(new Map<string, monaco.editor.ICodeEditorViewState | null>());
  const filesRef = useRef(files);
  filesRef.current = files;
  const connRef = useRef(connId);
  connRef.current = connId;
  const activeRef = useRef(activePath);
  activeRef.current = activePath;

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
  };

  const toggleDir = (path: string) => {
    const open = !expanded.has(path);
    setExpanded((s) => {
      const next = new Set(s);
      if (open) next.add(path); else next.delete(path);
      return next;
    });
    if (open && !dirs[path]?.listing) void loadDir(path);
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

  // ---------- 编辑器 ----------
  useEffect(() => {
    const instance = monaco.editor.create(host.current!, {
      theme: "p2pcli-dark",
      automaticLayout: true,
      fontFamily: getComputedStyle(document.documentElement).getPropertyValue("--font-mono").trim() || "monospace",
      fontSize: 13,
      scrollBeyondLastLine: false,
      // 编辑器只占半个窗口，缩略图太占地方
      minimap: { enabled: false },
      // 中文标点会被当成“容易混淆的字符”框起来，写中文注释时满屏都是框
      unicodeHighlight: { ambiguousCharacters: false },
      model: null,
    });
    instance.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => {
      if (activeRef.current) void save(activeRef.current);
    });
    editor.current = instance;
    const models = loaded.current;
    return () => {
      instance.dispose();
      for (const { model } of models.values()) model.dispose();
      models.clear();
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const activeFile = files.find((f) => f.path === activePath) ?? null;

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
        if (active) instance.focus();
      }
    }
    instance.updateOptions({ readOnly: !online || !activeFile || activeFile.state !== "ready" || !!activeFile.meta?.readonly });
  }, [activeFile, online, active]);

  useEffect(() => { onDirty(files.filter((f) => f.dirty).length); }, [files]); // eslint-disable-line react-hooks/exhaustive-deps

  const openFile = async (path: string) => {
    setActivePath(path);
    if (filesRef.current.some((f) => f.path === path)) {
      void recheck(path);
      return;
    }
    const name = baseName(path);
    setFiles((list) => [...list, { path, name, state: "loading", dirty: false, saving: false, conflict: false, eol: "LF", mixed: false }]);
    const conn = connRef.current;
    if (conn == null) return update(path, { state: "error", error: t("连接已断开") });
    try {
      const { meta, text } = await api.readFile(conn, path);
      // 等待期间标签被关掉了
      if (!filesRef.current.some((f) => f.path === path)) return;
      const { eol, mixed } = detectEol(text);
      const model = monaco.editor.createModel(text, languageFor(name), monaco.Uri.from({ scheme: "p2pcli", authority: `f${++modelSeq}`, path: "/" + name }));
      model.setEOL(EOL(eol));
      const entry: Loaded = { model, saved: model.getAlternativeVersionId(), checkedAt: Date.now() };
      loaded.current.set(path, entry);
      model.onDidChangeContent(() => {
        const dirty = model.getAlternativeVersionId() !== entry.saved;
        if (filesRef.current.find((f) => f.path === path)?.dirty !== dirty) update(path, { dirty });
      });
      update(path, { state: "ready", meta, eol, mixed });
    } catch (e) {
      update(path, { state: "error", error: tr(String(e)) });
    }
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
    update(path, { meta, eol, mixed, dirty: false, conflict: false, saveError: undefined });
  };

  /** 没改过的文件：看一眼对方电脑上有没有被改，改了就换成新内容（比如终端里的 AI 刚改过） */
  const recheck = async (path: string) => {
    const entry = loaded.current.get(path);
    const file = filesRef.current.find((f) => f.path === path);
    const conn = connRef.current;
    if (!entry || !file || file.state !== "ready" || file.dirty || file.saving || conn == null) return;
    if (Date.now() - entry.checkedAt < RECHECK_MS) return;
    entry.checkedAt = Date.now();
    try {
      const { meta, text } = await api.readFile(conn, path);
      const now = filesRef.current.find((f) => f.path === path);
      if (!now || now.dirty || now.meta?.hash === meta.hash) return;
      replaceWith(path, meta, text);
    } catch {
      // 读不到（比如被删了）先不打扰，保存时再提示
    }
  };

  useEffect(() => {
    if (!active || !activePath) return;
    void recheck(activePath);
    const onFocus = () => activeRef.current && void recheck(activeRef.current);
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [active, activePath, online]); // eslint-disable-line react-hooks/exhaustive-deps

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
      update(path, { meta, saving: false, conflict: false, mixed: false, dirty: entry.model.getAlternativeVersionId() !== version });
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
    const entry = loaded.current.get(path);
    if (shownPath.current === path) {
      editor.current?.setModel(null);
      shownPath.current = null;
    }
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

  const parent = root != null ? dirs[root]?.listing?.parent ?? null : null;
  const dirtyCount = files.filter((f) => f.dirty).length;

  return (
    <section className="remote-editor" aria-label={t("代码编辑器")}>
      <header className="remote-editor-head">
        <Button variant="ghost" size="icon-xs" aria-label={t("上一级")} title={t("上一级")} isDisabled={!online || !parent}
          onPress={() => parent && void openRoot(parent)}><ArrowUp /></Button>
        <Button variant="ghost" size="icon-xs" aria-label={t("用户目录")} title={t("用户目录")} isDisabled={!online}
          onPress={() => void openRoot("")}><House /></Button>
        <span className="remote-editor-root" title={root ?? ""}><bdi>{root ?? requestedRoot}</bdi></span>
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
        <nav className="remote-editor-tree" aria-label={t("文件")}>
          {rootError && <div className="remote-editor-note is-error">{rootError}</div>}
          {root == null && !rootError && <div className="remote-editor-note">{t("读取中…")}</div>}
          {rows.map((row) => row.kind === "note"
            ? <div key={row.key} className={cn("remote-editor-note", row.error && "is-error")} style={{ paddingLeft: 10 + row.depth * 12 }}>{row.text}</div>
            : <button key={row.path} type="button" title={row.path}
              className={cn("remote-editor-node", row.path === activePath && "is-active", row.name.startsWith(".") && "is-hidden")}
              style={{ paddingLeft: 6 + row.depth * 12 }}
              onClick={() => row.dir ? toggleDir(row.path) : void openFile(row.path)}>
              {row.dir
                ? <>{expanded.has(row.path) ? <ChevronDown className="chevron" /> : <ChevronRight className="chevron" />}
                  {expanded.has(row.path) ? <FolderOpen /> : <Folder />}</>
                : <><span className="chevron" /><File /></>}
              <span className="truncate">{row.name}</span>
            </button>)}
        </nav>
        <div className="remote-editor-main">
          {files.length > 0 && <div className="remote-editor-tabs" role="tablist" aria-label={t("打开的文件")}>
            {files.map((f) => (
              <div key={f.path} role="tab" aria-selected={f.path === activePath} title={f.path}
                className={cn("remote-editor-tab", f.path === activePath && "is-active")}
                onClick={() => setActivePath(f.path)}
                onAuxClick={(e) => { if (e.button === 1) closeFile(f.path); }}>
                <span className="truncate">{f.name}</span>
                <button type="button" className="remote-editor-tab-close" data-dirty={f.dirty || undefined}
                  aria-label={t("关闭 {{name}}", { name: f.name })} onClick={(e) => { e.stopPropagation(); closeFile(f.path); }}>
                  <span className="dot" aria-hidden="true" /><X />
                </button>
              </div>
            ))}
          </div>}
          {activeFile?.conflict && <div className="remote-editor-banner is-warning" role="alert">
            <TriangleAlert />
            <span>{t("这个文件在对方电脑上已被修改（可能是终端里的程序或 AI 改的）。")}</span>
            <Button size="xs" variant="outline" onPress={() => void reload(activeFile.path)}>{t("用对方的版本")}</Button>
            <Button size="xs" variant="destructive" onPress={() => void save(activeFile.path, true)}>{t("用我的覆盖")}</Button>
          </div>}
          {activeFile?.saveError && <div className="remote-editor-banner is-error" role="alert">
            <CircleAlert /><span>{t("保存失败：{{error}}", { error: activeFile.saveError })}</span>
          </div>}
          <div className="remote-editor-surface">
            <div ref={host} className="remote-editor-monaco" hidden={activeFile?.state !== "ready"} />
            {!activeFile && <Placeholder>{t("在左边选择一个文件打开。编辑在本机进行，⌘/Ctrl+S 保存到对方电脑。")}</Placeholder>}
            {activeFile?.state === "loading" && <Placeholder><LoaderCircle className="animate-spin" />{t("正在打开…")}</Placeholder>}
            {activeFile?.state === "error" && <Placeholder error>{activeFile.error}</Placeholder>}
          </div>
          {activeFile?.state === "ready" && <footer className="remote-editor-status">
            <span>{languageLabel(activeFile.name)}</span>
            <span>{activeFile.meta?.bom ? "UTF-8 BOM" : "UTF-8"}</span>
            <span title={activeFile.mixed ? t("文件里混用了 LF 和 CRLF，保存时会统一成 {{eol}}", { eol: activeFile.eol }) : undefined}>
              {activeFile.eol}{activeFile.mixed && " ⚠"}
            </span>
            {activeFile.meta?.readonly && <span>{t("只读")}</span>}
            <span className="grow" />
            <span>{activeFile.saving ? t("正在保存…") : activeFile.dirty ? t("未保存") : t("已保存")}</span>
            <Button size="xs" variant="ghost" isDisabled={!online || !activeFile.dirty || activeFile.saving || !!activeFile.meta?.readonly}
              title={t("保存（⌘/Ctrl+S）")} onPress={() => void save(activeFile.path)}><Save />{t("保存")}</Button>
          </footer>}
        </div>
      </div>
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

function Placeholder({ children, error }: { children: ReactNode; error?: boolean }) {
  return <div className={cn("remote-editor-placeholder", error && "is-error")}>{children}</div>;
}

function languageLabel(name: string) {
  const id = languageFor(name);
  return monaco.languages.getLanguages().find((l) => l.id === id)?.aliases?.[0] ?? id;
}
