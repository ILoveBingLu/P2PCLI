import { useEffect, useState } from "react";
import {
  Alert, Chip, Description, Disclosure, Drawer, EmptyState, Input, Label, ListBox, Select, Spinner, Tabs, TextArea, TextField,
  ToggleButton, ToggleButtonGroup,
} from "@heroui/react";
import { ArrowLeft, ListTodo, Network, Plus, RefreshCw } from "lucide-react";
import { api } from "../api";
import {
  collaborationAgo, collaborationErrorText, expectCollaboration, sessionKey, sessionRef, taskFinished, taskStateColor, taskStateLabels,
  type CollaborationAction, type CollaborationSession,
} from "../lib/collaboration";
import { useCollaboration } from "./CollaborationContext";
import { CollaborationTaskDetail } from "./CollaborationTaskDetail";
import { CollaborationDevices } from "./CollaborationDevices";
import { CollaborationDeliveries } from "./CollaborationDeliveries";
import { CollaborationLinkDialog } from "./CollaborationLinkDialog";
import { Button } from "./ui/button";
import { toast } from "./ui/toast";
import "../collaboration.css";
import "../collaboration-links.css";
import { t } from "@/i18n";

/** 能派任务的权限：本机还没有项目时（第一次用），派任务时会自动建一个、本机就是管理员 */
export function useCollaborationRole() {
  const collab = useCollaboration();
  const own = collab.members.find((member) => member.device_id === collab.overview?.endpoint_id && !member.revoked);
  const fresh = !collab.project && !collab.target;
  return {
    canDispatch: fresh || own?.role === "admin" || own?.role === "dispatcher",
    canAdmin: fresh || own?.role === "admin",
  };
}

/** 能接任务的 AI：设备是有执行权限的成员、AI 在线 */
export function assignableSessions(sessions: CollaborationSession[], members: { device_id: string; role: string; revoked: boolean }[]) {
  const workers = new Set(members.filter((member) => !member.revoked && member.role !== "viewer").map((member) => member.device_id));
  return sessions.filter((session) => workers.has(session.device_id) && session.available);
}

/** 标题栏“协作”打开的面板：任务、设备两页；不常用的（协调设备、项目、投递记录）收在最底下 */
export function CollaborationDrawer() {
  const collab = useCollaboration();
  const { canDispatch } = useCollaborationRole();
  const [error, setError] = useState("");
  /** 在填新任务（抽屉里换成表单，不另外弹窗） */
  const [creating, setCreating] = useState(false);
  const task = collab.tasks.find((task) => task.id === collab.selected);
  useEffect(() => { if (!collab.open || collab.selected) setCreating(false); }, [collab.open, collab.selected]);
  const back = task ? () => collab.selectTask(null) : creating ? () => setCreating(false) : null;
  useEffect(() => { setError(""); }, [collab.selected, collab.target, collab.project?.id]);

  /** 改任务状态（验收、重做、停止、换人）。协调设备离线时请求会先排队，说一声 */
  const mutate = async (action: CollaborationAction) => {
    if (!collab.project || collab.busy || collab.loading || collab.error) return false;
    setError("");
    try {
      const response = await collab.run(() => api.collabRequest(collab.target, collab.project!.id, action));
      if (response.type === "queued" || response.type === "committed_unverified") {
        toast.notice(t("已提交，等协调设备确认"), { description: t("协调设备暂时连不上，恢复连接后会自动同步，不用重复操作。") });
      } else {
        expectCollaboration(response, "task");
        toast.success(t("已保存"));
      }
      return true;
    } catch (e) {
      setError(collaborationErrorText(e));
      return false;
    }
  };

  return <>
    <Drawer.Backdrop variant="transparent" isDismissable isOpen={collab.open} onOpenChange={collab.setOpen}>
      <Drawer.Content placement="right">
        <Drawer.Dialog aria-label={t("协作")} className="collab-panel">
          <Drawer.Header className="collab-panel-header">
            {back ? <Button variant="ghost" size="icon-sm" aria-label={t("返回任务列表")} title={t("返回任务列表")} onPress={back}><ArrowLeft /></Button>
              : <Network className="size-4" aria-hidden="true" />}
            <Drawer.Heading>{task ? t("任务详情") : creating ? t("新建任务") : t("协作")}</Drawer.Heading>
            <Button variant="ghost" size="icon-sm" aria-label={t("刷新")} title={t("刷新")} disabled={collab.busy || collab.loading} onPress={collab.refresh}>
              <RefreshCw className={collab.loading ? "animate-spin" : undefined} />
            </Button>
            <Drawer.CloseTrigger aria-label={t("关闭")} />
          </Drawer.Header>
          <Drawer.Body className="collab-panel-body">
            {(collab.error || error) && <Alert status="danger" role="alert">
              <Alert.Indicator />
              <Alert.Content><Alert.Description>{collab.error || error}</Alert.Description></Alert.Content>
            </Alert>}
            {task ? <CollaborationTaskDetail task={task} mutate={mutate} /> : creating ? <CreateTaskForm onDone={() => setCreating(false)} /> : (
              <Tabs selectedKey={collab.view} onSelectionChange={(key) => collab.setView(key === "devices" ? "devices" : "tasks")}>
                <Tabs.ListContainer>
                  <Tabs.List aria-label={t("协作")}>
                    <Tabs.Tab id="tasks">{t("任务")}<Tabs.Indicator /></Tabs.Tab>
                    <Tabs.Tab id="devices">{t("设备")}<Tabs.Indicator /></Tabs.Tab>
                  </Tabs.List>
                </Tabs.ListContainer>
                <Tabs.Panel id="tasks"><TaskList canDispatch={canDispatch} onCreate={() => setCreating(true)} /></Tabs.Panel>
                <Tabs.Panel id="devices"><CollaborationDevices /></Tabs.Panel>
              </Tabs>
            )}
            {!task && !creating && <MoreSettings />}
          </Drawer.Body>
        </Drawer.Dialog>
      </Drawer.Content>
    </Drawer.Backdrop>
    <CollaborationLinkDialog />
  </>;
}

