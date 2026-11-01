import { useState } from "react";
import { Alert, Button, Modal } from "@heroui/react";
import { ArrowLeftRight, Trash2 } from "lucide-react";
import type { CollaborationEndpoint, CollaborationLink } from "../lib/collaborationLinks";
import { relationAvailable, relationKey, taskWire } from "../lib/collaborationWires";
import { taskStateLabels } from "../lib/collaboration";
import { useCollaboration } from "./CollaborationContext";
import { t } from "@/i18n";

export function CollaborationWireDetails({ links, onClose }: { links: CollaborationLink[]; onClose: () => void }) {
  const collab = useCollaboration();
  const [removing, setRemoving] = useState<string | null>(null);
  const keys = new Set(links.map(relationKey));
  const records = collab.relations.filter((item) => keys.has(item.id));
  const activities = collab.activities.filter((item) => keys.has(relationKey(item)));
  const tasks = collab.tasks.filter((task) => keys.has(relationKey(taskWire(task, collab.sessions, collab.overview?.endpoint_id ?? ""))));
  const label = (endpoint: CollaborationEndpoint) => `${collab.deviceName(endpoint.deviceId)}${endpoint.sessionName ? ` / ${endpoint.sessionName}` : ""}`;
  return <Modal.Backdrop isOpen onOpenChange={(open) => { if (!open) onClose(); }}>
    <Modal.Container size="lg" scroll="inside">
      <Modal.Dialog>
        <Modal.CloseTrigger aria-label={t("关闭连接详情")} />
        <Modal.Header><Modal.Heading>{t("协作连接")}</Modal.Heading></Modal.Header>
        <Modal.Body>
          <ul className="collab-relation-list">
            {(records.length ? records : links).map((record) => <li key={relationKey(record)}>
              <div className="collab-relation-endpoints"><span>{label(record.source)}</span><ArrowLeftRight size={16} aria-hidden="true" /><span>{label(record.destination)}</span>
                <small>{collab.error || collab.loading ? t("状态未确认") :
                  relationAvailable(record, collab.overview, collab.sessions, collab.members, collab.now) ? t("当前可用") : t("离线或会话不可用")}</small>
              </div>
              {"id" in record && <span title={t("移除本机连线记录")}><Button variant="ghost" isIconOnly
                  aria-label={t("移除 {{value}} 的连线记录", { value: label(record.destination) })}
                  onPress={() => setRemoving(String(record.id))}><Trash2 size={16} /></Button></span>}
            </li>)}
          </ul>
          {activities.length > 0 && <ul className="collab-activity-list" aria-label={t("当前传递")}>
            {activities.map((activity) => <li key={activity.id} data-failed={activity.failed || undefined}>
              <strong>{activity.label}</strong><span>{label(activity.source)} → {label(activity.destination)}</span>
            </li>)}
          </ul>}
          {tasks.length > 0 && <ul className="collab-wire-tasks" aria-label={t("关联任务")}>
            {tasks.map((task) => <li key={task.id}><Button variant="ghost" onPress={() => {
              collab.selectTask(task.id); collab.setOpen(true); onClose();
            }}>{task.spec.title}<span>{taskStateLabels()[task.state]}</span></Button></li>)}
          </ul>}
          {collab.wireError && <Alert status="warning"><Alert.Indicator /><Alert.Content><Alert.Description>{collab.wireError}</Alert.Description></Alert.Content></Alert>}
          {removing && <Alert status="warning"><Alert.Indicator /><Alert.Content>
            <Alert.Title>{t("移除此连线记录？")}</Alert.Title>
            <Alert.Description>{t("仅移除本机显示记录，不会撤销权限、中断传输或取消任务。")}</Alert.Description>
          </Alert.Content></Alert>}
        </Modal.Body>
        <Modal.Footer>
          {removing ? <>
            <Button variant="ghost" onPress={() => setRemoving(null)}>{t("取消")}</Button>
            <Button variant="danger" onPress={() => { if (collab.forgetRelation(removing)) { setRemoving(null); onClose(); } }}>{t("确认移除")}</Button>
          </> : <Button variant="secondary" onPress={onClose}>{t("关闭")}</Button>}
        </Modal.Footer>
      </Modal.Dialog>
    </Modal.Container>
  </Modal.Backdrop>;
}
