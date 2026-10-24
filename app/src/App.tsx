import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  api,
  formatCode,
  pathLabel,
  platformLabel,
  type ConnectFail,
  type Diagnosis,
  type HostEvent,
  type IncomingRequest,
  type Overview,
  type PathInfo,
  type RemoteInfo,
  type SettingsView,
  type TermStatus,
} from "./api";
import { TerminalView } from "./TerminalView";
import { selftestSawOverview } from "./selftest";

interface Tab {
  key: number;
  connId: number;
  info: RemoteInfo;
  code: string | null;
  /** 打开中、在用、程序已退出、连接断了 */
  state: "opening" | "open" | "exited" | "closed";
  note: string | null;
}

let tabSeq = 1;
const APPROVE_SECONDS = 60;

export default function App() {
  const [ov, setOv] = useState<Overview | null>(null);
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [activeKey, setActiveKey] = useState<number | null>(null);
  const [paths, setPaths] = useState<Record<number, PathInfo>>({});
  const [requests, setRequests] = useState<IncomingRequest[]>([]);
  const [showSettings, setShowSettings] = useState(false);
  const [showDiag, setShowDiag] = useState(false);
  const [toast, setToast] = useState<string | null>(null);

  const refresh = useCallback(() => {
    api
      .overview()
      .then((o) => {
        setOv(o);
        selftestSawOverview(o);
      })
      .catch(() => {});
  }, []);

  const flash = useCallback((msg: string) => {
    setToast(msg);
    window.setTimeout(() => setToast((t) => (t === msg ? null : t)), 2200);
  }, []);

  useEffect(() => {
    refresh();
    const timer = window.setInterval(refresh, 5000);
    const unsubs = [
      listen<IncomingRequest>("host-request", (e) => {
        setRequests((r) => [...r, e.payload]);
        window.__ptTest?.onRequest?.(e.payload);
      }),
      listen<HostEvent>("host-event", (e) => {
        const ev = e.payload;
        if (ev.type === "request_cancelled") {
          setRequests((r) => r.filter((x) => x.id !== ev.id));
        }
        refresh();
      }),
      listen("server-status", () => refresh()),
      listen<{ conn_id: number; reason: string }>("remote-closed", (e) => {
        const { conn_id, reason } = e.payload;
        setTabs((ts) =>
          ts.map((t) => (t.connId === conn_id && t.state !== "exited" ? { ...t, state: "closed", note: reason } : t)),
        );
      }),
    ];
    return () => {
      window.clearInterval(timer);
      unsubs.forEach((p) => p.then((u) => u()));
    };
  }, [refresh]);

  // 当前标签的线路每 3 秒刷新一次
  const activeTab = tabs.find((t) => t.key === activeKey) ?? null;
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
    setTabs((ts) => [...ts, { key, connId, info, code, state: "opening", note: null }]);
    setActiveKey(key);
  };

  const onConnected = (connId: number, info: RemoteInfo, code: string | null, path: PathInfo) => {
    setPaths((m) => ({ ...m, [connId]: path }));
    addTab(connId, info, code);
    refresh();
  };

  const closeTab = (key: number) => {
    setTabs((ts) => {
      const tab = ts.find((t) => t.key === key);
      const rest = ts.filter((t) => t.key !== key);
      // 这条连接上没有别的标签了就断开
      if (tab && !rest.some((t) => t.connId === tab.connId)) api.disconnect(tab.connId);
      if (activeKey === key) setActiveKey(rest.length ? rest[rest.length - 1].key : null);
      return rest;
    });
  };

  const setTabState = (key: number, s: TermStatus) => {
    setTabs((ts) =>
      ts.map((t) => {
        if (t.key !== key) return t;
        if (s.type === "exited") return { ...t, state: "exited", note: `程序已退出${s.code != null ? `（退出码 ${s.code}）` : ""}` };
        return t.state === "closed" ? t : { ...t, state: "closed", note: s.reason ?? "终端已关闭" };
      }),
    );
  };

  const respond = (req: IncomingRequest, allow: boolean, trust: boolean) => {
    api.respond(req.id, allow, trust);
    setRequests((r) => r.filter((x) => x.id !== req.id));
  };
  if (window.__ptTest) window.__ptTest.respond = respond;

  return (
    <div className="app">
      <Sidebar
        ov={ov}
        onConnected={onConnected}
        onRefresh={refresh}
        onSettings={() => setShowSettings(true)}
        onDiag={() => setShowDiag(true)}
        flash={flash}
      />
      <main className="main">
        {ov && ov.sessions.length > 0 && (
          <div className="banner">
            <span className="pulse" />
            <span className="banner-text">
              {ov.sessions.map((s) => `${s.name}（${platformLabel(s.platform)}）`).join("、")} 正在使用本机终端
            </span>
            {ov.sessions.map((s) => (
              <button key={s.session} className="btn danger small" onClick={() => api.kick(s.session).then(refresh)}>
                断开 {ov.sessions.length > 1 ? s.name : ""}
              </button>
            ))}
          </div>
        )}
        {tabs.length > 0 ? (
          <>
            <div className="tabs">
              {tabs.map((t) => (
                <div
                  key={t.key}
                  className={`tab ${t.key === activeKey ? "active" : ""} ${t.state}`}
                  onClick={() => setActiveKey(t.key)}
                >
                  <span className={`dot ${t.state === "open" || t.state === "opening" ? paths[t.connId]?.kind ?? "unknown" : "off"}`} />
                  <span className="tab-name">{t.info.name}</span>
                  <button
                    className="tab-close"
                    title="关闭"
                    onClick={(e) => {
                      e.stopPropagation();
                      closeTab(t.key);
                    }}
                  >
                    ×
                  </button>
                </div>
              ))}
              {activeTab && activeTab.state !== "closed" && (
                <button className="tab-add" title="在这台设备上再开一个终端" onClick={() => addTab(activeTab.connId, activeTab.info, activeTab.code)}>
                  +
                </button>
              )}
              <div className="tabs-spacer" />
              {activeTab && (
                <div className={`path-badge ${activeTab.state === "closed" ? "off" : paths[activeTab.connId]?.kind ?? "unknown"}`} title={paths[activeTab.connId]?.remote ?? ""}>
                  {activeTab.state === "closed" ? "已断开" : pathLabel(paths[activeTab.connId])}
                </div>
              )}
            </div>
            <div className="terms">
              {tabs.map((t) => (
                <div key={t.key} className="term-wrap" style={{ display: t.key === activeKey ? "flex" : "none" }}>
                  <TerminalView
                    connId={t.connId}
                    active={t.key === activeKey}
                    onReady={() => setTabs((ts) => ts.map((x) => (x.key === t.key && x.state === "opening" ? { ...x, state: "open" } : x)))}
                    onStatus={(s) => setTabState(t.key, s)}
                  />
                  {(t.state === "closed" || t.state === "exited") && (
                    <div className="term-overlay">
                      <div className="overlay-card">
                        <div className="overlay-title">{t.state === "exited" ? "终端已结束" : "连接已断开"}</div>
                        <div className="overlay-note">{t.note}</div>
                        <div className="overlay-actions">
                          {t.state === "exited" && (
                            <button className="btn primary" onClick={() => addTab(t.connId, t.info, t.code)}>
                              再开一个终端
                            </button>
                          )}
                          <button className="btn" onClick={() => closeTab(t.key)}>
                            关闭标签
                          </button>
                        </div>
                      </div>
                    </div>
                  )}
                </div>
              ))}
            </div>
          </>
        ) : (
          <Empty ov={ov} />
        )}
      </main>
      {requests.length > 0 && <ApproveDialog key={requests[0].id} req={requests[0]} onRespond={respond} />}
      {showSettings && ov && <SettingsDialog ov={ov} onClose={() => setShowSettings(false)} onChanged={refresh} />}
      {showDiag && <DiagDialog onClose={() => setShowDiag(false)} />}
      {toast && <div className="toast">{toast}</div>}
    </div>
  );
}