/** 任务列表：进行中的在前，点开看详情 */
function TaskList({ canDispatch, onCreate }: { canDispatch: boolean; onCreate: () => void }) {
  const collab = useCollaboration();
  const [filter, setFilter] = useState<"active" | "finished">("active");
  const tasks = collab.tasks
    .filter((task) => (filter === "finished") === taskFinished(task.state))
    .sort((a, b) => b.updated_at - a.updated_at);
  const sessionName = (ref: { device_id: string; session_id: string }) =>
    collab.sessions.find((s) => s.id === ref.session_id && s.device_id === ref.device_id)?.name ?? t("AI 会话");
  return <div className="collab-tasks">
    <div className="collab-tasks-toolbar">
      {canDispatch && <Button size="sm" disabled={collab.busy || collab.loading || !!collab.error} onPress={onCreate}><Plus />{t("新建任务")}</Button>}
      <ToggleButtonGroup aria-label={t("任务范围")} size="sm" selectionMode="single" disallowEmptySelection
        selectedKeys={[filter]} onSelectionChange={(keys) => { const key = [...keys][0]; if (key) setFilter(key === "finished" ? "finished" : "active"); }}>
        <ToggleButton id="active">{t("进行中")}</ToggleButton>
        <ToggleButton id="finished">{t("已完结")}</ToggleButton>
      </ToggleButtonGroup>
    </div>
    {collab.loading && !collab.tasks.length ? <EmptyState className="collab-empty"><Spinner size="sm" />{t("正在读取")}</EmptyState>
      : !tasks.length ? <EmptyState className="collab-empty">
        <ListTodo className="size-6" aria-hidden="true" />
        <p>{filter === "active" ? t("没有进行中的任务") : t("还没有完结的任务")}</p>
        {filter === "active" && <Description>{t("新建一个任务交给某台设备上的 AI，或者在左边设备栏把一个 AI 会话拖到另一个上。")}</Description>}
      </EmptyState>
      : <ListBox aria-label={t("任务")} selectionMode="none" className="collab-task-list" onAction={(key) => collab.selectTask(String(key))}>
        {tasks.map((task) => <ListBox.Item key={task.id} id={task.id} textValue={task.spec.title} className="collab-task-item">
          <div className="collab-task-item-text">
            <Label>{task.spec.title}</Label>
            <Description>{t("交给 {{device}} · {{session}}", { device: collab.deviceName(task.spec.assignee.device_id), session: sessionName(task.spec.assignee) })} · {collaborationAgo(task.updated_at)}</Description>
          </div>
          <Chip size="sm" variant="soft" color={taskStateColor(task.state)}>{taskStateLabels()[task.state]}</Chip>
        </ListBox.Item>)}
      </ListBox>}
  </div>;
}

