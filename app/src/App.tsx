import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ComponentProps,
} from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import {
  FileUp,
  LoaderCircle,
  History,
  House,
  Plus,
  Radio,
  Unplug,
  X,
} from "lucide-react";
import { Tabs } from "@heroui/react";
import { toast, Toaster } from "@/components/ui/toast";
import {
  api,
  platformLabel,
  shellQuote,
  type FileRequest,
  type HostEvent,
  type IncomingRequest,
  type Overview,
  type PathInfo,
  type RemoteInfo,
  type TerminalSession,
  type TerminalSessionSnapshot,
  type TermStatus,
  type Transfer,
} from "./api";
import { TerminalView, type TerminalHandle } from "./TerminalView";
import { pollConnections } from "./lib/pollConnections";
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
import { AgentComposer, isMac } from "./components/AgentComposer";
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

/** 拖文件进窗口时的提示：发给谁，或者为什么发不了 */
type DropHint = { name: string } | { blocked: string };

export default function App() {
  const tabSeq = useRef(1);
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
  const termHandles = useRef(new Map<number, TerminalHandle>());
  const [sessionTarget, setSessionTarget] = useState<SessionTarget | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [showDiag, setShowDiag] = useState(false);
  /** 会话页里结束了会话就加一，各处共享的会话快照跟着在后台刷新 */
  const [sessionsRev, setSessionsRev] = useState(0);
  const [liveSessions, setLiveSessions] = useState<Record<number, TerminalSessionSnapshot>>({});
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
        setOv(o);
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
        if (ev.type === "files_received") toast.success(`收到 ${ev.from_name} 发来的 ${ev.files} 个文件`);
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
          ts.map((t) =>
            t.connId === conn_id ? { ...t, state: "closed", note: reason } : t,
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

  const activeTab = tabs.find((t) => t.key === activeKey) ?? null;
  // 远程会话里的 AI 等你确认、干完活时提醒；正看着的那个会话不打扰
  useAgentAlerts(
    ov,
    (endpointId, sessionId) => !showSettings && !sessionTarget &&
      activeTab?.info.endpoint_id === endpointId && activeTab.sessionId === sessionId,
    (endpointId, sessionId) => {
      const tab = tabs.find((t) => t.info.endpoint_id === endpointId && t.sessionId === sessionId && t.state !== "closed");
      return tab ? () => { setShowSettings(false); setSessionTarget(null); setActiveKey(tab.key); } : null;
    },
  );
  /** 标签、底部栏和侧边栏使用同一份实时快照，不再各自猜测当前状态。 */
  const tabAgent = (t: Tab) => {
    const snapshot = liveSessions[t.connId];
    if (!t.sessionId || t.state === "closed" || !snapshot?.sessions || snapshot.error) return null;
    return snapshot.sessions.find((s) => s.id === t.sessionId)?.agent ?? null;
  };
  const activeAgent = activeTab?.state === "open" && !showSettings && !sessionTarget ? tabAgent(activeTab) : null;
  const showComposer = !!activeAgent && !composerHidden;
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

  const addTab = (connId: number, info: RemoteInfo, code: string | null, sessionId?: string) => {
    if (sessionTarget && sessionTarget.connId !== connId && !tabs.some((tab) => tab.connId === sessionTarget.connId))
      void api.disconnect(sessionTarget.connId).catch((e) => toast.error(String(e)));
    const existing = sessionId ? tabs.find((t) => t.info.endpoint_id === info.endpoint_id && t.sessionId === sessionId && (t.state === "open" || t.state === "opening")) : null;
    if (existing) {
      if (existing.connId !== connId && !tabs.some((t) => t.connId === connId))
        void api.disconnect(connId).catch((e) => toast.error(String(e)));
      setActiveKey(existing.key); setSessionTarget(null); setShowSettings(false); return;
    }
    // A restored session gets a fresh view; remove the stale disconnected view of that same session.
    setSessionTarget(null);
    setShowSettings(false);
    const key = tabSeq.current++;
    setTabs((ts) => [
      ...ts.filter((t) => !sessionId || t.info.endpoint_id !== info.endpoint_id || t.sessionId !== sessionId),
      { key, connId, info, code, state: "opening", note: null, sessionId },
    ]);
    setActiveKey(key);
  };

  /** 在没连上的设备上点了会话：连上以后直接打开它（null 是新建），不停在会话页 */
  const pendingOpen = useRef<{ endpointId: string; sessionId: string | null } | null>(null);

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
    else if (pending && pending.endpointId === info.endpoint_id) {
      addTab(connId, info, code, pending.sessionId ?? undefined);
    } else {
      if (sessionTarget && sessionTarget.connId !== connId && !tabs.some((t) => t.connId === sessionTarget.connId))
        void api.disconnect(sessionTarget.connId).catch((e) => toast.error(String(e)));
      setSessionTarget({ connId, info, code });
    }
    setShowSettings(false);
    refresh();
    toast.success("已连接 " + info.name);
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
      toast.error(`扫描失败：${String(error)}`);
    } finally {
      scanPending.current = false;
      setScanningNearby(false);
    }
  };

  /** 某台设备的会话被结束了：关掉对应的标签，各处会话列表重读 */
  const sessionEnded = (endpointId: string, id: string) => {
    setSessionsRev((v) => v + 1);
    const rest = tabs.filter((t) => t.info.endpoint_id !== endpointId || t.sessionId !== id);
    setTabs(rest);
    if (activeKey !== null && !rest.some((t) => t.key === activeKey)) setActiveKey(rest.at(-1)?.key ?? null);
  };

  const closeTab = (key: number) => {
    const tab = tabs.find((t) => t.key === key);
    const rest = tabs.filter((t) => t.key !== key);
    if (tab && !rest.some((t) => t.connId === tab.connId))
      api.disconnect(tab.connId).catch((e) => toast.error(String(e)));
    setTabs(rest);
    if (activeKey === key)
      setActiveKey(rest.length ? rest[rest.length - 1].key : null);
  };

  const setTabState = (key: number, status: TermStatus) => {
    setTabs((ts) =>
      ts.map((t) => {
        if (t.key !== key || t.state === "closed") return t;
        if (status.type === "exited")
          return {
            ...t,
            state: "exited",
            note:
              "终端视图已结束" +
              (status.code != null ? "（退出码 " + status.code + "）" : ""),
          };
        return { ...t, state: "closed", note: status.reason ?? "已脱离会话" };
      }),
    );
  };

  /** 拖进来的文件发给哪台设备：正在看的终端，或者正在看的会话页；终端里的还要在发完后把路径填进去，
   *  拖到 AI 输入框上的填进输入框 */
  const dropTarget = (at?: { x: number; y: number }): { info: RemoteInfo; termId?: number; draftKey?: number } | { blocked: string } => {
    if (showSettings) return { blocked: "先回到终端，再把文件拖进来" };
    if (sessionTarget) return { info: sessionTarget.info };
    if (activeTab && (activeTab.state === "open" || activeTab.state === "opening")) {
      const box = composerRef.current?.closest(".agent-composer")?.getBoundingClientRect();
      // 拖放事件给的是物理像素，换成网页的坐标再比
      const x = (at?.x ?? -1) / window.devicePixelRatio, y = (at?.y ?? -1) / window.devicePixelRatio;
      if (box && x >= box.left && x <= box.right && y >= box.top && y <= box.bottom) return { info: activeTab.info, draftKey: activeTab.key };
      return { info: activeTab.info, termId: activeTab.termId };
    }
    return { blocked: "先打开一台设备的终端，再把文件拖进来" };
  };
  const dropTargetRef = useRef(dropTarget);
  dropTargetRef.current = dropTarget;
  /** 拖进终端或输入框的文件：传输编号 → 发完要填路径的终端或输入框 */
  const pendingInsert = useRef(new Map<string, { termId?: number; draftKey?: number; platform: string }>());

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
        toast.error(`发不了：${String(error)}`);
      }
    }).then((u) => { if (disposed) u(); else unlisten = u; }).catch(() => {});
    return () => { disposed = true; unlisten?.(); };
  }, []);

  // 拖进终端的文件发完了：把对方电脑上的路径填进那个终端，像本机拖文件进终端一样
  useEffect(() => {
    for (const [id, target] of pendingInsert.current) {
      const t = transfers.find((x) => x.id === id);
      if (!t || t.finished_at == null) continue;
      pendingInsert.current.delete(id);
      if (t.state !== "done" || !t.paths.length) continue;
      const quoted = t.paths.map((p) => shellQuote(p, target.platform)).join(" ") + " ";
      if (target.draftKey != null) {
        const key = target.draftKey;
        setDrafts((d) => ({ ...d, [key]: (d[key] ? d[key].replace(/\s*$/, " ") : "") + quoted }));
        // 传的时候切去别的标签了就不抢焦点，路径照样留在原来那个标签的草稿里
        if (activeTab?.key === key) composerRef.current?.focus();
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

  return (
    <div className="app-shell">
      <UpdateIsland updater={updater} />
      <Titlebar
        ov={ov}
        onRefresh={refresh}
        localDeviceOpen={showLocalDevice}
        onLocalDeviceOpenChange={setShowLocalDevice}
        terminalCount={tabs.length}
        onDiag={() => setShowDiag(true)}
        onSettings={() => setShowSettings(true)}
        settingsEnabled={!!ov}
        sidebarOpen={sidebar.open}
        onBack={showSettings ? () => setShowSettings(false) : undefined}
        onToggleSidebar={sidebar.toggle}
        connectionAction={
          <ConnectionPopover
            connection={connection}
            open={showConnect}
            onOpenChange={(open) => {
              setShowConnect(open);
              // 关掉面板就回到输设备码，下次从加号打开不会还停在局域网设备上
              if (!open) {
                connection.setLanTarget(null);
                // 没连上就关了面板：之前点的会话不再自动打开
                if (!connection.busy) pendingOpen.current = null;
              }
            }}
          />
        }
      />
      {!showSettings && <ConnectionSidebar
        ov={ov}
        liveSessions={liveSessions}
        onSessionEnded={sessionEnded}
        onSessionsChanged={() => setSessionsRev((v) => v + 1)}
        onSessionsLocalChange={updateLiveSessions}
        onPendingOpen={(endpointId, sessionId) => { pendingOpen.current = { endpointId, sessionId }; }}
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
      <main className={cn("workspace", showSettings && "workspace-full-width")}>
        {overviewError && (
          <div className="workspace-notice">
            <Alert variant="destructive">
              <AlertTitle>无法读取设备信息</AlertTitle>
              <AlertDescription>
                {overviewError}
                <Button variant="outline" size="sm" onClick={refresh}>
                  重试
                </Button>
              </AlertDescription>
            </Alert>
          </div>
        )}
        {ov && ov.sessions.length > 0 && (
          <div className="workspace-notice">
            <Alert variant="warning">
              <Radio />
              <AlertTitle>本机终端正在被远程使用</AlertTitle>
              <AlertDescription>
                <span>
                  {ov.sessions
                    .map(
                      (s) => s.name + "（" + platformLabel(s.platform) + "）",
                    )
                    .join("、")}
                </span>
                <div className="flex flex-wrap gap-2">
                  {ov.sessions.map((s) => (
                    <Button
                      key={s.session}
                      variant="outline"
                      size="sm"
                      onClick={() =>
                        api
                          .kick(s.session)
                          .then(refresh)
                          .catch((e) => toast.error(String(e)))
                      }
                    >
                      <Unplug data-icon="inline-start" />
                      断开{ov.sessions.length > 1 ? " " + s.name : "连接"}
                    </Button>
                  ))}
                </div>
              </AlertDescription>
            </Alert>
          </div>
        )}

        {showSettings && ov && (
          <SettingsPage ov={ov} onChanged={refresh} updater={updater} />
        )}
        {!showSettings && sessionTarget && (
          <RemoteSessions
            key={sessionTarget.connId}
            target={sessionTarget}
            sessionsRev={sessionsRev}
            openSessions={new Set(tabs
              .filter((t) => t.info.endpoint_id === sessionTarget.info.endpoint_id && (t.state === "open" || t.state === "opening") && t.sessionId)
              .map((t) => t.sessionId!))}
            onOpen={(id) => addTab(sessionTarget.connId, sessionTarget.info, sessionTarget.code, id)}
            onBack={() => {
              if (!tabs.some((t) => t.connId === sessionTarget.connId))
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
                  aria-label="工作区标签"
                  className="terminal-tab-list"
                >
                  <Tabs.Tab id="home" aria-label="首页" className="terminal-home-tab">
                    <House aria-hidden="true" />
                  </Tabs.Tab>
                  {tabs.map((t) => (
                    <Tabs.Tab
                      id={String(t.key)}
                      aria-label={t.info.name}
                      className="terminal-tab-trigger"
                      key={t.key}
                      render={(domProps) => (
                        <div
                          className={cn(
                            "terminal-tab",
                            t.key === activeKey && "is-active",
                          )}
                        >
                          <div {...(domProps as ComponentProps<"div">)} />
                          <Button
                            variant="ghost"
                            size="icon-sm"
                            aria-label={
                              "关闭终端 " + t.info.name + " #" + t.key
                            }
                            title="关闭标签，保留后台会话"
                            onPress={() => closeTab(t.key)}
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
                            t.state === "open" || t.state === "opening"
                              ? (paths[t.connId]?.kind ?? "unknown")
                              : "off"
                          }
                        />
                        <AgentTabTitle name={t.info.name} agent={tabAgent(t)} />
                      </span>
                    </Tabs.Tab>
                  ))}
                </Tabs.List>
              </Tabs.ListContainer>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  className="terminal-new-tab"
                  aria-label={activeTab && activeTab.state !== "closed" ? "新建终端" : "新建连接"}
                  title={activeTab && activeTab.state !== "closed" ? "在当前设备上新建终端" : "新建连接"}
                  onClick={() => activeTab && activeTab.state !== "closed"
                    ? addTab(activeTab.connId, activeTab.info, activeTab.code)
                    : setShowConnect(true)}
                >
                  <Plus />
                </Button>
              {activeTab && activeTab.state !== "closed" && (
                <Button variant="ghost" size="icon-sm" aria-label="远程会话" title="查看和恢复远程会话" onClick={() => setSessionTarget(activeTab)}>
                  <History />
                </Button>
              )}
            </div>
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
              {tabs.map((t) => (
                <Tabs.Panel
                  shouldForceMount
                  key={t.key}
                  id={String(t.key)}
                  className="terminal-panel"
                  hidden={t.key !== activeKey}
                >
                  <TerminalView
                    connId={t.connId}
                    sessionId={t.sessionId}
                    active={!showSettings && !sessionTarget && t.key === activeKey}
                    onReady={(termId, sessionId) =>
                      setTabs((ts) =>
                        ts.map((x) =>
                          x.key === t.key && x.state === "opening"
                            ? { ...x, state: "open", sessionId, termId }
                            : x,
                        ),
                      )
                    }
                    onStatus={(s) => setTabState(t.key, s)}
                    onHandle={(handle) => {
                      if (handle) termHandles.current.set(t.key, handle);
                      else termHandles.current.delete(t.key);
                    }}
                  />
                  {t.state === "opening" && (
                    <div className="terminal-loading" role="status">
                      <LoaderCircle className="animate-spin" />
                      正在打开终端…
                    </div>
                  )}
                  {(t.state === "closed" || t.state === "exited") && (
                    <div className="term-overlay">
                      <Card className="max-w-md">
                        <CardHeader>
                          <CardTitle>
                            {t.state === "exited" ? "已脱离终端" : "连接已断开"}
                          </CardTitle>
                        </CardHeader>
                        <CardContent>
                          <p className="text-muted-foreground break-words">
                            {t.note}
                            <span className="mt-2 block text-xs">关闭标签不会结束后台会话。会话列表中可恢复仍在运行的会话。</span>
                          </p>
                        </CardContent>
                        <CardFooter className="justify-center gap-2">
                          <Button
                            onClick={() => t.state === "closed"
                              ? void connection.doConnect(t.code ?? t.info.endpoint_id, "")
                              : setSessionTarget(t)}
                          >
                            恢复会话
                          </Button>
                          {t.state === "exited" && (
                            <Button
                              onClick={() => addTab(t.connId, t.info, t.code)}
                            >
                              <Plus data-icon="inline-start" />
                              新建终端
                            </Button>
                          )}
                          <Button
                            variant="outline"
                            onClick={() => closeTab(t.key)}
                          >
                            关闭标签
                          </Button>
                        </CardFooter>
                      </Card>
                    </div>
                  )}
                </Tabs.Panel>
              ))}
            </div>
            {showComposer && activeTab && activeAgent && (
              <AgentComposer
                key={`composer-${activeTab.key}`}
                agent={activeAgent}
                draft={drafts[activeTab.key] ?? ""}
                onDraft={(text) => setDrafts((d) => ({ ...d, [activeTab.key]: text }))}
                onSend={(text) => sendToAgent(activeTab.key, text)}
                onTerminal={() => termHandles.current.get(activeTab.key)?.focus()}
                onHide={() => { hideComposer(true); termHandles.current.get(activeTab.key)?.focus(); }}
                inputRef={composerRef}
              />
            )}
            {activeTab && <TerminalStatusbar
              key={activeTab.key}
              onShowComposer={activeAgent && composerHidden ? () => { hideComposer(false); window.requestAnimationFrame(() => composerRef.current?.focus()); } : undefined}
              sessionId={activeTab.sessionId}
              info={activeTab.info}
              state={activeTab.state}
              path={paths[activeTab.connId]}
              snapshot={liveSessions[activeTab.connId]}
            />}
          </Tabs>

        </div>
        <TransferPanel transfers={transfers} />
      </main>
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
                <strong className="text-sm">松开发给 {dropHint.name}</strong>
                <span className="text-xs text-muted-foreground">文件放在对方的下载文件夹里，拖进终端的会把路径填进去</span>
              </>
            )}
          </div>
        </div>
      )}
      <Toaster />
    </div>
  );
}
