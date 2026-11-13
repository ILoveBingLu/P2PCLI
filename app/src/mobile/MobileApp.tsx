import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { ArrowDown, ArrowUp, ChevronLeft, LoaderCircle } from "lucide-react";
import { api, type FileRequest, type HostEvent, type NearbyDevice, type Overview, type PathInfo, type RecentDevice, type TermStatus, type TerminalSessionSnapshot, type Transfer } from "../api";
import type { TerminalHandle } from "../TerminalView";
import { toast, Toaster } from "@/components/ui/toast";
import { Button } from "@/components/ui/button";
import { FileApproveDialog } from "../components/AppDialogs";
import { RemarksProvider } from "../components/Remarks";
import { SettingsPage } from "../components/SettingsPage";
import { useConnection } from "../components/useConnection";
import { useUpdater } from "../components/useUpdater";
import { sessionTitles } from "../components/sessionText";
import { pollConnections } from "../lib/pollConnections";
import { ConnectSheet } from "./ConnectSheet";
import { ScanScreen } from "./ScanScreen";
import type { PairInfo } from "@/lib/pairLink";
import { DeviceScreen } from "./DeviceScreen";
import { HomeScreen, type TermRow } from "./HomeScreen";
import { TerminalScreen, type TermMeta } from "./TerminalScreen";
import { useScreens } from "./useScreens";
import type { Conn, Term } from "./model";
import { t, tr } from "@/i18n";
import "./mobile.css";

/** 连上以后接着做什么：看这台设备的会话，或者直接打开一个终端（replace 是要换掉的那个断开的终端） */
type Intent = { endpointId: string | null; then: "device" | { open: string | null; replace?: number } };

/**
 * 手机端：只当连接的一方。页面是一层一层进的：设备列表 → 设备的会话 → 终端；
 * 终端都挂在后台，切页面、切终端都不断。
 */
