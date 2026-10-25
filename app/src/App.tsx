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
  type TermStatus,
  type Transfer,
} from "./api";
import { TerminalView } from "./TerminalView";
import { TerminalStatusbar } from "./components/TerminalStatusbar";
import { selftestSawOverview } from "./selftest";
import { ConnectionPopover } from "./components/ConnectionPopover";
import { ConnectionSidebar } from "./components/ConnectionSidebar";
import { useConnection } from "./components/useConnection";
import { useSidebar } from "./components/useSidebar";
import { useUpdater } from "./components/useUpdater";
import { UpdateIsland } from "./components/UpdateIsland";
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
  const [sessionTarget, setSessionTarget] = useState<SessionTarget | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [showDiag, setShowDiag] = useState(false);
  /** 会话页里结束了会话就加一，侧边栏的会话列表跟着在后台刷新 */
  const [sessionsRev, setSessionsRev] = useState(0);
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
  // One path snapshot per live connection, shared by the sidebar and status bar.
  const liveConnectionKey = [...new Set([
    ...tabs.filter((tab) => tab.state !== "closed").map((tab) => tab.connId),
    ...(sessionTarget ? [sessionTarget.connId] : []),
  ])].sort((a, b) => a - b).join(",");
  useEffect(() => {
    if (!liveConnectionKey) return;
    let disposed = false;
    let timer: number | undefined;
    const tick = async () => {
      await Promise.all(liveConnectionKey.split(",").map(async (id) => {
        const connId = Number(id);
        try {
          const path = await api.pathInfo(connId);
          if (!disposed) setPaths((current) => ({ ...current, [connId]: path }));
        } catch { /* Keep the last sample until a refresh or disconnect. */ }
      }));
      if (!disposed) timer = window.setTimeout(() => void tick(), 3000);
    };
    void tick();
    return () => { disposed = true; window.clearTimeout(timer); };
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

  /** 拖进来的文件发给哪台设备：正在看的终端，或者正在看的会话页；终端里的还要在发完后把路径填进去 */
  const dropTarget = (): { info: RemoteInfo; termId?: number } | { blocked: string } => {
    if (showSettings) return { blocked: "先回到终端，再把文件拖进来" };
    if (sessionTarget) return { info: sessionTarget.info };
    if (activeTab && (activeTab.state === "open" || activeTab.state === "opening")) return { info: activeTab.info, termId: activeTab.termId };
    return { blocked: "先打开一台设备的终端，再把文件拖进来" };
  };
  const dropTargetRef = useRef(dropTarget);
  dropTargetRef.current = dropTarget;
  /** 拖进终端的文件：传输编号 → 发完要填路径的终端 */
  const pendingInsert = useRef(new Map<string, { termId: number; platform: string }>());

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void getCurrentWebview().onDragDropEvent(async (event) => {
      const e = event.payload;
      if (e.type === "leave") return setDropHint(null);
      const target = dropTargetRef.current();
      if (e.type === "enter" || e.type === "over") {
        return setDropHint("blocked" in target ? { blocked: target.blocked } : { name: target.info.name });
      }
      setDropHint(null);
      if ("blocked" in target) return toast.error(target.blocked);
      if (!e.paths.length) return;
      try {
        const id = await api.sendFiles(target.info.endpoint_id, e.paths);
        if (target.termId != null) pendingInsert.current.set(id, { termId: target.termId, platform: target.info.platform });
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
      if (t.state === "done" && t.paths.length)
        void api.termInput(target.termId, t.paths.map((p) => shellQuote(p, target.platform)).join(" ") + " ").catch(() => {});
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
        sessionsRev={sessionsRev}
        onSessionEnded={sessionEnded}
        onSessionsChanged={() => setSessionsRev((v) => v + 1)}
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
                        <span className="truncate">{t.info.name}</span>
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
            {activeTab && <TerminalStatusbar
              key={activeTab.key}
              connId={activeTab.connId}
              sessionId={activeTab.sessionId}
              info={activeTab.info}
              state={activeTab.state}
              path={paths[activeTab.connId]}
              visible={!showSettings && !sessionTarget}
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
