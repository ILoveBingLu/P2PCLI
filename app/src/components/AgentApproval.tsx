import { useRef, useState } from "react";
import { ArrowUpRight, LoaderCircle, ShieldQuestion, TerminalSquare, X } from "lucide-react";
import type { AgentApproval as Approval } from "../api";
import { Button } from "./ui/button";
import { Card } from "./ui/card";
import { toast } from "./ui/toast";
import { t } from "@/i18n";

/** 单次确认的选项。由父级用 approval.id 作为 key，下一轮请求不会继承提交状态。
 *  提交成功后不再让点（等被控端返回新快照，免得同一个确认点两次）；提交失败可以重新选。 */
export function AgentApproval({ approval, name, onChoose, onTerminal, onDismiss }: {
  approval: Approval;
  name: string;
  onChoose: (approvalId: string, optionId: number) => Promise<void>;
  onTerminal: () => void;
  /** 收起这个确认框（在终端里自己处理） */
  onDismiss: () => void;
}) {
  const lock = useRef(false);
  const [pending, setPending] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [sent, setSent] = useState(false);
  const choose = async (id: number) => {
    if (lock.current) return;
    lock.current = true;
    setPending(id);
    setError(null);
    try {
      await onChoose(approval.id, id);
      setSent(true);
    } catch (e) {
      // 没提交成功（确认框刚变了、连接断了）：放开，可以重新选或者去终端里处理
      lock.current = false;
      setPending(null);
      setError(String(e));
      toast.error(String(e));
    }
  };
  return <Card className="agent-approval" role="region" aria-label={t("{{name}} 等待确认", { name })}>
    <div className="agent-approval-heading">
      <ShieldQuestion className="size-4" aria-hidden="true" />
      <span>{name} {t("· 等待确认")}</span>
      <Button variant="ghost" size="icon-xs" aria-label={t("在终端中处理确认")} title={t("在终端中处理")} onPress={onTerminal}>
        <TerminalSquare className="size-3.5" />
      </Button>
      <Button variant="ghost" size="icon-xs" aria-label={t("收起确认框")} title={t("收起，在终端里自己处理")} onPress={onDismiss}>
        <X className="size-3.5" />
      </Button>
    </div>
    <div className="agent-approval-context">{approval.context}</div>
    <ul className="agent-approval-options" aria-label={t("确认选项")}>
      {approval.options.map((option) => <li key={option.id}>
        <Button variant="ghost" className="agent-approval-option" isDisabled={pending !== null}
          onPress={() => void choose(option.id)}>
          <span className="agent-approval-number" aria-hidden="true">{option.id + 1}</span>
          <span className="agent-approval-label">{option.label}</span>
          {pending === option.id ? <LoaderCircle className="size-3.5 animate-spin" aria-hidden="true" /> : <ArrowUpRight className="size-3.5" aria-hidden="true" />}
        </Button>
      </li>)}
    </ul>
    {error ? <p className="agent-approval-feedback" role="alert">{error}</p>
      : pending !== null && <p className="agent-approval-feedback" role="status">{sent ? t("已提交，等待 AI 响应…") : t("正在提交…")}</p>}
  </Card>;
}

/** AI 在终端里等你选，但认不出是什么选项（比如它自己的菜单、多选框）：说清楚，给个去终端的入口 */
export function AgentWaitingNotice({ name, onTerminal, onDismiss }: { name: string; onTerminal: () => void; onDismiss: () => void }) {
  return <Card className="agent-approval" role="region" aria-label={t("{{name}} 等待确认", { name })}>
    <div className="agent-approval-heading">
      <ShieldQuestion className="size-4" aria-hidden="true" />
      <span>{name} {t("· 在终端里等你选择")}</span>
      <Button variant="ghost" size="icon-xs" aria-label={t("收起确认框")} title={t("收起")} onPress={onDismiss}>
        <X className="size-3.5" />
      </Button>
    </div>
    <p className="agent-approval-feedback">{t("这个界面的选项 P2PCLI 认不出来，请在终端里用方向键和回车选择。")}</p>
    <Button variant="secondary" size="sm" className="self-start" onPress={onTerminal}>
      <TerminalSquare className="size-3.5" />{t("去终端处理")}
    </Button>
  </Card>;
}
