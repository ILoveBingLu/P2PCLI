import { useRef, type ReactNode } from "react";
import { Description, Dropdown, Header, Label } from "@heroui/react";
import { Box, Brain, LoaderCircle, ShieldCheck, type LucideIcon } from "lucide-react";
import type { AgentInfo, AgentSettings } from "../api";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipTrigger } from "@/components/ui/tooltip";
import { t } from "@/i18n";

interface Choice {
  id: string;
  label: string;
  hint?: string;
}

/** Claude Code 的 /model 认这几个别名，具体是哪个版本由它按账号定 */
export const CLAUDE_MODELS: Choice[] = [
  { id: "opus", label: "Opus" },
  { id: "sonnet", label: "Sonnet" },
  { id: "haiku", label: "Haiku" },
  { id: "fable", label: "Fable" },
];

function effortLabels(): Record<string, string> {
  return { auto: t("自动"), minimal: t("最低"), low: t("低"), medium: t("中"), high: t("高"), xhigh: t("很高"), max: t("最高") };
}
const effortChoice = (id: string): Choice => ({ id, label: effortLabels()[id] ?? id, hint: id === "auto" ? t("按模型的默认强度") : id });

export function claudeEfforts(): Choice[] {
  return [
    effortChoice("auto"),
    effortChoice("low"),
    effortChoice("medium"),
    effortChoice("high"),
    effortChoice("xhigh"),
    { ...effortChoice("max"), hint: t("max，有的模型没有") },
  ];
}

/** 权限模式，顺序照 Shift+Tab 轮换的顺序 */
function claudeModes(): Choice[] {
  return [
    { id: "default", label: t("手动确认"), hint: t("改文件、跑命令都先问你") },
    { id: "acceptEdits", label: t("自动改文件"), hint: t("改文件不问，跑命令还问") },
    { id: "plan", label: t("只做计划"), hint: t("只看不改，先拿方案给你") },
    { id: "auto", label: t("自动模式"), hint: t("它自己判断哪些要问你") },
    { id: "bypassPermissions", label: t("全部放行"), hint: t("什么都不问；启动时开了才有") },
  ];
}
function zcodeModes(): Choice[] {
  return [
    { id: "build", label: t("正常确认"), hint: t("只读的直接做，改东西、跑命令先问你") },
    { id: "edit", label: t("自动改文件"), hint: t("改项目里的文件不问，别的还问") },
    { id: "yolo", label: t("全部放行"), hint: t("什么都不问") },
    { id: "plan", label: t("只做计划"), hint: t("只看不改，先拿方案给你") },
  ];
}
function modeLabels(): Record<string, string> {
  return { dontAsk: t("不问就拒") };
}

export const modeChoices = (kind: string) => (kind === "zcode" ? zcodeModes() : claudeModes());

export const choiceLabel = (choices: Choice[], id: string | null | undefined) =>
  choices.find((c) => c.id === id)?.label ?? (id ? modeLabels()[id] ?? id : null);

/** 设置条直接点就能切的 AI */
export const switchable = (kind: string) => kind === "claude" || kind === "zcode";

export type AgentSetting = "model" | "effort" | "mode";

/** 菜单底下那行：切了以后影响到哪儿，各家规矩不一样 */
function settingNotes(kind: string): Record<AgentSetting, string> | undefined {
  const notes: Record<string, Record<AgentSetting, string>> = {
    claude: {
      model: t("也会成为那台电脑上新开 Claude Code 的默认值"),
      effort: t("也会成为那台电脑上新开 Claude Code 的默认值"),
      mode: t("只改这个会话，按 Shift+Tab 轮换过去"),
    },
    zcode: {
      model: t("只改这个会话，默认模型不变"),
      effort: t("也会成为那台电脑上新开 ZCode 的默认强度"),
      mode: t("也会成为这个目录下新开 ZCode 的默认模式"),
    },
  };
  return notes[kind];
}

/** 输入框旁边的模型、思考强度、权限模式。Claude Code、ZCode 点了直接切；Codex 的选项随账号变，打开它自己的选择框。
 *  在等你确认时都不让切：这时按的键会被当成选项；模型和强度要等它这一轮干完 */
