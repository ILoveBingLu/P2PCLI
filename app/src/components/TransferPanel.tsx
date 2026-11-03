import { useEffect, useRef, useState } from "react";
import { ArrowDown, ArrowUp, Copy, FolderOpen, X } from "lucide-react";
import { api, formatSize, type Transfer } from "../api";
import { writeClipboardText } from "../lib/clipboard";
import { Button } from "@/components/ui/button";
import { toast } from "@/components/ui/toast";
import { t } from "@/i18n";

/** 最近几秒的平均速度（字节每秒）；刚开始、还没攒够一秒的数据时是 null */
function useSpeed(transfer: Transfer): number | null {
  const samples = useRef<{ at: number; done: number }[]>([]);
  const [speed, setSpeed] = useState<number | null>(null);
  useEffect(() => {
    if (transfer.state !== "running") {
      samples.current = [];
      setSpeed(null);
      return;
    }
    const at = Date.now();
    const list = samples.current;
    list.push({ at, done: transfer.done });
    while (list.length > 1 && at - list[0].at > 4000) list.shift();
    const elapsed = (at - list[0].at) / 1000;
    setSpeed(elapsed >= 1 ? Math.max(0, (transfer.done - list[0].done) / elapsed) : null);
  }, [transfer.state, transfer.done]);
  return speed;
}

/** 一个传输：进度、速度、传完以后的操作。显示在灵动岛展开的卡片里 */
export function TransferRow({ transfer, onDismiss }: { transfer: Transfer; onDismiss: () => void }) {
  const send = transfer.direction === "send";
  const speed = useSpeed(transfer);
  const percent = transfer.total ? Math.min(100, Math.floor((transfer.done * 100) / transfer.total)) : transfer.state === "done" ? 100 : 0;
  const status = {
    connecting: t("连接中…"),
    waiting: send ? t("等对方同意…") : t("准备中…"),
    running:
      `${percent}% · ${formatSize(transfer.done)} / ${formatSize(transfer.total)}` +
      (speed != null ? ` · ${formatSize(speed)}/s` : "") +
      ((transfer.lanes ?? 0) > 1 ? t(" · {{lanes}} 条并发", { lanes: transfer.lanes }) : ""),
    done: send ? t("已发到 {{peer_name}}，{{value}}", { peer_name: transfer.peer_name, value: formatSize(transfer.total) }) : t("已收到，{{value}}", { value: formatSize(transfer.total) }),
    failed: transfer.error ?? t("失败"),
  }[transfer.state];
  const copyPaths = () =>
    void writeClipboardText(transfer.paths.join("\n")).then(() => toast.success(t("已复制对方电脑上的路径")), () => toast.error(t("复制失败")));
  return (
    <div className="transfer-row" data-state={transfer.state}>
      <span className="transfer-icon" aria-hidden="true">{send ? <ArrowUp /> : <ArrowDown />}</span>
      <div className="min-w-0 flex-1">
        <div className="flex min-w-0 items-center gap-1.5 text-xs">
          <span className="truncate font-medium">{transfer.label}</span>
          <span className="shrink-0 text-muted-foreground">{send ? t("发给") : t("来自")} {transfer.peer_name}</span>
        </div>
        {transfer.finished_at == null && (
          <div className="transfer-bar" aria-hidden="true">
            <span style={{ width: `${percent}%` }} />
          </div>
        )}
        <div className="transfer-status" title={transfer.diag ? `${status}\n${transfer.diag}` : status}>{status}</div>
      </div>
      {transfer.state === "done" && !send && transfer.paths[0] && (
        <Button variant="ghost" size="icon-sm" title={t("在文件夹里显示")} aria-label={t("在文件夹里显示")}
          onClick={() => void api.revealPath(transfer.paths[0]).catch((e) => toast.error(String(e)))}>
          <FolderOpen />
        </Button>
      )}
      {transfer.state === "done" && send && transfer.paths.length > 0 && (
        <Button variant="ghost" size="icon-sm" title={t("复制对方电脑上的路径")} aria-label={t("复制对方电脑上的路径")} onClick={() => void copyPaths()}>
          <Copy />
        </Button>
      )}
      {transfer.finished_at != null && (
        <Button variant="ghost" size="icon-sm" title={t("关闭")} aria-label={t("关闭")} onClick={onDismiss}>
          <X />
        </Button>
      )}
    </div>
  );
}
