import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  LoaderCircle,
  Plus,
  Radio,
  ShieldCheck,
  Unplug,
  X,
} from "lucide-react";
import { toast, Toaster } from "sonner";
import {
  api,
  pathLabel,
  platformLabel,
  type HostEvent,
  type IncomingRequest,
  type Overview,
  type PathInfo,
  type RemoteInfo,
  type TermStatus,
} from "./api";
import { TerminalView } from "./TerminalView";
import { selftestSawOverview } from "./selftest";
import {
  ConnectionSidebar,
  ServerStatus,
} from "./components/ConnectionSidebar";
import {
  ApproveDialog,
  DiagDialog,
  SettingsDialog,
} from "./components/AppDialogs";
import { Welcome } from "./components/Welcome";
import { Titlebar } from "./components/Titlebar";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { cn } from "@/lib/utils";

interface Tab {
  key: number;
  connId: number;
  info: RemoteInfo;
  code: string | null;
  state: "opening" | "open" | "exited" | "closed";
  note: string | null;
}

let tabSeq = 1;

export default function App() {
  const [ov, setOv] = useState<Overview | null>(null);
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [activeKey, setActiveKey] = useState<number | null>(null);
  const [paths, setPaths] = useState<Record<number, PathInfo>>({});
  const [requests, setRequests] = useState<IncomingRequest[]>([]);
  const [showSettings, setShowSettings] = useState(false);
  const [showDiag, setShowDiag] = useState(false);
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
        if (ev.type === "request_cancelled")
          setRequests((r) => r.filter((x) => x.id !== ev.id));
        refresh();
      }),
      listen("server-status", refresh),
      listen<{ conn_id: number; reason: string }>("remote-closed", (e) => {
        const { conn_id, reason } = e.payload;
        setTabs((ts) =>
          ts.map((t) =>
            t.connId === conn_id ? { ...t, state: "closed", note: reason } : t,
          ),
        );
      }),
    ];
    return () => {
      window.clearInterval(timer);
      unsubs.forEach((p) => p.then((u) => u()));
    };
  }, [refresh]);

  const activeTab = tabs.find((t) => t.key === activeKey) ?? null;
  const activePath = activeTab ? paths[activeTab.connId] : undefined;
  useEffect(() => {
    if (!activeTab || activeTab.state === "closed") return;
    const connId = activeTab.connId;
    const tick = () =>
      api
        .pathInfo(connId)
        .then((p) => setPaths((m) => ({ ...m, [connId]: p })))
        .catch(() => {});
    tick();
    const timer = window.setInterval(tick, 3000);
    return () => window.clearInterval(timer);
  }, [activeTab?.connId, activeTab?.state]);

  const addTab = (connId: number, info: RemoteInfo, code: string | null) => {
    const key = tabSeq++;
    setTabs((ts) => [
      ...ts,
      { key, connId, info, code, state: "opening", note: null },
    ]);
    setActiveKey(key);
  };

  const onConnected = (
    connId: number,
    info: RemoteInfo,
    code: string | null,
    path: PathInfo,
  ) => {
    setPaths((m) => ({ ...m, [connId]: path }));
    addTab(connId, info, code);
    refresh();
    toast.success("已连接 " + info.name);
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
              "程序已退出" +
              (status.code != null ? "（退出码 " + status.code + "）" : ""),
          };
        return { ...t, state: "closed", note: status.reason ?? "终端已关闭" };
      }),
    );
  };

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
      <Titlebar
        deviceName={activeTab?.info.name}
        terminalCount={tabs.length}
        onDiag={() => setShowDiag(true)}
        onSettings={() => setShowSettings(true)}
        settingsEnabled={!!ov}
      />
      <ConnectionSidebar
        ov={ov}
        onConnected={onConnected}
        onRefresh={refresh}
      />
      <main className="workspace">
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

        {tabs.length > 0 ? (
          <Tabs
            value={String(activeKey)}
            onValueChange={(key) => setActiveKey(Number(key))}
            className="terminal-workspace"
          >
            <div className="terminal-toolbar">
              <TabsList variant="terminal" aria-label="远程终端标签">
                {tabs.map((t) => (
                  <div
                    className={cn(
                      "terminal-tab",
                      t.key === activeKey && "is-active",
                    )}
                    key={t.key}
                  >
                    <TabsTrigger variant="terminal" value={String(t.key)}>
                      <span
                        className="status-dot"
                        data-kind={
                          t.state === "open" || t.state === "opening"
                            ? (paths[t.connId]?.kind ?? "unknown")
                            : "off"
                        }
                      />
                      <span className="truncate">{t.info.name}</span>
                    </TabsTrigger>
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      aria-label={"关闭终端 " + t.info.name + " #" + t.key}
                      title="关闭终端"
                      onClick={() => closeTab(t.key)}
                    >
                      <X />
                    </Button>
                  </div>
                ))}
              </TabsList>
              {activeTab && activeTab.state !== "closed" && (
                <Button
                  variant="ghost"
                  size="icon-sm"
                  aria-label="新建终端"
                  title="在当前设备上新建终端"
                  onClick={() =>
                    addTab(activeTab.connId, activeTab.info, activeTab.code)
                  }
                >
                  <Plus />
                </Button>
              )}
            </div>
            <div className="terminals">
              {tabs.map((t) => (
                <TabsContent
                  forceMount
                  key={t.key}
                  value={String(t.key)}
                  className="terminal-panel"
                  hidden={t.key !== activeKey}
                >
                  <TerminalView
                    connId={t.connId}
                    active={t.key === activeKey}
                    onReady={() =>
                      setTabs((ts) =>
                        ts.map((x) =>
                          x.key === t.key && x.state === "opening"
                            ? { ...x, state: "open" }
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
                            {t.state === "exited" ? "终端已结束" : "连接已断开"}
                          </CardTitle>
                        </CardHeader>
                        <CardContent>
                          <p className="text-muted-foreground break-words">
                            {t.note}
                          </p>
                        </CardContent>
                        <CardFooter className="justify-center gap-2">
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
                </TabsContent>
              ))}
            </div>
          </Tabs>
        ) : (
          <Welcome ov={ov} onSettings={() => setShowSettings(true)} />
        )}

        <footer className="workspace-statusbar">
          <div className="flex min-w-0 items-center gap-4">
            <ServerStatus ov={ov} />
            <span className="statusbar-device truncate">
              {activeTab
                ? activeTab.info.user + "@" + activeTab.info.name
                : "等待远程连接"}
            </span>
          </div>
          {activeTab ? (
            <Badge
              variant={
                activeTab.state === "closed"
                  ? "destructive"
                  : activePath?.kind === "relay"
                    ? "warning"
                    : activePath?.kind === "ipv6" || activePath?.kind === "ipv4"
                      ? "success"
                      : "secondary"
              }
              title={activePath?.remote ?? undefined}
            >
              {activeTab.state === "closed" ? <Unplug /> : <Radio />}
              {activeTab.state === "closed" ? "已断开" : pathLabel(activePath)}
            </Badge>
          ) : (
            <span className="statusbar-security">
              <ShieldCheck />
              端到端加密
            </span>
          )}
        </footer>
      </main>
      {showSettings && ov && (
        <SettingsDialog
          ov={ov}
          onClose={() => setShowSettings(false)}
          onChanged={refresh}
        />
      )}
      {showDiag && <DiagDialog onClose={() => setShowDiag(false)} />}
      {requests.length > 0 && (
        <ApproveDialog
          key={requests[0].id}
          req={requests[0]}
          onRespond={respond}
        />
      )}
      <Toaster theme="dark" position="bottom-right" closeButton richColors />
    </div>
  );
}
