import { useCallback, useEffect, useRef, useState } from "react";
import { ListBox, Select, Tabs } from "@heroui/react";
import { Check, Copy, FolderKanban, ListTodo, LoaderCircle, Network, RefreshCw, Search, Terminal, Users, WifiOff } from "lucide-react";
import { api, type Overview } from "../api";
import { writeClipboardText } from "../lib/clipboard";
import {
  collaborationErrorText, collaborationTime, expectCollaboration, roleLabels, sessionKey, sessionRef, shortId, taskStateLabels,
  type CollaborationAction, type CollaborationMember, type CollaborationProject, type CollaborationRole, type CollaborationSession, type CollaborationTask, type TaskState,
} from "../lib/collaboration";
import { Button } from "./ui/button";
import { Input } from "./ui/input";
import { CollaborationAddMember, CollaborationCreateProject, CollaborationCreateTask } from "./CollaborationForms";
import { CollaborationTaskDetail } from "./CollaborationTaskDetail";
import { CollaborationDeliveries } from "./CollaborationDeliveries";
import { toast } from "./ui/toast";
import "../collaboration.css";

export function Collaboration({ overview }: { overview: Overview | null }) {
  const [target, setTarget] = useState<string | null>(null);
  const [remote, setRemote] = useState(false);
  const [draft, setDraft] = useState("");
  return <div className="collab-workspace">
    <header className="collab-header">
      <h1><Network />协作</h1>
      <form className="collab-coordinator" onSubmit={(e) => { e.preventDefault(); if (draft.trim()) setTarget(draft.trim()); }}>
        <Select aria-label="协调设备" selectedKey={remote ? "remote" : "local"} onSelectionChange={(key) => {
          const isRemote = key === "remote";
          setRemote(isRemote);
          if (!isRemote) setTarget(null);
        }}>
          <Select.Trigger className="collab-select-trigger"><Select.Value /><Select.Indicator /></Select.Trigger>
          <Select.Popover><ListBox>
            <ListBox.Item id="local">本机</ListBox.Item>
            <ListBox.Item id="remote">指定设备</ListBox.Item>
          </ListBox></Select.Popover>
        </Select>
        {remote && <>
          <Input aria-label="协调设备 ID 或名称" placeholder="设备 ID 或名称" value={draft} onChange={(e) => setDraft(e.target.value)} />
          <Button type="submit" size="sm" variant="secondary" disabled={!draft.trim() || target === draft.trim()}>连接</Button>
        </>}
      </form>
    </header>
    <div className="collab-target-line">当前协调设备：<strong title={target ?? overview?.endpoint_id}>{target ? shortId(target) : overview?.name ?? "本机"}</strong>{remote && target === null && <span>尚未连接指定设备</span>}</div>
    <CollaborationDeliveries />
    {remote && target === null
      ? <div className="collab-empty collab-project-placeholder"><Network />输入协调设备 ID 或名称</div>
      : <CollaborationProjects key={target ?? "local"} target={target} overview={overview} />}
  </div>;
}