export function AgentSettingsBar({ agent, switching, onSwitch, onPicker, onPalette }: {
  agent: AgentInfo;
  /** 正在切哪一项 */
  switching: AgentSetting | null;
  onSwitch: (what: AgentSetting, id: string) => void;
  /** 在终端里打开 AI 自己的选择框，比如 Codex 的 /model */
  onPicker: (command: string) => void;
  /** 打开命令面板并搜索，只定位入口，不替用户确认权限变更。 */
  onPalette: (query: string) => void;
}) {
  // 等你确认时确认框会盖住 ZCode 的状态栏、Claude Code 的权限模式，读不到的项沿用上次的，按钮不跟着闪
  const last = useRef<AgentSettings>({});
  const now = agent.settings ?? {};
  const settings: AgentSettings = {
    ...now,
    model: now.model ?? last.current.model,
    effort: now.effort ?? last.current.effort,
    mode: now.mode ?? last.current.mode,
    models: now.models?.length ? now.models : last.current.models,
    efforts: now.efforts?.length ? now.efforts : last.current.efforts,
  };
  last.current = settings;

  const waiting = agent.state === "waiting";
  if (agent.kind === "opencode") {
    const disabled = agent.state !== "idle" || switching != null;
    const reason = waiting ? t("先在终端里完成当前确认") : agent.state === "working" ? t("等当前任务完成后再切换") : null;
    return (
      <div className="agent-settings">
        <IconAction label={t("模型")} icon={Box} disabled={disabled}
          tip={reason ? t("模型：{{reason}}", { reason }) : t("模型：打开 OpenCode 的模型列表")} onPress={() => onPicker("/models")} />
        <IconAction label={t("思考强度")} icon={Brain} disabled={disabled}
          tip={reason ? t("思考强度：{{reason}}", { reason }) : t("思考强度：选择当前模型支持的档位")} onPress={() => onPicker("/variants")} />
        <IconAction label={t("权限")} icon={ShieldCheck} disabled={disabled}
          tip={reason ? t("权限：{{reason}}", { reason }) : t("权限：打开命令入口，在终端内确认（默认 Ctrl+P）")} onPress={() => onPalette("permissions")} />
      </div>
    );
  }
  if (agent.kind === "codex") {
    return (
      <div className="agent-settings">
        <IconAction label={t("模型和思考强度")} icon={Box} disabled={waiting}
          tip={waiting ? t("模型和思考强度：先在终端里完成当前确认") : t("模型和思考强度：打开 Codex 的选择菜单")} onPress={() => onPicker("/model")} />
        <IconAction label={t("权限")} icon={ShieldCheck} disabled={waiting}
          tip={waiting ? t("权限：先在终端里完成当前确认") : t("权限：打开 Codex 的权限选择")} onPress={() => onPicker("/permissions")} />
      </div>
    );
  }
  if (!switchable(agent.kind) || !(settings.model || settings.mode)) return null;

  const zcode = agent.kind === "zcode";
  const models: Choice[] = zcode ? (settings.models ?? []).map((m) => ({ id: m.id, label: m.name, hint: m.id })) : CLAUDE_MODELS;
  // Claude Code 显示的是“Opus 5.5”，按别名对；ZCode 显示的就是 ID
  const model = zcode
    ? models.find((m) => m.id.toLowerCase() === settings.model?.toLowerCase())?.id ?? null
    : CLAUDE_MODELS.find((m) => settings.model?.toLowerCase().startsWith(m.id))?.id ?? null;
  const modelText = zcode ? choiceLabel(models, model) ?? settings.model?.split("/").pop() : settings.model;
  const efforts = zcode ? (settings.efforts ?? []).map(effortChoice) : claudeEfforts();
  const notes = settingNotes(agent.kind) ?? { model: "", effort: "", mode: "" };

  const busy = switching != null;
  const blocked = waiting ? t("{{name}} 在等你确认，先在终端里选", { name: agent.name }) : undefined;
  const lockedByWork = blocked ?? (agent.state === "working" ? t("等它这一轮干完再切") : undefined);
  return (
    <div className="agent-settings">
      <Chip
        icon={Box}
        title={t("模型")}
        value={modelText ?? t("模型")}
        choices={models}
        current={model}
        note={notes.model}
        spinning={switching === "model"}
        disabled={busy || !!lockedByWork || !models.length}
        reason={lockedByWork}
        onPick={(id) => onSwitch("model", id)}
      />
      {(efforts.length > 0 || !zcode) && (
        <Chip
          icon={Brain}
          title={t("思考强度")}
          value={choiceLabel(efforts, settings.effort) ?? t("默认")}
          choices={efforts}
          current={settings.effort ?? null}
          note={notes.effort}
          spinning={switching === "effort"}
          disabled={busy || !!lockedByWork}
          reason={lockedByWork}
          onPick={(id) => onSwitch("effort", id)}
        />
      )}
      <Chip
        icon={ShieldCheck}
        title={t("权限模式")}
        value={choiceLabel(modeChoices(agent.kind), settings.mode) ?? t("权限")}
        choices={modeChoices(agent.kind)}
        current={settings.mode ?? null}
        note={notes.mode}
        spinning={switching === "mode"}
        // 等你确认时读不到当前模式，也切不了
        disabled={busy || waiting || !settings.mode}
        reason={blocked}
        onPick={(id) => onSwitch("mode", id)}
      />
    </div>
  );
}

