import { useRef, useState } from "react";
import { ArrowUpRight, LoaderCircle, ShieldQuestion, TerminalSquare } from "lucide-react";
import type { AgentApproval as Approval } from "../api";
import { Button } from "./ui/button";
import { Card } from "./ui/card";
import { toast } from "./ui/toast";

/** 单次确认的选项。由父级用 approval.id 作为 key，下一轮请求不会继承提交状态。 */
export function AgentApproval({ approval, name, onChoose, onTerminal }: {
  approval: Approval;
  name: string;
  onChoose: (approvalId: string, optionId: number) => Promise<void>;
  onTerminal: () => void;
}) {
  const lock = useRef(false);
  const [pending, setPending] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [sent, setSent] = useState(false);
  const choose = async (id: number) => {
    if (lock.current) return;
    lock.current = true;
    setPending(id);
    try { await onChoose(approval.id, id); setSent(true); }
    catch (e) { setError(String(e)); toast.error(String(e)); }
    // 成功或结果不明时都不重发同一个确认；等待被控端返回新快照。
  };
  return <Card className="agent-approval" role="region" aria-label={`${name} 等待确认`}>
    <div className="agent-approval-heading">
      <ShieldQuestion className="size-4" aria-hidden="true" />
      <span>{name} · 等待确认</span>
      <Button variant="ghost" size="icon-xs" aria-label="在终端中处理确认" title="在终端中处理" onPress={onTerminal}>
        <TerminalSquare className="size-3.5" />
      </Button>
    </div>
    <div className="agent-approval-context">{approval.context}</div>
    <ul className="agent-approval-options" aria-label="确认选项">
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
      : pending !== null && <p className="agent-approval-feedback" role="status">{sent ? "已提交，等待 AI 响应…" : "正在提交…"}</p>}
  </Card>;
}
