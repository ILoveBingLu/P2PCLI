import { useEffect, useState } from "react";
import { Alert, Chip, Description, Disclosure, Label, ListBox, Select, Spinner } from "@heroui/react";
import { Check, FileBox, RefreshCw, Square } from "lucide-react";
import { api, formatSize } from "../api";
import {
  collaborationAgo, collaborationErrorText, collaborationTime, expectCollaboration, sessionKey, sessionRef, taskStateColor, taskStateLabels,
  type CollaborationAction, type CollaborationArtifact, type CollaborationTask, type SessionRef,
} from "../lib/collaboration";
import { taskTeam } from "../lib/collaborationLinks";
import { useCollaboration } from "./CollaborationContext";
import { assignableSessions, useCollaborationRole } from "./CollaborationDrawer";
import { Button } from "./ui/button";
import { t } from "@/i18n";

/** 一个任务：交给谁、要做什么、做到哪了、结果；能验收、重做、让它停下、换个 AI */
export function CollaborationTaskDetail({ task, mutate }: {
  task: CollaborationTask;
  mutate: (action: CollaborationAction) => Promise<boolean>;
}) {
  const collab = useCollaboration();
  const { canDispatch } = useCollaborationRole();
  const [artifacts, setArtifacts] = useState<CollaborationArtifact[]>([]);
  const [artifactError, setArtifactError] = useState("");
  const [artifactLoading, setArtifactLoading] = useState(true);
  const [reassign, setReassign] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    setArtifactLoading(true); setArtifactError("");
    api.collabRequest(collab.target, task.project_id, { type: "artifacts_list", task_id: task.id }).then((response) => {
      if (active) setArtifacts(expectCollaboration(response, "artifacts").artifacts);
    }).catch((error) => { if (active) setArtifactError(collaborationErrorText(error)); })
      .finally(() => { if (active) setArtifactLoading(false); });
    return () => { active = false; };
  }, [collab.target, task.project_id, task.id, task.version]);

  const who = (ref: SessionRef) => {
    const name = collab.sessions.find((s) => s.id === ref.session_id && s.device_id === ref.device_id)?.name ?? t("AI 会话");
    return `${collab.deviceName(ref.device_id)} · ${name}`;
  };
  const team = taskTeam(task);
  const last = task.attempts.at(-1);
  const result = last?.result;
  // 验收前，结果里说的文件都得已经传到协调设备
  const required = result?.artifact_ids ?? [];
  const allReceived = required.every((id) => artifacts.some((a) => a.id === id && a.received && a.local_path && a.attempt_id === last?.id));
  const disabled = collab.busy || collab.loading || !!collab.error;
  const sessions = assignableSessions(collab.sessions, collab.members);
  const target = sessions.find((session) => sessionKey(sessionRef(session)) === reassign);
  const act = (action: CollaborationAction) => void mutate(action);
  const version = { task_id: task.id, expected_version: task.version };

  return <div className="collab-detail">
    <div className="collab-detail-title">
      <h3>{task.spec.title}</h3>
      <Chip size="sm" variant="soft" color={taskStateColor(task.state)}>{taskStateLabels()[task.state]}</Chip>
    </div>
    <dl className="collab-facts">
      <dt>{team ? t("主会话") : t("交给")}</dt><dd>{who(task.spec.assignee)}</dd>
      {team && <><dt>{t("一起做的")}</dt><dd>{team.participants.map(who).join("、")}</dd></>}
      <dt>{t("更新")}</dt><dd>{collaborationAgo(task.updated_at)}</dd>
    </dl>

    {canDispatch && <div className="collab-detail-actions">
      {task.state === "result_ready" && <Button size="sm" disabled={disabled || artifactLoading || !!artifactError || !allReceived}
        onPress={() => act({ type: "task_accept", ...version })}><Check />{t("验收通过")}</Button>}
      {["result_ready", "failed", "cancelled"].includes(task.state) && <Button size="sm" variant="secondary"
        disabled={disabled || (!!last && !last.stopped)}
        onPress={() => act({ type: "task_requeue", ...version })}><RefreshCw />{t("重新做")}</Button>}
      {["queued", "running", "blocked"].includes(task.state) && <Button size="sm" variant="destructive" disabled={disabled}
        onPress={() => act({ type: "task_cancel", ...version })}><Square />{task.state === "queued" ? t("取消任务") : t("让它停下")}</Button>}
    </div>}
    {task.state === "queued" && <Alert status="accent">
      <Alert.Indicator />
      <Alert.Content><Alert.Description>{t("等它来领：那边的 AI 要在运行、并且主动领取任务才会开始做。")}</Alert.Description></Alert.Content>
    </Alert>}
    {task.state === "cancel_requested" && <Alert status="warning">
      <Alert.Indicator />
      <Alert.Content><Alert.Description>{t("已经让它停下，等那边确认。")}</Alert.Description></Alert.Content>
    </Alert>}
    {task.state === "result_ready" && !artifactLoading && !artifactError && !allReceived && <Alert status="warning">
      <Alert.Indicator />
      <Alert.Content><Alert.Description>{t("结果里的文件还没传完，传完才能验收。")}</Alert.Description></Alert.Content>
    </Alert>}

    {canDispatch && task.state === "queued" && !team && sessions.length > 1 && <div className="collab-reassign">
      <Select placeholder={t("换一个 AI")} selectedKey={reassign} isDisabled={disabled}
        onSelectionChange={(key) => setReassign(key == null ? null : String(key))}>
        <Label>{t("改交给")}</Label>
        <Select.Trigger><Select.Value /><Select.Indicator /></Select.Trigger>
        <Select.Popover><ListBox>
          {sessions.filter((s) => sessionKey(sessionRef(s)) !== sessionKey(task.spec.assignee)).map((session) =>
            <ListBox.Item key={sessionKey(sessionRef(session))} id={sessionKey(sessionRef(session))} textValue={`${collab.deviceName(session.device_id)} · ${session.name}`}>
              <Label>{session.name}</Label><Description>{collab.deviceName(session.device_id)}</Description><ListBox.ItemIndicator />
            </ListBox.Item>)}
        </ListBox></Select.Popover>
      </Select>
      <Button size="sm" variant="secondary" disabled={disabled || !target}
        onPress={() => target && act({ type: "task_assign", ...version, assignee: sessionRef(target) })}>{t("改交给它")}</Button>
    </div>}

    <section className="collab-section"><h4>{t("要做什么")}</h4><p>{task.spec.goal}</p></section>
    {task.spec.acceptance && <section className="collab-section"><h4>{t("怎样算完成")}</h4><p>{task.spec.acceptance}</p></section>}
    {task.progress && <section className="collab-section"><h4>{t("最新进度")}</h4><p>{task.progress}</p></section>}
    {result && <section className="collab-section">
      <h4>{result.failed ? t("结果（它说没做成）") : t("结果")}</h4>
      <p>{result.summary}</p>
      {([[t("做完的"), result.completed], [t("还没做的"), result.remaining], [t("怎么验证的"), result.verification]] as const)
        .filter(([, items]) => items.length > 0)
        .map(([label, items]) => <div key={label}><h5>{label}</h5><ul>{items.map((item, i) => <li key={i}>{item}</li>)}</ul></div>)}
    </section>}

    {(artifactLoading || artifactError || artifacts.length > 0) && <section className="collab-section">
      <h4>{t("文件")}</h4>
      {artifactLoading ? <Spinner size="sm" /> : artifactError ? <p className="collab-error-text" role="alert">{artifactError}</p>
        : artifacts.map((artifact) => <div className="collab-artifact" key={artifact.id}>
          <FileBox className="size-4" aria-hidden="true" />
          <div><Label>{artifact.name}</Label>
            <Description>{formatSize(artifact.size)} · {artifact.received ? t("已传到") : t("还在传")}{artifact.local_path ? ` · ${artifact.local_path}` : ""}</Description></div>
        </div>)}
    </section>}

    <Disclosure className="collab-more">
      <Disclosure.Heading>
        <Disclosure.Trigger className="collab-more-trigger">{t("执行记录")} ({task.attempts.length})<Disclosure.Indicator /></Disclosure.Trigger>
      </Disclosure.Heading>
      <Disclosure.Content>
        <Disclosure.Body className="collab-more-body">
          {!task.attempts.length && <Description>{t("还没有 AI 领取这个任务")}</Description>}
          {[...task.attempts].reverse().map((attempt, i) => <div key={attempt.id} className="collab-attempt">
            <Label>{t("第 {{n}} 次 · {{who}}", { n: task.attempts.length - i, who: who(attempt.assignee) })}</Label>
            <Description>{t("开始于 {{start}}，最后活动 {{last}}", { start: collaborationTime(attempt.started_at), last: collaborationTime(attempt.last_seen) })}
              {" · "}{attempt.result ? (attempt.result.failed ? t("回报没做成") : t("已回报")) : attempt.stopped ? t("已停下") : t("还在做")}</Description>
          </div>)}
          <Description>{t("任务编号")} <code>{task.id}</code></Description>
        </Disclosure.Body>
      </Disclosure.Content>
    </Disclosure>
  </div>;
}
