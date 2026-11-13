import { AgentView, DeviceView, PathView, SessionView } from './Model';
import { Hero } from './Hero';
import { t } from './I18n';

// 设备、会话、AI 状态显示成文字：和安卓的 Common.kt、iOS 的 Format.swift 一样

export function pathText(path: PathView | null): string {
  if (path === null) {
    return t('连接中');
  }
  let kind = '';
  switch (path.kind) {
    case 'ipv6': kind = t('IPv6 直连'); break;
    case 'ipv4': kind = t('IPv4 直连'); break;
    case 'relay': kind = t('中继'); break;
    case 'local': kind = t('本机'); break;
    default: return t('连接中');
  }
  return path.rtt_ms !== null ? `${kind} · ${path.rtt_ms} ms` : kind;
}

export class StatusText {
  text: string = '';
  color: string = Hero.muted;

  constructor(text: string, color: string) {
    this.text = text;
    this.color = color;
  }
}

/** 设备一行的状态：文字和颜色 */
export function presenceOf(d: DeviceView, connected: boolean): StatusText {
  if (connected) {
    return new StatusText(t('已连接'), Hero.success);
  }
  switch (d.presence) {
    case 'online': {
      const rtt = d.path !== null && d.path.rtt_ms !== null ? ` · ${d.path.rtt_ms} ms` : '';
      const text = (d.lan_online ? t('局域网在线') : t('在线')) + rtt;
      return new StatusText(text, d.path !== null && d.path.kind === 'relay' ? Hero.warning : Hero.success);
    }
    case 'checking': return new StatusText(t('检测中…'), Hero.muted);
    case 'old_version': return new StatusText(t('对方需要更新 P2PCLI'), Hero.warning);
    default: return new StatusText(t('离线'), Hero.muted);
  }
}

export function agentText(a: AgentView): string {
  if (a.stalled) {
    return t('可能卡住');
  }
  if (a.state === 'working') {
    return t('干活中');
  }
  if (a.state === 'waiting') {
    return t('等你确认');
  }
  return t('空闲');
}

export function agentColor(a: AgentView): string {
  if (a.stalled || a.state === 'waiting') {
    return Hero.warning;
  }
  if (a.state === 'working') {
    return Hero.accent;
  }
  return Hero.muted;
}

/** 会话的名字：有备注用备注；跑着 AI 用 AI 的名字（同名的加编号）；不然按创建先后叫“会话 1、会话 2” */
export function sessionTitles(sessions: SessionView[]): Map<string, string> {
  const sorted = sessions.slice().sort((a: SessionView, b: SessionView) =>
    a.created_at !== b.created_at ? a.created_at - b.created_at : (a.id < b.id ? -1 : 1));
  const numbers = new Map<string, string>();
  sorted.forEach((s: SessionView, i: number) => numbers.set(s.id, t('会话 {{value}}', { value: i + 1 } as Record<string, string | number>)));
  const counts = new Map<string, number>();
  sessions.forEach((s: SessionView) => {
    if (s.agent !== null) {
      counts.set(s.agent.name, (counts.get(s.agent.name) ?? 0) + 1);
    }
  });
  const out = new Map<string, string>();
  sessions.forEach((s: SessionView) => {
    const num = numbers.get(s.id) ?? t('会话');
    if (s.remark !== null && s.remark.trim().length > 0) {
      out.set(s.id, s.remark);
    } else if (s.agent !== null) {
      out.set(s.id, (counts.get(s.agent.name) ?? 0) > 1 ? `${s.agent.name} · ${num}` : s.agent.name);
    } else {
      out.set(s.id, num);
    }
  });
  return out;
}

/** 刚刚、3 分钟前、2 小时前、5 天前 */
export function relativeTime(secs: number): string {
  const diff = Math.floor(Date.now() / 1000) - secs;
  if (diff < 60) {
    return t('刚刚');
  }
  if (diff < 3600) {
    return t('{{value}} 分钟前', { value: Math.floor(diff / 60) } as Record<string, string | number>);
  }
  if (diff < 86400) {
    return t('{{value}} 小时前', { value: Math.floor(diff / 3600) } as Record<string, string | number>);
  }
  return t('{{value}} 天前', { value: Math.floor(diff / 86400) } as Record<string, string | number>);
}

/** /Users/me/Projects/x → ~/Projects/x，太长只留最后两段 */
export function shortPath(path: string | null): string {
  if (path === null || path.trim().length === 0) {
    return '';
  }
  const home = path.replace(new RegExp('^/(Users|home)/[^/]+'), '~').replace(new RegExp('^[A-Za-z]:\\\\Users\\\\[^\\\\]+'), '~');
  const parts = home.split(new RegExp('[/\\\\]')).filter((p: string) => p.length > 0);
  return parts.length > 3 ? '…/' + parts.slice(parts.length - 2).join('/') : home;
}
