import { useCallback, useEffect, useState } from "react";
import { ArrowLeft, Check, Globe2, Network, LoaderCircle, ShieldCheck, Trash2 } from "lucide-react";
import { Tabs, Checkbox, Description } from "@heroui/react";
import { api, platformLabel, type Overview, type SettingsView } from "../api";
import { toast } from "@/components/ui/toast";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Field, FieldDescription, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { PlatformIcon } from "./PlatformIcon";

export function SettingsPage({
  ov,
  onClose,
  onChanged,
}: {
  ov: Overview;
  onClose: () => void;
  onChanged: () => void;
}) {
  const [settings, setSettings] = useState<SettingsView | null>(null);
  const [server, setServer] = useState("");
  const [relay, setRelay] = useState("");
  const [proxyEnabled, setProxyEnabled] = useState(false);
  const [proxy, setProxy] = useState("");
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [removing, setRemoving] = useState<string | null>(null);
  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const value = await api.getSettings();
      setSettings(value);
      setServer(value.server ?? "");
      setRelay(value.relay ?? "");
      setName(value.device_name ?? "");
      setProxyEnabled(!!value.proxy);
      setProxy(value.proxy ?? "");
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);
  const save = async () => {
    if (!settings || saving) return;
    setSaving(true);
    setError(null);
    try {
      await api.saveSettings(server, name, relay, proxyEnabled ? proxy : null);
      onChanged();
      toast.success("设置已保存并立即应用");
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };
  const removeTrusted = async (id: string) => {
    setRemoving(id);
    try {
      await api.removeTrusted(id);
      onChanged();
      toast.success("已移除受信设备");
    } catch (e) {
      toast.error(String(e));
    } finally {
      setRemoving(null);
    }
  };
  return (
    <section className="settings-page" aria-labelledby="settings-title">
      <div className="settings-page-inner">
        <header className="flex items-center gap-3">
          <Button variant="ghost" size="icon-sm" aria-label="返回工作区" title="返回工作区" onClick={onClose} disabled={saving}>
            <ArrowLeft />
          </Button>
          <div>
            <h1 id="settings-title" className="text-lg font-semibold">设置</h1>
            <p className="text-sm text-muted-foreground">管理本机、网络代理和受信设备。</p>
          </div>
        </header>
        <Tabs defaultSelectedKey="general">
          <Tabs.List aria-label="设置分类" className="w-full">
            <Tabs.Tab id="general">
              <Globe2 />
              连接设置
              <Tabs.Indicator />
            </Tabs.Tab>
            <Tabs.Tab id="proxy">
              <Network />
              代理设置
              <Tabs.Indicator />
            </Tabs.Tab>
            <Tabs.Tab id="trusted">
              <ShieldCheck />
              受信设备（{ov.trusted.length}）
              <Tabs.Indicator />
            </Tabs.Tab>
          </Tabs.List>
          <div>
            <Tabs.Panel id="general" className="pt-5">
              {loading ? (
                <div className="loading-state" role="status">
                  <LoaderCircle className="animate-spin" />
                  正在读取设置…
                </div>
              ) : !settings ? (
                <Button variant="outline" onClick={load}>
                  重新加载设置
                </Button>
              ) : (
                <FieldGroup className="gap-5">
                  <Field data-disabled={saving}>
                    <FieldLabel htmlFor="device-name">设备名称</FieldLabel>
                    <Input
                      id="device-name"
                      variant="secondary"
                      value={name}
                      placeholder={ov.name}
                      onChange={(e) => { setName(e.target.value); }}
                      disabled={saving}
                    />
                    <FieldDescription>
                      对方看到的名称，留空使用电脑名称。
                    </FieldDescription>
                  </Field>
                  <Field data-disabled={saving}>
                    <FieldLabel htmlFor="server-address">牵线服务器</FieldLabel>
                    <Input
                      id="server-address"
                      variant="secondary"
                      value={server}
                      placeholder={
                        settings.effective_server ??
                        "公钥@IP:端口 或 wss://域名"
                      }
                      onChange={(e) => { setServer(e.target.value); }}
                      spellCheck={false}
                      disabled={saving}
                    />
                    <FieldDescription>
                      公钥@IP:端口 走 UDP，wss://域名 走
                      WebSocket。留空使用默认配置。
                    </FieldDescription>
                  </Field>
                  <Field data-disabled={saving}>
                    <FieldLabel htmlFor="relay-address">中继服务器</FieldLabel>
                    <Input
                      id="relay-address"
                      variant="secondary"
                      value={relay}
                      placeholder={
                        settings.effective_relay ?? "https://relay.example.com"
                      }
                      onChange={(e) => { setRelay(e.target.value); }}
                      spellCheck={false}
                      disabled={saving}
                    />
                    <FieldDescription>
                      直连失败时转发加密数据，多个地址用逗号分隔。
                    </FieldDescription>
                  </Field>
                  <Separator />
                  <div className="settings-details">
                    <span>数据目录</span>
                    <code>{settings.data_dir}</code>
                    <span>本机公钥</span>
                    <code>{ov.endpoint_id}</code>
                  </div>
                </FieldGroup>
              )}
            </Tabs.Panel>
            <Tabs.Panel id="proxy" className="pt-5">
              {loading ? (
                <div className="loading-state" role="status"><LoaderCircle className="animate-spin" />正在读取设置…</div>
              ) : !settings ? (
                <Button variant="outline" onClick={load}>重新加载设置</Button>
              ) : (
                <FieldGroup className="gap-5">
                  <Checkbox isSelected={proxyEnabled} onChange={(enabled) => { setProxyEnabled(enabled); }} isDisabled={saving}>
                    <Checkbox.Content className="gap-3 text-sm">
                      <Checkbox.Control><Checkbox.Indicator><Check className="size-3" /></Checkbox.Indicator></Checkbox.Control>
                      使用 HTTP 代理
                    </Checkbox.Content>
                    <Description className="pl-7 text-xs text-muted-foreground">
                      用于 WebSocket 牵线和 HTTP(S) 中继连接，点对点直连不经过代理。
                    </Description>
                  </Checkbox>
                  <Field data-disabled={saving || !proxyEnabled}>
                    <FieldLabel htmlFor="proxy-address">代理地址</FieldLabel>
                    <Input id="proxy-address" variant="secondary" value={proxy}
                      placeholder="http://127.0.0.1:7890"
                      onChange={(e) => { setProxy(e.target.value); }}
                      disabled={saving || !proxyEnabled} spellCheck={false} autoComplete="off" />
                    <FieldDescription>填写 HTTP 或混合代理端口，使用 http:// 开头；暂不支持 SOCKS5 和账号密码认证。关闭后直接连接。</FieldDescription>
                  </Field>
                </FieldGroup>
              )}
            </Tabs.Panel>
            <Tabs.Panel id="trusted" className="pt-5">
              <p className="detail-note mb-4">
                受信设备连接本机时无需密码和确认。移除后，再次连接需要重新授权。
              </p>
              {ov.trusted.length === 0 ? (
                <Empty>
                  <EmptyHeader>
                    <EmptyMedia variant="icon">
                      <ShieldCheck />
                    </EmptyMedia>
                    <EmptyTitle>暂无受信设备</EmptyTitle>
                    <EmptyDescription>
                      在收到连接请求时，可以选择信任自己的设备。
                    </EmptyDescription>
                  </EmptyHeader>
                </Empty>
              ) : (
                <div className="trusted-list">
                  {ov.trusted.map((device) => (
                    <div className="trusted-device" key={device.endpoint_id}>
                      <div className="device-symbol">
                        <PlatformIcon platform={device.platform} />
                      </div>
                      <div className="min-w-0 flex-1">
                        <strong className="block truncate">
                          {device.name}
                        </strong>
                        <span>{platformLabel(device.platform)}</span>
                      </div>
                      <Button
                        variant="destructive"
                        size="sm"
                        disabled={removing !== null}
                        onClick={() => removeTrusted(device.endpoint_id)}
                      >
                        {removing === device.endpoint_id ? (
                          <LoaderCircle
                            data-icon="inline-start"
                            className="animate-spin"
                          />
                        ) : (
                          <Trash2 data-icon="inline-start" />
                        )}
                        移除
                      </Button>
                    </div>
                  ))}
                </div>
              )}
            </Tabs.Panel>
          </div>
        </Tabs>
        {error && (
          <Alert variant="destructive">
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        )}
        <footer className="flex flex-wrap items-center justify-end gap-3 border-t border-border pt-5">
          <span className="mr-auto text-xs text-muted-foreground">保存后即时生效。代理或中继变更会重连网络，后台终端会话保留。</span>
          <Button variant="outline" onClick={onClose} disabled={saving}>
            返回工作区
          </Button>
          <Button onClick={save} disabled={loading || !settings || saving || (proxyEnabled && !proxy.trim())}>
            {saving && (
              <LoaderCircle data-icon="inline-start" className="animate-spin" />
            )}
            保存设置
          </Button>
        </footer>
      </div>
    </section>
  );
}