function ServerDot({ ov }: { ov: Overview | null }) {
  const s = ov?.server;
  if (!s) return null;
  const map = {
    none: ["off", "未配置牵线服务器"],
    connecting: ["warn", "正在连接服务器"],
    online: ["ok", "在线"],
    offline: ["bad", "离线"],
  } as const;
  const [cls, text] = map[s.state];
  return (
    <span className={`server ${cls}`} title={s.state === "offline" ? s.error : ""}>
      <span className="server-dot" />
      {text}
    </span>
  );
}

function Sidebar(props: {
  ov: Overview | null;
  onConnected: (connId: number, info: RemoteInfo, code: string | null, path: PathInfo) => void;
  onRefresh: () => void;
  onSettings: () => void;
  onDiag: () => void;
  flash: (m: string) => void;
}) {
  const { ov, flash } = props;
  const [target, setTarget] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const pwRef = useRef<HTMLInputElement>(null);

  const copy = (text: string, what: string) => {
    navigator.clipboard.writeText(text).then(() => flash(`${what}已复制`));
  };

  if (window.__ptTest) window.__ptTest.connect = (t, pw) => doConnect(t, pw);

  const doConnect = async (t: string, pw: string) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const c = await api.connect(t, pw || null);
      props.onConnected(c.conn_id, c.info, c.code, c.path);
      setPassword("");
    } catch (e) {
      const f = e as ConnectFail;
      setError(f.message ?? String(e));
      if (f.reject === "need_password" || f.reject === "bad_password") {
        setTarget(formatCode(t.replace(/\D/g, "")) || t);
        window.setTimeout(() => pwRef.current?.focus(), 0);
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <aside className="sidebar">
      <div className="brand">
        <span className="brand-name">P2PCLI</span>
        <ServerDot ov={ov} />
      </div>

      <section className="card me">
        <div className="label">本机设备码</div>
        <div className="big-row">
          <span className="code">{ov?.code ? formatCode(ov.code) : ov?.server.state === "none" ? "未配置服务器" : "获取中…"}</span>
          {ov?.code && (
            <button className="icon-btn" title="复制" onClick={() => copy(ov.code!, "设备码")}>
              复制
            </button>
          )}
        </div>
        <div className="label">临时密码</div>
        <div className="big-row">
          <span className="code pw">{ov?.password ?? ""}</span>
          <button className="icon-btn" title="复制" onClick={() => ov && copy(ov.password, "临时密码")}>
            复制
          </button>
          <button
            className="icon-btn"
            title="换一个密码，旧密码立即失效"
            onClick={() => api.refreshPassword().then(() => { props.onRefresh(); flash("已换新密码"); })}
          >
            换一个
          </button>
        </div>
        <div className="me-name">
          {ov?.name} · {ov ? platformLabel(ov.platform) : ""}
        </div>
      </section>

      <section className="card">
        <div className="card-title">连接远程设备</div>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            doConnect(target, password);
          }}
        >
          <input
            className="input"
            placeholder="对方的设备码"
            value={target}
            onChange={(e) => setTarget(e.target.value)}
            spellCheck={false}
            autoComplete="off"
          />
          <input
            ref={pwRef}
            className="input"
            placeholder="对方的临时密码"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            spellCheck={false}
            autoComplete="off"
          />
          <button className="btn primary block" disabled={busy || !target.trim()}>
            {busy ? "等待对方同意…" : "连接"}
          </button>
        </form>
        {error && <div className="error">{error}</div>}
      </section>

      {ov && ov.recent.length > 0 && (
        <section className="card recent">
          <div className="card-title">最近连接</div>
          {ov.recent.map((r) => (
            <div key={r.code} className="recent-item" onClick={() => doConnect(r.code, password)} title="点一下直接连接">
              <div className="recent-main">
                <span className="recent-name">{r.name}</span>
                <span className="recent-meta">
                  {platformLabel(r.platform)} · {formatCode(r.code)}
                </span>
              </div>
              <button
                className="icon-btn subtle"
                title="从列表移除"
                onClick={(e) => {
                  e.stopPropagation();
                  api.forgetRecent(r.code).then(props.onRefresh);
                }}
              >
                ×
              </button>
            </div>
          ))}
        </section>
      )}

      <div className="sidebar-foot">
        <button className="link" onClick={props.onSettings}>
          设置{ov && ov.trusted.length > 0 ? ` · 受信设备 ${ov.trusted.length} 台` : ""}
        </button>
        <button className="link" onClick={props.onDiag}>
          网络诊断
        </button>
      </div>
    </aside>
  );
}

