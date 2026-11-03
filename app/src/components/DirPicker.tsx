import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { Popover } from "@heroui/react";
import { ArrowUp, CornerDownLeft, Folder, House, LoaderCircle } from "lucide-react";
import { api, type DirListing } from "../api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

/**
 * 底部栏的当前目录：点开是对方电脑上的目录列表，单击进入子目录，点“切换到这里”让终端 cd 过去。
 * 只在终端停在空闲提示符时能切；跑着程序（包括命令行 AI）时只能看，免得 cd 被当成输入发给程序。
 */
export function DirPicker({ connId, sessionId, currentPath, canChange, blockedReason }: {
  connId: number;
  sessionId: string;
  currentPath: string | null;
  canChange: boolean;
  blockedReason: string;
}) {
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
      setListing(next);
      setFilter("");
      setActive(0);
    } catch (e) {
      if (id === request.current) setError(String(e));
    } finally {
      if (id === request.current) setLoading(false);
    }
  };

  useEffect(() => {
    if (!open) {
      request.current++;
      setListing(null);
      setError(null);
      setFilter("");
      return;
    }
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
      setError(String(e));
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

  const here = listing?.path ?? currentPath ?? "";
  const atCurrent = listing != null && listing.path === currentPath;

  return (
    <Popover isOpen={open} onOpenChange={setOpen}>
      <Button variant="ghost" size="xs" className="terminal-status-dir-button" aria-label={`当前目录：${currentPath ?? "用户目录"}，点击选择目录`} title="选择目录">
        <Folder /><span className="truncate">{currentPath ?? "~"}</span>
      </Button>
      <Popover.Content placement="top start" offset={8} className="dir-picker">
        <Popover.Dialog aria-label="选择目录" className="flex min-h-0 flex-col">
          <div className="dir-picker-head">
            <Button variant="ghost" size="icon-xs" aria-label="上一级" title="上一级（删除键）"
              isDisabled={!listing?.parent || loading} onPress={() => listing?.parent && void browse(listing.parent)}>
              <ArrowUp />
            </Button>
            <Button variant="ghost" size="icon-xs" aria-label="用户目录" title="用户目录"
              isDisabled={loading} onPress={() => void browse("")}>
              <House />
            </Button>
            <span className="dir-picker-path" title={here}><bdi>{here}</bdi></span>
            {loading && <LoaderCircle className="dir-picker-spin animate-spin" aria-label="读取中" />}
          </div>
          <div className="dir-picker-filter">
            <Input autoFocus value={filter} placeholder="筛选子目录" aria-label="筛选子目录"
              onChange={(e) => { setFilter(e.target.value); setActive(0); }} onKeyDown={onKeyDown} />
          </div>
          <div ref={listRef} className="dir-picker-list" role="listbox" aria-label="子目录">
            {listing && shown.map((name, i) => (
              <button key={name} type="button" role="option" data-index={i} aria-selected={i === active}
                className="dir-picker-item"
                onMouseEnter={() => setActive(i)}
                onClick={() => void browse(join(name))}>
                <Folder /><span className="truncate">{name}</span>
              </button>
            ))}
            {listing && shown.length === 0 && !loading &&
              <div className="dir-picker-empty">{filter ? "没有匹配的子目录" : "没有子目录"}</div>}
            {listing?.truncated && <div className="dir-picker-empty">子目录太多，只列出了前 2000 个，可以输入名字筛选</div>}
          </div>
          {error && <div className="dir-picker-error" role="alert">{error}</div>}
          <div className="dir-picker-foot">
            <span className="dir-picker-hint">{canChange ? (atCurrent ? "终端已在这个目录" : "⌘/Ctrl + 回车 也可切换") : blockedReason}</span>
            <Button size="xs" isDisabled={!canChange || !listing || atCurrent || changing || loading}
              onPress={() => listing && void change(listing.path)}>
              {changing ? <LoaderCircle className="dir-picker-spin animate-spin" /> : <CornerDownLeft />}切换到这里
            </Button>
          </div>
        </Popover.Dialog>
      </Popover.Content>
    </Popover>
  );
}