/** 新建任务：要做什么、怎样算完成、交给哪个 AI。在抽屉里直接填，不另外弹窗（弹窗会被抽屉压住） */
function CreateTaskForm({ onDone }: { onDone: () => void }) {
  const collab = useCollaboration();
  const [title, setTitle] = useState("");
  const [goal, setGoal] = useState("");
  const [acceptance, setAcceptance] = useState("");
  const [assignee, setAssignee] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const sessions = assignableSessions(collab.sessions, collab.members);
  const selected = sessions.find((session) => sessionKey(sessionRef(session)) === assignee);
  const submit = async () => {
    if (busy || !selected || !title.trim() || !goal.trim()) return;
    setBusy(true); setError("");
    try {
      const response = await collab.run(async () => {
        const project = await collab.ensureProject();
        return api.collabRequest(collab.target, project.id, {
          type: "task_create",
          spec: { title: title.trim(), goal: goal.trim(), acceptance: acceptance.trim(), inputs: {}, assignee: sessionRef(selected) },
        });
      });
      if (response.type === "task") collab.selectTask(response.task.id);
      else toast.notice(t("已提交，等协调设备确认"), { description: t("协调设备暂时连不上，恢复连接后会自动同步，不用重复创建。") });
      onDone();
    } catch (e) {
      setError(collaborationErrorText(e));
    } finally {
      setBusy(false);
    }
  };
  return <form className="collab-form" onSubmit={(e) => { e.preventDefault(); void submit(); }}>
        <Description>{t("任务交给一个 AI 会话去做，做完回报结果，由你验收。")}</Description>
        <TextField isRequired value={title} onChange={setTitle} isDisabled={busy}>
          <Label>{t("任务名")}</Label>
          <Input autoFocus maxLength={200} placeholder={t("比如：修好登录页的报错")} />
        </TextField>
        <TextField isRequired value={goal} onChange={setGoal} isDisabled={busy}>
          <Label>{t("要做什么")}</Label>
          <TextArea rows={4} maxLength={16000} placeholder={t("写清楚要它做的事，越具体越好")} />
        </TextField>
        <TextField value={acceptance} onChange={setAcceptance} isDisabled={busy}>
          <Label>{t("怎样算完成（可不填）")}</Label>
          <TextArea rows={2} maxLength={4000} placeholder={t("比如：测试全部通过、页面不再报错")} />
        </TextField>
        <Select isRequired placeholder={t("选择一个 AI 会话")} selectedKey={assignee} isDisabled={busy || !sessions.length}
          onSelectionChange={(key) => setAssignee(key == null ? null : String(key))}>
          <Label>{t("交给谁")}</Label>
          <Select.Trigger><Select.Value /><Select.Indicator /></Select.Trigger>
          <Select.Popover><ListBox>
            {sessions.map((session) => <ListBox.Item key={sessionKey(sessionRef(session))} id={sessionKey(sessionRef(session))}
              textValue={`${collab.deviceName(session.device_id)} · ${session.name}`}>
              <Label>{session.name}</Label>
              <Description>{collab.deviceName(session.device_id)}</Description>
              <ListBox.ItemIndicator />
            </ListBox.Item>)}
          </ListBox></Select.Popover>
        </Select>
        {!sessions.length && <Alert status="warning">
          <Alert.Indicator />
          <Alert.Content><Alert.Description>{t("还没有能接任务的 AI。先在左边设备栏把一个 AI 会话拖到另一个上，接入协作。")}</Alert.Description></Alert.Content>
        </Alert>}
        {error && <Alert status="danger" role="alert"><Alert.Indicator /><Alert.Content><Alert.Description>{error}</Alert.Description></Alert.Content></Alert>}
        <div className="collab-form-actions">
          <Button type="button" variant="ghost" disabled={busy} onPress={onDone}>{t("取消")}</Button>
          <Button type="submit" disabled={busy || !selected || !title.trim() || !goal.trim()}>{busy && <Spinner size="sm" />}{t("交给它做")}</Button>
        </div>
      </form>;
}

/** 平时用不到的：用哪台设备记协作（协调设备）、哪个项目、本机待同步的请求 */
function MoreSettings() {
  const collab = useCollaboration();
  const devices = new Map([
    ...(collab.overview?.recent ?? []).map((device) => [device.endpoint_id, device.name] as const),
    ...(collab.overview?.nearby ?? []).map((device) => [device.endpoint_id, device.name] as const),
  ]);
  devices.delete(collab.overview?.endpoint_id ?? "");
  const locked = collab.busy || !!collab.link;
  return <Disclosure className="collab-more">
    <Disclosure.Heading>
      <Disclosure.Trigger className="collab-more-trigger">{t("更多设置")}<Disclosure.Indicator /></Disclosure.Trigger>
    </Disclosure.Heading>
    <Disclosure.Content>
      <Disclosure.Body className="collab-more-body">
        <Select selectedKey={collab.target ?? "local"} isDisabled={locked}
          onSelectionChange={(key) => collab.setTarget(key == null || key === "local" ? null : String(key))}>
          <Label>{t("协作记录存在哪台设备")}</Label>
          <Select.Trigger><Select.Value /><Select.Indicator /></Select.Trigger>
          <Select.Popover><ListBox>
            <ListBox.Item id="local" textValue={t("本机")}>{t("本机")}<ListBox.ItemIndicator /></ListBox.Item>
            {[...devices].map(([id, name]) => <ListBox.Item key={id} id={id} textValue={name}>{name}<ListBox.ItemIndicator /></ListBox.Item>)}
          </ListBox></Select.Popover>
          <Description>{t("几台设备一起协作时，选同一台来记任务；一般用本机就行。")}</Description>
        </Select>
        {collab.projects.length > 1 && <Select selectedKey={collab.project?.id ?? null} isDisabled={locked}
          onSelectionChange={(key) => { if (key != null) collab.selectProject(String(key)); }}>
          <Label>{t("项目")}</Label>
          <Select.Trigger><Select.Value /><Select.Indicator /></Select.Trigger>
          <Select.Popover><ListBox>
            {collab.projects.map((project) => <ListBox.Item key={project.id} id={project.id} textValue={project.name}>{project.name}<ListBox.ItemIndicator /></ListBox.Item>)}
          </ListBox></Select.Popover>
        </Select>}
        <CollaborationDeliveries />
      </Disclosure.Body>
    </Disclosure.Content>
  </Disclosure>;
}
