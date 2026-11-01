import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { api, type Overview, type Transfer } from "../api";
import {
  collaborationErrorText, expectCollaboration,
  type CollaborationMember, type CollaborationProject, type CollaborationSession, type CollaborationTask,
} from "../lib/collaboration";
import type { CollaborationDraft, CollaborationEndpoint, CollaborationLink } from "../lib/collaborationLinks";
import { useCollaborationWires } from "./useCollaborationWires";
import type { CollaborationRelation, WireActivity } from "../lib/collaborationWires";
import { t } from "@/i18n";

interface Snapshot {
  projects: CollaborationProject[];
  project: CollaborationProject | null;
  members: CollaborationMember[];
  sessions: CollaborationSession[];
  tasks: CollaborationTask[];
}
const empty: Snapshot = { projects: [], project: null, members: [], sessions: [], tasks: [] };
interface CollaborationContextValue extends Snapshot {
  overview: Overview | null;
  terminals: CollaborationEndpoint[];
  target: string | null;
  setTarget: (target: string | null) => void;
  selectProject: (id: string) => void;
  loading: boolean;
  error: string;
  busy: boolean;
  run: <T>(operation: () => Promise<T>) => Promise<T>;
  refresh: () => void;
  ensureProject: () => Promise<CollaborationProject>;
  open: boolean;
  setOpen: (open: boolean) => void;
  advanced: boolean;
  setAdvanced: (open: boolean) => void;
  selected: string | null;
  selectTask: (id: string | null) => void;
  link: CollaborationLink | null;
  setLink: (link: CollaborationLink | null) => void;
  linking: boolean;
  setLinking: (linking: boolean) => void;
  onPrepare: (drafts: CollaborationDraft[]) => void;
  deviceName: (id: string) => string;
  relations: CollaborationRelation[];
  activities: WireActivity[];
  wireError: string;
  now: number;
  forgetRelation: (id: string) => boolean;
}
const CollaborationContext = createContext<CollaborationContextValue | null>(null);
export function useCollaboration() {
  const value = useContext(CollaborationContext);
  if (!value) throw new Error("CollaborationProvider is missing");
  return value;
}

export function CollaborationProvider({ overview, terminals, transfers, open, onOpenChange, onPrepare, children }: {
  overview: Overview | null; terminals: CollaborationEndpoint[];
  transfers: Transfer[];
  open: boolean; onOpenChange: (open: boolean) => void;
  onPrepare: (drafts: CollaborationDraft[]) => void;
  children: ReactNode;
}) {
  const [target, changeTarget] = useState<string | null>(null);
  const [projectId, setProjectId] = useState<string | null>(null);
  const [snapshot, setSnapshot] = useState<Snapshot>(empty);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const writing = useRef(false);
  const [revision, setRevision] = useState(0);
  const [selected, selectTask] = useState<string | null>(null);
  const [link, setLink] = useState<CollaborationLink | null>(null);
  const [linking, setLinking] = useState(false);
  const [advanced, setAdvanced] = useState(false);
  const identity = overview?.endpoint_id;
  const refresh = useCallback(() => setRevision((value) => value + 1), []);
  const wires = useCollaborationWires(overview, snapshot.project, snapshot.sessions, snapshot.tasks, transfers);

  useEffect(() => {
    if (!identity) return;
    let active = true;
    let reading = false;
    const load = async () => {
      if (reading || writing.current) return;
      reading = true;
      try {
        const projects = expectCollaboration(await api.collabRequest(target, null, { type: "projects_list" }), "projects").projects;
        const project = projects.find((item) => item.id === projectId) ?? projects[0] ?? null;
        let next: Snapshot = { ...empty, projects, project };
        if (project) {
          const [members, sessions, tasks] = await Promise.all([
            api.collabRequest(target, project.id, { type: "members_list" }),
            api.collabRequest(target, project.id, { type: "sessions_list" }),
            api.collabRequest(target, project.id, { type: "tasks_list" }),
          ]);
          next = {
            projects, project,
            members: expectCollaboration(members, "members").members,
            sessions: expectCollaboration(sessions, "sessions").sessions,
            tasks: expectCollaboration(tasks, "tasks").tasks,
          };
        }
        if (active && !writing.current) { setSnapshot(next); setError(""); }
      } catch (e) {
        if (active) setError(collaborationErrorText(e));
      } finally {
        reading = false;
        if (active) setLoading(false);
      }
    };
    void load();
    const timer = window.setInterval(() => void load(), 5000);
    return () => { active = false; window.clearInterval(timer); };
  }, [identity, target, projectId, revision]);

  const run = async <T,>(operation: () => Promise<T>): Promise<T> => {
    if (writing.current) throw new Error(t("上一操作尚未完成"));
    writing.current = true;
    setBusy(true);
    try { return await operation(); }
    finally { writing.current = false; setBusy(false); refresh(); }
  };
  const ensureProject = async () => {
    if (error || loading || !overview) throw new Error(t("协作状态尚未确认，请刷新后重试"));
    if (snapshot.project) return snapshot.project;
    if (target) throw new Error(t("请先在协调设备本机创建项目"));
    const response = expectCollaboration(await api.collabRequest(null, null, { type: "project_create", name: t("设备协作") }), "project");
    setSnapshot({ ...empty, projects: [response.project], project: response.project,
      members: [{ device_id: overview.endpoint_id, role: "admin", revoked: false }] });
    setProjectId(response.project.id);
    return response.project;
  };
  const deviceName = (id: string) => overview?.remarks[id]?.device || (id === overview?.endpoint_id ? overview.name :
    overview?.recent.find((device) => device.endpoint_id === id)?.name ??
    overview?.nearby.find((device) => device.endpoint_id === id)?.name ?? id.slice(0, 10));

  return <CollaborationContext value={{
    ...snapshot, ...wires, overview, terminals, target, loading, error, busy, run, refresh, ensureProject,
    open, setOpen: onOpenChange, advanced, setAdvanced, selected, selectTask,
    link, setLink, linking, setLinking, onPrepare, deviceName,
    setTarget: (value) => {
      changeTarget(value); setProjectId(null); setSnapshot(empty); setLoading(true); setError(""); selectTask(null);
    },
    selectProject: (id) => {
      setProjectId(id); setSnapshot((value) => ({ ...empty, projects: value.projects })); setLoading(true); setError(""); selectTask(null);
    },
  }}>{children}</CollaborationContext>;
}
