import { useEffect, useRef, useState } from "react";
import { ArrowLeft, ArrowRight, LoaderCircle, Network, RefreshCw, Settings2, X } from "lucide-react";
import { api } from "../api";
import { collaborationErrorText, expectCollaboration, taskStateLabels, type CollaborationAction } from "../lib/collaboration";
import { useCollaboration } from "./CollaborationContext";
import { CollaborationTaskDetail } from "./CollaborationTaskDetail";
import { CollaborationDeliveries } from "./CollaborationDeliveries";
import { Collaboration } from "./Collaboration";
import { CollaborationLinkDialog } from "./CollaborationLinkDialog";
import { Button } from "./ui/button";
import { toast } from "./ui/toast";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "./ui/dialog";
import { taskDestination, taskSource } from "../lib/collaborationLinks";
import "../collaboration.css";
import "../collaboration-links.css";
import { t } from "@/i18n";

export function CollaborationDrawer() {
  const collab = useCollaboration();
  const [error, setError] = useState("");
  const [filter, setFilter] = useState<"active" | "all">("active");
  const closeRef = useRef<HTMLButtonElement>(null);
  const task = collab.tasks.find((task) => task.id === collab.selected);
  const own = collab.members.find((member) => member.device_id === collab.overview?.endpoint_id && !member.revoked);
  const canDispatch = own?.role === "admin" || own?.role === "dispatcher";
  const tasks = [...collab.tasks].filter((task) => filter === "all" || !["completed", "cancelled"].includes(task.state))
    .sort((a, b) => b.updated_at - a.updated_at);
  useEffect(() => { if (collab.open) closeRef.current?.focus(); }, [collab.open]);
  useEffect(() => { setError(""); }, [collab.selected, collab.target, collab.project?.id]);
  const mutate = async (action: CollaborationAction) => {
    if (!collab.project || collab.busy || collab.loading || collab.error) return false;
    setError("");
    try {
      await collab.run(async () => {
        expectCollaboration(await api.collabRequest(collab.target, collab.project!.id, action), "task");
      });
      toast.success(t("已保存"));
      return true;
    } catch (e) { setError(collaborationErrorText(e)); return false; }
  };
  const devices = new Map([
    ...(collab.overview?.recent ?? []).map((device) => [device.endpoint_id, device.name] as const),
    ...(collab.overview?.nearby ?? []).map((device) => [device.endpoint_id, device.name] as const),
  ]);
  return <>
    {collab.open && <aside className="collab-drawer" aria-label={t("协作详情")}
      onKeyDown={(event) => { if (event.key === "Escape" && !collab.link) { event.stopPropagation(); collab.setOpen(false); } }}>
      <header className="collab-drawer-header">
        {task ? <Button variant="ghost" size="icon-sm" title={t("所有协作")} aria-label={t("所有协作")} onPress={() => collab.selectTask(null)}><ArrowLeft /></Button> : <Network />}
        <h2>{task ? t("协作详情") : t("协作")}</h2>
        <Button variant="ghost" size="icon-sm" title={t("刷新协作")} aria-label={t("刷新协作")} disabled={collab.busy || collab.loading} onPress={collab.refresh}><RefreshCw /></Button>
        <Button variant="ghost" size="icon-sm" title={t("高级管理")} aria-label={t("高级管理")} onPress={() => collab.setAdvanced(true)}><Settings2 /></Button>
        <Button ref={closeRef} variant="ghost" size="icon-sm" title={t("关闭协作详情")} aria-label={t("关闭协作详情")} onPress={() => collab.setOpen(false)}><X /></Button>
      </header>
      {(collab.error || error) && <p className="collab-drawer-error" role="alert">{collab.error || error}</p>}
      {task ? <CollaborationTaskDetail task={task} sessions={collab.sessions} target={collab.target}
        compact deviceName={collab.deviceName}
        canDispatch={canDispatch} disabled={collab.busy || collab.loading || !!collab.error} mutate={mutate}
        onBack={() => collab.selectTask(null)} /> : <div className="collab-drawer-scroll">
        <div className="collab-drawer-filters" role="group" aria-label={t("任务范围")}>
          <Button variant={filter === "active" ? "secondary" : "ghost"} size="sm" aria-pressed={filter === "active"} onPress={() => setFilter("active")}>{t("进行中")}</Button>
          <Button variant={filter === "all" ? "secondary" : "ghost"} size="sm" aria-pressed={filter === "all"} onPress={() => setFilter("all")}>{t("全部")}</Button>
          <span>{tasks.length}</span>
        </div>
        {collab.loading ? <div className="collab-empty"><LoaderCircle className="animate-spin" />{t("同步中")}</div> :
          tasks.length === 0 ? <div className="collab-empty"><Network />{collab.error ? t("协作暂不可用") : t("暂无协作任务")}</div> :
          tasks.map((task) => <Button key={task.id} variant="ghost" className="collab-link-task" onPress={() => collab.selectTask(task.id)}>
            <div><span className="collab-state" data-state={task.state}>{taskStateLabels()[task.state]}</span><strong>{task.spec.title}</strong></div>
            <p>{collab.deviceName(taskSource(task).deviceId)}<ArrowRight />{collab.deviceName(taskDestination(task).deviceId)}</p>
            {task.progress && <small>{task.progress}</small>}
          </Button>)}
        <details className="collab-drawer-scope">
          <summary>{t("协作范围")}</summary>
          <label className="collab-link-field">{t("协调设备")}<select aria-label={t("协调设备")} value={collab.target ?? ""}
            disabled={collab.busy || !!collab.link} onChange={(event) => collab.setTarget(event.target.value || null)}>
            <option value="">{t("本机")}</option>
            {[...devices].filter(([id]) => id !== collab.overview?.endpoint_id).map(([id, name]) => <option key={id} value={id}>{name}</option>)}
          </select></label>
          {collab.projects.length > 0 && <label className="collab-link-field">{t("项目")}<select aria-label={t("协作项目")} value={collab.project?.id ?? ""}
            disabled={collab.busy || !!collab.link} onChange={(event) => collab.selectProject(event.target.value)}>
            {collab.projects.map((project) => <option key={project.id} value={project.id}>{project.name}</option>)}
          </select></label>}
        </details>
        <CollaborationDeliveries />
      </div>}
    </aside>}
    <CollaborationLinkDialog />
    <Dialog open={collab.advanced} onOpenChange={(open) => { collab.setAdvanced(open); if (!open) collab.refresh(); }}>
      <DialogContent className="collab-management-dialog">
        <DialogHeader><DialogTitle>{t("协作高级管理")}</DialogTitle></DialogHeader>
        <Collaboration overview={collab.overview} />
      </DialogContent>
    </Dialog>
  </>;
}
