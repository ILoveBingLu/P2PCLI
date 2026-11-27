// 用本机 VS Code（或 Cursor）的 Remote-SSH 打开对方电脑上的文件夹：SSH 走 P2PCLI 的管道，不用对方开公网端口
import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { ArrowUp, Code2, Folder, House, LoaderCircle } from "lucide-react";
import { api, type DirListing } from "../api";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { toast } from "@/components/ui/toast";
import { t, tr } from "@/i18n";

type Editor = "vscode" | "cursor";
const EDITOR_KEY = "p2pcli.ssh.editor";
/** 每台设备上次打开的文件夹，下次从这里开始浏览 */
const LAST_PATH_KEY = "p2pcli.ssh.lastPath.";
function loadEditor(): Editor {
  try { return localStorage.getItem(EDITOR_KEY) === "cursor" ? "cursor" : "vscode"; } catch { return "vscode"; }
}
function loadLastPath(endpointId: string): string {
  try { return localStorage.getItem(LAST_PATH_KEY + endpointId) ?? ""; } catch { return ""; }
}
function saveLastPath(endpointId: string, path: string) {
  try { localStorage.setItem(LAST_PATH_KEY + endpointId, path); } catch { /* 记不住就算了 */ }
}

export function OpenInEditorDialog({ open, onOpenChange, connId, endpointId, deviceName }: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  connId: number;
  endpointId: string;
  deviceName: string;
}) {
  const [editor, setEditor] = useState<Editor>(loadEditor);
  const [busy, setBusy] = useState(false);
  /** 还没在 ~/.ssh/config 里加 Include：先问一句 */
  const [needInclude, setNeedInclude] = useState<string | null>(null);
  // 对方电脑上的文件夹：浏览到哪里就打开哪里，路径框里也能直接改了回车跳过去
  const [listing, setListing] = useState<DirListing | null>(null);
  const [pathText, setPathText] = useState("");
  const [loading, setLoading] = useState(false);
  const [browseError, setBrowseError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [active, setActive] = useState(0);
  const request = useRef(0);
  const listRef = useRef<HTMLDivElement>(null);

  const browse = async (path: string, fallbackHome = false) => {
    const id = ++request.current;
    setLoading(true);
    setBrowseError(null);
    try {
      const next = await api.listDir(connId, path);
      if (id !== request.current) return;
      setListing(next);
      setPathText(next.path === "\\" ? "" : next.path);
      setFilter("");
      setActive(0);
    } catch (e) {
      if (id !== request.current) return;
      // 上次的文件夹已经没了：回到用户目录
      if (fallbackHome && path) return void browse("");
      setBrowseError(tr(String(e)));
    } finally {
      if (id === request.current) setLoading(false);
    }
  };

  useEffect(() => {
    if (!open) {
      request.current++;
      return;
    }
    setNeedInclude(null);
    setListing(null);
    void browse(loadLastPath(endpointId), true);
    // 只在打开时从上次的位置开始
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, connId, endpointId]);

  const shown = useMemo(() => {
    const dirs = listing?.dirs ?? [];
    const query = filter.trim().toLowerCase();
    return query ? dirs.filter((name) => name.toLowerCase().includes(query)) : dirs;
  }, [listing, filter]);

  useEffect(() => {
    listRef.current?.querySelector<HTMLElement>(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const join = (name: string) => {
    const base = listing!.path;
    const sep = base.includes("\\") && !base.includes("/") ? "\\" : "/";
    return base.endsWith(sep) ? base + name : base + sep + name;
  };

  // Windows 电脑上在盘的根目录再往上是“所有磁盘”这一层（路径是 \），只能选盘进去，不能打开
  const atDrives = listing?.path === "\\";
  const target = listing && !atDrives ? listing.path : null;

  const editorName = editor === "cursor" ? "Cursor" : "VS Code";
  const run = async (addInclude: boolean) => {
    if (!target) return;
    setBusy(true);
    try {
      if (addInclude) await api.sshAddInclude();
      try { localStorage.setItem(EDITOR_KEY, editor); } catch { /* 记不住就算了 */ }
      const opened = await api.openInEditor(connId, target, editor);
      saveLastPath(endpointId, opened.path);
      onOpenChange(false);
      toast.success(t("已在 {{editor}} 里打开 {{alias}}:{{path}}", { editor: editorName, alias: opened.alias, path: opened.path }));
    } catch (e) {
      const message = String(e);
      if (message.includes("ssh_include_missing")) {
        const status = await api.sshStatus().catch(() => null);
        setNeedInclude(status?.config_path ?? "~/.ssh/config");
      } else {
        toast.error(tr(message));
      }
    } finally {
      setBusy(false);
    }
  };

  const onListKey = (e: KeyboardEvent) => {
    if (!listing) return;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((i) => Math.min(i + 1, shown.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter" && !e.nativeEvent.isComposing) {
      e.preventDefault();
      // ⌘/Ctrl + 回车：打开现在这个文件夹；回车：进入选中的子文件夹
      if (e.metaKey || e.ctrlKey) void run(false);
      else if (shown[active]) void browse(join(shown[active]));
    } else if (e.key === "Backspace" && !filter && listing.parent) {
      e.preventDefault();
      void browse(listing.parent);
    }
  };

  const rows = useMemo(() => shown.map((name, i) => (
    <button key={name} type="button" role="option" data-index={i} aria-selected={i === active}
      className="dir-picker-item" onClick={() => void browse(join(name))}>
      <Folder /><span className="truncate">{name}</span>
    </button>
    // browse/join 只依赖 listing 和 connId，shown 跟着 listing 变
    // eslint-disable-next-line react-hooks/exhaustive-deps
  )), [shown, active, connId]);

  return (
    <Dialog open={open} onOpenChange={(next) => !busy && onOpenChange(next)}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t("用本机编辑器打开 {{name}} 上的文件夹", { name: deviceName })}</DialogTitle>
          <DialogDescription>
            {t("用 VS Code 或 Cursor 的 Remote-SSH 连过去，SSH 走 P2PCLI 的加密通道，不用对方开公网端口。对方是 Mac、Linux 时不用开远程登录、不用密码（两边都要是新版 P2PCLI）；对方是 Windows 时要装好 OpenSSH 服务器。本机编辑器要装 Remote-SSH 扩展。")}
          </DialogDescription>
        </DialogHeader>
        {needInclude ? (
          <div className="flex flex-col gap-2 text-sm">
            <p>{t("第一次用要在 SSH 配置（{{path}}）最前面加一行：", { path: needInclude })}</p>
            <pre className="overflow-x-auto rounded-md bg-muted-surface px-3 py-2 text-xs">Include ~/.ssh/p2pcli_hosts</pre>
            <p className="text-muted-foreground">{t("原有的内容不动。P2PCLI 只维护 ~/.ssh/p2pcli_hosts 这一个文件，里面每台设备一个别名，终端里也能直接 ssh 它。")}</p>
          </div>
        ) : (
          <div className="flex min-w-0 flex-col gap-3">
            <div className="editor-folder">
              <div className="dir-picker-head">
                <Button variant="ghost" size="icon-xs" aria-label={t("上一级")} title={t("上一级（删除键）")}
                  isDisabled={!listing?.parent || loading} onPress={() => listing?.parent && void browse(listing.parent)}>
                  <ArrowUp />
                </Button>
                <Button variant="ghost" size="icon-xs" aria-label={t("用户目录")} title={t("用户目录")}
                  isDisabled={loading} onPress={() => void browse("")}>
                  <House />
                </Button>
                <Input className="editor-folder-path" value={atDrives ? "" : pathText} placeholder={atDrives ? t("所有磁盘") : "~/projects/app"}
                  aria-label={t("对方电脑上的文件夹")} title={t("可以直接改路径，回车跳过去")}
                  onChange={(e) => setPathText(e.currentTarget.value)}
                  onKeyDown={(e) => { if (e.key === "Enter" && !e.nativeEvent.isComposing) { e.preventDefault(); void browse(pathText.trim()); } }} />
                {loading && <LoaderCircle className="dir-picker-spin animate-spin" aria-label={t("读取中")} />}
              </div>
              <div className="dir-picker-filter">
                <Input autoFocus value={filter} placeholder={t("筛选子目录")} aria-label={t("筛选子目录")}
                  onChange={(e) => { setFilter(e.target.value); setActive(0); }} onKeyDown={onListKey} />
              </div>
              <div ref={listRef} className="dir-picker-list editor-folder-list" role="listbox" aria-label={t("子目录")}>
                {listing ? rows : loading && <div className="dir-picker-empty">{t("读取中…")}</div>}
                {listing && shown.length === 0 && !loading &&
                  <div className="dir-picker-empty">{filter ? t("没有匹配的子目录") : t("没有子目录")}</div>}
                {listing?.truncated && <div className="dir-picker-empty">{t("子目录太多，只列出了前 {{shown}} 个，可以输入名字筛选", { shown: listing.dirs.length })}</div>}
              </div>
              {browseError && <div className="dir-picker-error" role="alert">{browseError}</div>}
            </div>
            <p className="text-xs text-muted-foreground">
              {atDrives ? t("先选一个盘") : t("单击进入子文件夹，打开的是上面这个文件夹；⌘/Ctrl + 回车 也可以打开")}
            </p>
            <div className="flex gap-2" role="radiogroup" aria-label={t("编辑器")}>
              {(["vscode", "cursor"] as const).map((id) => (
                <Button key={id} size="sm" variant={editor === id ? "outline" : "ghost"} aria-pressed={editor === id} onPress={() => setEditor(id)}>
                  {id === "cursor" ? "Cursor" : "VS Code"}
                </Button>
              ))}
            </div>
          </div>
        )}
        <DialogFooter>
          <Button variant="ghost" onPress={() => onOpenChange(false)} isDisabled={busy}>{t("取消")}</Button>
          <Button onPress={() => void run(!!needInclude)} isDisabled={busy || !target || loading}>
            {busy ? <LoaderCircle data-icon="inline-start" className="animate-spin" /> : <Code2 data-icon="inline-start" />}
            {needInclude ? t("加上并打开") : t("用 {{editor}} 打开", { editor: editorName })}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
