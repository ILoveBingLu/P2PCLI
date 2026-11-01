import { useRef, useState } from "react";
import { ArrowRight, Check, Copy, LoaderCircle, Send, Terminal } from "lucide-react";
import { api } from "../api";
import {
  CollaborationError, collaborationErrorText, expectCollaboration, sessionRef,
  type TaskSpec,
} from "../lib/collaboration";
import {
  endpointKey, endpointLabel, linkInputs, matchingSessions, registrationPrompt, sessionReady,
  type CollaborationEndpoint,
} from "../lib/collaborationLinks";
import { useCollaboration } from "./CollaborationContext";
import { Button } from "./ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "./ui/dialog";
import { toast } from "./ui/toast";

export function CollaborationLinkDialog() {
  const collab = useCollaboration();
  return collab.link ? <LinkDialog key={`${endpointKey(collab.link.source)}:${endpointKey(collab.link.destination)}`} /> : null;
}

function LinkDialog() {
  const collab = useCollaboration();
  const link = collab.link!;
  const [goal, setGoal] = useState("");
  const [acceptance, setAcceptance] = useState("");
  const [sessionId, setSessionId] = useState("");
  const [terminalId, setTerminalId] = useState(link.destination.terminalId ?? "");
  const [consent, setConsent] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [prompt, setPrompt] = useState("");
  const [uncertain, setUncertain] = useState(false);
  const [submitted, setSubmitted] = useState(false);
  const [preparing, setPreparing] = useState(false);
  const submission = useRef<{ projectId: string; requestId: string; spec: TaskSpec } | null>(null);
  const owner = collab.overview?.endpoint_id ?? "";
  const matches = matchingSessions(collab.sessions, link.destination, owner);
  const ready = matches.filter((session) => sessionReady(session, collab.members));
  const selected = ready.find((session) => session.id === sessionId) ?? (ready.length === 1 ? ready[0] : null);
  const terminals = collab.terminals.filter((terminal) => terminal.deviceId === link.destination.deviceId &&
    (!link.destination.terminalId || terminal.terminalId === link.destination.terminalId));
  const terminal = terminals.find((terminal) => terminal.terminalId === terminalId) ?? (terminals.length === 1 ? terminals[0] : null);
  const destination: CollaborationEndpoint = terminal ?? link.destination;
  const member = collab.members.find((member) => member.device_id === destination.deviceId);
  const own = collab.members.find((member) => member.device_id === owner && !member.revoked);
  const canDispatch = !collab.project ? !collab.target : own?.role === "admin" || own?.role === "dispatcher";
  const canGrant = !collab.project ? !collab.target : own?.role === "admin";
  const needsGrant = destination.deviceId !== owner && !member;
  const denied = !!member && (member.revoked || member.role === "viewer");
  const blocked = collab.loading || collab.busy || !!collab.error || !canDispatch;
  const frozen = !!submission.current;

  const prepare = async (copyOnly: boolean) => {
    if (blocked || preparing || denied || !terminal || (needsGrant && (!consent || !canGrant))) return;
    setError(""); setNotice(""); setPreparing(true);
    try {
      const text = await collab.run(async () => {
        const project = await collab.ensureProject();
        const members = expectCollaboration(await api.collabRequest(collab.target, project.id, { type: "members_list" }), "members").members;
        const existing = members.find((item) => item.device_id === destination.deviceId);
        if (existing?.revoked || existing?.role === "viewer") throw new Error("该设备的协作访问已受限，请在高级管理中核对权限");
        if (!existing) {
          if (!consent) throw new Error("请先确认设备授权");
          expectCollaboration(await api.collabRequest(collab.target, project.id, {
            type: "member_set", device_id: destination.deviceId, role: "worker", revoked: false,
          }), "ack");
        }
        return registrationPrompt(project.id, project.coordinator_id, owner, destination);
      });
      setPrompt(text);
      if (copyOnly) {
        await navigator.clipboard.writeText(text);
        toast.success("已复制接入请求");
      } else {
        collab.onPrepare(destination, text);
        collab.setLink(null);
        toast.notice("接入请求已准备", { description: "打开目标会话后确认输入草稿；完成登记后再分配任务。" });
      }
    } catch (e) { setError(collaborationErrorText(e)); }
    finally { setPreparing(false); }
  };

  const submit = async () => {
    if (blocked || submitted || (!submission.current && (!selected || !goal.trim()))) return;
    setError("");
    try {
      const response = await collab.run(async () => {
        if (!submission.current) {
          const project = await collab.ensureProject();
          const [sessionsResult, membersResult] = await Promise.all([
            api.collabRequest(collab.target, project.id, { type: "sessions_list" }),
            api.collabRequest(collab.target, project.id, { type: "members_list" }),
          ]);
          const sessions = expectCollaboration(sessionsResult, "sessions").sessions;
          const members = expectCollaboration(membersResult, "members").members;
          const session = matchingSessions(sessions, link.destination, owner).find((session) => session.id === selected?.id);
          if (!session || !sessionReady(session, members)) throw new Error("执行会话已不可用，请重新选择或接入");
          submission.current = {
            projectId: project.id, requestId: crypto.randomUUID().replaceAll("-", ""),
            spec: {
              title: goal.trim().split("\n")[0].slice(0, 120), goal: goal.trim(), acceptance: acceptance.trim(),
              inputs: linkInputs(link.source, owner), assignee: sessionRef(session),
            },
          };
        }
        const request = submission.current;
        return api.collabRequest(collab.target, request.projectId, { type: "task_create", spec: request.spec }, request.requestId);
      });
      if (response.type === "queued" || response.type === "committed_unverified") {
        setSubmitted(true); setUncertain(false);
        setNotice(response.type === "queued" ? "任务已进入待发队列，尚未得到协调设备确认。" : "已有提交记录，远端状态待核实。请勿重复创建任务。");
        return;
      }
      const task = expectCollaboration(response, "task").task;
      collab.selectTask(task.id); collab.setOpen(true); collab.setLink(null);
      toast.success("任务已创建，等待执行会话领取");
    } catch (e) {
      if (e instanceof CollaborationError) { submission.current = null; setUncertain(false); }
      else setUncertain(!!submission.current);
      setError(collaborationErrorText(e));
    }
  };
  const noAccess = !canDispatch && !collab.loading && !collab.error;
  return <Dialog open onOpenChange={(open) => { if (!open && !collab.busy && !preparing) collab.setLink(null); }}>
    <DialogContent className="collab-link-dialog">
      <DialogHeader>
        <DialogTitle>连接协作</DialogTitle>
        <DialogDescription>{endpointLabel(link.source)} → {endpointLabel(link.destination)}</DialogDescription>
      </DialogHeader>
      <div className="collab-link-route" aria-hidden="true">
        <div><Terminal /><span>{link.source.sessionName ?? link.source.deviceName}</span></div><ArrowRight />
        <div><Terminal /><span>{link.destination.sessionName ?? link.destination.deviceName}</span></div>
      </div>
      {collab.loading ? <p className="collab-link-message" role="status"><LoaderCircle className="animate-spin" />读取协作会话</p> :
        collab.error ? <p className="collab-error-text" role="alert">{collab.error}</p> :
        noAccess ? <p className="collab-error-text" role="alert">当前设备没有分配任务的权限。</p> :
        submitted ? <p className="collab-notice" role="status">{notice}</p> :
        ready.length > 0 ? <form onSubmit={(event) => { event.preventDefault(); void submit(); }}>
          {ready.length > 1 && <label className="collab-link-field">执行会话
            <select aria-label="执行会话" value={sessionId} disabled={blocked || frozen} onChange={(event) => setSessionId(event.target.value)}>
              <option value="">选择会话</option>
              {ready.map((session) => <option key={session.id} value={session.id}>{session.name}</option>)}
            </select>
          </label>}
          {selected && <p className="collab-link-ready"><Check />{selected.name}<span>{selected.mode === "manual_pull" ? "等待会话主动领取" : "托管执行"}</span></p>}
          <label className="collab-link-field">让它做什么
            <textarea autoFocus aria-label="让它做什么" rows={4} maxLength={16000} value={goal}
              disabled={blocked || frozen} onChange={(event) => setGoal(event.target.value)} />
          </label>
          <details className="collab-link-advanced">
            <summary>更多选项</summary>
            <label className="collab-link-field">验收条件
              <textarea aria-label="验收条件" rows={2} value={acceptance} disabled={blocked || frozen} onChange={(event) => setAcceptance(event.target.value)} />
            </label>
            <p>{collab.project?.name} · 记录保存在{collab.deviceName(collab.target ?? owner)}</p>
          </details>
          {error && <p className="collab-error-text" role="alert">{error}</p>}
          {uncertain && <p className="collab-notice">提交结果未确认。重试将沿用原请求，不会再次创建任务。</p>}
          <DialogFooter>
            <Button variant="ghost" onPress={() => collab.setLink(null)} disabled={collab.busy}>取消</Button>
            <Button type="submit" disabled={blocked || (!frozen && (!goal.trim() || !selected))}>
              {collab.busy ? <LoaderCircle className="animate-spin" /> : <Send />}{uncertain ? "重试原请求" : "确认协作"}
            </Button>
          </DialogFooter>
        </form> : <div>
          <p className="collab-link-message"><Terminal />{denied ? "此设备的协作访问受限" : matches.length ? "会话已失联或不可用" : "待接入"}</p>
          {!link.destination.terminalId && terminals.length > 0 && <label className="collab-link-field">接入会话
            <select aria-label="接入会话" value={terminal?.terminalId ?? ""} disabled={blocked} onChange={(event) => setTerminalId(event.target.value)}>
              <option value="">选择 AI 会话</option>
              {terminals.map((terminal) => <option key={terminal.terminalId} value={terminal.terminalId}>{terminal.sessionName}</option>)}
            </select>
          </label>}
          {!terminals.length && !link.destination.terminalId && <p className="collab-note">此设备暂无可接入的 AI 会话。</p>}
          {!terminal && link.destination.terminalId && <p className="collab-note">此终端未检测到可接入的 AI 会话。</p>}
          {needsGrant && <label className="collab-grant"><input type="checkbox" checked={consent} disabled={!canGrant || blocked}
            onChange={(event) => setConsent(event.target.checked)} />
            <span>允许{destination.deviceName}作为执行者加入{collab.project?.name ?? "设备协作"}，读取分配给它的任务并回报结果。</span>
          </label>}
          {denied && <Button variant="secondary" size="sm" onPress={() => { collab.setLink(null); collab.setAdvanced(true); }}>查看权限</Button>}
          {prompt && <details className="collab-link-advanced"><summary>接入请求</summary><pre>{prompt}</pre></details>}
          {error && <p className="collab-error-text" role="alert">{error}</p>}
          <DialogFooter>
            <Button variant="ghost" size="icon-sm" title="复制接入请求" aria-label="复制接入请求"
              disabled={blocked || preparing || denied || !terminal || (needsGrant && (!consent || !canGrant))}
              onPress={() => void prepare(true)}><Copy /></Button>
            <Button disabled={blocked || preparing || denied || !terminal || (needsGrant && (!consent || !canGrant))}
              onPress={() => void prepare(false)}>{preparing ? <LoaderCircle className="animate-spin" /> : <Terminal />}准备接入请求</Button>
          </DialogFooter>
        </div>}
      {(collab.error || noAccess || submitted) && <DialogFooter>
        <Button variant="secondary" onPress={() => { collab.setLink(null); collab.setOpen(true); }}>查看协作</Button>
      </DialogFooter>}
    </DialogContent>
  </Dialog>;
}
