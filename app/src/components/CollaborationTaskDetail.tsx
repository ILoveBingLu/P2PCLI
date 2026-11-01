import { useEffect, useState } from "react";
import { ArrowLeft, Check, FileBox, LoaderCircle, RefreshCw, Square, UserRound } from "lucide-react";
import { ListBox, Select } from "@heroui/react";
import { api, formatSize } from "../api";
import {
  collaborationErrorText, collaborationTime, expectCollaboration, sessionKey, sessionRef, shortId, taskStateLabels,
  type CollaborationArtifact, type CollaborationSession, type CollaborationTask,
} from "../lib/collaboration";
import { Button } from "./ui/button";
import { CollaborationField, type CollaborationMutation } from "./CollaborationForms";
import { taskTeam } from "../lib/collaborationLinks";

export function CollaborationTaskDetail({ task, sessions, target, canDispatch, disabled, mutate, onBack, compact = false, deviceName = shortId }: {
  task: CollaborationTask; sessions: CollaborationSession[]; target: string | null;
  canDispatch: boolean; disabled: boolean; mutate: CollaborationMutation; onBack: () => void;
  compact?: boolean; deviceName?: (id: string) => string;
}) {
  const [assignee, setAssignee] = useState("");
  const [artifacts, setArtifacts] = useState<CollaborationArtifact[]>([]);
  const [artifactError, setArtifactError] = useState("");
  const [artifactLoading, setArtifactLoading] = useState(true);
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    let active = true;
    setArtifactLoading(true); setArtifactError("");
    api.collabRequest(target, task.project_id, { type: "artifacts_list", task_id: task.id }).then((response) => {
      if (active) setArtifacts(expectCollaboration(response, "artifacts").artifacts);
    }).catch((error) => { if (active) setArtifactError(collaborationErrorText(error)); })
      .finally(() => { if (active) setArtifactLoading(false); });
    return () => { active = false; };
  }, [target, task.project_id, task.id, task.version, revision]);
  const name = (ref: { device_id: string; session_id: string }) => sessions.find((s) => s.id === ref.session_id && s.device_id === ref.device_id)?.name ?? shortId(ref.session_id);
  const selected = sessions.find((s) => sessionKey(sessionRef(s)) === assignee);
  const team = taskTeam(task);
  const result = task.attempts.at(-1)?.result;
  const requiredArtifacts = result?.artifact_ids ?? [];
  const allReceived = requiredArtifacts.every((id) => artifacts.some((artifact) => artifact.id === id && artifact.received && artifact.local_path && artifact.task_id === task.id && artifact.attempt_id === task.attempts.at(-1)?.id));
  return <section className="collab-detail" aria-label="任务详情">
    <header className="collab-detail-heading">
      <Button variant="ghost" size="icon-sm" title="返回任务列表" aria-label="返回任务列表" onPress={onBack}><ArrowLeft /></Button>
      <span className="collab-state" data-state={task.state}>{taskStateLabels[task.state]}</span>
      <span className="collab-note">v{task.version}</span>
    </header>
    <div className="collab-detail-scroll">
      <h2>{task.spec.title}</h2>
      <dl className="collab-metadata">
        <dt>{team ? "主会话" : "执行会话"}</dt><dd>{name(task.spec.assignee)}</dd>
        {team && <><dt>参与会话</dt><dd>{team.participants.map(name).join("、")}</dd></>}
        <dt>设备</dt><dd title={task.spec.assignee.device_id}>{deviceName(task.spec.assignee.device_id)}</dd>
        {!compact && <><dt>任务 ID</dt><dd className="collab-code">{task.id}</dd></>}
        <dt>最近更新</dt><dd>{collaborationTime(task.updated_at)}</dd>
      </dl>
      {canDispatch && <div className="collab-task-actions">
        {task.state === "result_ready" && <Button size="sm" disabled={disabled || artifactLoading || !!artifactError || !allReceived} onPress={() => void mutate({ type: "task_accept", task_id: task.id, expected_version: task.version })}><Check />验收通过</Button>}
        {["result_ready", "failed", "cancelled"].includes(task.state) && <Button size="sm" variant="secondary" disabled={disabled || (task.attempts.length > 0 && !task.attempts.at(-1)?.stopped)} onPress={() => void mutate({ type: "task_requeue", task_id: task.id, expected_version: task.version })}><RefreshCw />重新排队</Button>}
        {["queued", "running", "blocked"].includes(task.state) && <Button size="sm" variant="destructive" disabled={disabled} onPress={() => void mutate({ type: "task_cancel", task_id: task.id, expected_version: task.version })}><Square />{task.state === "queued" ? "取消任务" : "请求停止"}</Button>}
      </div>}
      {task.state === "cancel_requested" && <p className="collab-notice">等待执行会话确认停止，当前不能重新分配。</p>}
      {task.state === "result_ready" && !artifactLoading && !artifactError && !allReceived && <p className="collab-notice">结果产物尚未全部接收，暂不能验收。</p>}
      {canDispatch && task.state === "queued" && !compact && !team && <form className="collab-assignment" onSubmit={(e) => {
        e.preventDefault();
        if (selected?.available && !disabled) void mutate({ type: "task_assign", task_id: task.id, expected_version: task.version, assignee: sessionRef(selected) });
      }}>
        <CollaborationField label="调整分配">
          <Select
            aria-label="调整分配"
            placeholder="选择执行会话"
            selectedKey={assignee || null}
            onSelectionChange={(key) => setAssignee(key == null ? "" : String(key))}
          >
            <Select.Trigger><Select.Value /><Select.Indicator /></Select.Trigger>
            <Select.Popover>
              <ListBox>
                {sessions.map((session) => (
                  <ListBox.Item
                    key={sessionKey(sessionRef(session))}
                    id={sessionKey(sessionRef(session))}
                    isDisabled={!session.available}
                    textValue={`${session.name} · ${shortId(session.device_id)}${!session.available ? " · 不可用" : ""}`}
                  >
                    {session.name} · {shortId(session.device_id)}{!session.available ? " · 不可用" : ""}
                  </ListBox.Item>
                ))}
              </ListBox>
            </Select.Popover>
          </Select>
        </CollaborationField>
        <Button type="submit" variant="secondary" size="icon-sm" title="确认分配" aria-label="确认分配" disabled={disabled || !selected?.available || assignee === sessionKey(task.spec.assignee)}><UserRound /></Button>
      </form>}
      <section className="collab-detail-section"><h3>目标</h3><p>{task.spec.goal}</p></section>
      {(!compact || task.spec.acceptance) && <section className="collab-detail-section"><h3>验收条件</h3><p>{task.spec.acceptance || "未填写"}</p></section>}
      {task.progress && <section className="collab-detail-section"><h3>最新进度</h3><p>{task.progress}</p></section>}
      {!compact && <section className="collab-detail-section"><h3>输入资料</h3><pre>{JSON.stringify(task.spec.inputs, null, 2)}</pre></section>}
      <section className="collab-detail-section"><h3>执行历史 <span>{task.attempts.length}</span></h3>
        {!task.attempts.length && <p className="collab-note">尚未领取</p>}
        {[...task.attempts].reverse().map((attempt, index) => <article className="collab-attempt" key={attempt.id}>
          <div className="collab-row"><strong>执行 {task.attempts.length - index} · {name(attempt.assignee)}</strong><span className="collab-note">{attempt.result ? "已回报" : attempt.stopped ? "已停止" : "执行中"}</span></div>
          <p className="collab-note">开始 {collaborationTime(attempt.started_at)} · 最近活动 {collaborationTime(attempt.last_seen)}</p>
          <code className="collab-note">{attempt.id}</code>
          {attempt.result && <>
            <p>{attempt.result.summary}</p>
            {attempt.result.failed && <p className="collab-error-text">本次执行报告失败</p>}
            {([["完成项", attempt.result.completed], ["待完成", attempt.result.remaining], ["验证", attempt.result.verification]] as const).map(([label, values]) => values.length > 0 && <div key={label}><h4>{label}</h4><ul>{values.map((value, i) => <li key={i}>{value}</li>)}</ul></div>)}
          </>}
        </article>)}
      </section>
      {compact && <details className="collab-link-advanced collab-detail-section">
        <summary>技术信息</summary><p>任务 ID <code>{task.id}</code></p>
        <pre>{JSON.stringify(task.spec.inputs, null, 2)}</pre>
      </details>}
      <section className="collab-detail-section">
        <div className="collab-row"><h3>产物</h3><Button variant="ghost" size="icon-sm" title="刷新产物" aria-label="刷新产物" disabled={artifactLoading} onPress={() => setRevision((v) => v + 1)}><RefreshCw /></Button></div>
        {artifactLoading ? <p className="collab-note"><LoaderCircle className="animate-spin" />读取产物</p> : artifactError ? <p role="alert" className="collab-error-text">{artifactError}</p> : !artifacts.length ? <p className="collab-note">暂无产物</p> : artifacts.map((artifact) => <div className="collab-artifact" key={artifact.id}>
          <FileBox /><div><strong>{artifact.name}</strong><p className="collab-note">{formatSize(artifact.size)} · {artifact.received ? "已接收" : "等待接收"}</p><code>{artifact.hash}</code>{artifact.local_path && <p className="collab-note">协调设备：{artifact.local_path}</p>}</div>
        </div>)}
      </section>
    </div>
  </section>;
}