function ControlTip({ tip, children }: { tip: string; children: ReactNode }) {
  return (
    <Tooltip content={tip}>
      <TooltipTrigger<"span"> render={(props) => <span {...props} />}
        role="presentation" tabIndex={-1} className="agent-chip-wrap">
        {children}
      </TooltipTrigger>
    </Tooltip>
  );
}

function IconAction({ label, icon: Icon, tip, disabled, onPress }: {
  label: string; icon: LucideIcon; tip: string; disabled: boolean; onPress: () => void;
}) {
  return (
    <ControlTip tip={tip}>
      <Button variant="ghost" size="icon-sm" className="agent-chip" aria-label={label}
        isDisabled={disabled} onPress={onPress}>
        <Icon aria-hidden="true" />
      </Button>
    </ControlTip>
  );
}

function Chip({ icon: Icon, title, value, choices, current, note, spinning, disabled, reason, onPick }: {
  icon: LucideIcon;
  title: string;
  value: string;
  choices: Choice[];
  current: string | null;
  note: string;
  spinning: boolean;
  disabled: boolean;
  /** 为什么现在不能切，鼠标放上去看得到 */
  reason?: string;
  onPick: (id: string) => void;
}) {
  return (
    <Dropdown>
      <ControlTip tip={`${title}：${value}${reason ? ` · ${reason}` : ""}`}>
        <Dropdown.Trigger className="agent-chip" isDisabled={disabled} aria-label={`${title}：${value}`} aria-busy={spinning}>
          {spinning ? <LoaderCircle className="animate-spin" aria-hidden="true" /> : <Icon aria-hidden="true" />}
        </Dropdown.Trigger>
      </ControlTip>
      <Dropdown.Popover placement="top end" className="agent-chip-menu">
        <Dropdown.Menu
          aria-label={title}
          selectionMode="single"
          disallowEmptySelection
          selectedKeys={current ? [current] : []}
          onSelectionChange={(keys) => {
            const id = keys === "all" ? null : [...keys][0];
            if (id != null && id !== current) onPick(String(id));
          }}
        >
          <Dropdown.Section>
            <Header>{title}</Header>
            {choices.map((c) => (
              <Dropdown.Item key={c.id} id={c.id} textValue={c.label}>
                <div className="flex min-w-0 flex-col">
                  <Label>{c.label}</Label>
                  {c.hint && <Description>{c.hint}</Description>}
                </div>
                <Dropdown.ItemIndicator />
              </Dropdown.Item>
            ))}
          </Dropdown.Section>
        </Dropdown.Menu>
        <p className="agent-chip-note">{note}</p>
      </Dropdown.Popover>
    </Dropdown>
  );
}
