import { useState, type ReactNode } from "react";
import { LoaderCircle, Plus, UserPlus } from "lucide-react";
import { ListBox, Select, TextArea } from "@heroui/react";
import { Button } from "./ui/button";
import { Input } from "./ui/input";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "./ui/dialog";
import {
  roleLabels, sessionKey, sessionRef, shortId,
  type CollaborationAction, type CollaborationRole, type CollaborationSession,
} from "../lib/collaboration";

export type CollaborationMutation = (action: CollaborationAction) => Promise<boolean>;
export function CollaborationField({ label, children }: { label: string; children: ReactNode }) {
  return <label className="collab-field"><span>{label}</span>{children}</label>;
}

export function CollaborationCreateProject({ create, disabled }: { create: (name: string) => Promise<boolean>; disabled: boolean }) {
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  return <>
    <Button size="icon-sm" variant="ghost" title="创建项目" aria-label="创建项目" disabled={disabled} onPress={() => setOpen(true)}><Plus /></Button>
    <Dialog open={open} onOpenChange={(value) => { if (!busy) setOpen(value); }}>
      <DialogContent className="collab-dialog">
        <DialogHeader><DialogTitle>创建协作项目</DialogTitle><DialogDescription>项目保存在当前协调设备。</DialogDescription></DialogHeader>
        <form onSubmit={async (e) => {
          e.preventDefault(); if (busy || !name.trim()) return;
          setBusy(true);
          try { if (await create(name.trim())) { setOpen(false); setName(""); } } finally { setBusy(false); }
        }}>
          <CollaborationField label="项目名称"><Input autoFocus required maxLength={160} value={name} onChange={(e) => setName(e.target.value)} /></CollaborationField>
          <DialogFooter><Button type="button" variant="ghost" disabled={busy} onPress={() => setOpen(false)}>取消</Button><Button type="submit" disabled={busy || !name.trim()}>{busy && <LoaderCircle className="animate-spin" />}创建项目</Button></DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  </>;
}

export function CollaborationCreateTask({ sessions, mutate, disabled }: { sessions: CollaborationSession[]; mutate: CollaborationMutation; disabled: boolean }) {
  const [open, setOpen] = useState(false);
  const [title, setTitle] = useState("");
  const [goal, setGoal] = useState("");
  const [acceptance, setAcceptance] = useState("");
  const [inputs, setInputs] = useState("{}");
  const [assignee, setAssignee] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  return <>
    <Button size="sm" disabled={disabled} onPress={() => { setError(""); setOpen(true); }}><Plus />创建任务</Button>
    <Dialog open={open} onOpenChange={(value) => { if (!busy) setOpen(value); }}>
      <DialogContent className="collab-dialog">
        <DialogHeader><DialogTitle>创建任务</DialogTitle><DialogDescription>指定已登记的协作会话。</DialogDescription></DialogHeader>
        <form onSubmit={async (e) => {
          e.preventDefault();
          if (busy) return;
          const selected = sessions.find((session) => sessionKey(sessionRef(session)) === assignee);
          if (!selected?.available) { setError("请选择可用的执行会话"); return; }
          let parsed: unknown;
          try { parsed = JSON.parse(inputs); } catch { setError("输入资料必须是有效 JSON"); return; }
          setBusy(true); setError("");
          try {
            if (await mutate({ type: "task_create", spec: { title: title.trim(), goal: goal.trim(), acceptance: acceptance.trim(), inputs: parsed, assignee: sessionRef(selected) } })) {
              setOpen(false); setTitle(""); setGoal(""); setAcceptance(""); setInputs("{}");
            } else setError("创建未确认，请关闭弹窗查看错误并刷新任务列表。");
          } finally { setBusy(false); }
        }}>
          <CollaborationField label="标题"><Input autoFocus required maxLength={200} value={title} onChange={(e) => setTitle(e.target.value)} /></CollaborationField>
          <CollaborationField label="执行会话">
            <Select
              aria-label="执行会话"
              placeholder="选择会话"
              isRequired
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
          {!sessions.length && <p className="collab-note">暂无协作会话。在会话页查看 CLI 登记入口。</p>}
          <CollaborationField label="任务目标"><TextArea required rows={3} value={goal} onChange={(e) => setGoal(e.target.value)} /></CollaborationField>
          <CollaborationField label="验收条件"><TextArea rows={2} value={acceptance} onChange={(e) => setAcceptance(e.target.value)} /></CollaborationField>
          <CollaborationField label="输入资料 (JSON)"><TextArea className="collab-code" rows={3} spellCheck={false} value={inputs} onChange={(e) => setInputs(e.target.value)} /></CollaborationField>
          {error && <p role="alert" className="collab-error-text">{error}</p>}
          <DialogFooter><Button type="button" variant="ghost" disabled={busy} onPress={() => setOpen(false)}>取消</Button><Button type="submit" disabled={disabled || busy || !title.trim() || !goal.trim() || !assignee}>{busy && <LoaderCircle className="animate-spin" />}创建任务</Button></DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  </>;
}

export function CollaborationAddMember({ mutate, disabled }: { mutate: CollaborationMutation; disabled: boolean }) {
  const [device, setDevice] = useState("");
  const [role, setRole] = useState<CollaborationRole>("worker");
  return <form className="collab-member-form" onSubmit={async (e) => {
    e.preventDefault();
    if (disabled || !device.trim()) return;
    if (await mutate({ type: "member_set", device_id: device.trim(), role, revoked: false })) setDevice("");
  }}>
    <CollaborationField label="设备 ID"><Input required placeholder="完整设备公钥" value={device} onChange={(e) => setDevice(e.target.value)} /></CollaborationField>
    <CollaborationField label="角色">
      <Select
        aria-label="角色"
        selectedKey={role}
        onSelectionChange={(key) => { if (key != null) setRole(String(key) as CollaborationRole); }}
      >
        <Select.Trigger><Select.Value /><Select.Indicator /></Select.Trigger>
        <Select.Popover>
          <ListBox>
            {Object.entries(roleLabels).map(([value, label]) => (
              <ListBox.Item key={value} id={value} textValue={label}>{label}</ListBox.Item>
            ))}
          </ListBox>
        </Select.Popover>
      </Select>
    </CollaborationField>
    <Button type="submit" size="sm" disabled={disabled || !device.trim()}><UserPlus />添加成员</Button>
  </form>;
}
