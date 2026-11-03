import { memo, useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { Popover } from "@heroui/react";
import { useTranslation } from "react-i18next";
import { ArrowUp, CornerDownLeft, FileCode2, Folder, House, LoaderCircle } from "lucide-react";
import { api, type DirListing } from "../api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { t, tr } from "@/i18n";

function sameListing(a: DirListing | null, b: DirListing) {
  return a != null && a.path === b.path && a.parent === b.parent && a.truncated === b.truncated &&
    a.dirs.length === b.dirs.length && a.dirs.every((name, i) => name === b.dirs[i]);
}

/**
 * 底部栏的当前目录：点开是对方电脑上的目录列表，单击进入子目录，点“切换到这里”让终端 cd 过去。
 * 只在终端停在空闲提示符时能切；跑着程序（包括命令行 AI）时只能看，免得 cd 被当成输入发给程序。
 */
// 状态栏随会话轮询整页重渲染，属性都是原始值，没变就不用跟着重画列表
export const DirPicker = memo(function DirPicker({ connId, sessionId, currentPath, canChange, blockedReason, onOpenEditor }: {
  connId: number;
  sessionId: string;
  currentPath: string | null;
  canChange: boolean;
  blockedReason: string;
  /** 在代码编辑器里打开正在看的目录 */
  onOpenEditor?: (path: string) => void;
}) {
  // memo 会挡住父组件跟着换语言的重渲染，这里自己订阅一下
  useTranslation();
  const [open, setOpen] = useState(false);
  const [listing, setListing] = useState<DirListing | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [active, setActive] = useState(0);
  const [changing, setChanging] = useState(false);
  const request = useRef(0);
  const listRef = useRef<HTMLDivElement>(null);

  const browse = async (path: string) => {
    const id = ++request.current;
    setLoading(true);
    setError(null);
    try {
      const next = await api.listDir(connId, path);
      if (id !== request.current) return;
      // 重新打开时是刷新同一个目录，别把已经输入的筛选清掉
      if (next.path !== listing?.path) {
        setFilter("");
        setActive(0);
      }
      // 内容没变就留着旧对象，不然几千行又得整个重画一遍
      if (!sameListing(listing, next)) setListing(next);
    } catch (e) {
      if (id === request.current) setError(tr(String(e)));
    } finally {
      if (id === request.current) setLoading(false);
    }
  };

  useEffect(() => {
    if (!open) {
      request.current++;
      setError(null);
      setFilter("");
      setActive(0);
      return;
    }
    // 上次看的正好是当前目录就先摆出来，后台刷新；远程一来一回期间不至于空着
    if (listing?.path !== currentPath) setListing(null);
    void browse(currentPath ?? "");
    // 只在打开时从当前目录开始；之后浏览到哪里由用户决定
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

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

  const change = async (path: string) => {
    if (!canChange || changing) return;
    setChanging(true);
    setError(null);
    try {
      await api.changeDir(connId, sessionId, path);
      setOpen(false);
    } catch (e) {
      setError(tr(String(e)));
    } finally {
      setChanging(false);
    }
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (!listing) return;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((i) => Math.min(i + 1, shown.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (e.metaKey || e.ctrlKey) void change(listing.path);
      else if (shown[active]) void browse(join(shown[active]));
    } else if (e.key === "Backspace" && !filter && listing.parent) {
      e.preventDefault();
      void browse(listing.parent);
    }
  };

  // 鼠标悬停只靠 CSS 高亮，active 只跟键盘走，免得每划过一行就把整个列表重画一遍
  const rows = useMemo(() => shown.map((name, i) => (
    <button key={name} type="button" role="option" data-index={i} aria-selected={i === active}
      className="dir-picker-item" onClick={() => void browse(join(name))}>
      <Folder /><span className="truncate">{name}</span>
    </button>
    // browse/join 只依赖 listing 和 connId，shown 跟着 listing 变
    // eslint-disable-next-line react-hooks/exhaustive-deps
  )), [shown, active, connId]);

  const here = listing?.path ?? currentPath ?? "";
  const atCurrent = listing != null && listing.path === currentPath;

  return (
    <Popover isOpen={open} onOpenChange={setOpen}>
      <Button variant="ghost" size="xs" className="terminal-status-dir-button" aria-label={t("当前目录：{{path}}，点击选择目录", { path: currentPath ?? t("用户目录") })} title={t("选择目录")}>
        <Folder /><span className="truncate">{currentPath ?? "~"}</span>
      </Button>
      <Popover.Content placement="top start" offset={8} className="dir-picker">
        <Popover.Dialog aria-label={t("选择目录")} className="flex min-h-0 flex-col">
          <div className="dir-picker-head">
            <Button variant="ghost" size="icon-xs" aria-label={t("上一级")} title={t("上一级（删除键）")}
              isDisabled={!listing?.parent || loading} onPress={() => listing?.parent && void browse(listing.parent)}>
              <ArrowUp />
            </Button>
            <Button variant="ghost" size="icon-xs" aria-label={t("用户目录")} title={t("用户目录")}
              isDisabled={loading} onPress={() => void browse("")}>
              <House />
            </Button>
            <span className="dir-picker-path" title={here}><bdi>{here}</bdi></span>
            {loading && <LoaderCircle className="dir-picker-spin animate-spin" aria-label={t("读取中")} />}
          </div>
          <div className="dir-picker-filter">
            <Input autoFocus value={filter} placeholder={t("筛选子目录")} aria-label={t("筛选子目录")}
              onChange={(e) => { setFilter(e.target.value); setActive(0); }} onKeyDown={onKeyDown} />
          </div>
          <div ref={listRef} className="dir-picker-list" role="listbox" aria-label={t("子目录")}>
            {listing ? rows : loading && <div className="dir-picker-empty">{t("读取中…")}</div>}
            {listing && shown.length === 0 && !loading &&
              <div className="dir-picker-empty">{filter ? t("没有匹配的子目录") : t("没有子目录")}</div>}
            {listing?.truncated && <div className="dir-picker-empty">{t("子目录太多，只列出了前 {{shown}} 个，可以输入名字筛选", { shown: listing.dirs.length })}</div>}
          </div>
          {error && <div className="dir-picker-error" role="alert">{error}</div>}
          <div className="dir-picker-foot">
            {onOpenEditor && <Button size="xs" variant="ghost" isDisabled={!listing} title={t("在代码编辑器里打开这个目录")}
              onPress={() => { if (listing) { onOpenEditor(listing.path); setOpen(false); } }}>
              <FileCode2 />{t("在编辑器中打开")}
            </Button>}
            <span className="dir-picker-hint">{canChange ? (atCurrent ? t("终端已在这个目录") : t("⌘/Ctrl + 回车 也可切换")) : blockedReason}</span>
            <Button size="xs" isDisabled={!canChange || !listing || atCurrent || changing || loading}
              onPress={() => listing && void change(listing.path)}>
              {changing ? <LoaderCircle className="dir-picker-spin animate-spin" /> : <CornerDownLeft />}{t("切换到这里")}
            </Button>
          </div>
        </Popover.Dialog>
      </Popover.Content>
    </Popover>
  );
});
