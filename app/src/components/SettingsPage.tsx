import { useCallback, useEffect, useState } from "react";
import { ArrowDownUp, BookOpen, Bot, Check, Download, FolderOpen, Globe2, Info, Network, LoaderCircle, RefreshCw, Save, ShieldCheck, SquareTerminal, Trash2 } from "lucide-react";
import { Tabs, Checkbox, Description, Radio, RadioGroup } from "@heroui/react";
import { api, platformLabel, type CliStatus, type InstalledAgent, type Overview, type SettingsView } from "../api";
import { AboutPanel } from "./AboutPanel";
import { AiHintSettings } from "./AiHintSettings";
import { AgentIcon } from "./Agent";
import { Badge } from "@/components/ui/badge";
import { toast } from "@/components/ui/toast";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Field, FieldDescription, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { OverlayScroll } from "./OverlayScroll";
import { PlatformIcon } from "./PlatformIcon";
import { type useUpdater } from "./useUpdater";

/** 牵线服务器选“自定义”时的取值，预设项的取值就是它的地址 */
const CUSTOM_SERVER = "custom";

/** 左边菜单的分类，右边标题也用这个名字 */
const SECTIONS = [
  { id: "general", label: "连接设置", icon: Globe2 },
  { id: "proxy", label: "代理设置", icon: Network },
  { id: "files", label: "文件传输", icon: ArrowDownUp },
  { id: "agents", label: "AI 启动", icon: Bot },
  { id: "aihint", label: "多机互动", icon: BookOpen },
  { id: "trusted", label: "受信设备", icon: ShieldCheck },
  { id: "about", label: "关于", icon: Info },
];