export default function MobileApp() {
  const nav = useScreens();
  const [ov, setOv] = useState<Overview | null>(null);
  const [conns, setConns] = useState<Record<string, Conn>>({});
  const [terms, setTerms] = useState<Term[]>([]);
  const [paths, setPaths] = useState<Record<number, PathInfo>>({});
  const [liveSessions, setLiveSessions] = useState<Record<number, TerminalSessionSnapshot>>({});
  const [transfers, setTransfers] = useState<Transfer[]>([]);
  const [fileRequests, setFileRequests] = useState<FileRequest[]>([]);
  const [connectOpen, setConnectOpen] = useState(false);
  const [connecting, setConnecting] = useState<string | null>(null);
  const [scanning, setScanning] = useState(false);
  /** 扫码页开着 */
  const [scanOpen, setScanOpen] = useState(false);
  /** 扫码连的设备名：列表里可能还没有它，连接中时在首页顶上显示一张卡片 */
  const [pendingName, setPendingName] = useState<string | null>(null);
  const [sessionsRev, setSessionsRev] = useState(0);
  const termSeq = useRef(1);
  const handles = useRef(new Map<number, TerminalHandle>());
  const intent = useRef<Intent | null>(null);
  // 连接是异步的，回调里要读最新的终端列表和当前页面
  const termsRef = useRef(terms);
  termsRef.current = terms;
  const topRef = useRef(nav.top);
  topRef.current = nav.top;
  const updater = useUpdater();

  const refresh = useCallback(() => {
    api.overview().then(setOv).catch(() => {});
  }, []);

  useEffect(() => {
    refresh();
    const timer = window.setInterval(refresh, 5000);
    const unsubs = [
      listen<Transfer[]>("transfers", (e) => setTransfers(e.payload)),
      listen<FileRequest>("file-request", (e) => setFileRequests((r) => r.some((x) => x.id === e.payload.id) ? r : [...r, e.payload])),
      listen<HostEvent>("host-event", (e) => {
        const ev = e.payload;
        if (ev.type === "request_cancelled") setFileRequests((r) => r.filter((x) => x.id !== ev.id));
        if (ev.type === "files_received") toast.success(t("收到 {{from_name}} 发来的 {{files}} 个文件", { from_name: ev.from_name, files: ev.files }));
        refresh();
      }),
      listen("server-status", refresh),
      listen<{ conn_id: number; reason: string }>("remote-closed", (e) => {
        const { conn_id, reason } = e.payload;
        setConns((c) => Object.fromEntries(Object.entries(c).filter(([, conn]) => conn.connId !== conn_id)));
        setTerms((ts) => ts.map((term) => term.connId === conn_id && term.state !== "exited" ? { ...term, state: "closed", note: reason } : term));
      }),
    ];
    void api.transfers().then(setTransfers).catch(() => {});
    return () => {
      window.clearInterval(timer);
      unsubs.forEach((p) => p.then((u) => u()));
    };
  }, [refresh]);

  // 每条连接各轮询一次会话列表和线路，首页、设备页、终端页都用这一份
  const liveKey = Object.values(conns).map((c) => c.connId).sort((a, b) => a - b).join(",");
  useEffect(() => {
    if (!liveKey) return;
    return pollConnections(
      liveKey.split(",").map(Number),
      (connId) => api.listTerminals(connId),
      (connId, sessions) => setLiveSessions((cur) => ({ ...cur, [connId]: { sessions, error: null } })),
      (connId, error) => setLiveSessions((cur) => ({ ...cur, [connId]: { sessions: cur[connId]?.sessions ?? null, error: String(error) } })),
    );
  }, [liveKey, sessionsRev]);
  useEffect(() => {
    if (!liveKey) return;
    return pollConnections(
      liveKey.split(",").map(Number),
      (connId) => api.pathInfo(connId),
      (connId, path) => setPaths((cur) => ({ ...cur, [connId]: path })),
      () => {},
    );
  }, [liveKey]);

  // 发出去的文件传完了说一声；进度在首页和设备页上
  const finished = useRef(new Set<string>());
  useEffect(() => {
    for (const x of transfers) {
      if (x.finished_at == null || finished.current.has(x.id)) continue;
      finished.current.add(x.id);
      if (x.direction !== "send") continue;
      if (x.state === "done") toast.success(t("已发给 {{name}}：{{label}}", { name: x.peer_name, label: x.label }));
      else if (x.state === "failed") toast.error(t("没发出去：{{error}}", { error: x.error ?? x.label }));
    }
  }, [transfers]);

  const deviceName = (endpointId: string) =>
    ov?.remarks[endpointId]?.device || conns[endpointId]?.info.name
    || ov?.recent.find((d) => d.endpoint_id === endpointId)?.name
    || ov?.nearby.find((d) => d.endpoint_id === endpointId)?.name || endpointId.slice(0, 8);
  const devicePlatform = (endpointId: string) =>
    conns[endpointId]?.info.platform || ov?.recent.find((d) => d.endpoint_id === endpointId)?.platform
    || ov?.nearby.find((d) => d.endpoint_id === endpointId)?.platform || "";

  const showDevice = (endpointId: string) => {
    const top = topRef.current;
    if (top.name === "device" && top.endpointId === endpointId) return;
    nav.push({ name: "device", endpointId });
  };
  const goTerminal = (key: number) => {
    if (topRef.current.name === "terminal") nav.replace({ name: "terminal", key });
    else nav.push({ name: "terminal", key });
  };
  /** 打开一个终端；sessionId 不写是新建。已经开着同一个会话就切过去 */
  const openTerminal = (connId: number, endpointId: string, sessionId?: string, replace?: number) => {
    const existing = sessionId && termsRef.current.find((x) => x.endpointId === endpointId && x.sessionId === sessionId
      && x.key !== replace && (x.state === "open" || x.state === "opening"));
    if (existing) return goTerminal(existing.key);
    const key = termSeq.current++;
    setTerms((ts) => [
      ...ts.filter((x) => x.key !== replace && !(sessionId && x.endpointId === endpointId && x.sessionId === sessionId)),
      { key, connId, endpointId, sessionId, state: "opening", note: null },
    ]);
    goTerminal(key);
  };

  const connection = useConnection({
    onConnected: (connId, info, code, path) => {
      setConns((c) => ({ ...c, [info.endpoint_id]: { connId, info, code } }));
      setPaths((p) => ({ ...p, [connId]: path }));
      const next = intent.current;
      intent.current = null;
      if (next && next.then !== "device" && (next.endpointId === null || next.endpointId === info.endpoint_id))
        openTerminal(connId, info.endpoint_id, next.then.open ?? undefined, next.then.replace);
      else showDevice(info.endpoint_id);
      // 连上就进设备页或终端，不再弹“已连接”，免得挡住顶栏
      refresh();
    },
    onOpenChange: (open, fail) => {
      if (!open) return hideConnect();
      // 要密码时弹出面板填密码；别的原因（对方拒绝、离线、超时）提示一下就行，面板开着时错误显示在面板里
      if (fail?.reject === "need_password" || fail?.reject === "bad_password") showConnect(false);
      else if (!connectOpen) {
        intent.current = null;
        toast.error(tr(fail?.message ?? String(fail)));
      }
    },
  });

  const showConnect = (fresh: boolean) => {
    if (fresh) {
      intent.current = null;
      connection.setError(null);
      connection.setLanTarget(null);
    }
    setConnectOpen(true);
    nav.openOverlay(() => setConnectOpen(false));
  };
  function hideConnect() {
    setConnectOpen(false);
    nav.closeOverlay();
  }

  /** 连一台设备：局域网里在线就直接连，不然按设备码连 */
  const reach = (endpointId: string): { lan: NearbyDevice } | { code: string } | null => {
    const lan = ov?.nearby.find((d) => d.endpoint_id === endpointId && d.online);
    if (lan) return { lan };
    const recent = ov?.recent.find((d) => d.endpoint_id === endpointId);
    return recent ? { code: recent.code } : null;
  };
  const connectTo = (endpointId: string, then: Intent["then"]) => {
    if (connection.busy) return;
    const how = reach(endpointId);
    if (!how) return void toast.error(t("找不到这台设备的设备码，请重新扫码连接"));
    intent.current = { endpointId, then };
    setConnecting(endpointId);
    if ("lan" in how) void connection.connectLan(how.lan);
    else void connection.doConnect(how.code, "");
  };
  const openDevice = (device: RecentDevice) => {
    if (conns[device.endpoint_id]) showDevice(device.endpoint_id);
    else connectTo(device.endpoint_id, "device");
  };
  const openNearby = (device: NearbyDevice) => {
    if (conns[device.endpoint_id]) return showDevice(device.endpoint_id);
    if (connection.busy) return;
    intent.current = { endpointId: device.endpoint_id, then: "device" };
    setConnecting(device.endpoint_id);
    void connection.connectLan(device);
  };
  /** 打开扫码页；从连接面板里点的就接着用面板那条返回记录 */
  const openScan = () => {
    if (connection.busy) return;
    setConnectOpen(false);
    setScanOpen(true);
    nav.openOverlay(() => setScanOpen(false));
  };
  const closeScan = () => {
    setScanOpen(false);
    nav.closeOverlay();
  };
  const scanned = (pair: PairInfo) => {
    closeScan();
    intent.current = null;
    setConnecting(pair.id);
    setPendingName(pair.name || t("电脑"));
    void connection.connectPair(pair);
  };
  /** 扫码页里点“输入设备码”：换成连接面板 */
  const scanToManual = () => {
    setScanOpen(false);
    connection.setError(null);
    connection.setLanTarget(null);
    setConnectOpen(true);
    nav.openOverlay(() => setConnectOpen(false));
  };
  const scanNearby = async () => {
    if (scanning) return;
    setScanning(true);
    try {
      const nearby = await api.scanNearby();
      setOv((cur) => cur ? { ...cur, nearby } : cur);
    } catch (error) {
      toast.error(t("扫描失败：{{error}}", { error: String(error) }));
    } finally {
      setScanning(false);
    }
  };

  const closeTerminal = (key: number) => {
    handles.current.delete(key);
    setTerms((ts) => ts.filter((x) => x.key !== key));
    const top = topRef.current;
    if (top.name === "terminal" && top.key === key) nav.back();
  };
  const disconnect = (endpointId: string) => {
    const conn = conns[endpointId];
    if (!conn) return;
    void api.disconnect(conn.connId).catch((e) => toast.error(String(e)));
    setConns((c) => Object.fromEntries(Object.entries(c).filter(([id]) => id !== endpointId)));
    setTerms((ts) => ts.filter((x) => x.connId !== conn.connId));
    nav.home();
  };
  const endSession = async (endpointId: string, sessionId: string) => {
    const conn = conns[endpointId];
    if (!conn) return;
    try {
      await api.endTerminal(conn.connId, sessionId);
      setTerms((ts) => ts.filter((x) => !(x.endpointId === endpointId && x.sessionId === sessionId)));
      setSessionsRev((v) => v + 1);
      toast.success(t("会话已结束"));
    } catch (error) {
      toast.error(t("结束会话失败：{{error}}", { error: String(error) }));
      throw error;
    }
  };
  const sendFiles = async (endpointId: string) => {
    try {
      const picked = await api.pickPaths(false);
      if (picked.length) await api.sendFiles(endpointId, picked);
    } catch (error) {
      toast.error(t("发不了：{{error}}", { error: String(error) }));
    }
  };
  /** 断开的终端重新连上，接着打开原来那个会话 */
  const reconnect = (term: Term) => {
    const conn = conns[term.endpointId];
    if (conn) return openTerminal(conn.connId, term.endpointId, term.sessionId, term.key);
    connectTo(term.endpointId, { open: term.sessionId ?? null, replace: term.key });
  };
  const newTerminal = (term: Term) => {
    const conn = conns[term.endpointId];
    if (conn) openTerminal(conn.connId, term.endpointId);
    else connectTo(term.endpointId, { open: null });
  };
  const setTermStatus = (key: number, status: TermStatus) => {
    setTerms((ts) => ts.map((x) => {
      if (x.key !== key || x.state === "closed") return x;
      if (status.type === "exited")
        return { ...x, state: "exited", note: status.code != null ? t("终端视图已结束（退出码 {{code}}）", { code: status.code }) : t("终端视图已结束") };
      return { ...x, state: "closed", note: status.reason ?? t("已脱离会话") };
    }));
  };

  const meta = (term: Term): TermMeta => {
    const snapshot = liveSessions[term.connId];
    const sessions = snapshot && !snapshot.error ? snapshot.sessions ?? [] : [];
    const session = term.sessionId ? sessions.find((s) => s.id === term.sessionId) : undefined;
    const title = term.sessionId ? sessionTitles(sessions).get(term.sessionId) ?? t("终端") : t("新终端");
    return {
      title,
      device: deviceName(term.endpointId),
      platform: devicePlatform(term.endpointId),
      agent: term.state === "open" || term.state === "opening" ? session?.agent ?? null : null,
      path: paths[term.connId],
    };
  };
  const termRows: TermRow[] = terms.map((term) => {
    const m = meta(term);
    return { term, title: m.title, device: m.device, agent: m.agent };
  });

  const top = nav.top;
  const activeTerm = top.name === "terminal" ? top.key : null;
  // 扫码连的设备已经在列表里时，那一行自己会转圈，不用再在顶上放一张卡片
  const listed = (id: string | null) => !!id && (!!ov?.recent.some((d) => d.endpoint_id === id) || !!ov?.nearby.some((d) => d.endpoint_id === id));
  const transfersNode = <Transfers transfers={transfers}
    connectingName={connection.busy && !connectOpen && top.name === "home" && !listed(connecting) ? pendingName : null} />;

  return (
    <RemarksProvider snapshot={ov?.remarks}>
      <div className="m-app" data-dir={nav.dir}>
        {top.name === "home" && (
          <HomeScreen
            ov={ov}
            conns={conns}
            terms={termRows}
            liveSessions={liveSessions}
            paths={paths}
            connecting={connection.busy ? connecting : null}
            scanning={scanning}
            transfers={transfersNode}
            onDevice={openDevice}
            onNearby={openNearby}
            onTerminal={goTerminal}
            onConnectNew={() => showConnect(true)}
            onScan={openScan}
            onScanNearby={() => void scanNearby()}
            onSettings={() => nav.push({ name: "settings" })}
          />
        )}
        {top.name === "device" && (() => {
          const id = top.endpointId;
          const conn = conns[id];
          return (
            <DeviceScreen
              key={id}
              name={deviceName(id)}
              platform={devicePlatform(id)}
              conn={conn}
              status={ov?.devices.find((d) => d.endpoint_id === id)}
              snapshot={conn ? liveSessions[conn.connId] : undefined}
              path={conn ? paths[conn.connId] : undefined}
              terms={terms.filter((x) => x.endpointId === id)}
              busy={connection.busy && connecting === id}
              transfers={transfersNode}
              onBack={nav.back}
              onConnect={() => connectTo(id, "device")}
              onOpen={(sessionId) => conn && openTerminal(conn.connId, id, sessionId)}
              onEnd={(sessionId) => endSession(id, sessionId)}
              onSendFiles={() => void sendFiles(id)}
              onDisconnect={() => disconnect(id)}
              onRefresh={() => setSessionsRev((v) => v + 1)}
              sheet={{ open: nav.openOverlay, close: nav.closeOverlay }}
            />
          );
        })()}
        {top.name === "settings" && ov && (
          <div className="m-screen">
            <header className="m-nav">
              <Button variant="ghost" size="icon" className="m-icon-button" aria-label={t("返回")} onPress={nav.back}>
                <ChevronLeft />
              </Button>
              <div className="m-nav-title"><span className="m-nav-name">{t("设置")}</span></div>
            </header>
            <div className="m-settings">
              <SettingsPage ov={ov} onChanged={refresh} updater={updater} />
            </div>
          </div>
        )}
        {terms.length > 0 && (
          <TerminalScreen
            visible={top.name === "terminal"}
            activeKey={activeTerm}
            terms={terms}
            meta={meta}
            handles={handles}
            onBack={nav.back}
            onSwitch={goTerminal}
            onClose={closeTerminal}
            onNew={newTerminal}
            onSendFiles={(term) => void sendFiles(term.endpointId)}
            onReady={(key, termId, sessionId) => setTerms((ts) => ts.map((x) =>
              x.key === key && x.state === "opening" ? { ...x, state: "open", termId, sessionId } : x))}
            onStatus={setTermStatus}
            onReconnect={reconnect}
            onChoose={async (term, approvalId, optionId) => {
              if (!term.sessionId || term.state !== "open") throw new Error(t("会话已断开"));
              try { await api.chooseAgentOption(term.connId, term.sessionId, approvalId, optionId); }
              finally { setSessionsRev((v) => v + 1); }
            }}
            sheet={{ open: nav.openOverlay, close: nav.closeOverlay }}
          />
        )}
      </div>
      {scanOpen && <ScanScreen onResult={scanned} onClose={closeScan} onManual={scanToManual} />}
      <ConnectSheet open={connectOpen} connection={connection} onScan={openScan} onClose={() => {
        hideConnect();
        // 没连上就关了面板：之前点的设备不再自动接着打开
        if (!connection.busy) intent.current = null;
      }} />
      {fileRequests.length > 0 && (
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
      <Toaster />
    </RemarksProvider>
  );
}

/** 首页和设备页顶上：正在收发的文件和进度 */
function Transfers({ transfers, connectingName }: { transfers: Transfer[]; connectingName: string | null }) {
  const active = transfers.filter((x) => x.finished_at == null);
  if (!active.length && !connectingName) return null;
  return (
    <div className="m-transfers" role="status">
      {connectingName && (
        <div className="m-transfer">
          <span className="m-transfer-icon" aria-hidden="true"><LoaderCircle className="animate-spin" /></span>
          <span className="m-row-text">
            <span className="m-row-title">{t("正在连接 {{name}}", { name: connectingName })}</span>
            <span className="m-row-sub">{t("等对方在电脑上点同意")}</span>
          </span>
        </div>
      )}
      {active.map((x) => {
        const percent = x.total ? Math.min(100, Math.round((x.done / x.total) * 100)) : 0;
        return (
          <div key={x.id} className="m-transfer">
            <span className="m-transfer-icon" aria-hidden="true">{x.direction === "send" ? <ArrowUp /> : <ArrowDown />}</span>
            <span className="m-row-text">
              <span className="m-row-title">{x.label}</span>
              <span className="m-row-sub">
                {x.state === "waiting" ? t("等 {{name}} 同意", { name: x.peer_name })
                  : x.state === "connecting" ? t("正在连接 {{name}}", { name: x.peer_name })
                  : `${x.direction === "send" ? t("发给 {{name}}", { name: x.peer_name }) : t("来自 {{name}}", { name: x.peer_name })} · ${percent}%`}
              </span>
              <span className="m-progress" aria-hidden="true"><span style={{ width: `${percent}%` }} /></span>
            </span>
          </div>
        );
      })}
    </div>
  );
}
