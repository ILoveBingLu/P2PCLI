import { useEffect, useRef, useState } from "react";
import { ArrowDown, ArrowUp, Copy, FolderOpen, X } from "lucide-react";
import { api, formatSize, type Transfer } from "../api";
import { Button } from "@/components/ui/button";
import { toast } from "@/components/ui/toast";

/** 传完的停留多久再收起 */
const DONE_KEEP_MS = 8000;
/** 失败的留着等人关，太久以前的就不再显示 */
const FAILED_KEEP_MS = 10 * 60 * 1000;
const MAX_ROWS = 4;

/** 工作区左下角的传输列表：进行中的显示进度，传完的停留一会儿，失败的等人关 */
export function TransferPanel({ transfers }: { transfers: Transfer[] }) {
  const [dismissed, setDismissed] = useState<Set<string>>(new Set());
  const [now, setNow] = useState(Date.now());
  const busy = transfers.some((t) => t.finished_at == null);
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);
  const shown = transfers
    .filter((t) => {
      if (dismissed.has(t.id)) return false;
      if (t.finished_at == null) return true;
      const age = now - t.finished_at * 1000;
      return t.state === "failed" ? age < FAILED_KEEP_MS : age < DONE_KEEP_MS;
    })
    .slice(0, MAX_ROWS);
  if (!shown.length) return null;
  return (
    <div className="transfer-anchor">
      <div className="transfer-panel" role="status" aria-live="polite" aria-busy={busy}>
        {shown.map((t) => (
          <TransferRow key={t.id} t={t} onDismiss={() => setDismissed((ids) => new Set(ids).add(t.id))} />
        ))}
      </div>
    </div>
  );
}

/** 最近几秒的平均速度（字节每秒）；刚开始、还没攒够一秒的数据时是 null */
function useSpeed(t: Transfer): number | null {
  const samples = useRef<{ at: number; done: number }[]>([]);
  const [speed, setSpeed] = useState<number | null>(null);
  useEffect(() => {
    if (t.state !== "running") {
      samples.current = [];
      setSpeed(null);
      return;
    }
    const at = Date.now();
    const list = samples.current;
    list.push({ at, done: t.done });
    while (list.length > 1 && at - list[0].at > 4000) list.shift();
    const elapsed = (at - list[0].at) / 1000;
    setSpeed(elapsed >= 1 ? Math.max(0, (t.done - list[0].done) / elapsed) : null);
  }, [t.state, t.done]);
  return speed;
}

function TransferRow({ t, onDismiss }: { t: Transfer; onDismiss: () => void }) {
  const send = t.direction === "send";
  const speed = useSpeed(t);
  const percent = t.total ? Math.min(100, Math.floor((t.done * 100) / t.total)) : t.state === "done" ? 100 : 0;
  const status = {
    connecting: "连接中…",
    waiting: send ? "等对方同意…" : "准备中…",
    running:
      `${percent}% · ${formatSize(t.done)} / ${formatSize(t.total)}` +
      (speed != null ? ` · ${formatSize(speed)}/s` : "") +
      ((t.lanes ?? 0) > 1 ? ` · ${t.lanes} 条并发` : ""),
    done: send ? `已发到 ${t.peer_name}，${formatSize(t.total)}` : `已收到，${formatSize(t.total)}`,
    failed: t.error ?? "失败",
  }[t.state];
  const copyPaths = () =>
    navigator.clipboard.writeText(t.paths.join("\n")).then(() => toast.success("已复制对方电脑上的路径"), () => toast.error("复制失败"));
  return (
    <div className="transfer-row" data-state={t.state}>
      <span className="transfer-icon" aria-hidden="true">{send ? <ArrowUp /> : <ArrowDown />}</span>
      <div className="min-w-0 flex-1">
        <div className="flex min-w-0 items-center gap-1.5 text-xs">
          <span className="truncate font-medium">{t.label}</span>
          <span className="shrink-0 text-muted-foreground">{send ? "发给" : "来自"} {t.peer_name}</span>
        </div>
        {t.finished_at == null && (
          <div className="transfer-bar" aria-hidden="true">
            <span style={{ width: `${percent}%` }} />
          </div>
        )}
        <div className="transfer-status" title={status}>{status}</div>
      </div>
      {t.state === "done" && !send && t.paths[0] && (
        <Button variant="ghost" size="icon-sm" title="在文件夹里显示" aria-label="在文件夹里显示"
          onClick={() => void api.revealPath(t.paths[0]).catch((e) => toast.error(String(e)))}>
          <FolderOpen />
        </Button>
      )}
      {t.state === "done" && send && t.paths.length > 0 && (
        <Button variant="ghost" size="icon-sm" title="复制对方电脑上的路径" aria-label="复制对方电脑上的路径" onClick={() => void copyPaths()}>
          <Copy />
        </Button>
      )}
      {t.finished_at != null && (
        <Button variant="ghost" size="icon-sm" title="关闭" aria-label="关闭" onClick={onDismiss}>
          <X />
        </Button>
      )}
    </div>
  );
}
