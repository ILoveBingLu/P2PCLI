import { useEffect, useMemo, useRef, useState } from "react";
import { api, type Overview, type Transfer } from "../api";
import { expectCollaboration, type CollaborationProject, type CollaborationSession, type CollaborationTask } from "../lib/collaboration";
import {
  activityFromEvent, EventCursor, hideRelation, mergeActivities, readRelations, RELATIONS_CHANGED,
  relationStorageKey, rememberRelation, taskWire, TransferActivityTracker,
  type CollaborationRelation, type WireActivity,
} from "../lib/collaborationWires";
import { t } from "@/i18n";

export function useCollaborationWires(overview: Overview | null, project: CollaborationProject | null,
  sessions: CollaborationSession[], tasks: CollaborationTask[], transfers: Transfer[]) {
  const ownerId = overview?.endpoint_id;
  const scope = useMemo(() => ownerId && project ? {
    ownerId, coordinatorId: project.coordinator_id, projectId: project.id,
  } : null, [ownerId, project?.coordinator_id, project?.id]);
  const scopeKey = scope ? relationStorageKey(scope) : "";
  const latestSessions = useRef(sessions);
  latestSessions.current = sessions;
  const [records, setRecords] = useState<{ key: string; items: CollaborationRelation[] }>({ key: "", items: [] });
  const [events, setEvents] = useState<{ key: string; items: WireActivity[] }>({ key: "", items: [] });
  const [files, setFiles] = useState<WireActivity[]>([]);
  const [storageError, setStorageError] = useState("");
  const [eventError, setEventError] = useState("");
  const [now, setNow] = useState(Date.now);
  const tracker = useRef(new TransferActivityTracker());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 500);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    tracker.current = new TransferActivityTracker();
    setFiles([]);
  }, [ownerId]);
  useEffect(() => {
    if (!overview) return;
    setFiles(tracker.current.update(transfers, { deviceId: overview.endpoint_id, deviceName: overview.name }, Date.now()));
  }, [transfers, ownerId]);

  useEffect(() => {
    if (!scope) return;
    const load = () => setRecords({ key: scopeKey, items: readRelations(scope) });
    load();
    const changed = (event: Event) => {
      if (event instanceof StorageEvent ? event.key === scopeKey || event.key === null : (event as CustomEvent).detail === scopeKey) load();
    };
    window.addEventListener("storage", changed);
    window.addEventListener(RELATIONS_CHANGED, changed);
    return () => {
      window.removeEventListener("storage", changed);
      window.removeEventListener(RELATIONS_CHANGED, changed);
    };
  }, [scope, scopeKey]);

  useEffect(() => {
    if (!scope) return;
    try {
      for (const task of [...tasks].sort((a, b) => a.created_at - b.created_at).slice(-100)) {
        rememberRelation(scope, taskWire(task, sessions, scope.ownerId), true);
      }
    } catch { setStorageError(t("连线记录未能保存")); }
  }, [scope, tasks, sessions]);

  useEffect(() => {
    setEventError("");
    setStorageError("");
    if (!scope) return;
    let active = true;
    let reading = false;
    let cursor = new EventCursor();
    let generation = 0;
    const poll = async () => {
      if (reading || document.hidden) return;
      reading = true;
      const started = generation;
      const additions: WireActivity[] = [];
      try {
        // Catch up without animating historical events. Bound each poll to avoid flooding the coordinator.
        for (let page = 0; page < 4 && active; page++) {
          const previous = cursor.cursor;
          const reply = expectCollaboration(await api.collabRequest(scope.coordinatorId === scope.ownerId ? null : scope.coordinatorId,
            scope.projectId, { type: "events_read", after: previous, limit: 100 }), "events");
          if (!active || started !== generation) return;
          for (const event of cursor.accept(reply.events, reply.cursor)) {
            const activity = activityFromEvent(event, latestSessions.current, scope.ownerId, Date.now());
            if (activity) additions.push(activity);
          }
          if (reply.cursor === previous) break;
        }
        if (active) {
          setEvents((current) => ({ key: scopeKey, items: mergeActivities(current.key === scopeKey ? current.items : [], additions, Date.now()) }));
          setEventError("");
        }
      } catch {
        if (active) {
          setEvents({ key: scopeKey, items: [] });
          setEventError(t("活动状态未同步"));
          cursor = new EventCursor();
        }
      } finally { reading = false; }
    };
    const visibility = () => {
      generation++;
      cursor = new EventCursor();
      setEvents({ key: scopeKey, items: [] });
      if (!document.hidden) void poll();
    };
    void poll();
    const timer = window.setInterval(() => void poll(), 2000);
    document.addEventListener("visibilitychange", visibility);
    return () => { active = false; window.clearInterval(timer); document.removeEventListener("visibilitychange", visibility); };
  }, [scope, scopeKey]);

  return {
    relations: records.key === scopeKey ? records.items.filter((item) => !item.hidden) : [],
    activities: [...(events.key === scopeKey ? events.items : []), ...files].filter((item) => item.expiresAt > now),
    wireError: storageError || eventError, now,
    forgetRelation: (id: string) => {
      if (!scope) return false;
      try { hideRelation(scope, id); setStorageError(""); return true; }
      catch { setStorageError(t("连线记录未能移除")); return false; }
    },
  };
}