export function SettingsPage({
  ov,
  onChanged,
  updater,
}: {
  ov: Overview;
  onChanged: () => void;
  updater: ReturnType<typeof useUpdater>;
}) {
  const [tab, setTab] = useState("general");
  const [settings, setSettings] = useState<SettingsView | null>(null);
  const [serverChoice, setServerChoice] = useState(CUSTOM_SERVER);
  const [customServer, setCustomServer] = useState("");
  const [relay, setRelay] = useState("");
  const [proxyEnabled, setProxyEnabled] = useState(false);
  const [lanVisible, setLanVisible] = useState(true);
  const [proxy, setProxy] = useState("");
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [removing, setRemoving] = useState<string | null>(null);
  const [cli, setCli] = useState<CliStatus | null>(null);
  const [installingCli, setInstallingCli] = useState(false);
  const [agents, setAgents] = useState<InstalledAgent[]>([]);
  const [agentCommands, setAgentCommands] = useState<Record<string, string>>({});
  const [agentsLoading, setAgentsLoading] = useState(true);
  const [agentsError, setAgentsError] = useState<string | null>(null);
  useEffect(() => {
    api.cliStatus().then(setCli).catch(() => {});
  }, []);
  const installCli = async () => {
    setInstallingCli(true);
    try {
      setCli(await api.installCli());
      toast.success("命令行工具装好了，新开的终端窗口里就能用");
    } catch (e) {
      toast.error(String(e));
    } finally {
      setInstallingCli(false);
    }
  };
  const loadAgents = useCallback(async (refresh = false) => {
    setAgentsLoading(true);
    setAgentsError(null);
    try {
      setAgents(await api.localInstalledAgents(refresh));
    } catch (e) {
      setAgentsError(String(e));
    } finally {
      setAgentsLoading(false);
    }
  }, []);
  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const value = await api.getSettings();
      setSettings(value);
      setAgentCommands(value.agent_commands);
      // 没设置过就是默认服务器；设置的地址正好是某个预设就选中它，否则当作自定义
      const preset = value.server_presets.find((p) => p.address === (value.server ?? value.default_server));
      setServerChoice(preset ? preset.address : CUSTOM_SERVER);
      setCustomServer(preset ? "" : (value.server ?? ""));
      setRelay(value.relay ?? "");
      setName(value.device_name ?? "");
      setProxyEnabled(!!value.proxy);
      setLanVisible(value.lan_visible);
      setProxy(value.proxy ?? "");
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);
  useEffect(() => {
    void load();
    void loadAgents();
  }, [load, loadAgents]);
  const save = async () => {
    if (!settings || saving) return;
    setSaving(true);
    setError(null);
    try {
      // 选默认服务器时存空值，以后默认地址换了也跟着走
      const server = serverChoice === CUSTOM_SERVER
        ? customServer
        : serverChoice === settings.default_server ? null : serverChoice;
      await api.saveSettings(server, name, relay, proxyEnabled ? proxy : null, lanVisible, agentCommands);
      onChanged();
      toast.success("设置已保存并立即应用");
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };
  const removeTrusted = async (id: string, paired = false) => {
    setRemoving(id);
    try {
      await (paired ? api.removePaired(id) : api.removeTrusted(id));
      onChanged();
      toast.success(paired ? "已移除配对设备" : "已移除受信设备");
    } catch (e) {
      toast.error(String(e));
    } finally {
      setRemoving(null);
    }
  };
  const current = SECTIONS.find((item) => item.id === tab) ?? SECTIONS[0];
  return (
    <section className="settings-page" aria-labelledby="settings-title">
      <Tabs orientation="vertical" selectedKey={tab} onSelectionChange={(key) => setTab(String(key))} className="settings-tabs">
        <nav className="settings-nav">
          <h1 id="settings-title" className="settings-nav-title">设置</h1>
          <Tabs.List aria-label="设置分类">
            {SECTIONS.map(({ id, label, icon: Icon }) => (
              <Tabs.Tab key={id} id={id}>
                <Icon />
                {label}
                {id === "trusted" && <span className="settings-nav-count">{ov.trusted.length}</span>}
                <Tabs.Indicator />
              </Tabs.Tab>
            ))}
          </Tabs.List>
        </nav>
        <div className="settings-main">
          <h2 className="settings-main-title">{current.label}</h2>
          <OverlayScroll className="settings-scroll">
            <div className="settings-content">
              <Tabs.Panel id="general">
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
                    <RadioGroup
                      value={serverChoice}
                      onChange={setServerChoice}
                      isDisabled={saving}
                    >
                      <FieldLabel>牵线服务器</FieldLabel>
                      {settings.server_presets.map((p) => (
                        <Radio key={p.address} value={p.address}>
                          <Radio.Content className="gap-3 text-sm">
                            <Radio.Control><Radio.Indicator /></Radio.Control>
                            {p.label}
                            {p.address === settings.default_server && <Badge variant="secondary">默认</Badge>}
                          </Radio.Content>
                          <Description className="pl-7 text-xs text-muted-foreground">
                            {p.note}。<span className="font-mono">{p.address}</span>
                          </Description>
                        </Radio>
                      ))}
                      <Radio value={CUSTOM_SERVER}>
                        <Radio.Content className="gap-3 text-sm">
                          <Radio.Control><Radio.Indicator /></Radio.Control>
                          自定义
                        </Radio.Content>
                        <Description className="pl-7 text-xs text-muted-foreground">
                          填自己部署的牵线服务器：公钥@IP或域名:端口 走 UDP，wss://域名 走 WebSocket。
                        </Description>
                      </Radio>
                    </RadioGroup>
                    {serverChoice === CUSTOM_SERVER && (
                      <Field data-disabled={saving} className="-mt-2 pl-7">
                        <Input
                          id="server-address"
                          aria-label="自定义牵线服务器地址"
                          variant="secondary"
                          value={customServer}
                          placeholder="公钥@IP:端口 或 wss://域名"
                          onChange={(e) => { setCustomServer(e.target.value); }}
                          spellCheck={false}
                          autoComplete="off"
                          disabled={saving}
                        />
                      </Field>
                    )}
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
                    <Checkbox isSelected={lanVisible} onChange={setLanVisible} isDisabled={saving}>
                      <Checkbox.Content className="gap-3 text-sm">
                        <Checkbox.Control><Checkbox.Indicator><Check className="size-3" /></Checkbox.Indicator></Checkbox.Control>
                        在局域网中显示本机
                      </Checkbox.Content>
                      <Description className="pl-7 text-xs text-muted-foreground">
                        同一网络里的其他 P2PCLI 能看到本机名称，点一下就能发起连接，仍然要配对密码和你的同意。在公共 Wi-Fi 上可以关掉。
                      </Description>
                    </Checkbox>
                  </FieldGroup>
                )}
              </Tabs.Panel>
              <Tabs.Panel id="proxy">
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
              <Tabs.Panel id="files">
                <FieldGroup className="gap-6">
                  <Field>
                    <FieldLabel>收到的文件放在</FieldLabel>
                    <div className="flex min-w-0 items-center gap-2">
                      <code className="min-w-0 flex-1 truncate rounded-md bg-muted-surface px-2 py-1.5 text-xs" title={ov.inbox_dir}>{ov.inbox_dir}</code>
                      <Button variant="outline" size="sm" onClick={() => void api.openFolder(ov.inbox_dir).catch((e) => toast.error(String(e)))}>
                        <FolderOpen data-icon="inline-start" />
                        打开
                      </Button>
                    </div>
                    <FieldDescription>
                      按发送的设备分文件夹，重名的自动加编号，不会覆盖。配对过的设备、你连过的设备发来的直接收；别的设备发来要你点同意。
                    </FieldDescription>
                  </Field>
                  <Field>
                    <FieldLabel>发文件</FieldLabel>
                    <FieldDescription>
                      在左边设备上右键选发送文件；或者把文件拖进某台设备的终端窗口，发完会把文件在对方电脑上的路径填进终端，正在用的 AI 直接就能读。
                    </FieldDescription>
                  </Field>
                  <Field>
                    <FieldLabel>命令行工具 p2pcli</FieldLabel>
                    <FieldDescription>
                      终端里的 AI 也能用它在设备之间传文件、拉文件、跑命令，要 P2PCLI 开着。
                      跑命令、拉文件和推到指定目录用的是和开终端一样的认证：先在界面里连过一次，对方没勾受信时每次连接要它点同意。
                    </FieldDescription>
                    <pre className="overflow-x-auto rounded-md bg-muted-surface px-3 py-2 text-xs leading-relaxed [font-variant-ligatures:none]">{[
                      "p2pcli devices                      列出设备、在线和延迟",
                      "p2pcli send 设备名 文件...            发文件或文件夹，打印对方电脑上的路径",
                      "p2pcli send 设备名 文件... --to 目录   放到对方电脑的这个目录，同名盖掉",
                      "p2pcli pull 设备名 对方的路径...       把对方电脑上的文件拉到当前目录（--to 换目录）",
                      "p2pcli exec 设备名 -- 命令            在对方电脑上跑一条命令，退出码带回来",
                      "p2pcli inbox                        看最近收到的文件",
                      "p2pcli devices --json               加 --json 输出给程序和 AI 读",
                    ].join("\n")}</pre>
                    <div className="flex flex-wrap items-center gap-2">
                      <SquareTerminal className="size-4 text-muted-foreground" />
                      <span className="min-w-0 flex-1 text-xs text-muted-foreground">
                        {!cli
                          ? "正在检查…"
                          : !cli.bundled
                            ? "开发版没有带命令行工具，正式安装包里才有。"
                            : cli.installed && cli.on_path
                              ? `已装好：${cli.path}，终端里直接用 p2pcli。`
                              : cli.installed
                                ? `已装在 ${cli.path}，但这个目录还不在终端的 PATH 里，可以用完整路径，或者点右边加进去。`
                                : "还没装到终端里。"}
                      </span>
                      {cli?.bundled && !(cli.installed && cli.on_path) && (
                        <Button size="sm" variant="outline" onClick={() => void installCli()} disabled={installingCli}>
                          {installingCli ? <LoaderCircle data-icon="inline-start" className="animate-spin" /> : <Download data-icon="inline-start" />}
                          {cli.installed ? "加到 PATH" : "安装"}
                        </Button>
                      )}
                    </div>
                  </Field>
                </FieldGroup>
              </Tabs.Panel>
              <Tabs.Panel id="agents">
                <FieldGroup className="gap-5">
                  <Alert>
                    <AlertDescription>
                      为这台电脑上的每种 AI 设置默认启动命令。其他设备连接本机后点击启动胶囊时会使用它；命令只保存在本机，不会传给连接方。留空并保存即可恢复自动检测到的默认命令。
                    </AlertDescription>
                  </Alert>
                  <div className="flex items-center gap-2">
                    <p className="detail-note mr-auto">可填写完整路径、环境变量和启动参数。命令会交给本机终端 Shell 执行，请勿把令牌或密码写在这里。</p>
                    <Button variant="ghost" size="sm" onClick={() => void loadAgents(true)} disabled={agentsLoading || saving}>
                      <RefreshCw data-icon="inline-start" className={agentsLoading ? "animate-spin" : undefined} />重新检测
                    </Button>
                  </div>
                  {agentsLoading ? (
                    <div className="loading-state" role="status"><LoaderCircle className="animate-spin" />正在检测本机 AI…</div>
                  ) : agentsError ? (
                    <Alert variant="destructive"><AlertDescription>{agentsError}</AlertDescription></Alert>
                  ) : agents.length === 0 ? (
                    <Empty>
                      <EmptyHeader><EmptyMedia variant="icon"><Bot /></EmptyMedia><EmptyTitle>未检测到 AI</EmptyTitle><EmptyDescription>安装到终端 PATH 后点击“重新检测”。</EmptyDescription></EmptyHeader>
                    </Empty>
                  ) : agents.map((agent) => (
                    <Field key={agent.kind} data-disabled={saving}>
                      <div className="flex items-center gap-2">
                        <AgentIcon kind={agent.kind} className="size-4" />
                        <FieldLabel htmlFor={`agent-command-${agent.kind}`} className="mr-auto">{agent.name}</FieldLabel>
                        {(agentCommands[agent.kind] ?? "").trim() && (
                          <Button variant="ghost" size="sm" onClick={() => setAgentCommands((current) => {
                            const next = { ...current };
                            delete next[agent.kind];
                            return next;
                          })} disabled={saving}>恢复默认</Button>
                        )}
                      </div>
                      <Input
                        id={`agent-command-${agent.kind}`}
                        variant="secondary"
                        value={agentCommands[agent.kind] ?? ""}
                        placeholder={agent.command}
                        onChange={(e) => setAgentCommands((current) => ({ ...current, [agent.kind]: e.target.value }))}
                        disabled={saving}
                        spellCheck={false}
                        autoComplete="off"
                        maxLength={2048}
                      />
                      <FieldDescription>默认命令：<code>{agent.command}</code></FieldDescription>
                      {agent.kind === "claude" && (
                        <FieldDescription>
                          需要启动后跳过 AI 工具确认时，可填 <code>claude --dangerously-skip-permissions</code>。保存后普通点击胶囊也会全部放行，仅在可信项目中使用；这不等于 sudo，系统权限仍不变。
                        </FieldDescription>
                      )}
                    </Field>
                  ))}
                </FieldGroup>
              </Tabs.Panel>
              <Tabs.Panel id="aihint">
                <AiHintSettings />
              </Tabs.Panel>
              <Tabs.Panel id="trusted">
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
                <h3 className="mt-8 mb-1 text-sm font-medium">配对过的设备（{ov.paired.length}）</h3>
                <p className="detail-note mb-4">
                  你同意过一次的设备。它们能在后台看到本机在不在线、延迟多少，以及它自己开在本机上的会话（只有编号、状态，和里面的 AI 在干活还是在等它，看不到目录、程序和屏幕内容）。打开终端照常要配对密码和你的同意，受信设备除外。移除后就看不到了，下次你再同意它连接时会重新加进来。
                </p>
                {ov.paired.length === 0 ? (
                  <p className="text-xs text-muted-foreground">暂无配对过的设备。</p>
                ) : (
                  <div className="trusted-list">
                    {ov.paired.map((device) => (
                      <div className="trusted-device" key={device.endpoint_id}>
                        <div className="device-symbol">
                          <PlatformIcon platform={device.platform} />
                        </div>
                        <div className="min-w-0 flex-1">
                          <strong className="block truncate">{device.name}</strong>
                          <span>{platformLabel(device.platform)}</span>
                        </div>
                        <Button
                          variant="destructive"
                          size="sm"
                          disabled={removing !== null}
                          onClick={() => removeTrusted(device.endpoint_id, true)}
                        >
                          {removing === device.endpoint_id ? (
                            <LoaderCircle data-icon="inline-start" className="animate-spin" />
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
              <Tabs.Panel id="about">
                <AboutPanel updater={updater} />
              </Tabs.Panel>
              {error && (
                <Alert variant="destructive">
                  <AlertDescription>{error}</AlertDescription>
                </Alert>
              )}
            </div>
          </OverlayScroll>
          {/* 切到别的分类再回来，没保存的修改还在；AI 启动命令与连接配置共用同一份本机设置。 */}
          {(tab === "general" || tab === "proxy" || tab === "agents") && (
            <footer className="settings-footer">
              <span className="mr-auto text-xs text-muted-foreground">{tab === "agents" ? "保存后立即用于之后的 AI 启动，不会中断现有会话。" : "保存后即时生效。代理、中继或局域网显示变更会重连网络，后台终端会话保留。"}</span>
              <Button
                onClick={save}
                disabled={loading || !settings || saving || (proxyEnabled && !proxy.trim()) || (serverChoice === CUSTOM_SERVER && !customServer.trim())}
              >
                {saving ? (
                  <LoaderCircle data-icon="inline-start" className="animate-spin" />
                ) : <Save data-icon="inline-start" />}
                保存设置
              </Button>
            </footer>
          )}
        </div>
      </Tabs>
    </section>
  );
}
