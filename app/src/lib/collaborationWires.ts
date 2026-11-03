import type { Overview, Transfer } from "../api";
import type { CollaborationEvent, CollaborationMember, CollaborationSession, CollaborationTask, SessionRef } from "./collaboration";
import { canLink, endpointKey, matchingSessions, sessionReady, taskSource, type CollaborationEndpoint, type CollaborationLink } from "./collaborationLinks";
import { t } from "@/i18n";

export interface WireScope { ownerId: string; coordinatorId: string; projectId: string }
export interface CollaborationRelation extends CollaborationLink {
  id: string; connectedAt: number; hidden: boolean;
}
export type ActivityKind = "task" | "file" | "result" | "message";
export interface WireActivity extends CollaborationLink {
  id: string; kind: ActivityKind; label: string; expiresAt: number; failed?: boolean; taskId?: string;
}
export const RELATIONS_CHANGED = "p2pcli:relations-changed";
export const relationKey = (link: CollaborationLink) =>
  JSON.stringify([endpointKey(link.source), endpointKey(link.destination)].sort());
export const relationStorageKey = (scope: WireScope) =>
  `p2pcli.collaboration.relations.v1:${JSON.stringify([scope.ownerId, scope.coordinatorId, scope.projectId])}`;
const object = (value: unknown): value is Record<string, unknown> =>
  !!value && typeof value === "object" && !Array.isArray(value);
const endpoint = (value: unknown): value is CollaborationEndpoint =>
  object(value) && typeof value.deviceId === "string" && !!value.deviceId && typeof value.deviceName === "string" &&
  (value.terminalId === undefined || typeof value.terminalId === "string");

export function parseRelations(raw: string | null): CollaborationRelation[] {
  try {
    const value: unknown = JSON.parse(raw ?? "null");
    if (!object(value) || value.version !== 1 || !Array.isArray(value.relations)) return [];
    return value.relations.filter((item): item is CollaborationRelation =>
      object(item) && endpoint(item.source) && endpoint(item.destination) &&
      canLink(item.source, item.destination) && item.id === relationKey({ source: item.source, destination: item.destination }) &&
      typeof item.connectedAt === "number" && Number.isFinite(item.connectedAt) && typeof item.hidden === "boolean").slice(-500);
  } catch { return []; }
}

export function mergeRelation(records: CollaborationRelation[], link: CollaborationLink, now: number, onlyNew = false) {
  if (!canLink(link.source, link.destination)) return records;
  const id = relationKey(link);
  const existing = records.find((item) => item.id === id);
  if (existing && (onlyNew || !existing.hidden)) return records;
  return [...records.filter((item) => item.id !== id), { ...link, id, connectedAt: now, hidden: false }].slice(-500);
}

export function readRelations(scope: WireScope): CollaborationRelation[] {
  try { return parseRelations(localStorage.getItem(relationStorageKey(scope))); } catch { return []; }
}
function saveRelations(scope: WireScope, records: CollaborationRelation[]) {
  localStorage.setItem(relationStorageKey(scope), JSON.stringify({ version: 1, relations: records }));
  window.dispatchEvent(new CustomEvent(RELATIONS_CHANGED, { detail: relationStorageKey(scope) }));
}
export function rememberRelation(scope: WireScope, link: CollaborationLink, onlyNew = false) {
  const current = readRelations(scope);
  const next = mergeRelation(current, link, Date.now(), onlyNew);
  if (next !== current) saveRelations(scope, next);
}
export function hideRelation(scope: WireScope, id: string) {
  saveRelations(scope, readRelations(scope).map((item) => item.id === id ? { ...item, hidden: true } : item));
}

export function sessionEndpoint(ref: SessionRef, sessions: CollaborationSession[], ownerId: string): CollaborationEndpoint {
  const session = sessions.find((item) => item.id === ref.session_id && item.device_id === ref.device_id);
  const terminal = session?.terminal;
  return {
    deviceId: ref.device_id, deviceName: ref.device_id,
    ...(terminal?.owner_device_id === ownerId && terminal.host_device_id === ref.device_id ?
      { terminalId: terminal.terminal_id, sessionName: session?.name } : {}),
  };
}
export function taskWire(task: CollaborationTask, sessions: CollaborationSession[], ownerId: string): CollaborationLink {
  // Team metadata describes the eventual team, not the actual recipient of this task.
  const source = task.creator_session ? sessionEndpoint({ device_id: task.creator_device, session_id: task.creator_session }, sessions, ownerId) : taskSource(task);
  return { source, destination: sessionEndpoint(task.spec.assignee, sessions, ownerId) };
}

export function relationAvailable(link: CollaborationLink, overview: Overview | null,
  sessions: CollaborationSession[], members: CollaborationMember[], now: number) {
  if (!overview) return false;
  return [link.source, link.destination].every((item) => {
    const local = item.deviceId === overview.endpoint_id;
    const online = local || overview.devices.some((device) => device.endpoint_id === item.deviceId && device.state === "online");
    const member = members.find((member) => member.device_id === item.deviceId);
    if (!online || !member || member.revoked || member.role === "viewer") return false;
    return local && !item.terminalId || matchingSessions(sessions, item, overview.endpoint_id).some((session) => sessionReady(session, members, now / 1000));
  });
}

