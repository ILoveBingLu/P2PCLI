// 编辑器里的 Git 面板：看改动、暂存、提交、推送、拉取。命令都在对方电脑上跑，用的是那边的 git 配置和账号
import { useEffect, useMemo, useState, type KeyboardEvent } from "react";
import { ArrowDown, ArrowUp, File, Folder, GitBranch, GitCommitHorizontal, LoaderCircle, Minus, Plus, RefreshCw, SaveAll, Undo2 } from "lucide-react";
import { api, type GitCommit, type GitStatus } from "../../api";
import { Button } from "@/components/ui/button";
import { toast } from "@/components/ui/toast";
import { cn } from "@/lib/utils";
import { t, tr } from "@/i18n";
import { baseName, joinPath, sepOf } from "./paths";

/** 面板里的一条改动 */
interface Change {
  /** 相对仓库根目录，git 给的写法 */
  rel: string;
  /** 对方电脑上的完整路径，打开文件用 */
  path: string;
  /** 显示的字母：M 改了、A 新加、D 删了、R 改名、U 没跟踪、! 冲突 */
  code: string;
  untracked: boolean;
  dir: boolean;
}

/** 把 git status 的两个字母拆成“暂存的”“没暂存的”“冲突的”三组 */
export function splitChanges(status: GitStatus, root: string | null) {
  const sep = sepOf(root ?? status.top);
  const pathOf = (rel: string) => {
    const clean = rel.replace(/\/$/, "");
    if (root != null && clean.startsWith(status.prefix)) return joinPath(root, clean.slice(status.prefix.length).split("/").join(sep));
    return joinPath(status.top, clean.split("/").join(sepOf(status.top)));
  };
  const staged: Change[] = [];
  const unstaged: Change[] = [];
  const conflicts: Change[] = [];
  for (const { path: rel, status: xy } of status.entries) {
    const make = (code: string, untracked = false): Change => ({ rel, path: pathOf(rel), code, untracked, dir: rel.endsWith("/") });
    const [x, y] = [xy[0], xy[1]];
    if (x === "U" || y === "U" || xy === "AA" || xy === "DD") conflicts.push(make("!"));
    else if (xy === "??") unstaged.push(make("U", true));
    else {
      if (x !== " ") staged.push(make(x));
      if (y !== " ") unstaged.push(make(y));
    }
  }
  return { staged, unstaged, conflicts };
}

function ago(seconds: number) {
  const diff = Math.max(0, Date.now() / 1000 - seconds);
  if (diff < 60) return t("刚刚");
  if (diff < 3600) return t("{{count}} 分钟前", { count: Math.floor(diff / 60) });
  if (diff < 86400) return t("{{count}} 小时前", { count: Math.floor(diff / 3600) });
  if (diff < 86400 * 30) return t("{{count}} 天前", { count: Math.floor(diff / 86400) });
  return new Date(seconds * 1000).toLocaleDateString();
}