function CollaborationProjects({ target, overview }: { target: string | null; overview: Overview | null }) {
  const [projects, setProjects] = useState<CollaborationProject[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const alive = useRef(true);
  const sequence = useRef(0);
  const refresh = useCallback(async () => {
    const request = ++sequence.current;
    setLoading(true);
    try {
      const result = expectCollaboration(await api.collabRequest(target, null, { type: "projects_list" }), "projects");
      if (!alive.current || request !== sequence.current) return;
      setProjects(result.projects);
      setSelected((current) => result.projects.some((p) => p.id === current) ? current : result.projects[0]?.id ?? null);
      setError("");
    } catch (e) { if (alive.current && request === sequence.current) setError(collaborationErrorText(e)); }
    finally { if (alive.current && request === sequence.current) setLoading(false); }
  }, [target]);
  useEffect(() => {
    alive.current = true;
    void refresh();
    return () => { alive.current = false; sequence.current++; };
  }, [refresh]);
  const project = projects.find((p) => p.id === selected);
  return <div className="collab-body">
    <aside className="collab-projects" aria-label="协作项目">
      <div className="collab-row collab-project-heading"><strong>项目 <span className="collab-note">{projects.length}</span></strong><div className="collab-row">
        <Button variant="ghost" size="icon-sm" title="刷新项目" aria-label="刷新项目" disabled={loading} onPress={() => void refresh()}><RefreshCw className={loading ? "animate-spin" : ""} /></Button>
        <CollaborationCreateProject disabled={target !== null || loading || !!error || !overview} create={async (name) => {
          try {
            const result = expectCollaboration(await api.collabRequest(target, null, { type: "project_create", name }), "project");
            if (!alive.current) return false;
            sequence.current++;
            setProjects((current) => [...current.filter((p) => p.id !== result.project.id), result.project]);
            setSelected(result.project.id); setError(""); return true;
          } catch (e) { if (alive.current) { setError(collaborationErrorText(e)); toast.error(collaborationErrorText(e)); } return false; }
        }} />
      </div></div>
      {target !== null && <p className="collab-note collab-remote-note">新项目需在协调设备本机创建。</p>}
      {error && <div className="collab-error" role="alert"><WifiOff /><span>{error}</span></div>}
      {loading && !projects.length && <div className="collab-empty"><LoaderCircle className="animate-spin" />加载项目</div>}
      {!loading && !error && !projects.length && <div className="collab-empty"><FolderKanban />暂无项目</div>}
      <nav>{projects.map((item) => <Button variant="ghost" className="collab-project" aria-current={item.id === selected ? "page" : undefined} key={item.id} onPress={() => setSelected(item.id)}>
        <FolderKanban /><span><strong>{item.name}</strong><small>{shortId(item.id)}</small></span>
      </Button>)}</nav>
    </aside>
    {project ? <CollaborationProjectView key={project.id} project={project} target={target} overview={overview} /> : <div className="collab-empty collab-project-placeholder"><FolderKanban /><span>{error ? "协调设备暂不可用" : loading ? "正在读取项目" : "创建项目，开始协作"}</span></div>}
  </div>;
}

function CollaborationProjectView({ project, target, overview }: { project: CollaborationProject; target: string | null; overview: Overview | null }) {
  const [snapshot, setSnapshot] = useState<{ tasks: CollaborationTask[]; sessions: CollaborationSession[]; members: CollaborationMember[] } | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState("");
  const [operationError, setOperationError] = useState("");
  const [queuedNotice, setQueuedNotice] = useState("");
  const [busy, setBusy] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<TaskState | "all">("all");
  const [view, setView] = useState("tasks");
  const [updated, setUpdated] = useState<number | null>(null);
  const alive = useRef(true);
  const sequence = useRef(0);
  const pending = useRef(false);
  const reading = useRef(false);
  const refresh = useCallback(async () => {
    if (reading.current) return;
    reading.current = true;
    const request = ++sequence.current;
    setLoading(true);
    try {
      const [tasks, sessions, members] = await Promise.all([
        api.collabRequest(target, project.id, { type: "tasks_list" }),
        api.collabRequest(target, project.id, { type: "sessions_list" }),
        api.collabRequest(target, project.id, { type: "members_list" }),
      ]);
      const next = {
        tasks: expectCollaboration(tasks, "tasks").tasks,
        sessions: expectCollaboration(sessions, "sessions").sessions,
        members: expectCollaboration(members, "members").members,
      };
      if (!alive.current || request !== sequence.current) return;
      setSnapshot(next); setLoadError(""); setUpdated(Date.now());
      setSelected((current) => current && next.tasks.some((task) => task.id === current) ? current : null);
    } catch (e) { if (alive.current && request === sequence.current) setLoadError(collaborationErrorText(e)); }
    finally {
      if (request === sequence.current) {
        reading.current = false;
        if (alive.current) setLoading(false);
      }
    }
  }, [target, project.id]);
  useEffect(() => {
    alive.current = true; reading.current = false; void refresh();
    const timer = window.setInterval(() => { if (!pending.current) void refresh(); }, 5000);
    return () => { alive.current = false; sequence.current++; window.clearInterval(timer); };
  }, [refresh]);
  const mutate = async (action: CollaborationAction) => {
    if (pending.current || reading.current || loadError || !snapshot) return false;
    pending.current = true; setBusy(true); setOperationError(""); setQueuedNotice("");
    try {
      const result = await api.collabRequest(target, project.id, action);
      if (!alive.current) return false;
      if (result.type === "queued") {
        setQueuedNotice(`请求已进入本机待发队列，请查看投递记录确认同步结果。${result.message} · ${shortId(result.request_id)}`);
        toast.notice("待同步", { description: "请求已进入本机待发队列，协调设备尚未确认。" });
        return true;
      }
      if (result.type === "committed_unverified") {
        setQueuedNotice(`已提交但尚未重新核实。恢复连接后请刷新核对，勿重复创建请求。${result.message} · ${shortId(result.request_id)}`);
        toast.notice("提交记录待核实", { description: "本机保留了提交记录，当前权限和远端状态尚未重新确认。" });
        return true;
      }
      expectCollaboration(result, action.type === "member_set" ? "ack" : "task");
      if (result.type === "task") {
        setSnapshot((current) => current && { ...current, tasks: [...current.tasks.filter((task) => task.id !== result.task.id), result.task] });
        setSelected(result.task.id);
      }
      toast.success("已保存");
      await refresh();
      return true;
    } catch (e) {
      if (alive.current) { setOperationError(collaborationErrorText(e)); await refresh(); }
      return false;
    } finally { pending.current = false; if (alive.current) setBusy(false); }
  };
  const ownMember = snapshot?.members.find((member) => member.device_id === overview?.endpoint_id && !member.revoked);
  const canDispatch = ownMember?.role === "admin" || ownMember?.role === "dispatcher";
  const canAdmin = ownMember?.role === "admin";
  const disabled = busy || loading || !!loadError || !snapshot;
  const tasks = snapshot?.tasks ?? [];
  const sessions = snapshot?.sessions ?? [];
  const members = snapshot?.members ?? [];
  const task = tasks.find((item) => item.id === selected);
  const filtered = tasks.filter((item) => (filter === "all" || item.state === filter) && `${item.spec.title} ${item.spec.goal} ${item.id}`.toLowerCase().includes(query.toLowerCase())).sort((a, b) => b.updated_at - a.updated_at || a.id.localeCompare(b.id));
  const activeMembers = new Set(members.filter((member) => !member.revoked && member.role !== "viewer").map((member) => member.device_id));
  const assignableSessions = sessions.filter((session) => activeMembers.has(session.device_id));
  return <section className="collab-project-main">
    <header className="collab-project-title"><div><h2>{project.name}</h2><p className="collab-note" title={project.coordinator_id}>协调设备 {shortId(project.coordinator_id)} · {ownMember ? roleLabels[ownMember.role] : "权限待确认"}</p></div><div className="collab-row">
      <span className="collab-sync" role="status">{busy ? "正在保存" : loadError ? "数据未同步" : loading ? "正在同步" : updated ? `更新于 ${new Date(updated).toLocaleTimeString("zh-CN")}` : ""}</span>
      <Button variant="ghost" size="icon-sm" title="刷新协作数据" aria-label="刷新协作数据" disabled={loading || busy} onPress={() => void refresh()}><RefreshCw className={loading ? "animate-spin" : ""} /></Button>
    </div></header>
    {(loadError || operationError) && <div className="collab-error" role="alert"><WifiOff /><div>{loadError || operationError}{loadError && snapshot && <p>当前显示上次成功读取的数据，写操作已暂停。</p>}</div>{operationError && !loadError && <Button variant="ghost" size="icon-sm" title="关闭提示" aria-label="关闭提示" onPress={() => setOperationError("")}><Check /></Button>}</div>}
    {queuedNotice && <div className="collab-notice collab-queued-notice" role="status">{queuedNotice}<Button variant="ghost" size="icon-sm" title="关闭提交状态提示" aria-label="关闭提交状态提示" onPress={() => setQueuedNotice("")}><Check /></Button></div>}
    <Tabs selectedKey={view} onSelectionChange={(key) => setView(String(key))} className="collab-tabs">
      <Tabs.ListContainer><Tabs.List aria-label="协作视图">
        <Tabs.Tab id="tasks"><ListTodo />任务 <span>{tasks.length}</span><Tabs.Indicator /></Tabs.Tab>
        <Tabs.Tab id="sessions"><Terminal />会话 <span>{sessions.length}</span><Tabs.Indicator /></Tabs.Tab>
        <Tabs.Tab id="members"><Users />成员 <span>{members.filter((member) => !member.revoked).length}</span><Tabs.Indicator /></Tabs.Tab>
      </Tabs.List></Tabs.ListContainer>
      <Tabs.Panel id="tasks" className="collab-tab-panel">
        <div className="collab-task-layout" data-detail={!!task}>
          <div className="collab-tasks">
            <div className="collab-task-toolbar">
              <div className="collab-search"><Search /><Input aria-label="搜索任务" placeholder="搜索任务" value={query} onChange={(e) => setQuery(e.target.value)} /></div>
              <Select aria-label="任务状态筛选" selectedKey={filter} onSelectionChange={(key) => key && setFilter(String(key) as typeof filter)}>
                <Select.Trigger className="collab-select-trigger"><Select.Value /><Select.Indicator /></Select.Trigger>
                <Select.Popover><ListBox>
                  <ListBox.Item id="all">全部状态</ListBox.Item>
                  {Object.entries(taskStateLabels).map(([value, label]) => <ListBox.Item key={value} id={value}>{label}</ListBox.Item>)}
                </ListBox></Select.Popover>
              </Select>
              {canDispatch && <CollaborationCreateTask sessions={assignableSessions} disabled={disabled} mutate={mutate} />}
            </div>
            <div className="collab-task-list">
              {loading && !snapshot ? <div className="collab-empty"><LoaderCircle className="animate-spin" />加载任务</div> : !filtered.length ? <div className="collab-empty"><ListTodo />{loadError ? "任务列表不可用" : tasks.length ? "没有匹配的任务" : "暂无任务"}</div> : filtered.map((item) => {
                const session = sessions.find((s) => s.id === item.spec.assignee.session_id && s.device_id === item.spec.assignee.device_id);
                return <Button variant="ghost" className="collab-task-row" key={item.id} aria-pressed={selected === item.id} onPress={() => setSelected(item.id)}>
                  <span className="collab-state" data-state={item.state}>{taskStateLabels[item.state]}</span>
                  <strong>{item.spec.title}</strong>
                  <span className="collab-task-owner">{session?.name ?? shortId(item.spec.assignee.session_id)} · {shortId(item.spec.assignee.device_id)}</span>
                  <span className="collab-note">{collaborationTime(item.updated_at)}</span>
                </Button>;
              })}
            </div>
          </div>
          {task && <CollaborationTaskDetail key={task.id} task={task} sessions={assignableSessions} target={target} canDispatch={canDispatch} disabled={disabled} mutate={mutate} onBack={() => setSelected(null)} />}
        </div>
      </Tabs.Panel>
      <Tabs.Panel id="sessions" className="collab-tab-panel collab-scroll">
        <div className="collab-section-heading"><h3>协作会话</h3><Button size="sm" variant="secondary" onPress={() => {
          const command = `p2pcli collab session register --help`;
          void writeClipboardText(command).then(() => toast.success("已复制 CLI 登记入口")).catch(() => toast.error(command));
        }}><Copy />CLI 登记入口</Button></div>
        {!sessions.length && <div className="collab-empty"><Terminal />{loading ? "加载会话" : "暂无已登记会话"}</div>}
        {sessions.map((session) => {
          const stale = Date.now() / 1000 - session.last_seen > 120;
          return <article key={sessionKey(sessionRef(session))} className="collab-session-row">
            <Terminal /><div><strong>{session.name}</strong><p className="collab-note">{shortId(session.device_id)} · {session.mode === "manual_pull" ? "手动领取" : "托管适配器"}</p><p className="collab-note">{session.cwd ?? "未指定目录"}</p><code className="collab-note">{session.id}</code></div>
            <div className="collab-session-meta"><span className="collab-presence" data-live={session.available && !stale}>{!activeMembers.has(session.device_id) ? "成员不可执行" : stale ? "心跳过期" : session.available ? "可用" : "不可用"}</span><span className="collab-note">{collaborationTime(session.last_seen)}</span></div>
          </article>;
        })}
        <p className="collab-note collab-footnote">项目 ID <code>{project.id}</code></p>
      </Tabs.Panel>
      <Tabs.Panel id="members" className="collab-tab-panel collab-scroll">
        <div className="collab-section-heading"><h3>项目成员</h3><span className="collab-note">{canAdmin ? "管理员可调整角色与访问" : "仅管理员可管理成员"}</span></div>
        {canAdmin && <CollaborationAddMember mutate={mutate} disabled={disabled} />}
        {members.map((member) => <CollaborationMemberRow key={member.device_id} member={member} coordinator={project.coordinator_id} self={overview?.endpoint_id} canAdmin={canAdmin} disabled={disabled} mutate={mutate} />)}
        {!members.length && <div className="collab-empty"><Users />{loading ? "加载成员" : "暂无成员数据"}</div>}
      </Tabs.Panel>
    </Tabs>
  </section>;
}

function CollaborationMemberRow({ member, coordinator, self, canAdmin, disabled, mutate }: {
  member: CollaborationMember; coordinator: string; self?: string; canAdmin: boolean; disabled: boolean;
  mutate: (action: CollaborationAction) => Promise<boolean>;
}) {
  const [role, setRole] = useState(member.role);
  useEffect(() => { setRole(member.role); }, [member.role]);
  const protectedMember = member.device_id === coordinator || member.device_id === self;
  return <div className="collab-member-row" data-revoked={member.revoked}>
    <Users /><div className="collab-member-identity"><strong>{member.device_id === self ? "本机" : member.device_id === coordinator ? "协调设备" : shortId(member.device_id)}</strong><code>{member.device_id}</code></div>
    {canAdmin && !protectedMember ? <div className="collab-member-actions">
      <Select aria-label={`成员 ${member.device_id} 角色`} selectedKey={role} isDisabled={disabled || member.revoked} onSelectionChange={(key) => { if (key != null) setRole(String(key) as CollaborationRole); }}>
        <Select.Trigger className="collab-select-trigger"><Select.Value /><Select.Indicator /></Select.Trigger>
        <Select.Popover><ListBox>{Object.entries(roleLabels).map(([value, label]) => <ListBox.Item key={value} id={value}>{label}</ListBox.Item>)}</ListBox></Select.Popover>
      </Select>
      {role !== member.role && <Button size="icon-sm" variant="secondary" title="保存角色" aria-label="保存角色" disabled={disabled} onPress={() => void mutate({ type: "member_set", device_id: member.device_id, role, revoked: false })}><Check /></Button>}
      <Button size="sm" variant={member.revoked ? "secondary" : "destructive"} disabled={disabled} onPress={() => void mutate({ type: "member_set", device_id: member.device_id, role: member.role, revoked: !member.revoked })}>{member.revoked ? "恢复访问" : "撤销访问"}</Button>
      {member.revoked && <span className="collab-error-text">已撤销</span>}
    </div> : <span className="collab-note">{roleLabels[member.role]}{member.revoked && " · 已撤销"}</span>}
  </div>;
}