function sessionRef(value: unknown): value is SessionRef {
  return object(value) && typeof value.device_id === "string" && typeof value.session_id === "string";
}
function eventTask(value: unknown): value is CollaborationTask {
  return object(value) && typeof value.id === "string" && typeof value.creator_device === "string" &&
    (value.creator_session === null || typeof value.creator_session === "string") &&
    object(value.spec) && typeof value.spec.title === "string" && sessionRef(value.spec.assignee);
}
export function activityFromEvent(event: CollaborationEvent, sessions: CollaborationSession[], ownerId: string, now: number): WireActivity | null {
  const data = event.data;
  const base = { id: `event:${event.seq}`, expiresAt: now + 3000 };
  if (["task_claimed", "task_result", "task_result_after_cancel"].includes(event.kind) && eventTask(data)) {
    const link = taskWire(data, sessions, ownerId);
    if (!canLink(link.source, link.destination)) return null;
    if (event.kind === "task_claimed") return { ...base, ...link, kind: "task", label: t("任务已领取：{{title}}", { title: data.spec.title }), taskId: data.id };
    const attempts = Array.isArray(data.attempts) ? data.attempts : [];
    const result = attempts.at(-1)?.result;
    return { ...base, source: link.destination, destination: link.source, kind: "result",
      label: t("结果已回传：{{title}}", { title: data.spec.title }), taskId: data.id, failed: !!result?.failed };
  }
  if (event.kind === "message_delivered" && object(data) && sessionRef(data.from) && sessionRef(data.to)) {
    const link = { source: sessionEndpoint(data.from, sessions, ownerId), destination: sessionEndpoint(data.to, sessions, ownerId) };
    if (canLink(link.source, link.destination)) return { ...base, ...link, kind: "message", label: t("协作消息已送达") };
  }
  return null;
}

export function mergeActivities(current: WireActivity[], additions: WireActivity[], now: number) {
  // Coalesce bursts by route, kind and direction, without replaying one pulse per heartbeat.
  const result = new Map<string, WireActivity>();
  for (const activity of [...current, ...additions]) {
    if (activity.expiresAt <= now) continue;
    const key = JSON.stringify([endpointKey(activity.source), endpointKey(activity.destination), activity.kind]);
    result.set(key, activity);
  }
  return [...result.values()].slice(-32);
}

export class EventCursor {
  cursor = 0;
  private baseline = true;
  accept(events: CollaborationEvent[], cursor: number): CollaborationEvent[] {
    if (!Number.isSafeInteger(cursor) || cursor < this.cursor) return [];
    const previous = this.cursor;
    this.cursor = cursor;
    if (this.baseline) {
      if (cursor === previous) this.baseline = false;
      return [];
    }
    const seen = new Set<number>();
    return events.filter((event) => {
      if (!Number.isSafeInteger(event.seq) || event.seq <= previous || event.seq > cursor || seen.has(event.seq)) return false;
      seen.add(event.seq);
      return true;
    });
  }
}

export class TransferActivityTracker {
  private previous = new Map<string, { done: number; state: string; movedUntil: number; failedUntil: number }>();
  update(transfers: Transfer[], local: CollaborationEndpoint, now: number): WireActivity[] {
    const next = new Map<string, { done: number; state: string; movedUntil: number; failedUntil: number }>();
    const activities: WireActivity[] = [];
    for (const transfer of transfers) {
      const previous = this.previous.get(transfer.id);
      const moved = !!previous && transfer.done > previous.done && transfer.state === "running";
      const movedUntil = moved ? now + 3000 : previous?.movedUntil ?? 0;
      const failedUntil = previous && previous.state !== "failed" && transfer.state === "failed" ? now + 4000 : previous?.failedUntil ?? 0;
      next.set(transfer.id, { done: transfer.done, state: transfer.state, movedUntil, failedUntil });
      const failed = transfer.state === "failed" && failedUntil > now;
      if (!failed && (transfer.state !== "running" || movedUntil <= now)) continue;
      const peer = { deviceId: transfer.peer, deviceName: transfer.peer_name };
      const progress = transfer.total > 0 ? ` · ${Math.min(100, Math.floor(100 * transfer.done / transfer.total))}%` : "";
      activities.push({
        id: `file:${transfer.id}`, kind: "file",
        source: transfer.direction === "send" ? local : peer,
        destination: transfer.direction === "send" ? peer : local,
        label: failed ? t("传输失败：{{label}}", { label: transfer.label }) : `${transfer.direction === "send" ? t("发送") : t("接收")} ${transfer.label}${progress}`,
        expiresAt: failed ? failedUntil : movedUntil, failed,
      });
    }
    this.previous = next;
    return activities;
  }
}
