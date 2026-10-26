import { useCallback, useEffect, useState } from "react";
import { BookOpen, Check, Download, LoaderCircle, RefreshCw, Trash2 } from "lucide-react";
import { Checkbox, Description } from "@heroui/react";
import { api, type AiHintState, type AiHintStatus } from "../api";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { toast } from "@/components/ui/toast";

const STATE_LABEL: Record<AiHintState, { text: string; variant: "success" | "warning" | "secondary" }> = {
  current: { text: "已安装", variant: "success" },
  outdated: { text: "有新版", variant: "warning" },
  missing: { text: "未安装", variant: "secondary" },
  foreign: { text: "已有同名技能", variant: "warning" },
};

/** 给本机的 AI 装 p2pcli 使用说明：检测到哪些 AI、各自装没装，可以自己决定装还是删 */
export function AiHintSettings() {
  const [status, setStatus] = useState<AiHintStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** 正在操作的那一行的路径；"all" 是全部 */
  const [busy, setBusy] = useState<string | null>(null);

  const load = useCallback(async () => {
    setBusy("load");
    try {
      setStatus(await api.aiHintStatus());
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  /** 装、更新或删除；path 不写就是全部。出错时提示原因并重新读一遍状态 */
  const apply = async (action: "install" | "remove", path?: string, done?: string) => {
    setBusy(path ?? "all");
    try {
      setStatus(await api.aiHintApply(action, path));
      if (done) toast.success(done);
    } catch (e) {
      toast.error(String(e));
      setStatus(await api.aiHintStatus().catch(() => status));
    } finally {
      setBusy(null);
    }
  };

  const setAuto = async (enabled: boolean) => {
    setBusy("auto");
    try {
      setStatus(await api.setAiHintAuto(enabled));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(null);
    }
  };

  if (!status) {
    return error ? (
      <Alert variant="destructive"><AlertDescription>{error}</AlertDescription></Alert>
    ) : (
      <div className="loading-state" role="status"><LoaderCircle className="animate-spin" />正在检测本机的 AI…</div>
    );
  }

  const canInstall = status.rows.some((r) => r.state === "missing" || r.state === "outdated");
  const canRemove = status.rows.some((r) => r.state === "current" || r.state === "outdated");
  const working = busy !== null;

  return (
    <div className="flex flex-col gap-5">
      <Alert>
        <AlertDescription>
          让本机的 AI 能和你的其他设备互动：在别的电脑上跑命令、发文件过去、把文件拉回来。做法是给本机的 AI 放一份 p2pcli 使用说明，
          它们新开的会话就知道怎么用，不用你再教。只写 AI 自己技能目录里一个叫 p2pcli 的文件夹，不改你的 CLAUDE.md、AGENTS.md，也不碰别人放的同名技能。
        </AlertDescription>
      </Alert>

      <Checkbox isSelected={status.auto} onChange={(on) => void setAuto(on)} isDisabled={working}>
        <Checkbox.Content className="gap-3 text-sm">
          <Checkbox.Control><Checkbox.Indicator><Check className="size-3" /></Checkbox.Indicator></Checkbox.Control>
          启动时自动安装和更新
        </Checkbox.Content>
        <Description className="pl-7 text-xs text-muted-foreground">
          开着：每次打开 P2PCLI 都检查一遍，缺了就装、旧了就更新（也会补上之后才装的 AI）。关掉：不再自动动，下面的安装和删除只在你点的时候做，已装的不会因为关掉而被删。
        </Description>
      </Checkbox>

      <div className="flex flex-wrap items-center gap-2">
        <p className="detail-note mr-auto">检测到的 AI（以它们的配置目录在不在为准）</p>
        <Button variant="ghost" size="sm" onClick={() => void load()} disabled={working}>
          <RefreshCw data-icon="inline-start" className={busy === "load" ? "animate-spin" : undefined} />重新检测
        </Button>
        {canInstall && (
          <Button variant="outline" size="sm" onClick={() => void apply("install", undefined, "已装好，已经开着的 AI 窗口要重开才会读到")} disabled={working}>
            {busy === "all" ? <LoaderCircle data-icon="inline-start" className="animate-spin" /> : <Download data-icon="inline-start" />}全部安装
          </Button>
        )}
        {canRemove && (
          <Button variant="ghost" size="sm" onClick={() => void apply("remove", undefined, "已删除")} disabled={working}>
            <Trash2 data-icon="inline-start" />全部删除
          </Button>
        )}
      </div>

      {status.rows.length === 0 ? (
        <Empty>
          <EmptyHeader>
            <EmptyMedia variant="icon"><BookOpen /></EmptyMedia>
            <EmptyTitle>没检测到已安装的 AI</EmptyTitle>
            <EmptyDescription>装好 Claude Code、Codex 等 AI 之后点“重新检测”，或者重启 P2PCLI。</EmptyDescription>
          </EmptyHeader>
        </Empty>
      ) : (
        <div className="flex flex-col gap-2">
          {status.rows.map((row) => {
            const label = STATE_LABEL[row.state];
            const rowBusy = busy === row.path;
            return (
              <div key={row.path} className="flex flex-wrap items-center gap-x-3 gap-y-2 rounded-md border border-border px-3 py-2.5">
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <strong className="truncate text-sm">{row.ais.join("、")}</strong>
                    <Badge variant={label.variant}>{label.text}</Badge>
                  </div>
                  <div className="mt-0.5 break-all font-mono text-xs text-muted-foreground">{row.path}</div>
                  {row.state === "foreign" && (
                    <div className="mt-0.5 text-xs text-muted-foreground">那里已有一个叫 p2pcli 的技能，不是 P2PCLI 装的，不会覆盖也不会删。</div>
                  )}
                </div>
                {(row.state === "missing" || row.state === "outdated") && (
                  <Button variant="outline" size="sm" onClick={() => void apply("install", row.path, "已装好，这个 AI 新开的会话就会读到")} disabled={working}>
                    {rowBusy ? <LoaderCircle data-icon="inline-start" className="animate-spin" /> : <Download data-icon="inline-start" />}
                    {row.state === "outdated" ? "更新" : "安装"}
                  </Button>
                )}
                {(row.state === "current" || row.state === "outdated") && (
                  <Button variant="ghost" size="sm" onClick={() => void apply("remove", row.path, "已删除")} disabled={working}>
                    {rowBusy && row.state === "current" ? <LoaderCircle data-icon="inline-start" className="animate-spin" /> : <Trash2 data-icon="inline-start" />}
                    删除
                  </Button>
                )}
              </div>
            );
          })}
        </div>
      )}

      <p className="detail-note">已经开着的 AI 窗口要重开才会读到。</p>
    </div>
  );
}
