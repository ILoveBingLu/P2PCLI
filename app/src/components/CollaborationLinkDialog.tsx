import { useId, useRef, useState } from "react";
import {
  Accordion, Alert, Avatar, AvatarGroup, Button, Checkbox, Description, Fieldset, Form, Label,
  ListBox, Modal, Select, Spinner, TextArea, TextField, Tooltip,
} from "@heroui/react";
import { Copy, Monitor, Send, Terminal } from "lucide-react";
import { api } from "../api";
import { writeClipboardText } from "../lib/clipboard";
import {
  CollaborationError, collaborationErrorText, expectCollaboration, sessionRef,
  type CollaborationSession, type TaskSpec,
} from "../lib/collaboration";
import {
  canCoordinate, endpointKey, linkInputs, matchingSessions, registrationDrafts, registrationDraftText, sessionReady, teamParticipants, teamTaskSpec,
  type CollaborationEndpoint,
} from "../lib/collaborationLinks";
import { useCollaboration } from "./CollaborationContext";
import { AgentIcon } from "./Agent";
import { PlatformIcon } from "./PlatformIcon";
import { toast } from "./ui/toast";
import { t } from "@/i18n";

const ALL_SESSIONS = "all-sessions";

export function CollaborationLinkDialog() {
  const collab = useCollaboration();
  return collab.link ? <LinkDialog key={`${endpointKey(collab.link.source)}:${endpointKey(collab.link.destination)}`} /> : null;
}