function Empty({ ov }: { ov: Overview | null }) {
  return (
    <div className="empty">
      <div className="empty-title">用对方电脑的终端</div>
      <ol className="empty-steps">
        <li>让对方打开 P2PCLI，把设备码和临时密码发给你</li>
        <li>在左边填好，点连接，等对方点同意</li>
        <li>连上后就像在对方电脑上开了个终端，claude、codex 这些命令行工具都能直接用</li>
      </ol>
      {ov && ov.server.state === "none" && <div className="error">还没配置牵线服务器，到左下角设置里填写。</div>}
      <div className="empty-note">连接优先 IPv6 直连，其次 IPv4 打洞，都不通才走中继；全程端到端加密。</div>
    </div>
  );
}

function ApproveDialog({ req, onRespond }: { req: IncomingRequest; onRespond: (r: IncomingRequest, allow: boolean, trust: boolean) => void }) {
  const [trust, setTrust] = useState(false);
  const [left, setLeft] = useState(APPROVE_SECONDS);
  useEffect(() => {
    const timer = window.setInterval(() => setLeft((s) => s - 1), 1000);
    return () => window.clearInterval(timer);
  }, []);
  useEffect(() => {
    if (left <= 0) onRespond(req, false, false);
  }, [left]);
  return (
    <div className="modal-backdrop">
      <div className="modal approve">
        <div className="modal-title">有人请求使用你的终端</div>
        <div className="approve-who">
          <span className="who-name">{req.name}</span>
          <span className="who-meta">{platformLabel(req.platform)}</span>
        </div>
        <div className="approve-id">设备指纹 {req.remote.slice(0, 16)}</div>
        <p className="warn-text">同意后，对方能在你的电脑上执行任何命令，和你自己坐在电脑前一样。只同意你认识的人。</p>
        <label className="check">
          <input type="checkbox" checked={trust} onChange={(e) => setTrust(e.target.checked)} />
          以后这台设备连我不用密码、不用确认（只给自己的电脑勾）
        </label>
        <div className="modal-actions">
          <button className="btn" onClick={() => onRespond(req, false, false)}>
            拒绝（{Math.max(left, 0)}）
          </button>
          <button className="btn primary" onClick={() => onRespond(req, true, trust)}>
            允许
          </button>
        </div>
      </div>
    </div>
  );
}

