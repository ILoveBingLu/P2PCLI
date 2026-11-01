// Wire types mirror crates/core/src/collaboration/model.rs.
export type CollaborationRole = "admin" | "dispatcher" | "worker" | "viewer";
export type TaskState = "queued" | "running" | "blocked" | "result_ready" | "completed" | "failed" | "cancel_requested" | "cancelled";
export interface SessionRef { device_id: string; session_id: string }
export interface CollaborationProject { id: string; name: string; coordinator_id: string; created_at: number }
export interface CollaborationMember { device_id: string; role: CollaborationRole; revoked: boolean }
export interface CollaborationSession {
  id: string; project_id: string; device_id: string; name: string;
  mode: "manual_pull" | "managed_adapter";
  terminal: { host_device_id: string; owner_device_id: string; terminal_id: string } | null;
  cwd: string | null; available: boolean; last_seen: number;
}
export interface TaskSpec { title: string; goal: string; acceptance: string; inputs: unknown; assignee: SessionRef }
export interface TaskResult {
  summary: string; completed: string[]; remaining: string[]; verification: string[]; artifact_ids: string[]; failed: boolean;
}
export interface TaskAttempt {
  id: string; assignee: SessionRef; started_at: number; last_seen: number; stopped: boolean; result: TaskResult | null;
}
export interface CollaborationTask {
  id: string; project_id: string; creator_device: string; creator_session: string | null;
  spec: TaskSpec; state: TaskState; version: number; attempts: TaskAttempt[]; progress: string | null;
  created_at: number; updated_at: number;
}
export interface CollaborationArtifact {
  id: string; task_id: string; attempt_id: string; provider_device: string; name: string;
  size: number; hash: string; received: boolean; local_path: string | null;
}
export type CollaborationAction =
  | { type: "projects_list" | "project_get" | "members_list" | "sessions_list" | "tasks_list" }
  | { type: "project_create"; name: string }
  | { type: "member_set"; device_id: string; role: CollaborationRole; revoked: boolean }
  | { type: "task_create"; spec: TaskSpec }
  | { type: "task_get" | "artifacts_list"; task_id: string }
  | { type: "task_accept" | "task_cancel" | "task_requeue"; task_id: string; expected_version: number }
  | { type: "task_assign"; task_id: string; expected_version: number; assignee: SessionRef };
export interface CollaborationRequest { request_id: string; project_id: string | null; action: CollaborationAction }
export type CollaborationResponse =
  | { type: "queued"; request_id: string; message: string }
  | { type: "committed_unverified"; request_id: string; message: string }
  | { type: "project"; project: CollaborationProject }
  | { type: "projects"; projects: CollaborationProject[] }
  | { type: "members"; members: CollaborationMember[] }
  | { type: "sessions"; sessions: CollaborationSession[] }
  | { type: "task"; task: CollaborationTask }
  | { type: "tasks"; tasks: CollaborationTask[] }
  | { type: "artifacts"; artifacts: CollaborationArtifact[] }
  | { type: "ack" };
export interface CollaborationReply { response?: CollaborationResponse; error?: { code: string; message: string } }
export interface CollaborationDelivery {
  id: string; request_id: string; target: string | null; project_id: string | null;
  operation: string; state: string; attempts: number; updated_at: number; last_error: string | null;
}

export class CollaborationError extends Error {
  constructor(public code: string, message: string) { super(message); this.name = "CollaborationError"; }
}
export function unwrapCollaboration(reply: CollaborationReply): CollaborationResponse {
  if (reply?.error) throw new CollaborationError(reply.error.code, reply.error.message);
  if (!reply?.response) throw new CollaborationError("invalid_response", "协调设备未返回有效响应");
  return reply.response;
}
export function expectCollaboration<T extends CollaborationResponse["type"]>(
  response: CollaborationResponse, type: T,
): Extract<CollaborationResponse, { type: T }> {
  if (response.type !== type) throw new CollaborationError("invalid_response", `预期 ${type}，收到 ${response.type}`);
  return response as Extract<CollaborationResponse, { type: T }>;
}
export const taskStateLabels: Record<TaskState, string> = {
  queued: "待领取", running: "执行中", blocked: "待处理", result_ready: "待验收",
  completed: "已完成", failed: "失败", cancel_requested: "等待停止确认", cancelled: "已取消",
};
export const roleLabels: Record<CollaborationRole, string> = { admin: "管理员", dispatcher: "调度者", worker: "执行者", viewer: "观察者" };
export const shortId = (id: string) => id.length > 16 ? `${id.slice(0, 8)}…${id.slice(-4)}` : id;
export const sessionKey = (ref: SessionRef) => `${ref.device_id}/${ref.session_id}`;
export const sessionRef = (session: CollaborationSession): SessionRef => ({ device_id: session.device_id, session_id: session.id });
export function collaborationTime(seconds: number) {
  return new Date(seconds * 1000).toLocaleString("zh-CN", { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" });
}
export function collaborationErrorText(error: unknown) {
  if (error instanceof CollaborationError) {
    if (error.code === "version_conflict") return "任务已被其他会话更新。请刷新，核对最新状态后再操作。";
    if (error.code === "forbidden") return `没有此操作的权限：${error.message}`;
    return `${error.message} (${error.code})`;
  }
  return `连接或请求失败：${String(error)}。请刷新核对操作是否已提交。`;
}