function LinkDialog() {
  const collab = useCollaboration();
  const formId = useId();
  const descriptionId = useId();
  const link = collab.link!;
  const [goal, setGoal] = useState("");
  const [acceptance, setAcceptance] = useState("");
  const [sessionId, setSessionId] = useState("");
  const [leadId, setLeadId] = useState("");
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
  const allSessions = sessionId === ALL_SESSIONS && !link.destination.terminalId;
  const selected = allSessions ? null : ready.find((session) => session.id === sessionId) ?? (ready.length === 1 ? ready[0] : null);
  const leads = matchingSessions(collab.sessions, link.source, owner).filter((session) => canCoordinate(session, collab.members));
  const lead = leads.find((session) => session.id === leadId) ?? (!leadId && leads.length === 1 ? leads[0] : null);
  const participants = teamParticipants(ready, lead);
  const hasAssignee = allSessions ? !!lead && participants.length > 0 : !!selected;
  const terminals = collab.terminals.filter((terminal) => terminal.deviceId === link.destination.deviceId &&
    (!link.destination.terminalId || terminal.terminalId === link.destination.terminalId));
  const allTerminals = terminalId === ALL_SESSIONS && !link.destination.terminalId;
  const terminal = allTerminals ? null : terminals.find((terminal) => terminal.terminalId === terminalId) ??
    (!terminalId && terminals.length === 1 ? terminals[0] : null);
  const selectedTerminals = allTerminals ? terminals : terminal ? [terminal] : [];
  const destination: CollaborationEndpoint = terminal ?? link.destination;
  const member = collab.members.find((member) => member.device_id === destination.deviceId);
  const own = collab.members.find((member) => member.device_id === owner && !member.revoked);
  const canDispatch = !collab.project ? !collab.target : own?.role === "admin" || own?.role === "dispatcher";
  const canGrant = !collab.project ? !collab.target : own?.role === "admin";
  const needsGrant = destination.deviceId !== owner && !member;
  const denied = !!member && (member.revoked || member.role === "viewer");
  const blocked = collab.loading || collab.busy || !!collab.error || !canDispatch;
  const frozen = !!submission.current;
  const agentIcon = (deviceId: string, terminalId?: string) => {
    const kind = collab.terminals.find((item) => item.deviceId === deviceId && item.terminalId === terminalId)?.agentKind;
    return kind ? <AgentIcon kind={kind} className="size-4" /> : <Terminal size={16} aria-hidden="true" />;
  };
  const sessionAvatars = (sessions: CollaborationSession[]) => <AvatarGroup max={5} size="sm" aria-label={t("{{length}} 个参与会话", { length: sessions.length })}>
    {sessions.map((session) => <Avatar key={session.id} aria-label={session.name} title={session.name}>
      <Avatar.Fallback>{agentIcon(session.device_id, session.terminal?.terminal_id)}</Avatar.Fallback>
    </Avatar>)}
  </AvatarGroup>;
  const terminalAvatars = <AvatarGroup max={5} size="sm" aria-label={t("{{length}} 个接入会话", { length: terminals.length })}>
    {terminals.map((item) => <Avatar key={endpointKey(item)} aria-label={item.sessionName} title={item.sessionName}>
      <Avatar.Fallback>{agentIcon(item.deviceId, item.terminalId)}</Avatar.Fallback>
    </Avatar>)}
  </AvatarGroup>;
  const endpointContent = (endpoint: CollaborationEndpoint) => {
    const platform = endpoint.deviceId === owner ? collab.overview?.platform :
      collab.overview?.nearby.find((device) => device.endpoint_id === endpoint.deviceId)?.platform ??
      collab.overview?.recent.find((device) => device.endpoint_id === endpoint.deviceId)?.platform;
    return <>
      {platform ? <PlatformIcon platform={platform} decorative /> : <Monitor size={16} aria-hidden="true" />}
      {" "}{endpoint.deviceName}
      {endpoint.sessionName && <> / {agentIcon(endpoint.deviceId, endpoint.terminalId)}{" "}{endpoint.sessionName}</>}
    </>;
  };

  const prepare = async (copyOnly: boolean) => {
    if (blocked || preparing || denied || !selectedTerminals.length || (needsGrant && (!consent || !canGrant))) return;
    setError(""); setNotice(""); setPreparing(true);
    try {
      const drafts = await collab.run(async () => {
        const project = await collab.ensureProject();
        const members = expectCollaboration(await api.collabRequest(collab.target, project.id, { type: "members_list" }), "members").members;
        const existing = members.find((item) => item.device_id === destination.deviceId);
        if (existing?.revoked || existing?.role === "viewer") throw new Error(t("该设备的协作访问已受限，请在高级管理中核对权限"));
        if (!existing) {
          if (!consent) throw new Error(t("请先确认设备授权"));
          expectCollaboration(await api.collabRequest(collab.target, project.id, {
            type: "member_set", device_id: destination.deviceId, role: "worker", revoked: false,
          }), "ack");
        }
        return registrationDrafts(project.id, project.coordinator_id, owner, selectedTerminals)
          .map((draft) => ({ ...draft, source: link.source }));
      });
      const text = registrationDraftText(drafts);
      setPrompt(text);
      if (copyOnly) {
        await writeClipboardText(text);
        toast.success(t("已复制接入请求"));
      } else {
        collab.onPrepare(drafts);
        collab.setLink(null);
      }
    } catch (e) { setError(collaborationErrorText(e)); }
    finally { setPreparing(false); }
  };

  const submit = async () => {
    if (blocked || submitted || (!submission.current && (!hasAssignee || !goal.trim()))) return;
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
          const freshTargets = matchingSessions(sessions, link.destination, owner);
          let spec: TaskSpec;
          if (allSessions) {
            const freshLead = matchingSessions(sessions, link.source, owner).find((session) => session.id === lead?.id);
            if (!freshLead) throw new Error(t("主会话已不可用，请重新选择或接入"));
            const freshParticipants = participants.map((participant) => {
              const session = freshTargets.find((session) => session.id === participant.id);
              if (!session) throw new Error(t("参与会话已变更，请重新选择"));
              return session;
            });
            spec = teamTaskSpec({
              source: link.source, destination: link.destination, ownerId: owner, coordinatorId: project.coordinator_id, goal, acceptance,
              lead: freshLead, participants: freshParticipants, members,
            });
          } else {
            const session = freshTargets.find((session) => session.id === selected?.id);
            if (!session || !sessionReady(session, members)) throw new Error(t("执行会话已不可用，请重新选择或接入"));
            spec = {
              title: goal.trim().split("\n")[0].slice(0, 120), goal: goal.trim(), acceptance: acceptance.trim(),
              inputs: linkInputs(link.source, owner), assignee: sessionRef(session),
            };
          }
          submission.current = {
            projectId: project.id, requestId: crypto.randomUUID().replaceAll("-", ""),
            spec,
          };
        }
        const request = submission.current;
        return api.collabRequest(collab.target, request.projectId, { type: "task_create", spec: request.spec }, request.requestId);
      });
      if (response.type === "queued" || response.type === "committed_unverified") {
        setSubmitted(true); setUncertain(false);
        setNotice(response.type === "queued" ? t("任务已进入待发队列，尚未得到协调设备确认。") : t("已有提交记录，远端状态待核实。请勿重复创建任务。"));
        return;
      }
      const task = expectCollaboration(response, "task").task;
      collab.selectTask(task.id); collab.setOpen(true); collab.setLink(null);
      toast.success(allSessions ? t("主任务已创建，等待主会话领取并分配") : t("任务已创建，等待执行会话领取"));
    } catch (e) {
      if (e instanceof CollaborationError) { submission.current = null; setUncertain(false); }
      else setUncertain(!!submission.current);
      setError(collaborationErrorText(e));
    }
  };
  const noAccess = !canDispatch && !collab.loading && !collab.error;
  return <Modal.Backdrop isOpen isDismissable={!collab.busy && !preparing}
    isKeyboardDismissDisabled={collab.busy || preparing}
    onOpenChange={(open) => { if (!open && !collab.busy && !preparing) collab.setLink(null); }}>
    <Modal.Container size="lg" scroll="inside">
      <Modal.Dialog aria-describedby={descriptionId}>
        <Modal.CloseTrigger aria-label={t("关闭")} isDisabled={collab.busy || preparing} />
        <Modal.Header>
          <Modal.Heading>{t("连接协作")}</Modal.Heading>
          <Description id={descriptionId}>{endpointContent(link.source)} ↔ {endpointContent(link.destination)}</Description>
        </Modal.Header>
        <Modal.Body>
          {collab.loading ? <Alert role="status">
            <Alert.Indicator><Spinner size="sm" /></Alert.Indicator>
            <Alert.Content><Alert.Title>{t("读取协作会话")}</Alert.Title></Alert.Content>
          </Alert> : collab.error || noAccess ? <Alert status="danger" role="alert">
            <Alert.Indicator />
            <Alert.Content><Alert.Description>{collab.error || t("当前设备没有分配任务的权限。")}</Alert.Description></Alert.Content>
          </Alert> : submitted ? <Alert status="warning" role="status">
            <Alert.Indicator />
            <Alert.Content><Alert.Description>{notice}</Alert.Description></Alert.Content>
          </Alert> : (ready.length > 0 || frozen) ? <Form id={formId} onSubmit={(event) => { event.preventDefault(); void submit(); }}>
            <Fieldset.Group>
              {(!link.destination.terminalId || ready.length > 1) && <Select placeholder={t("选择会话")} selectedKey={sessionId || selected?.id || null}
                isDisabled={blocked || frozen} onSelectionChange={(key) => setSessionId(key == null ? "" : String(key))}>
                <Label>{t("执行会话")}</Label>
                <Select.Trigger><Select.Value /><Select.Indicator /></Select.Trigger>
                <Select.Popover><ListBox>
                  {!link.destination.terminalId && <ListBox.Item id={ALL_SESSIONS} textValue={t("全部会话")} aria-label={t("全部会话")}>
                    {sessionAvatars(participants)}{" "}<Label>{t("全部会话")}</Label><ListBox.ItemIndicator />
                  </ListBox.Item>}
                  {ready.map((session) => <ListBox.Item key={session.id} id={session.id} textValue={session.name}>
                    {agentIcon(session.device_id, session.terminal?.terminal_id)}{" "}
                    <Label>{session.name}</Label><ListBox.ItemIndicator />
                  </ListBox.Item>)}
                </ListBox></Select.Popover>
              </Select>}
              {allSessions && <>
                {(leads.length > 1 || !!leadId) && <Select placeholder={t("选择主会话")} selectedKey={leadId || null}
                  isDisabled={blocked || frozen} onSelectionChange={(key) => setLeadId(key == null ? "" : String(key))}>
                  <Label>{t("主会话")}</Label>
                  <Select.Trigger><Select.Value /><Select.Indicator /></Select.Trigger>
                  <Select.Popover><ListBox>
                    {leads.map((session) => <ListBox.Item key={session.id} id={session.id} textValue={session.name}>
                      {agentIcon(session.device_id, session.terminal?.terminal_id)}{" "}
                      <Label>{session.name}</Label><ListBox.ItemIndicator />
                    </ListBox.Item>)}
                  </ListBox></Select.Popover>
                </Select>}
                {lead && participants.length > 0 ? <Alert status="success">
                  <Alert.Indicator>{agentIcon(lead.device_id, lead.terminal?.terminal_id)}</Alert.Indicator>
                  <Alert.Content>
                    <Alert.Title>{t("主会话：")}{lead.name}</Alert.Title>
                    <Alert.Description>{participants.length} {t("个参与会话 · 等待主会话领取并拆分分配")}</Alert.Description>
                  </Alert.Content>
                </Alert> : <Alert status="warning" role="status">
                  <Alert.Indicator />
                  <Alert.Content><Alert.Description>{!leads.length ?
                    t("起点没有可用且具备调度权限的主会话，请先接入主会话并由管理员确认权限。") :
                    !lead ? t("请选择主会话。") : t("没有不同于主会话的可用参与会话。")}</Alert.Description></Alert.Content>
                </Alert>}
              </>}
              {selected && <Alert status="success">
                <Alert.Indicator>{agentIcon(selected.device_id, selected.terminal?.terminal_id)}</Alert.Indicator>
                <Alert.Content>
                  <Alert.Title>{selected.name}</Alert.Title>
                  <Alert.Description>{selected.mode === "manual_pull" ? t("等待会话主动领取") : t("托管执行")}</Alert.Description>
                </Alert.Content>
              </Alert>}
              <TextField value={goal} onChange={setGoal} isDisabled={blocked || frozen}>
                <Label>{t("让它做什么")}</Label>
                <TextArea autoFocus rows={4} maxLength={16000} />
              </TextField>
              <Accordion>
                <Accordion.Item>
                  <Accordion.Heading><Accordion.Trigger>{t("更多选项")}<Accordion.Indicator /></Accordion.Trigger></Accordion.Heading>
                  <Accordion.Panel><Accordion.Body>
                    <TextField value={acceptance} onChange={setAcceptance} isDisabled={blocked || frozen}>
                      <Label>{t("验收条件")}</Label>
                      <TextArea rows={2} />
                      <Description>{collab.project?.name} {t("· 记录保存在")}{collab.deviceName(collab.target ?? owner)}</Description>
                    </TextField>
                  </Accordion.Body></Accordion.Panel>
                </Accordion.Item>
              </Accordion>
              {error && <Alert status="danger" role="alert">
                <Alert.Indicator /><Alert.Content><Alert.Description>{error}</Alert.Description></Alert.Content>
              </Alert>}
              {uncertain && <Alert status="warning" role="status">
                <Alert.Indicator />
                <Alert.Content><Alert.Description>{t("提交结果未确认。重试将沿用原请求，不会再次创建任务。")}</Alert.Description></Alert.Content>
              </Alert>}
            </Fieldset.Group>
          </Form> : <Fieldset.Group>
            <Alert status={denied ? "warning" : "default"}>
              <Alert.Indicator><Terminal /></Alert.Indicator>
              <Alert.Content><Alert.Title>{denied ? t("此设备的协作访问受限") : matches.length ? t("会话已失联或不可用") : t("待接入")}</Alert.Title></Alert.Content>
            </Alert>
            {!link.destination.terminalId && terminals.length > 0 && <Select placeholder={t("选择 AI 会话")}
              selectedKey={allTerminals ? ALL_SESSIONS : terminal?.terminalId ?? null} isDisabled={blocked || preparing}
              onSelectionChange={(key) => setTerminalId(key == null ? "" : String(key))}>
              <Label>{t("接入会话")}</Label>
              <Select.Trigger><Select.Value /><Select.Indicator /></Select.Trigger>
              <Select.Popover><ListBox>
                <ListBox.Item id={ALL_SESSIONS} textValue={t("全部会话")} aria-label={t("全部会话")}>
                  {terminalAvatars}{" "}<Label>{t("全部会话")}</Label><ListBox.ItemIndicator />
                </ListBox.Item>
                {terminals.map((terminal) => <ListBox.Item key={terminal.terminalId} id={terminal.terminalId} textValue={terminal.sessionName}>
                  {agentIcon(terminal.deviceId, terminal.terminalId)}{" "}
                  <Label>{terminal.sessionName}</Label><ListBox.ItemIndicator />
                </ListBox.Item>)}
              </ListBox></Select.Popover>
            </Select>}
            {!terminals.length && !link.destination.terminalId && <Description>{t("此设备暂无可接入的 AI 会话。")}</Description>}
            {!terminal && link.destination.terminalId && <Description>{t("此终端未检测到可接入的 AI 会话。")}</Description>}
            {needsGrant && <Checkbox isSelected={consent} isDisabled={!canGrant || blocked || preparing} onChange={setConsent}>
              <Checkbox.Content>
                <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
                {t("允许")}{destination.deviceName}{t("作为执行者加入")}{collab.project?.name ?? t("设备协作")}
              </Checkbox.Content>
              <Description>{t("读取分配给它的任务并回报结果。")}</Description>
            </Checkbox>}
            {denied && <Button variant="secondary" size="sm" onPress={() => { collab.setLink(null); collab.setAdvanced(true); }}>{t("查看权限")}</Button>}
            {prompt && <Accordion>
              <Accordion.Item>
                <Accordion.Heading><Accordion.Trigger>{t("接入请求")}<Accordion.Indicator /></Accordion.Trigger></Accordion.Heading>
                <Accordion.Panel><Accordion.Body>
                  <TextField aria-label={t("接入请求")} value={prompt} isReadOnly><TextArea rows={6} /></TextField>
                </Accordion.Body></Accordion.Panel>
              </Accordion.Item>
            </Accordion>}
            {error && <Alert status="danger" role="alert">
              <Alert.Indicator /><Alert.Content><Alert.Description>{error}</Alert.Description></Alert.Content>
            </Alert>}
          </Fieldset.Group>}
        </Modal.Body>
        {!collab.loading && <Modal.Footer>
          {collab.error || noAccess || submitted ?
            <Button variant="secondary" onPress={() => { collab.setLink(null); collab.setOpen(true); }}>{t("查看协作")}</Button> :
            (ready.length > 0 || frozen) ? <>
              <Button variant="ghost" onPress={() => collab.setLink(null)} isDisabled={collab.busy}>{t("取消")}</Button>
              <Button type="submit" form={formId} isDisabled={blocked || (!frozen && (!goal.trim() || !hasAssignee))}>
                {collab.busy ? <Spinner size="sm" color="current" /> : <Send />}{uncertain ? t("重试原请求") : t("确认协作")}
              </Button>
            </> : <>
              <Tooltip>
                <Button variant="ghost" isIconOnly aria-label={t("复制接入请求")}
                  isDisabled={blocked || preparing || denied || !selectedTerminals.length || (needsGrant && (!consent || !canGrant))}
                  onPress={() => void prepare(true)}><Copy /></Button>
                <Tooltip.Content>{t("复制接入请求")}</Tooltip.Content>
              </Tooltip>
              <Button isDisabled={blocked || preparing || denied || !selectedTerminals.length || (needsGrant && (!consent || !canGrant))}
                onPress={() => void prepare(false)}>{preparing ? <Spinner size="sm" color="current" /> : <Terminal />}{t("连接协作")}</Button>
            </>}
        </Modal.Footer>}
      </Modal.Dialog>
    </Modal.Container>
  </Modal.Backdrop>;
}
