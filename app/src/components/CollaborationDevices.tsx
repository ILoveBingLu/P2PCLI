import { useState } from "react";
import { Alert, Chip, Description, Label, ListBox, Select } from "@heroui/react";
import { Monitor, Terminal } from "lucide-react";
import { api } from "../api";
import {
  collaborationAgo, collaborationErrorText, expectCollaboration, roleLabels,
  type CollaborationMember, type CollaborationRole,
} from "../lib/collaboration";
import { useCollaboration } from "./CollaborationContext";
import { useCollaborationRole } from "./CollaborationDrawer";
import { Button } from "./ui/button";
import { toast } from "./ui/toast";
import { t } from "@/i18n";

/** 会话超过这么久没报到就算离线 */
const STALE_SECONDS = 120;

function roleHints(): Record<CollaborationRole, string> {
  return {
    admin: t("能派任务、管设备"),
    dispatcher: t("能派任务"),
    worker: t("能接任务"),
    viewer: t("只能看"),
  };
}

/** 设备页：哪些设备在协作里、各能做什么、上面接入了哪些 AI。管理员能改权限、移出、加设备 */
export function CollaborationDevices() {
  const collab = useCollaboration();
  const { canAdmin } = useCollaborationRole();
  const [adding, setAdding] = useState<string | null>(null);
  const [error, setError] = useState("");
  const self = collab.overview?.endpoint_id;
  const coordinator = collab.project?.coordinator_id;
  const disabled = collab.busy || collab.loading || !!collab.error;

  // 成员和接入了 AI 的设备合在一起列：本机在最前
  const ids = [...new Set([...collab.members.map((m) => m.device_id), ...collab.sessions.map((s) => s.device_id)])]
    .sort((a, b) => Number(b === self) - Number(a === self) || collab.deviceName(a).localeCompare(collab.deviceName(b)));
  const known = new Map([
    ...(collab.overview?.recent ?? []).map((d) => [d.endpoint_id, d.name] as const),
    ...(collab.overview?.nearby ?? []).map((d) => [d.endpoint_id, d.name] as const),
  ]);
  const addable = [...known].filter(([id]) => id !== self && !collab.members.some((m) => m.device_id === id));

  const setMember = async (deviceId: string, role: CollaborationRole, revoked: boolean) => {
    setError("");
    try {
      await collab.run(async () => {
        const project = await collab.ensureProject();
        const response = await api.collabRequest(collab.target, project.id, { type: "member_set", device_id: deviceId, role, revoked });
        if (response.type === "queued" || response.type === "committed_unverified") {
          toast.notice(t("已提交，等协调设备确认"));
        } else {
          expectCollaboration(response, "ack");
        }
      });
      return true;
    } catch (e) {
      setError(collaborationErrorText(e));
      return false;
    }
  };

  return <div className="collab-devices">
    {error && <Alert status="danger" role="alert"><Alert.Indicator /><Alert.Content><Alert.Description>{error}</Alert.Description></Alert.Content></Alert>}
    {!ids.length && !collab.loading && <Description>{t("还没有设备加入协作。")}</Description>}
    {ids.map((id) => {
      const member = collab.members.find((m) => m.device_id === id);
      const sessions = collab.sessions.filter((s) => s.device_id === id);
      const editable = canAdmin && !!member && id !== self && id !== coordinator;
      return <section key={id} className="collab-device" data-revoked={member?.revoked || undefined}>
        <div className="collab-device-head">
          <Monitor className="size-4" aria-hidden="true" />
          <Label className="collab-device-name">{id === self ? t("{{name}}（本机）", { name: collab.deviceName(id) }) : collab.deviceName(id)}</Label>
          {member?.revoked ? <Chip size="sm" variant="soft" color="danger">{t("已移出")}</Chip>
            : !editable ? <Chip size="sm" variant="soft">{member ? roleLabels()[member.role] : t("未加入")}</Chip>
            : <RoleSelect member={member} disabled={disabled} onChange={(role) => void setMember(id, role, false)} />}
        </div>
        {sessions.map((session) => {
          const live = session.available && Date.now() / 1000 - session.last_seen < STALE_SECONDS;
          return <div key={session.id} className="collab-device-session">
            <Terminal className="size-3.5" aria-hidden="true" />
            <span className="collab-device-session-name">{session.name}</span>
            <Chip size="sm" variant="soft" color={live ? "success" : "default"}>{live ? t("在线") : t("离线")}</Chip>
            <Description>{collaborationAgo(session.last_seen)}</Description>
          </div>;
        })}
        {editable && <div className="collab-device-actions">
          <Button size="sm" variant={member.revoked ? "secondary" : "ghost"} disabled={disabled}
            onPress={() => void setMember(id, member.role, !member.revoked)}>{member.revoked ? t("恢复") : t("移出协作")}</Button>
        </div>}
      </section>;
    })}

    {canAdmin && addable.length > 0 && <div className="collab-device-add">
      <Select placeholder={t("选一台设备")} selectedKey={adding} isDisabled={disabled} onSelectionChange={(key) => setAdding(key == null ? null : String(key))}>
        <Label>{t("加一台设备进来")}</Label>
        <Select.Trigger><Select.Value /><Select.Indicator /></Select.Trigger>
        <Select.Popover><ListBox>
          {addable.map(([id, name]) => <ListBox.Item key={id} id={id} textValue={name}>{name}<ListBox.ItemIndicator /></ListBox.Item>)}
        </ListBox></Select.Popover>
        <Description>{t("加进来的设备上的 AI 能接任务。")}</Description>
      </Select>
      <Button size="sm" variant="secondary" disabled={disabled || !adding}
        onPress={() => { if (adding) void setMember(adding, "worker", false).then((ok) => ok && setAdding(null)); }}>{t("加入")}</Button>
    </div>}

    <Alert className="collab-tip">
      <Alert.Indicator />
      <Alert.Content>
        <Alert.Title>{t("怎么让 AI 接入协作")}</Alert.Title>
        <Alert.Description>{t("在左边设备栏，把一个 AI 会话右边的小圆点拖到另一个会话上。按提示确认后，它们就能互相派任务了。")}</Alert.Description>
      </Alert.Content>
    </Alert>
  </div>;
}

function RoleSelect({ member, disabled, onChange }: { member: CollaborationMember; disabled: boolean; onChange: (role: CollaborationRole) => void }) {
  const hints = roleHints();
  return <Select aria-label={t("权限")} selectedKey={member.role} isDisabled={disabled} className="collab-role"
    onSelectionChange={(key) => { if (key != null && key !== member.role) onChange(String(key) as CollaborationRole); }}>
    <Select.Trigger><Select.Value /><Select.Indicator /></Select.Trigger>
    <Select.Popover><ListBox>
      {Object.entries(roleLabels()).map(([role, label]) => <ListBox.Item key={role} id={role} textValue={label}>
        <Label>{label}</Label><Description>{hints[role as CollaborationRole]}</Description><ListBox.ItemIndicator />
      </ListBox.Item>)}
    </ListBox></Select.Popover>
  </Select>;
}
