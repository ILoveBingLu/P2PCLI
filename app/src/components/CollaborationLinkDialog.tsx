import { useId, useRef, useState } from "react";
import {
  Accordion, Alert, Button, Checkbox, Description, Fieldset, Form, Label,
  ListBox, Modal, Select, Spinner, TextArea, TextField, Tooltip,
} from "@heroui/react";
import { Copy, Monitor, Send, Terminal } from "lucide-react";
import { api } from "../api";
import {
  CollaborationError, collaborationErrorText, expectCollaboration, sessionRef,
  type TaskSpec,
} from "../lib/collaboration";
import {
  endpointKey, linkInputs, matchingSessions, registrationPrompt, sessionReady,
  type CollaborationEndpoint,
} from "../lib/collaborationLinks";
import { useCollaboration } from "./CollaborationContext";
import { AgentIcon } from "./Agent";
import { PlatformIcon } from "./PlatformIcon";
import { toast } from "./ui/toast";

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
  const agentIcon = (deviceId: string, terminalId?: string) => {
    const kind = collab.terminals.find((item) => item.deviceId === deviceId && item.terminalId === terminalId)?.agentKind;
    return kind ? <AgentIcon kind={kind} className="size-4" /> : <Terminal size={16} aria-hidden="true" />;
  };
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
  return <Modal.Backdrop isOpen isDismissable={!collab.busy && !preparing}
    isKeyboardDismissDisabled={collab.busy || preparing}
    onOpenChange={(open) => { if (!open && !collab.busy && !preparing) collab.setLink(null); }}>
    <Modal.Container size="lg" scroll="inside">
      <Modal.Dialog aria-describedby={descriptionId}>
        <Modal.CloseTrigger aria-label="关闭" isDisabled={collab.busy || preparing} />
        <Modal.Header>
          <Modal.Heading>连接协作</Modal.Heading>
          <Description id={descriptionId}>{endpointContent(link.source)} ↔ {endpointContent(link.destination)}</Description>
        </Modal.Header>
        <Modal.Body>
          {collab.loading ? <Alert role="status">
            <Alert.Indicator><Spinner size="sm" /></Alert.Indicator>
            <Alert.Content><Alert.Title>读取协作会话</Alert.Title></Alert.Content>
          </Alert> : collab.error || noAccess ? <Alert status="danger" role="alert">
            <Alert.Indicator />
            <Alert.Content><Alert.Description>{collab.error || "当前设备没有分配任务的权限。"}</Alert.Description></Alert.Content>
          </Alert> : submitted ? <Alert status="warning" role="status">
            <Alert.Indicator />
            <Alert.Content><Alert.Description>{notice}</Alert.Description></Alert.Content>
          </Alert> : ready.length > 0 ? <Form id={formId} onSubmit={(event) => { event.preventDefault(); void submit(); }}>
            <Fieldset.Group>
              {ready.length > 1 && <Select placeholder="选择会话" selectedKey={sessionId || null}
                isDisabled={blocked || frozen} onSelectionChange={(key) => setSessionId(key == null ? "" : String(key))}>
                <Label>执行会话</Label>
                <Select.Trigger><Select.Value /><Select.Indicator /></Select.Trigger>
                <Select.Popover><ListBox>
                  {ready.map((session) => <ListBox.Item key={session.id} id={session.id} textValue={session.name}>
                    {agentIcon(session.device_id, session.terminal?.terminal_id)}{" "}
                    <Label>{session.name}</Label><ListBox.ItemIndicator />
                  </ListBox.Item>)}
                </ListBox></Select.Popover>
              </Select>}
              {selected && <Alert status="success">
                <Alert.Indicator>{agentIcon(selected.device_id, selected.terminal?.terminal_id)}</Alert.Indicator>
                <Alert.Content>
                  <Alert.Title>{selected.name}</Alert.Title>
                  <Alert.Description>{selected.mode === "manual_pull" ? "等待会话主动领取" : "托管执行"}</Alert.Description>
                </Alert.Content>
              </Alert>}
              <TextField value={goal} onChange={setGoal} isDisabled={blocked || frozen}>
                <Label>让它做什么</Label>
                <TextArea autoFocus rows={4} maxLength={16000} />
              </TextField>
              <Accordion>
                <Accordion.Item>
                  <Accordion.Heading><Accordion.Trigger>更多选项<Accordion.Indicator /></Accordion.Trigger></Accordion.Heading>
                  <Accordion.Panel><Accordion.Body>
                    <TextField value={acceptance} onChange={setAcceptance} isDisabled={blocked || frozen}>
                      <Label>验收条件</Label>
                      <TextArea rows={2} />
                      <Description>{collab.project?.name} · 记录保存在{collab.deviceName(collab.target ?? owner)}</Description>
                    </TextField>
                  </Accordion.Body></Accordion.Panel>
                </Accordion.Item>
              </Accordion>
              {error && <Alert status="danger" role="alert">
                <Alert.Indicator /><Alert.Content><Alert.Description>{error}</Alert.Description></Alert.Content>
              </Alert>}
              {uncertain && <Alert status="warning" role="status">
                <Alert.Indicator />
                <Alert.Content><Alert.Description>提交结果未确认。重试将沿用原请求，不会再次创建任务。</Alert.Description></Alert.Content>
              </Alert>}
            </Fieldset.Group>
          </Form> : <Fieldset.Group>
            <Alert status={denied ? "warning" : "default"}>
              <Alert.Indicator><Terminal /></Alert.Indicator>
              <Alert.Content><Alert.Title>{denied ? "此设备的协作访问受限" : matches.length ? "会话已失联或不可用" : "待接入"}</Alert.Title></Alert.Content>
            </Alert>
            {!link.destination.terminalId && terminals.length > 0 && <Select placeholder="选择 AI 会话"
              selectedKey={terminal?.terminalId ?? null} isDisabled={blocked || preparing}
              onSelectionChange={(key) => setTerminalId(key == null ? "" : String(key))}>
              <Label>接入会话</Label>
              <Select.Trigger><Select.Value /><Select.Indicator /></Select.Trigger>
              <Select.Popover><ListBox>
                {terminals.map((terminal) => <ListBox.Item key={terminal.terminalId} id={terminal.terminalId} textValue={terminal.sessionName}>
                  {agentIcon(terminal.deviceId, terminal.terminalId)}{" "}
                  <Label>{terminal.sessionName}</Label><ListBox.ItemIndicator />
                </ListBox.Item>)}
              </ListBox></Select.Popover>
            </Select>}
            {!terminals.length && !link.destination.terminalId && <Description>此设备暂无可接入的 AI 会话。</Description>}
            {!terminal && link.destination.terminalId && <Description>此终端未检测到可接入的 AI 会话。</Description>}
            {needsGrant && <Checkbox isSelected={consent} isDisabled={!canGrant || blocked || preparing} onChange={setConsent}>
              <Checkbox.Content>
                <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
                允许{destination.deviceName}作为执行者加入{collab.project?.name ?? "设备协作"}
              </Checkbox.Content>
              <Description>读取分配给它的任务并回报结果。</Description>
            </Checkbox>}
            {denied && <Button variant="secondary" size="sm" onPress={() => { collab.setLink(null); collab.setAdvanced(true); }}>查看权限</Button>}
            {prompt && <Accordion>
              <Accordion.Item>
                <Accordion.Heading><Accordion.Trigger>接入请求<Accordion.Indicator /></Accordion.Trigger></Accordion.Heading>
                <Accordion.Panel><Accordion.Body>
                  <TextField aria-label="接入请求" value={prompt} isReadOnly><TextArea rows={6} /></TextField>
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
            <Button variant="secondary" onPress={() => { collab.setLink(null); collab.setOpen(true); }}>查看协作</Button> :
            ready.length > 0 ? <>
              <Button variant="ghost" onPress={() => collab.setLink(null)} isDisabled={collab.busy}>取消</Button>
              <Button type="submit" form={formId} isDisabled={blocked || (!frozen && (!goal.trim() || !selected))}>
                {collab.busy ? <Spinner size="sm" color="current" /> : <Send />}{uncertain ? "重试原请求" : "确认协作"}
              </Button>
            </> : <>
              <Tooltip>
                <Button variant="ghost" isIconOnly aria-label="复制接入请求"
                  isDisabled={blocked || preparing || denied || !terminal || (needsGrant && (!consent || !canGrant))}
                  onPress={() => void prepare(true)}><Copy /></Button>
                <Tooltip.Content>复制接入请求</Tooltip.Content>
              </Tooltip>
              <Button isDisabled={blocked || preparing || denied || !terminal || (needsGrant && (!consent || !canGrant))}
                onPress={() => void prepare(false)}>{preparing ? <Spinner size="sm" color="current" /> : <Terminal />}准备接入请求</Button>
            </>}
        </Modal.Footer>}
      </Modal.Dialog>
    </Modal.Container>
  </Modal.Backdrop>;
}