export function GitPanel({ connId, root, status, dirtyCount, onRefresh, onOpen, onDiff, onSaveAll, confirm }: {
  connId: number | null;
  root: string | null;
  status: GitStatus | null;
  /** 编辑器里还没保存的文件数：提交里不会有这些修改，提醒一下 */
  dirtyCount: number;
  onRefresh: () => Promise<void>;
  onOpen: (path: string) => void;
  /** 和最新提交对比 */
  onDiff: (path: string) => void;
  onSaveAll: () => Promise<void>;
  confirm: (options: { title: string; body: string; confirm: string; onConfirm: () => void }) => void;
}) {
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [commits, setCommits] = useState<GitCommit[] | null>(null);
  const changes = useMemo(() => status ? splitChanges(status, root) : null, [status, root]);

  const loadLog = async () => {
    if (connId == null || root == null) return;
    setCommits(await api.gitLog(connId, root, 15).catch(() => null));
  };
  // 换了分支、提交数变了就重新读提交记录
  useEffect(() => { void loadLog(); }, [connId, root, status?.branch, status?.ahead, status?.behind]); // eslint-disable-line react-hooks/exhaustive-deps

  if (!status || !changes || connId == null || root == null) {
    return <div className="remote-editor-search"><div className="remote-editor-note">{t("这个目录不在 Git 仓库里，或者对方电脑上没装 git。")}</div></div>;
  }

  /** 跑一个操作：期间按钮都禁用，完了刷新状态 */
  const run = async (name: string, action: () => Promise<unknown>, done?: (result: unknown) => void) => {
    setBusy(name);
    try {
      const result = await action();
      done?.(result);
    } catch (e) {
      toast.error(tr(String(e).replace(/^Error: /, "")));
    } finally {
      setBusy(null);
      await onRefresh();
    }
  };

  const stage = (list: Change[]) => run("stage", () => api.gitStage(connId, root, list.map((c) => c.rel)));
  const unstage = (list: Change[]) => run("unstage", () => api.gitUnstage(connId, root, list.map((c) => c.rel)));
  const discard = (change: Change) => confirm({
    title: t("放弃对“{{name}}”的修改？", { name: baseName(change.rel) }),
    body: change.untracked
      ? t("这个新文件会移到对方电脑的废纸篓（回收站），需要时可以从那里找回。")
      : t("会换回最近一次暂存或提交的内容，这些修改找不回来。"),
    confirm: t("放弃修改"),
    onConfirm: () => void run("discard", () => api.gitDiscard(connId, root, change.untracked ? [] : [change.rel], change.untracked ? [change.rel] : [])),
  });

  const nothingStaged = changes.staged.length === 0;
  const commit = () => {
    const text = message.trim();
    if (!text || busy) return;
    const doCommit = () => void run("commit", async () => {
      // 没有暂存的：和 VS Code 一样，问过以后全部暂存再提交
      if (nothingStaged) await api.gitStage(connId, root, []);
      return api.gitCommit(connId, root, text);
    }, () => {
      setMessage("");
      toast.success(t("已提交：{{subject}}", { subject: text.split("\n")[0] }));
      void loadLog();
    });
    if (nothingStaged) {
      confirm({
        title: t("没有暂存的更改"),
        body: t("要把全部 {{count}} 个更改都暂存再提交吗？", { count: changes.unstaged.length }),
        confirm: t("全部暂存并提交"),
        onConfirm: doCommit,
      });
    } else doCommit();
  };
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && !e.nativeEvent.isComposing) { e.preventDefault(); commit(); }
  };

  const ahead = status.ahead ?? 0;
  const behind = status.behind ?? 0;
  const disabled = busy != null;
  const row = (change: Change, group: "staged" | "unstaged" | "conflict") => {
    const name = change.rel.replace(/\/$/, "").split("/").pop() ?? change.rel;
    const dir = change.rel.replace(/\/$/, "").split("/").slice(0, -1).join("/");
    // 改过的文件点开和最新提交对比；新文件、冲突直接打开；删掉的打不开
    const open = () => {
      if (change.dir || change.code === "D") return;
      if (change.code === "M" || change.code === "R") onDiff(change.path);
      else onOpen(change.path);
    };
    return (
      <div key={group + change.rel} className={cn("git-row", `git-code-${change.code === "!" ? "conflict" : change.code}`)}>
        <button type="button" className="git-row-main" title={change.rel} onClick={open}>
          {change.dir ? <Folder /> : <File />}
          <span className={cn("truncate", change.code === "D" && "line-through")}>{name}</span>
          <span className="remote-editor-search-dir truncate">{dir}</span>
        </button>
        {group === "unstaged" && <Button size="icon-xs" variant="ghost" aria-label={t("放弃修改")} title={t("放弃修改")} isDisabled={disabled}
          onPress={() => discard(change)}><Undo2 /></Button>}
        {group === "staged"
          ? <Button size="icon-xs" variant="ghost" aria-label={t("取消暂存")} title={t("取消暂存")} isDisabled={disabled} onPress={() => void unstage([change])}><Minus /></Button>
          : <Button size="icon-xs" variant="ghost" aria-label={t("暂存")} title={t("暂存")} isDisabled={disabled} onPress={() => void stage([change])}><Plus /></Button>}
        <span className="git-code">{change.code}</span>
      </div>
    );
  };

  return (
    <div className="remote-editor-search git-panel">
      <div className="git-head">
        <span className="git-branch truncate" title={status.upstream ? t("跟踪 {{upstream}}", { upstream: status.upstream }) : t("没有上游分支，推送时会推到 origin 的同名分支")}>
          <GitBranch />{status.branch ?? t("没有分支")}
        </span>
        <Button size="xs" variant="ghost" isDisabled={disabled} aria-label={t("拉取")} title={t("拉取（只快进，有分叉时请在终端里处理）")}
          onPress={() => void run("pull", () => api.gitPull(connId, root), (text) => toast.success(/up to date|最新/i.test(String(text)) ? t("已经是最新的") : t("已拉取")))}>
          {busy === "pull" ? <LoaderCircle className="animate-spin" /> : <ArrowDown />}{behind > 0 && behind}
        </Button>
        <Button size="xs" variant="ghost" isDisabled={disabled} aria-label={t("推送")} title={t("推送")}
          onPress={() => void run("push", () => api.gitPush(connId, root), () => toast.success(t("已推送")))}>
          {busy === "push" ? <LoaderCircle className="animate-spin" /> : <ArrowUp />}{ahead > 0 && ahead}
        </Button>
        <Button size="icon-xs" variant="ghost" aria-label={t("刷新")} title={t("刷新")} isDisabled={disabled} onPress={() => void onRefresh()}><RefreshCw /></Button>
      </div>

      <div className="git-commit-box">
        <textarea value={message} rows={3} placeholder={t("提交说明（⌘/Ctrl+Enter 提交）")} aria-label={t("提交说明")}
          onChange={(e) => setMessage(e.target.value)} onKeyDown={onKeyDown} />
        <Button size="sm" isDisabled={disabled || !message.trim() || (nothingStaged && changes.unstaged.length === 0)} onPress={commit}>
          {busy === "commit" ? <LoaderCircle className="animate-spin" /> : <GitCommitHorizontal />}
          {nothingStaged ? t("全部暂存并提交") : t("提交 {{count}} 个文件", { count: changes.staged.length })}
        </Button>
        {dirtyCount > 0 && <div className="remote-editor-note is-warning">
          {t("编辑器里还有 {{count}} 个文件没保存，提交里不会有这些修改。", { count: dirtyCount })}
          <Button size="xs" variant="outline" onPress={() => void onSaveAll()}><SaveAll />{t("全部保存")}</Button>
        </div>}
      </div>

      <div className="remote-editor-search-results">
        {changes.conflicts.length > 0 && <section>
          <h4 className="git-group">{t("冲突")}<span className="remote-editor-search-count">{changes.conflicts.length}</span></h4>
          {changes.conflicts.map((c) => row(c, "conflict"))}
        </section>}
        {changes.staged.length > 0 && <section>
          <h4 className="git-group">{t("暂存的更改")}
            <Button size="icon-xs" variant="ghost" aria-label={t("全部取消暂存")} title={t("全部取消暂存")} isDisabled={disabled} onPress={() => void unstage([])}><Minus /></Button>
            <span className="remote-editor-search-count">{changes.staged.length}</span>
          </h4>
          {changes.staged.map((c) => row(c, "staged"))}
        </section>}
        <section>
          <h4 className="git-group">{t("更改")}
            {changes.unstaged.length > 0 && <Button size="icon-xs" variant="ghost" aria-label={t("全部暂存")} title={t("全部暂存")} isDisabled={disabled} onPress={() => void stage([])}><Plus /></Button>}
            <span className="remote-editor-search-count">{changes.unstaged.length}</span>
          </h4>
          {changes.unstaged.map((c) => row(c, "unstaged"))}
          {!changes.unstaged.length && !changes.staged.length && !changes.conflicts.length && <div className="remote-editor-note">{t("没有改动")}</div>}
        </section>
        {commits && commits.length > 0 && <section>
          <h4 className="git-group">{t("最近的提交")}</h4>
          {commits.map((c) => (
            <div key={c.hash} className="git-commit" title={`${c.hash}\n${c.author}\n${new Date(c.time * 1000).toLocaleString()}`}>
              <span className="git-hash">{c.short}</span>
              <span className="truncate">{c.subject}</span>
              <span className="remote-editor-search-dir">{ago(c.time)}</span>
            </div>
          ))}
        </section>}
      </div>
    </div>
  );
}
