import { useEffect, useRef, useState, type ReactNode } from "react";
import { Popover } from "@heroui/react";
import { getVersion } from "@tauri-apps/api/app";
import { Signal, SignalHigh, SignalLow, SignalMedium } from "lucide-react";
import { api, pathLabel, platformLabel, type ConnStats, type PathInfo, type RemoteInfo } from "../api";
import { Button } from "@/components/ui/button";

const PATH_NAMES = { ipv6: "IPv6 直连", ipv4: "IPv4 直连", relay: "中继", unknown: "连接中" } as const;

/** 延迟分档，和信号格对应 */
function quality(rtt: number | null | undefined) {
  if (rtt == null) return { Icon: Signal, level: "unknown" };
  if (rtt < 80) return { Icon: SignalHigh, level: "good" };
  if (rtt < 200) return { Icon: SignalMedium, level: "fair" };
  return { Icon: SignalLow, level: "poor" };
}

function rate(bytesPerSecond: number) {
  const bits = bytesPerSecond * 8;
  if (bits < 1000) return `${Math.round(bits)} bps`;
  if (bits < 1_000_000) return `${(bits / 1000).toFixed(bits < 10_000 ? 1 : 0)} kbps`;
  return `${(bits / 1_000_000).toFixed(1)} Mbps`;
}

/** “[240e::1]:7788”“1.2.3.4:5678”只留 IP；中继地址只留主机名 */
function host(addr: string | null | undefined) {
  if (!addr) return null;
  if (/^https?:\/\//.test(addr)) {
    try {
      // iroh 的中继地址带着结尾的点（relay.example.com.），显示时去掉
      return new URL(addr).hostname.replace(/\.$/, "");
    } catch {
      return addr;
    }
  }
  const v6 = /^\[(.+)\]:\d+$/.exec(addr);
  if (v6) return v6[1];
  return addr.replace(/:\d+$/, "");
}

/** 内网和本机地址：IPv4 走 NAT 时，对外的公网地址另外显示 */
function privateIp(ip: string) {
  return /^(10\.|192\.168\.|172\.(1[6-9]|2\d|3[01])\.|127\.|169\.254\.|f[cd]|fe80:)/i.test(ip);
}

interface Sample {
  at: number;
  stats: ConnStats;
}

/**
 * 底部栏右边的线路和延时，点开是这条连接的详情：网络（延时、按键回显、带宽、丢包）、通道（线路、加密、两头地址）、
 * 对方设备（版本、系统、会话管理器）。打开时每秒取一次统计，用前后两次的差算带宽和丢包率；关着不取
 */
export function ConnectionCard({ connId, info, path, echoLatency }: {
  connId: number;
  info: RemoteInfo;
  path?: PathInfo;
  echoLatency: () => number | null;
}) {
  const [open, setOpen] = useState(false);
  const [now, setNow] = useState<Sample | null>(null);
  const [before, setBefore] = useState<Sample | null>(null);
  const [echo, setEcho] = useState<number | null>(null);
  const [myVersion, setMyVersion] = useState<string | null>(null);
  const echoRef = useRef(echoLatency);
  echoRef.current = echoLatency;

  useEffect(() => {
    if (!open) return;
    let stop = false;
    let last: Sample | null = null;
    const tick = async () => {
      setEcho(echoRef.current());
      try {
        const sample = { at: performance.now(), stats: await api.connectionStats(connId) };
        if (stop) return;
        setBefore(last);
        setNow(sample);
        last = sample;
      } catch {
        // 连接刚断，卡片上留着最后一次的数
      }
    };
    void tick();
    const timer = window.setInterval(() => void tick(), 1000);
    getVersion().then(setMyVersion).catch(() => {});
    return () => {
      stop = true;
      window.clearInterval(timer);
      setBefore(null);
    };
  }, [open, connId]);

  const current = now?.stats.path ?? path;
  const rtt = current?.rtt_ms ?? null;
  const { Icon, level } = quality(rtt);

  // 两次取样的差：带宽按秒算；丢包率在这一秒发得够多时按这一秒算，发得少就看从连上到现在的
  let up: string | null = null;
  let down: string | null = null;
  let loss: string | null = null;
  if (now) {
    const s = now.stats;
    if (before) {
      const seconds = Math.max(0.001, (now.at - before.at) / 1000);
      up = rate(Math.max(0, s.sent_bytes - before.stats.sent_bytes) / seconds);
      down = rate(Math.max(0, s.recv_bytes - before.stats.recv_bytes) / seconds);
    }
    const sent = before ? s.sent_packets - before.stats.sent_packets : 0;
    const lost = before ? s.lost_packets - before.stats.lost_packets : 0;
    const ratio = sent >= 20 ? lost / sent : s.sent_packets ? s.lost_packets / s.sent_packets : 0;
    loss = `${(Math.max(0, ratio) * 100).toFixed(1)}%`;
  }

  const remoteHost = host(current?.remote);
  const local = now?.stats.local ?? null;
  const publicV4 = now?.stats.public_v4 ?? null;
  const localText = local
    ? local.includes(":") || !privateIp(local) || !publicV4 ? local : `${publicV4}（内网 ${local}）`
    : current?.kind === "relay" ? "经中继，不直连" : null;

  return (
    <Popover isOpen={open} onOpenChange={setOpen}>
      <Button variant="ghost" size="xs" className="terminal-status-path-button" data-level={level} aria-label={`连接详情：${pathLabel(current)}`} title="连接详情">
        <Icon className="connection-signal" />
        <span>{pathLabel(current)}</span>
      </Button>
      <Popover.Content placement="top end" offset={8} className="connection-card">
        <Popover.Dialog aria-label="连接详情" className="flex flex-col">
          <div className="connection-card-head" data-level={level}>
            <Icon className="connection-signal" />
            <Popover.Heading>网络延时</Popover.Heading>
            <strong>{rtt != null ? `${rtt} ms` : "—"}</strong>
          </div>
          <Section>
            <Row label="按键回显" value={echo != null ? `${echo} ms` : "敲几个字后显示"} hint="从按键到屏幕上出现字的时间，比网络延时多算了对方程序处理的时间" />
            <Row label="带宽占用" value={up && down ? `↑ ${up}  ↓ ${down}` : "统计中…"} />
            <Row label="丢包率" value={loss ?? "统计中…"} />
          </Section>
          <Section>
            <Row label="线路" value={current ? PATH_NAMES[current.kind] : "—"} />
            <Row label="加密" value="QUIC · TLS 1.3 端到端" />
            <Row label="对方地址" value={remoteHost ?? "—"} mono />
            <Row label="本机地址" value={localText ?? "—"} mono />
          </Section>
          <Section>
            <Row label="对方版本" value={info.version ? `${info.version}${myVersion && myVersion !== info.version ? `（本机 ${myVersion}）` : ""}` : "0.1.4 以前的版本"} />
            <Row label="对方系统" value={info.os ?? platformLabel(info.platform)} />
            <Row label="会话管理" value={info.multiplexer ?? "—"} />
          </Section>
        </Popover.Dialog>
      </Popover.Content>
    </Popover>
  );
}

function Section({ children }: { children: ReactNode }) {
  return <dl className="connection-card-section">{children}</dl>;
}

function Row({ label, value, hint, mono }: { label: string; value: string; hint?: string; mono?: boolean }) {
  return (
    <div className="connection-card-row" title={hint}>
      <dt>{label}</dt>
      <dd className={mono ? "font-mono" : undefined}>{value}</dd>
    </div>
  );
}