const RATING_CLASS: Record<Diagnosis["rating"], string> = { 好: "ok", 一般: "warn", 差: "bad", 未知: "off" };

function DiagDialog({ onClose }: { onClose: () => void }) {
  const [d, setD] = useState<Diagnosis | null>(null);
  const [busy, setBusy] = useState(false);
  const run = () => {
    setBusy(true);
    api
      .diagnose()
      .then(setD)
      .finally(() => setBusy(false));
  };
  useEffect(run, []);
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal diag" onClick={(e) => e.stopPropagation()}>
        <div className="modal-title">网络诊断</div>
        {!d && <div className="muted">正在检测，大约要几秒…</div>}
        {d && (
          <>
            <div className={`diag-rating ${RATING_CLASS[d.rating]}`}>
              <span className="diag-grade">直连条件：{d.rating}</span>
              <span className="diag-summary">{d.summary}</span>
            </div>
            <ul className="diag-notes">
              {d.notes.map((n, i) => (
                <li key={i}>{n}</li>
              ))}
            </ul>
            <div className="muted small">
              公网 IPv4：{d.ipv4_public ?? "未知"}
              {d.ipv6_addrs.length > 0 && ` · 公网 IPv6：${d.ipv6_addrs.join("、")}`}
            </div>
            <div className="muted small">NAT 类型用小米 STUN（stun.miwifi.com）检测，测不通时用中继探测的结果。</div>
          </>
        )}
        <div className="modal-actions">
          <button className="btn" onClick={run} disabled={busy}>
            {busy ? "检测中…" : "重新检测"}
          </button>
          <button className="btn primary" onClick={onClose}>
            关闭
          </button>
        </div>
      </div>
    </div>
  );
}

function SettingsDialog({ ov, onClose, onChanged }: { ov: Overview; onClose: () => void; onChanged: () => void }) {
  const [s, setS] = useState<SettingsView | null>(null);
  const [server, setServer] = useState("");
  const [relay, setRelay] = useState("");
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    api.getSettings().then((v) => {
      setS(v);
      setServer(v.server ?? "");
      setRelay(v.relay ?? "");
      setName(v.device_name ?? "");
    });
  }, []);
  const save = async () => {
    setError(null);
    try {
      await api.saveSettings(server, name, relay);
      await api.restart();
    } catch (e) {
      setError(String(e));
    }
  };
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal settings" onClick={(e) => e.stopPropagation()}>
        <div className="modal-title">设置</div>
        <label className="field">
          <span>设备名称（对方看到的名字，留空用电脑名）</span>
          <input className="input" value={name} placeholder={ov.name} onChange={(e) => setName(e.target.value)} />
        </label>
        <label className="field">
          <span>牵线服务器（公钥@IP:端口 走 UDP，wss://域名 走 WebSocket；留空用默认）</span>
          <input
            className="input mono"
            value={server}
            placeholder={s?.effective_server ?? "公钥@IP:端口 或 wss://域名"}
            onChange={(e) => setServer(e.target.value)}
            spellCheck={false}
          />
        </label>
        <label className="field">
          <span>中继服务器（打洞失败时转发加密数据，留空用默认）</span>
          <input
            className="input mono"
            value={relay}
            placeholder={s?.effective_relay ?? "https://relay.example.com"}
            onChange={(e) => setRelay(e.target.value)}
            spellCheck={false}
          />
        </label>
        <div className="field">
          <span>受信设备（连我不用密码、不用确认）</span>
          {ov.trusted.length === 0 && <div className="muted">没有</div>}
          {ov.trusted.map((t) => (
            <div key={t.endpoint_id} className="trusted-item">
              <span>
                {t.name} · {platformLabel(t.platform)}
              </span>
              <button className="btn danger small" onClick={() => api.removeTrusted(t.endpoint_id).then(onChanged)}>
                移除
              </button>
            </div>
          ))}
        </div>
        <div className="muted small">数据目录：{s?.data_dir}</div>
        <div className="muted small">本机公钥：{ov.endpoint_id}</div>
        {error && <div className="error">{error}</div>}
        <div className="modal-actions">
          <button className="btn" onClick={onClose}>
            取消
          </button>
          <button className="btn primary" onClick={save}>
            保存并重启
          </button>
        </div>
      </div>
    </div>
  );
}
