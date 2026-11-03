import {
  Fragment,
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useRef,
  useState,
  type ComponentProps,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import {
  FileCode2,
  FileUp,
  LoaderCircle,
  History,
  House,
  Network,
  Plus,
  X,
} from "lucide-react";
import { Tabs } from "@heroui/react";
import { toast, Toaster } from "@/components/ui/toast";
import { AlertDialog, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";
import {
  api,
  shellQuote,
  type FileRequest,
  type HostEvent,
  type IncomingRequest,
  type Overview,
  type PathInfo,
  type RemoteInfo,
  type AgentSettings,
  type TerminalSession,
  type TerminalSessionSnapshot,
  type TermStatus,
  type Transfer,
} from "./api";
import { TerminalView, type TerminalHandle } from "./TerminalView";
import { pollConnections } from "./lib/pollConnections";
import { withoutClearedSessions } from "./lib/sessionCleanup";
import { TerminalStatusbar } from "./components/TerminalStatusbar";
import { selftestSawOverview } from "./selftest";
import { ConnectionPopover } from "./components/ConnectionPopover";
import { ConnectionSidebar } from "./components/ConnectionSidebar";
import { useConnection } from "./components/useConnection";
import { useSidebar } from "./components/useSidebar";
import { useUpdater } from "./components/useUpdater";
import { UpdateIsland } from "./components/UpdateIsland";
import { useAgentAlerts } from "./components/useAgentAlerts";
import { AgentTabTitle } from "./components/Agent";
import { GlobalContextMenu } from "./components/GlobalContextMenu";
import { AgentComposer, isMac } from "./components/AgentComposer";
import { AgentLauncher } from "./components/AgentLauncher";
import { AgentSettingsBar, CLAUDE_MODELS, choiceLabel, claudeEfforts, modeChoices, type AgentSetting } from "./components/AgentSettingsBar";
import {
  ApproveDialog,
  DiagDialog,
  FileApproveDialog,
} from "./components/AppDialogs";
import { TransferPanel } from "./components/TransferPanel";
import { RemoteSessions, type SessionTarget } from "./components/RemoteSessions";
import { SettingsPage } from "./components/SettingsPage";
import { Welcome } from "./components/Welcome";
import { Titlebar } from "./components/Titlebar";
import { CollaborationProvider } from "./components/CollaborationContext";
import { CollaborationPorts } from "./components/CollaborationPorts";
import { CollaborationDrawer } from "./components/CollaborationDrawer";
import { sessionTitles } from "./components/sessionText";
import { connectCollaborationSession, endpointKey, type CollaborationDraft, type CollaborationEndpoint } from "./lib/collaborationLinks";
import { expectCollaboration } from "./lib/collaboration";
import { rememberRelation } from "./lib/collaborationWires";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { cn } from "@/lib/utils";
import { RemarksProvider } from "./components/Remarks";
import { t } from "@/i18n";

interface Tab {
  key: number;
  connId: number;
  info: RemoteInfo;
  code: string | null;
  state: "opening" | "open" | "exited" | "closed";
  note: string | null;
  sessionId?: string;
  /** 终端打开以后才有；拖进来的文件发完把路径填进这个终端 */
  termId?: number;
}

// 代码编辑器带着 Monaco，十几 MB，第一次打开时才加载
const RemoteEditor = lazy(() => import("./components/editor/RemoteEditor"));

/** 代码编辑器按设备记：断开重连后打开的文件和未保存的修改都还在 */
interface EditorState {
  root: string;
  /** 外面要求换根目录时加一 */
  seq: number;
  shown: boolean;
}

/** 拖文件进窗口时的提示：发给谁，或者为什么发不了 */
type DropHint = { name: string } | { blocked: string };

export default function App() {
  const tabSeq = useRef(1);
  const clearedSessions = useRef(new Map<string, Set<string>>());
  const [ov, setOv] = useState<Overview | null>(null);
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [activeKey, setActiveKey] = useState<number | null>(null);
  const [paths, setPaths] = useState<Record<number, PathInfo>>({});
  const [requests, setRequests] = useState<IncomingRequest[]>([]);
  const [fileRequests, setFileRequests] = useState<FileRequest[]>([]);
  const [transfers, setTransfers] = useState<Transfer[]>([]);
  const [dropHint, setDropHint] = useState<DropHint | null>(null);
  // 认出命令行 AI 时终端下面的输入框：收没收起来记在本机，每个标签的草稿分开存
  const [composerHidden, setComposerHidden] = useState(() => {
    try { return localStorage.getItem("p2pcli.composer.hidden") === "1"; } catch { return false; }
  });
  const [drafts, setDrafts] = useState<Record<number, string>>({});
  const composerRef = useRef<HTMLTextAreaElement | null>(null);
  const pendingCollaborationDrafts = useRef(new Map<string, CollaborationDraft>());
  const onboardingInFlight = useRef(new Map<string, Promise<void>>());
  const onboardingController = useRef(new AbortController());
  useEffect(() => {
    const controller = new AbortController();
    onboardingController.current = controller;
    return () => controller.abort();
  }, []);
  const termHandles = useRef(new Map<number, TerminalHandle>());
  const [sessionTarget, setSessionTarget] = useState<SessionTarget | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [showCollaboration, setShowCollaboration] = useState(() => {
    const params = new URLSearchParams(location.search);
    return params.get("view") === "collab" || (import.meta.env.DEV && (params.get("s") ?? "").startsWith("collab"));
  });
  const [showDiag, setShowDiag] = useState(false);
  /** 会话页里结束了会话就加一，各处共享的会话快照跟着在后台刷新 */
  const [sessionsRev, setSessionsRev] = useState(0);
  const [liveSessions, setLiveSessions] = useState<Record<number, TerminalSessionSnapshot>>({});
  const [promptReady, setPromptReady] = useState<Record<number, boolean>>({});
  const [showConnect, setShowConnect] = useState(false);
  const [showLocalDevice, setShowLocalDevice] = useState(false);
  const [scanningNearby, setScanningNearby] = useState(false);
  const [nearbyReveal, setNearbyReveal] = useState(0);
  const scanPending = useRef(false);
  const [overviewError, setOverviewError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    api
      .overview()
      .then((o) => {
        // 已确认结束的会话不能被晚到的探测缓存重新放回侧边栏。
        setOv(withoutClearedSessions(o, clearedSessions.current));
        setOverviewError(null);
        selftestSawOverview(o);
      })
      .catch((e) => setOverviewError(String(e)));
  }, []);

  useEffect(() => {
    refresh();
    const timer = window.setInterval(refresh, 5000);
    const unsubs = [
      listen<IncomingRequest>("host-request", (e) => {
        setRequests((r) =>
          r.some((request) => request.id === e.payload.id)
            ? r
            : [...r, e.payload],
        );
        window.__ptTest?.onRequest?.(e.payload);
      }),
      listen<HostEvent>("host-event", (e) => {
        const ev = e.payload;
        if (ev.type === "request_cancelled") {
          setRequests((r) => r.filter((x) => x.id !== ev.id));
          setFileRequests((r) => r.filter((x) => x.id !== ev.id));
        }
        if (ev.type === "files_received") toast.success(t("收到 {{from_name}} 发来的 {{files}} 个文件", { from_name: ev.from_name, files: ev.files }));
        refresh();
      }),
      listen<FileRequest>("file-request", (e) => {
        setFileRequests((r) => r.some((x) => x.id === e.payload.id) ? r : [...r, e.payload]);
      }),
      listen<Transfer[]>("transfers", (e) => setTransfers(e.payload)),
      listen("server-status", refresh),
      listen<{ conn_id: number; reason: string }>("remote-closed", (e) => {
        const { conn_id, reason } = e.payload;
        setSessionTarget((target) => target?.connId === conn_id ? null : target);
        setTabs((ts) =>
          ts.map((tab) =>
            tab.connId === conn_id ? { ...tab, state: "closed", note: reason } : tab,
          ),
        );
      }),
    ];
    void api.transfers().then(setTransfers).catch(() => {});
    return () => {
      window.clearInterval(timer);
      unsubs.forEach((p) => p.then((u) => u()));
    };
  }, [refresh]);

  const activeTab = tabs.find((tab) => tab.key === activeKey) ?? null;
  const [editors, setEditors] = useState<Record<string, EditorState>>({});
  const [editorDirty, setEditorDirty] = useState<Record<string, number>>({});
  const [editorWidth, setEditorWidth] = useState(() => {
    const saved = Number(localStorage.getItem("p2pcli.editor.width"));
    return saved >= 0.25 && saved <= 0.8 ? saved : 0.55;
  });
  /** 关掉某台设备最后一个标签时，它的编辑器里还有没保存的文件：先问一下 */
  const [discardEditor, setDiscardEditor] = useState<{ count: number; onConfirm: () => void } | null>(null);
  const editorConnId = (endpointId: string) => tabs.find((tab) => tab.info.endpoint_id === endpointId && tab.state === "open")?.connId ?? null;
  const openEditor = (tab: Tab, path?: string) => {
    const id = tab.info.endpoint_id;
    const current = tab.sessionId
      ? liveSessions[tab.connId]?.sessions?.find((s) => s.id === tab.sessionId)?.details?.current_path
      : undefined;
    setEditors((all) => {
      const existing = all[id];
      // 已经开着又没指定目录：只是重新显示，不换根目录，免得收起的文件树被重置
      if (existing && path === undefined) return { ...all, [id]: { ...existing, shown: true } };
      return { ...all, [id]: { root: path ?? current ?? "", seq: (existing?.seq ?? 0) + 1, shown: true } };
    });
  };
  const closeEditor = (endpointId: string) => {
    setEditors(({ [endpointId]: _, ...rest }) => rest);
    setEditorDirty(({ [endpointId]: _, ...rest }) => rest);
  };
  // 目录选择用了 memo，传稳定的函数进去，里面再取最新的标签
  const openEditorRef = useRef<(path: string) => void>(() => {});
  openEditorRef.current = (path) => { if (activeTab) openEditor(activeTab, path); };
  const openEditorAt = useCallback((path: string) => openEditorRef.current(path), []);
  const dragEditorSplit = (event: ReactPointerEvent<HTMLDivElement>) => {
    const container = event.currentTarget.parentElement;
    if (!container) return;
    event.preventDefault();
    const handle = event.currentTarget;
    handle.setPointerCapture(event.pointerId);
    let frame = 0;
    let width = editorWidth;
    const move = (e: PointerEvent) => {
      const rect = container.getBoundingClientRect();
      width = Math.min(0.8, Math.max(0.25, (rect.right - e.clientX) / rect.width));
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => setEditorWidth(width));
    };
    const up = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
      handle.removeEventListener("pointercancel", up);
      try { localStorage.setItem("p2pcli.editor.width", String(width)); } catch { /* 存不了就算了 */ }
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
    handle.addEventListener("pointercancel", up);
  };
  useEffect(() => {
    setActiveKey((key) => key !== null && !tabs.some((tab) => tab.key === key) ? tabs.at(-1)?.key ?? null : key);
  }, [tabs]);
  // 远程会话里的 AI 等你确认、干完活时提醒；正看着的那个会话不打扰
  useAgentAlerts(
    ov,
    (endpointId, sessionId) => !showSettings && !sessionTarget &&
      activeTab?.info.endpoint_id === endpointId && activeTab.sessionId === sessionId,
    (endpointId, sessionId) => {
      const tab = tabs.find((tab) => tab.info.endpoint_id === endpointId && tab.sessionId === sessionId && tab.state !== "closed");
      return tab ? () => { setShowSettings(false); setShowCollaboration(false); setSessionTarget(null); setActiveKey(tab.key); } : null;
    },
  );
  /** 标签、底部栏和侧边栏使用同一份实时快照，不再各自猜测当前状态。 */
  const tabAgent = (tab: Tab) => {
    const snapshot = liveSessions[tab.connId];
    if (!tab.sessionId || tab.state === "closed" || !snapshot?.sessions || snapshot.error) return null;
    return snapshot.sessions.find((s) => s.id === tab.sessionId)?.agent ?? null;
  };
  const activeAgent = activeTab?.state === "open" && !showSettings && !sessionTarget ? tabAgent(activeTab) : null;
  const hideComposer = (hidden: boolean) => {
    setComposerHidden(hidden);
    try { localStorage.setItem("p2pcli.composer.hidden", hidden ? "1" : "0"); } catch { /* 记不住也能用 */ }
  };
  /** 输入框发消息：当成粘贴整段发给终端里的 AI，等它收完再补回车 */
  const sendToAgent = (tabKey: number, text: string) => {
    const handle = termHandles.current.get(tabKey);
    if (!handle) return;
    handle.paste(text);
    window.setTimeout(() => handle.enter(), 80);
  };
  /** 正在切 AI 设置的标签和哪一项；切的时候按钮转圈，不让再点 */
  const [switching, setSwitching] = useState<{ tabKey: number; what: AgentSetting } | null>(null);
  /** 切模型、强度、权限模式，照用户自己在终端里的做法：/model、/effort 命令；权限模式按 Shift+Tab 轮换
   *  （Claude Code 只能这样切，ZCode 也支持，而且干活的时候也能切）。
   *  切完马上重新拉一次会话列表核对，不等下一轮刷新；到点没变就说一声，让用户看终端里的提示 */
  const switchAgent = async (tab: Tab, what: AgentSetting, id: string) => {
    const handle = termHandles.current.get(tab.key);
    const kind = tabAgent(tab)?.kind ?? "";
    if (!handle || !tab.sessionId || switching) return;
    const read = async () => {
      const sessions = await api.listTerminals(tab.connId);
      updateLiveSessions(tab.connId, sessions);
      return sessions.find((s) => s.id === tab.sessionId)?.agent?.settings ?? null;
    };
    /** 隔一会儿读一次，直到 ok 或者超时 */
    const until = async (ok: (s: AgentSettings | null) => boolean, ms: number) => {
      for (const started = Date.now(); Date.now() - started < ms;) {
        await new Promise((r) => window.setTimeout(r, 250));
        const s = await read().catch(() => null);
        if (ok(s)) return s;
      }
      return null;
    };
    setSwitching({ tabKey: tab.key, what });
    try {
      if (what === "mode") {
        const label = choiceLabel(modeChoices(kind), id);
        let mode = tabAgent(tab)?.settings?.mode ?? null;
        const start = mode;
        // 轮一圈最多五档；每按一下等它真变了再按下一下，回到起点说明这一档这次启动没开
        for (let i = 0; mode && mode !== id && i < 6; i++) {
          handle.keys("\x1b[Z");
          const before = mode;
          mode = (await until((s) => !!s?.mode && s.mode !== before, 2000))?.mode ?? null;
          if (mode === start) break;
        }
        if (mode !== id)
          toast.notice(t("切不到{{label}}", { label }), {
            warning: true,
            description: kind === "claude" && id === "bypassPermissions" ? t("启动 Claude Code 时没加 --dangerously-skip-permissions，这一档不在轮换里") : t("终端里看看现在是什么情况"),
          });
      } else {
        handle.paste(`/${what} ${id}`);
        window.setTimeout(() => handle.enter(), 80);
        // Claude Code 切完显示“Opus 5.5”，按别名开头对；ZCode 显示的就是“提供方/模型”
        const model = (m: string | null | undefined) => (kind === "claude" ? !!m?.toLowerCase().startsWith(id) : m?.toLowerCase() === id.toLowerCase());
        const done = (s: AgentSettings | null) => (what === "model" ? model(s?.model) : s?.effort === id);
        if (!(await until(done, 4000))) {
          const name = tabAgent(tab)?.name ?? "AI";
          toast.notice(what === "model" ? t("没切到 {{value}}", { value: choiceLabel(CLAUDE_MODELS, id)?.split("/").pop() }) : t("思考强度没切到{{value}}", { value: choiceLabel(claudeEfforts(), id) }), {
            warning: true,
            description: t("终端里看看 {{name}} 的提示，可能这个账号或模型不支持", { name }),
          });
          handle.focus();
        }
      }
    } finally {
      setSwitching(null);
    }
  };
  // ⌘L（Windows 是 Ctrl+Shift+L）在终端和输入框之间切换；收起来时按它重新打开。
  // 在捕获阶段处理，终端拿不到这个按键
  const composerKeyRef = useRef<(e: KeyboardEvent) => void>(() => {});
  composerKeyRef.current = (e: KeyboardEvent) => {
    const hit = e.key.toLowerCase() === "l" && !e.altKey && (isMac ? e.metaKey && !e.ctrlKey && !e.shiftKey : e.ctrlKey && e.shiftKey && !e.metaKey);
    if (!hit || !activeAgent || !activeTab) return;
    e.preventDefault();
    e.stopPropagation();
    if (composerHidden) {
      hideComposer(false);
      window.requestAnimationFrame(() => composerRef.current?.focus());
    } else if (document.activeElement === composerRef.current) {
      termHandles.current.get(activeTab.key)?.focus();
    } else {
      composerRef.current?.focus();
    }
  };
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => composerKeyRef.current(e);
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);
  // One path snapshot per live connection, shared by the sidebar and status bar.
  const liveConnectionKey = [...new Set([
    ...tabs.filter((tab) => tab.state !== "closed").map((tab) => tab.connId),
    ...(sessionTarget ? [sessionTarget.connId] : []),
    // 本机那条是常驻的：侧边栏的“本机”随时要显示它的会话
    ...(ov?.local ? [ov.local.conn_id] : []),
  ])].sort((a, b) => a - b).join(",");
  const updateLiveSessions = useCallback((connId: number, sessions: TerminalSession[]) => {
    setLiveSessions((current) => ({
      ...current,
      [connId]: { sessions, error: current[connId]?.error ?? null },
    }));
  }, []);
  // 一条连接只轮询一次，底部栏、侧边栏和标签共同消费同一个快照。
  useEffect(() => {
    if (!liveConnectionKey) return;
    return pollConnections(
      liveConnectionKey.split(",").map(Number),
      (connId) => api.listTerminals(connId),
      (connId, sessions) => setLiveSessions((current) => ({ ...current, [connId]: { sessions, error: null } })),
      (connId, error) => setLiveSessions((current) => ({
        ...current,
        [connId]: { sessions: current[connId]?.sessions ?? null, error: String(error) },
      })),
    );
  }, [liveConnectionKey, sessionsRev]);
  useEffect(() => {
    if (!liveConnectionKey) return;
    return pollConnections(
      liveConnectionKey.split(",").map(Number),
      (connId) => api.pathInfo(connId),
      (connId, path) => setPaths((current) => ({ ...current, [connId]: path })),
      () => { /* Keep the last sample until a refresh or disconnect. */ },
    );
  }, [liveConnectionKey]);

  const addTab = (connId: number, info: RemoteInfo, code: string | null, sessionId?: string, keepConnection = false) => {
    if (sessionTarget && sessionTarget.connId !== connId && !tabs.some((tab) => tab.connId === sessionTarget.connId))
      void api.disconnect(sessionTarget.connId).catch((e) => toast.error(String(e)));
    const existing = sessionId ? tabs.find((tab) => tab.info.endpoint_id === info.endpoint_id && tab.sessionId === sessionId && (tab.state === "open" || tab.state === "opening")) : null;
    if (existing) {
      if (!keepConnection && existing.connId !== connId && !tabs.some((tab) => tab.connId === connId))
        void api.disconnect(connId).catch((e) => toast.error(String(e)));
      setActiveKey(existing.key); setSessionTarget(null); setShowSettings(false); return;
    }
    // A restored session gets a fresh view; remove the stale disconnected view of that same session.
    setSessionTarget(null);
    setShowSettings(false);
    const key = tabSeq.current++;
    setTabs((ts) => [
      ...ts.filter((tab) => !sessionId || tab.info.endpoint_id !== info.endpoint_id || tab.sessionId !== sessionId),
      { key, connId, info, code, state: "opening", note: null, sessionId },
    ]);
    setActiveKey(key);
  };
  const openCollaborationDrafts = (connId: number, info: RemoteInfo, code: string | null, drafts: CollaborationDraft[]) => {
    const needsConnection = tabs.some((tab) => tab.connId === connId) || drafts.some(({ endpoint }) =>
      !tabs.some((tab) => tab.info.endpoint_id === endpoint.deviceId && tab.sessionId === endpoint.terminalId &&
        (tab.state === "open" || tab.state === "opening")));
    // Reusing one tab must not disconnect a connection needed by another tab in this batch.
    for (const draft of drafts) addTab(connId, info, code, draft.endpoint.terminalId, true);
    setShowCollaboration(false);
    for (const draft of drafts) pendingCollaborationDrafts.current.delete(endpointKey(draft.endpoint));
    const signal = onboardingController.current.signal;
    toast.notice(t("正在接入 {{length}} 个会话", { length: drafts.length }), { description: t("等待会话登记确认；需要授权时会在目标会话中提示。") });
    const requests = drafts.map((draft) => {
      const key = `${draft.endpoint.deviceId}:${draft.requestId}`;
      const record = () => {
        if (!draft.source || signal.aborted) return;
        try {
          rememberRelation({ ownerId: draft.ownerId, coordinatorId: draft.coordinatorId, projectId: draft.projectId },
            { source: draft.source, destination: draft.endpoint });
        } catch { toast.error(t("会话已接入，但连线记录未能保存")); }
      };
      const existing = onboardingInFlight.current.get(key);
      if (existing) return existing.then(record);
      const request = connectCollaborationSession(draft, {
        signal,
        read: async () => {
          const target = draft.coordinatorId === draft.ownerId ? null : draft.coordinatorId;
          const [sessions, members] = await Promise.all([
            api.collabRequest(target, draft.projectId, { type: "sessions_list" }),
            api.collabRequest(target, draft.projectId, { type: "members_list" }),
          ]);
          return {
            sessions: expectCollaboration(sessions, "sessions").sessions,
            members: expectCollaboration(members, "members").members,
          };
        },
        send: () => {
          if (!draft.endpoint.agentKind) throw new Error(t("未识别目标 AI，未发送接入请求"));
          return api.sendAgentMessage(connId, draft.endpoint.terminalId!, draft.endpoint.agentKind, draft.requestId, draft.prompt);
        },
        wait: () => new Promise<void>((resolve) => window.setTimeout(resolve, 2000)),
      }).then(record).finally(() => onboardingInFlight.current.delete(key));
      onboardingInFlight.current.set(key, request);
      return request;
    });
    void Promise.allSettled(requests).then((results) => {
      if (signal.aborted) return;
      const failed = results.flatMap((result, index) => result.status === "rejected" ?
        [`${drafts[index].endpoint.sessionName ?? t("会话")}：${String(result.reason)}`] : []);
      if (!failed.length) toast.success(t("{{length}} 个会话已接入协作", { length: results.length }));
      else toast.notice(t("已确认接入 {{value}}/{{length}} 个会话", { value: results.length - failed.length, length: results.length }), {
        warning: true, description: failed.join("\n"),
        action: { label: t("查看协作"), onPress: () => setShowCollaboration(true) },
      });
      refresh();
    }).finally(() => {
      if (!needsConnection) void api.disconnect(connId).catch((e) => toast.error(String(e)));
    });
  };

  /** 连接后执行用户指定的会话操作；清除操作已在会话菜单中确认。 */
  const pendingOpen = useRef<{
    endpointId: string;
    sessionId: string | null;
    clear?: boolean;
  } | null>(null);

  const onConnected = (
    connId: number,
    info: RemoteInfo,
    code: string | null,
    path: PathInfo,
  ) => {
    setPaths((m) => ({ ...m, [connId]: path }));
    const pending = pendingOpen.current;
    pendingOpen.current = null;
    if (window.__ptTest) addTab(connId, info, code);
    else if (pending && !pending.clear && pending.endpointId === info.endpoint_id) {
      const drafts = [...pendingCollaborationDrafts.current.values()].filter((draft) => draft.endpoint.deviceId === info.endpoint_id);
      if (drafts.length) openCollaborationDrafts(connId, info, code, drafts);
      else addTab(connId, info, code, pending.sessionId ?? undefined);
    } else {
      if (sessionTarget && sessionTarget.connId !== connId && !tabs.some((tab) => tab.connId === sessionTarget.connId))
        void api.disconnect(sessionTarget.connId).catch((e) => toast.error(String(e)));
      setSessionTarget({ connId, info, code });
      if (pending?.clear) {
        if (pending.endpointId !== info.endpoint_id || !pending.sessionId) {
          toast.error(t("连接的设备与待清除会话不一致，未执行清除"));
        } else {
          const sessionId = pending.sessionId;
          void api.endTerminal(connId, sessionId).then(() => {
            sessionEnded(info.endpoint_id, sessionId);
            refresh();
            toast.success(t("会话已清除"));
          }).catch((error) => toast.error(t("清除会话失败：{{error}}", { error: String(error) })));
        }
      }
    }
    setShowSettings(false);
    refresh();
    toast.success(t("已连接 {{name}}", { name: info.name }));
  };

  const connection = useConnection({ onConnected, onOpenChange: setShowConnect });
  const sidebar = useSidebar();
  const updater = useUpdater();
  const scanNearby = async (reveal = false) => {
    if (reveal) {
      sidebar.show();
      setNearbyReveal((value) => value + 1);
    }
    if (scanPending.current) return;
    scanPending.current = true;
    setScanningNearby(true);
    try {
      const nearby = await api.scanNearby();
      setOv((current) => current ? { ...current, nearby } : current);
    } catch (error) {
      toast.error(t("扫描失败：{{error}}", { error: String(error) }));
    } finally {
      scanPending.current = false;
      setScanningNearby(false);
    }
  };

  /** 某台设备的会话被结束了：关掉对应的标签，各处会话列表重读 */
  const sessionEnded = (endpointId: string, id: string) => {
    const cleared = clearedSessions.current.get(endpointId) ?? new Set<string>();
    cleared.add(id);
    clearedSessions.current.set(endpointId, cleared);
    setOv((current) => current && withoutClearedSessions(current, clearedSessions.current));
    setSessionsRev((v) => v + 1);
    setTabs((current) => current.filter((tab) => tab.info.endpoint_id !== endpointId || tab.sessionId !== id));
    void api.refreshDevices().catch(() => { /* 下轮常规探测会继续同步缓存。 */ });
  };

  const closeTab = (key: number, discardEdits = false) => {
    const tab = tabs.find((tab) => tab.key === key);
    const rest = tabs.filter((tab) => tab.key !== key);
    // 这台设备最后一个标签关掉以后，它的编辑器也跟着关；有没保存的文件先问
    const endpointId = tab?.info.endpoint_id;
    if (endpointId && editors[endpointId] && !rest.some((other) => other.info.endpoint_id === endpointId)) {
      const dirty = editorDirty[endpointId] ?? 0;
      if (dirty && !discardEdits) {
        setDiscardEditor({ count: dirty, onConfirm: () => closeTab(key, true) });
        return;
      }
      closeEditor(endpointId);
    }
    if (tab && !rest.some((tab) => tab.connId === tab.connId))
      api.disconnect(tab.connId).catch((e) => toast.error(String(e)));
    setTabs(rest);
    if (activeKey === key)
      setActiveKey(rest.length ? rest[rest.length - 1].key : null);
  };

  const setTabState = (key: number, status: TermStatus) => {
    setTabs((ts) =>
      ts.map((tab) => {
        if (tab.key !== key || tab.state === "closed") return tab;
        if (status.type === "exited")
          return {
            ...tab,
            state: "exited",
            note: status.code != null
              ? t("终端视图已结束（退出码 {{code}}）", { code: status.code })
              : t("终端视图已结束"),
          };
        return { ...tab, state: "closed", note: status.reason ?? t("已脱离会话") };
      }),
    );
  };

  /** 拖进来的文件发给哪台设备：正在看的终端，或者正在看的会话页；终端里的还要在发完后把路径填进去，
   *  拖到 AI 输入框上的填进输入框 */
  const dropTarget = (at?: { x: number; y: number }): { info: RemoteInfo; termId?: number; draftKey?: number } | { blocked: string } => {
    if (showSettings) return { blocked: t("先回到终端，再把文件拖进来") };
    if (at && document.elementFromPoint(at.x, at.y)?.closest(".collab-drawer, .collab-management-dialog, .collab-link-dialog"))
      return { blocked: t("请把文件拖到目标终端") };
    if (sessionTarget) return { info: sessionTarget.info };
    if (activeTab && (activeTab.state === "open" || activeTab.state === "opening")) {
      const box = composerRef.current?.closest(".agent-composer")?.getBoundingClientRect();
      // 拖放事件给的是物理像素，换成网页的坐标再比
      const x = (at?.x ?? -1) / window.devicePixelRatio, y = (at?.y ?? -1) / window.devicePixelRatio;
      if (!composerHidden && box && x >= box.left && x <= box.right && y >= box.top && y <= box.bottom) return { info: activeTab.info, draftKey: activeTab.key };
      return { info: activeTab.info, termId: activeTab.termId };
    }
    return { blocked: t("先打开一台设备的终端，再把文件拖进来") };
  };
  const dropTargetRef = useRef(dropTarget);
  dropTargetRef.current = dropTarget;
  /** 拖进终端或输入框的文件：传输编号 → 发完要填路径的终端或输入框 */
  const pendingInsert = useRef(new Map<string, { termId?: number; draftKey?: number; platform: string }>());

  const uploadToComposer = async () => {
    const target = activeTab;
    if (!target || target.state !== "open") return;
    try {
      const paths = await api.pickPaths(false);
      if (!paths.length) return;
      const id = await api.sendFiles(target.info.endpoint_id, paths);
      pendingInsert.current.set(id, { draftKey: target.key, platform: target.info.platform });
      // 小文件可能在登记回填目标前就传完，主动读一次确保路径能进入草稿。
      void api.transfers().then(setTransfers).catch(() => {});
    } catch (error) {
      toast.error(t("上传失败：{{error}}", { error: String(error) }));
    }
  };

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void getCurrentWebview().onDragDropEvent(async (event) => {
      const e = event.payload;
      if (e.type === "leave") return setDropHint(null);
      const target = dropTargetRef.current("position" in e ? e.position : undefined);
      if (e.type === "enter" || e.type === "over") {
        return setDropHint("blocked" in target ? { blocked: target.blocked } : { name: target.info.name });
      }
      setDropHint(null);
      if ("blocked" in target) return toast.error(target.blocked);
      if (!e.paths.length) return;
      try {
        const id = await api.sendFiles(target.info.endpoint_id, e.paths);
        if (target.termId != null || target.draftKey != null)
          pendingInsert.current.set(id, { termId: target.termId, draftKey: target.draftKey, platform: target.info.platform });
      } catch (error) {
        toast.error(t("发不了：{{error}}", { error: String(error) }));
      }
    }).then((u) => { if (disposed) u(); else unlisten = u; }).catch(() => {});
    return () => { disposed = true; unlisten?.(); };
  }, []);

  // 拖进终端的文件发完了：把对方电脑上的路径填进那个终端，像本机拖文件进终端一样
  useEffect(() => {
    for (const [id, target] of pendingInsert.current) {
      const tab = transfers.find((x) => x.id === id);
      if (!tab || tab.finished_at == null) continue;
      pendingInsert.current.delete(id);
      if (tab.state !== "done" || !tab.paths.length) continue;
      const quoted = tab.paths.map((p) => shellQuote(p, target.platform)).join(" ") + " ";
      if (target.draftKey != null) {
        const key = target.draftKey;
        setDrafts((d) => ({ ...d, [key]: (d[key] ? d[key].replace(/\s*$/, " ") : "") + quoted }));
        // 传的时候切去别的标签了就不抢焦点，路径照样留在原来那个标签的草稿里
        if (activeTab?.key === key && !composerHidden && activeAgent) composerRef.current?.focus();
      } else if (target.termId != null) {
        void api.termInput(target.termId, quoted).catch(() => {});
      }
    }
  }, [transfers]);

  const respond = useCallback(
    async (req: IncomingRequest, allow: boolean, trust: boolean) => {
      await api.respond(req.id, allow, trust);
      setRequests((r) => r.filter((x) => x.id !== req.id));
      refresh();
    },
    [refresh],
  );
  if (window.__ptTest) window.__ptTest.respond = respond;

  const collaborationTerminals: CollaborationEndpoint[] = [];
  const deviceIds = new Set([
    ...(ov?.devices.map((device) => device.endpoint_id) ?? []), ...(ov?.endpoint_id ? [ov.endpoint_id] : []),
    ...tabs.map((tab) => tab.info.endpoint_id), ...(sessionTarget ? [sessionTarget.info.endpoint_id] : []),
  ]);
  for (const deviceId of deviceIds) {
    const target = deviceId === ov?.endpoint_id && ov.local ? { connId: ov.local.conn_id } :
      sessionTarget?.info.endpoint_id === deviceId ? sessionTarget :
      tabs.find((tab) => tab.info.endpoint_id === deviceId && tab.state !== "closed");
    const snapshot = target ? liveSessions[target.connId] : undefined;
    const device = ov?.devices.find((device) => device.endpoint_id === deviceId);
    const sessions = snapshot?.sessions && !snapshot.error ? snapshot.sessions :
      !target && device?.sessions_live && device.state === "online" ? device.sessions : [];
    const titles = sessionTitles(sessions);
    for (const session of sessions) {
      if (!session.agent) continue;
      collaborationTerminals.push({
        deviceId, deviceName: ov?.remarks[deviceId]?.device || (deviceId === ov?.endpoint_id ? ov.name : ov?.recent.find((device) => device.endpoint_id === deviceId)?.name ?? deviceId.slice(0, 8)),
        terminalId: session.id, sessionName: ov?.remarks[deviceId]?.sessions[session.id] || titles.get(session.id) || session.agent.name,
        agentKind: session.agent.kind,
      });
    }
  }
  const prepareCollaboration = (drafts: CollaborationDraft[]) => {
    const endpoint = drafts[0]?.endpoint;
    if (!endpoint || drafts.some((draft) => !draft.endpoint.terminalId)) throw new Error(t("请先选择目标 AI 会话"));
    if (drafts.some((draft) => draft.endpoint.deviceId !== endpoint.deviceId)) throw new Error(t("接入会话必须属于同一台设备"));
    if (connection.busy) throw new Error(t("设备正在连接，请稍后重试"));
    const local = ov?.local;
    const target = endpoint.deviceId === ov?.endpoint_id && local ? { connId: local.conn_id, info: local.info, code: null } :
      sessionTarget?.info.endpoint_id === endpoint.deviceId ? sessionTarget :
      tabs.find((tab) => tab.info.endpoint_id === endpoint.deviceId && tab.state !== "closed");
    const nearby = ov?.nearby.find((device) => device.endpoint_id === endpoint.deviceId && device.online);
    const recent = ov?.recent.find((device) => device.endpoint_id === endpoint.deviceId);
    if (!target && !nearby && !recent) throw new Error(t("此设备还未连接，请先连接设备"));
    pendingCollaborationDrafts.current = new Map(drafts.map((draft) => [endpointKey(draft.endpoint), draft]));
    pendingOpen.current = null;
    if (target) {
      openCollaborationDrafts(target.connId, target.info, target.code, drafts);
    }
    else {
      pendingOpen.current = { endpointId: endpoint.deviceId, sessionId: endpoint.terminalId! };
      if (nearby) void connection.connectLan(nearby);
      else if (recent) void connection.doConnect(recent.code, "");
    }
  };

  return (
    <RemarksProvider snapshot={ov?.remarks}>
    <CollaborationProvider overview={ov} terminals={collaborationTerminals} transfers={transfers} open={showCollaboration}
      onOpenChange={setShowCollaboration} onPrepare={prepareCollaboration}>
    <CollaborationPorts>
    <div className="app-shell">
      <UpdateIsland updater={updater} />
      <Titlebar
        ov={ov}
        onKick={(session) => void api.kick(session.session).then(refresh).catch((e) => toast.error(String(e)))}
        onRefresh={refresh}
        localDeviceOpen={showLocalDevice}
        onLocalDeviceOpenChange={setShowLocalDevice}
        terminalCount={tabs.length}
        onDiag={() => setShowDiag(true)}
        onSettings={() => { setShowCollaboration(false); setShowSettings(true); }}
        settingsEnabled={!!ov}
        sidebarOpen={sidebar.open}
        onBack={showSettings ? () => setShowSettings(false) : undefined}
        onToggleSidebar={sidebar.toggle}
        connectionAction={
          <>
            <Button
              variant={showCollaboration ? "secondary" : "ghost"}
              size="icon-sm"
              aria-label={t("协作")}
              title={t("跨设备协作")}
              onPress={() => { setShowSettings(false); setShowCollaboration((value) => !value); }}
            >
              <Network />
            </Button>
            <ConnectionPopover
              connection={connection}
              open={showConnect}
              onOpenChange={(open) => {
                setShowConnect(open);
                // 关掉面板就回到输设备码，下次从加号打开不会还停在局域网设备上
                if (!open) {
                  connection.setLanTarget(null);
                  // 没连上就关了面板：之前点的会话不再自动打开
                  if (!connection.busy) { pendingOpen.current = null; pendingCollaborationDrafts.current.clear(); }
                }
              }}
            />
          </>
        }
      />
      {!showSettings && <ConnectionSidebar
        ov={ov}
        liveSessions={liveSessions}
        onSessionEnded={sessionEnded}
        onSessionsChanged={() => setSessionsRev((v) => v + 1)}
        onSessionsLocalChange={updateLiveSessions}
        onPendingOpen={(endpointId, sessionId) => {
          pendingCollaborationDrafts.current.clear();
          pendingOpen.current = { endpointId, sessionId };
        }}
        onPendingClear={(endpointId, sessionId) => {
          pendingCollaborationDrafts.current.clear();
          pendingOpen.current = { endpointId, sessionId, clear: true };
        }}
        busy={connection.busy}
        open={sidebar.open}
        scanning={scanningNearby}
        nearbyReveal={nearbyReveal}
        onScanNearby={() => void scanNearby()}
        tabs={tabs}
        paths={paths}
        activeKey={activeKey}
        sessionTarget={sessionTarget}
        onOpenSession={(target, id) => addTab(target.connId, target.info, target.code, id)}
        onShowSessions={(target) => {
          if (sessionTarget && sessionTarget.connId !== target.connId && !tabs.some((tab) => tab.connId === sessionTarget.connId))
            void api.disconnect(sessionTarget.connId).catch((e) => toast.error(String(e)));
          setSessionTarget(target);
          setShowSettings(false);
        }}
        onSelectTab={(key) => {
          if (sessionTarget && !tabs.some((tab) => tab.connId === sessionTarget.connId))
            void api.disconnect(sessionTarget.connId).catch((e) => toast.error(String(e)));
          setActiveKey(key); setSessionTarget(null); setShowSettings(false);
        }}
        onConnect={(code) => void connection.doConnect(code, connection.password)}
        onConnectLan={(device) => void connection.connectLan(device)}
        onRefresh={refresh}
      />}
      <main className={cn("workspace", showSettings && "workspace-full-width", showCollaboration && !showSettings && "has-collaboration-details")}>
        {overviewError && (
          <div className="workspace-notice">
            <Alert variant="destructive">
              <AlertTitle>{t("无法读取设备信息")}</AlertTitle>
              <AlertDescription>
                {overviewError}
                <Button variant="outline" size="sm" onClick={refresh}>
                  {t("重试")}
                </Button>
              </AlertDescription>
            </Alert>
          </div>
        )}
        {showSettings && ov && (
          <SettingsPage ov={ov} onChanged={refresh} updater={updater} />
        )}
        {!showSettings && <CollaborationDrawer />}
        {!showSettings && sessionTarget && (
          <RemoteSessions
            key={sessionTarget.connId}
            target={sessionTarget}
            snapshot={liveSessions[sessionTarget.connId]}
            onRefresh={() => setSessionsRev((rev) => rev + 1)}
            onChangeSessions={(sessions) => updateLiveSessions(sessionTarget.connId, sessions)}
            openSessions={new Set(tabs
              .filter((tab) => tab.info.endpoint_id === sessionTarget.info.endpoint_id && (tab.state === "open" || tab.state === "opening") && tab.sessionId)
              .map((tab) => tab.sessionId!))}
            onOpen={(id) => addTab(sessionTarget.connId, sessionTarget.info, sessionTarget.code, id)}
            onBack={() => {
              if (!tabs.some((tab) => tab.connId === sessionTarget.connId))
                void api.disconnect(sessionTarget.connId).catch((e) => toast.error(String(e)));
              setSessionTarget(null);
            }}
            onEnded={(id) => sessionEnded(sessionTarget.info.endpoint_id, id)}
          />
        )}
        <div className="workspace-content" hidden={showSettings || !!sessionTarget}>
          <Tabs
            selectedKey={activeKey === null ? "home" : String(activeKey)}
            onSelectionChange={(key) => setActiveKey(key === "home" ? null : Number(key))}
            className="terminal-workspace"
          >
            <div className="terminal-toolbar">
              <Tabs.ListContainer className="terminal-list-container">
                <Tabs.List
                  aria-label={t("工作区标签")}
                  className="terminal-tab-list"
                >
                  <Tabs.Tab id="home" aria-label={t("首页")} className="terminal-home-tab">
                    <House aria-hidden="true" />
                  </Tabs.Tab>
                  {tabs.map((tab) => (
                    <Tabs.Tab
                      id={String(tab.key)}
                      aria-label={tab.info.name}
                      className="terminal-tab-trigger"
                      key={tab.key}
                      render={(domProps) => (
                        <div
                          className={cn(
                            "terminal-tab",
                            tab.key === activeKey && "is-active",
                          )}
                        >
                          <div {...(domProps as ComponentProps<"div">)} />
                          <Button
                            variant="ghost"
                            size="icon-sm"
                            aria-label={t("关闭终端 {{name}} #{{key}}", { name: tab.info.name, key: tab.key })}
                            title={t("关闭标签，保留后台会话")}
                            onPress={() => closeTab(tab.key)}
                          >
                            <X />
                          </Button>
                        </div>
                      )}
                    >
                      <span className="terminal-tab-label">
                        <span
                          className="status-dot"
                          data-kind={
                            tab.state === "open" || tab.state === "opening"
                              ? (paths[tab.connId]?.kind ?? "unknown")
                              : "off"
                          }
                        />
                        <AgentTabTitle name={tab.info.name} agent={tabAgent(tab)} />
                      </span>
                    </Tabs.Tab>
                  ))}
                </Tabs.List>
              </Tabs.ListContainer>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  className="terminal-new-tab"
                  aria-label={activeTab && activeTab.state !== "closed" ? t("新建终端") : t("新建连接")}
                  title={activeTab && activeTab.state !== "closed" ? t("在当前设备上新建终端") : t("新建连接")}
                  onClick={() => activeTab && activeTab.state !== "closed"
                    ? addTab(activeTab.connId, activeTab.info, activeTab.code)
                    : setShowConnect(true)}
                >
                  <Plus />
                </Button>
              {activeTab && activeTab.state !== "closed" && (
                <Button variant="ghost" size="icon-sm" aria-label={t("远程会话")} title={t("查看和恢复远程会话")} onClick={() => setSessionTarget(activeTab)}>
                  <History />
                </Button>
              )}
              {activeTab && (activeTab.state === "open" || editors[activeTab.info.endpoint_id]) && (
                <Button variant="ghost" size="icon-sm" className="terminal-editor-toggle"
                  data-active={editors[activeTab.info.endpoint_id]?.shown || undefined}
                  aria-label={t("代码编辑器")}
                  title={editors[activeTab.info.endpoint_id]?.shown ? t("收起代码编辑器（打开的文件会保留）") : t("在代码编辑器里打开这台设备上的文件")}
                  onClick={() => {
                    const id = activeTab.info.endpoint_id;
                    if (editors[id]?.shown) setEditors((all) => ({ ...all, [id]: { ...all[id], shown: false } }));
                    else openEditor(activeTab);
                  }}>
                  <FileCode2 />
                </Button>
              )}
            </div>
            <div className="workspace-split">
            <div className="terminals">
              <Tabs.Panel id="home" className="terminal-panel terminal-home-panel">
                <Welcome
                  ov={ov}
                  onLocalDevice={() => setShowLocalDevice(true)}
                  onScanNearby={() => void scanNearby(true)}
                  scanning={scanningNearby}
                  onSettings={() => setShowSettings(true)}
                  onConnect={() => setShowConnect(true)}
                />
              </Tabs.Panel>
              {tabs.map((tab) => (
                <Tabs.Panel
                  shouldForceMount
                  key={tab.key}
                  id={String(tab.key)}
                  className="terminal-panel"
                  hidden={tab.key !== activeKey}
                >
                  <TerminalView
                    connId={tab.connId}
                    sessionId={tab.sessionId}
                    tmuxWheel={tab.info.platform !== "windows"}
                    active={!showSettings && !sessionTarget && tab.key === activeKey}
                    onReady={(termId, sessionId) =>
                      setTabs((ts) =>
                        ts.map((x) =>
                          x.key === tab.key && x.state === "opening"
                            ? { ...x, state: "open", sessionId, termId }
                            : x,
                        ),
                      )
                    }
                    onStatus={(s) => setTabState(tab.key, s)}
                    onPromptReady={(ready) => setPromptReady((current) => current[tab.key] === ready ? current : { ...current, [tab.key]: ready })}
                    onHandle={(handle) => {
                      if (handle) termHandles.current.set(tab.key, handle);
                      else termHandles.current.delete(tab.key);
                    }}
                  />
                  {tab.state === "open" && tab.sessionId && (
                    <AgentLauncher key={`${tab.connId}:${tab.sessionId}`} connId={tab.connId} sessionId={tab.sessionId}
                      active={!showSettings && !sessionTarget && tab.key === activeKey}
                      visible={!showSettings && !sessionTarget && tab.key === activeKey && !!promptReady[tab.key]
                        && !liveSessions[tab.connId]?.error && !tabAgent(tab)
                        && liveSessions[tab.connId]?.sessions?.find((s) => s.id === tab.sessionId)?.details?.shell_ready === true}
                      canLaunch={() => termHandles.current.get(tab.key)?.canLaunch() ?? false}
                      onFocus={() => termHandles.current.get(tab.key)?.focus()}
                      onLaunched={() => {
                        setPromptReady((current) => ({ ...current, [tab.key]: false }));
                        setSessionsRev((rev) => rev + 1);
                      }} />
                  )}
                  {tab.state === "opening" && (
                    <div className="terminal-loading" role="status">
                      <LoaderCircle className="animate-spin" />
                      {t("正在打开终端…")}
                    </div>
                  )}
                  {(tab.state === "closed" || tab.state === "exited") && (
                    <div className="term-overlay">
                      <Card className="max-w-md">
                        <CardHeader>
                          <CardTitle>
                            {tab.state === "exited" ? t("已脱离终端") : t("连接已断开")}
                          </CardTitle>
                        </CardHeader>
                        <CardContent>
                          <p className="text-muted-foreground wrap-break-word">
                            {tab.note}
                            <span className="mt-2 block text-xs">{t("关闭标签不会结束后台会话。会话列表中可恢复仍在运行的会话。")}</span>
                          </p>
                        </CardContent>
                        <CardFooter className="justify-center gap-2">
                          <Button
                            onClick={() => tab.state === "closed"
                              ? void connection.doConnect(tab.code ?? tab.info.endpoint_id, "")
                              : setSessionTarget(tab)}
                          >
                            {t("恢复会话")}
                          </Button>
                          {tab.state === "exited" && (
                            <Button
                              onClick={() => addTab(tab.connId, tab.info, tab.code)}
                            >
                              <Plus data-icon="inline-start" />
                              {t("新建终端")}
                            </Button>
                          )}
                          <Button
                            variant="outline"
                            onClick={() => closeTab(tab.key)}
                          >
                            {t("关闭标签")}
                          </Button>
                        </CardFooter>
                      </Card>
                    </div>
                  )}
                </Tabs.Panel>
              ))}
            </div>
            {Object.entries(editors).map(([endpointId, editor]) => {
              const visible = editor.shown && activeTab?.info.endpoint_id === endpointId;
              return (
                <Fragment key={endpointId}>
                  {visible && <div className="workspace-splitter" role="separator" aria-orientation="vertical"
                    aria-label={t("拖动调整编辑器宽度")} onPointerDown={dragEditorSplit} />}
                  <div className="remote-editor-pane" hidden={!visible} style={{ flexBasis: `${editorWidth * 100}%` }}>
                    <Suspense fallback={<div className="remote-editor-placeholder"><LoaderCircle className="animate-spin" />{t("正在加载编辑器…")}</div>}>
                      <RemoteEditor
                        connId={editorConnId(endpointId)}
                        root={editor.root}
                        rootSeq={editor.seq}
                        active={visible && !showSettings && !sessionTarget}
                        onDirty={(count) => setEditorDirty((all) => all[endpointId] === count ? all : { ...all, [endpointId]: count })}
                        onClose={() => closeEditor(endpointId)}
                      />
                    </Suspense>
                  </div>
                </Fragment>
              );
            })}
            </div>
            {activeTab && activeAgent && (
              <AgentComposer
                key={`composer-${activeTab.key}`}
                agent={activeAgent}
                expanded={!composerHidden}
                draft={drafts[activeTab.key] ?? ""}
                onDraft={(text) => setDrafts((d) => ({ ...d, [activeTab.key]: text }))}
                onSend={(text) => sendToAgent(activeTab.key, text)}
                onTerminal={() => termHandles.current.get(activeTab.key)?.focus()}
                onToggle={() => {
                  hideComposer(!composerHidden);
                  if (composerHidden) window.requestAnimationFrame(() => composerRef.current?.focus());
                  else termHandles.current.get(activeTab.key)?.focus();
                }}
                onUpload={uploadToComposer}
                onChoose={async (approvalId, optionId) => {
                  if (!activeTab.sessionId || activeTab.state !== "open") throw new Error(t("会话已断开"));
                  try { await api.chooseAgentOption(activeTab.connId, activeTab.sessionId, approvalId, optionId); }
                  finally { setSessionsRev((rev) => rev + 1); }
                }}
                inputRef={composerRef}
                tools={
                  <AgentSettingsBar
                    agent={activeAgent}
                    switching={switching?.tabKey === activeTab.key ? switching.what : null}
                    onSwitch={(what, id) => void switchAgent(activeTab, what, id)}
                    onPicker={(command) => {
                      sendToAgent(activeTab.key, command);
                      termHandles.current.get(activeTab.key)?.focus();
                    }}
                    onPalette={(query) => {
                      const key = activeTab.key;
                      const handle = termHandles.current.get(key);
                      if (!handle || tabAgent(activeTab)?.state !== "idle") return;
                      handle.keys("\x10"); // OpenCode 默认 Ctrl+P 打开命令面板
                      handle.focus();
                      window.setTimeout(() => {
                        if (termHandles.current.get(key) !== handle) return;
                        // 只过滤权限命令，不发送回车，避免直接开启自动批准。
                        handle.paste(query);
                      }, 120);
                    }}
                  />
                }
              />
            )}
            {activeTab && <TerminalStatusbar
              key={activeTab.key}
              connId={activeTab.connId}
              info={activeTab.info}
              echoLatency={() => termHandles.current.get(activeTab.key)?.echoLatency() ?? null}
              sessionId={activeTab.sessionId}
              state={activeTab.state}
              path={paths[activeTab.connId]}
              snapshot={liveSessions[activeTab.connId]}
              onOpenEditor={openEditorAt}
            />}
          </Tabs>

        </div>
        <TransferPanel transfers={transfers} />
      </main>
      {discardEditor && (
        <AlertDialog open>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>{t("有 {{count}} 个文件未保存", { count: discardEditor.count })}</AlertDialogTitle>
              <AlertDialogDescription>{t("这是这台设备的最后一个标签，关掉以后代码编辑器也会关闭，未保存的修改会丢失。")}</AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <Button variant="outline" onPress={() => setDiscardEditor(null)}>{t("取消")}</Button>
              <Button variant="destructive" onPress={() => { const next = discardEditor; setDiscardEditor(null); next.onConfirm(); }}>{t("不保存，关闭标签")}</Button>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      )}
      {showDiag && <DiagDialog publicKey={ov?.endpoint_id ?? null} onClose={() => setShowDiag(false)} />}
      {requests.length > 0 && (
        <ApproveDialog
          key={requests[0].id}
          req={requests[0]}
          onRespond={respond}
        />
      )}
      {requests.length === 0 && fileRequests.length > 0 && (
        <FileApproveDialog
          key={fileRequests[0].id}
          req={fileRequests[0]}
          inboxDir={ov?.inbox_dir ?? null}
          onRespond={async (req, accept) => {
            await api.respondFiles(req.id, accept);
            setFileRequests((r) => r.filter((x) => x.id !== req.id));
          }}
        />
      )}
      {dropHint && (
        <div className="drop-overlay" data-blocked={"blocked" in dropHint ? "" : undefined} aria-hidden="true">
          <div>
            <FileUp />
            {"blocked" in dropHint ? (
              <span className="text-sm">{dropHint.blocked}</span>
            ) : (
              <>
                <strong className="text-sm">{t("松开发给")} {dropHint.name}</strong>
                <span className="text-xs text-muted-foreground">{t("文件放在对方的下载文件夹里，拖进终端的会把路径填进去")}</span>
              </>
            )}
          </div>
        </div>
      )}
      <Toaster />
      <GlobalContextMenu />
    </div>
    </CollaborationPorts>
    </CollaborationProvider>
    </RemarksProvider>
  );
}
