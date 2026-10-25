import { useRef } from "react";
import { Description, Dropdown, Header, Label } from "@heroui/react";
import { ChevronDown, LoaderCircle } from "lucide-react";
import type { AgentInfo, AgentSettings } from "../api";
import { Button } from "@/components/ui/button";

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

const EFFORT_LABELS: Record<string, string> = { auto: "自动", minimal: "最低", low: "低", medium: "中", high: "高", xhigh: "很高", max: "最高" };
const effortChoice = (id: string): Choice => ({ id, label: EFFORT_LABELS[id] ?? id, hint: id === "auto" ? "按模型的默认强度" : id });

export const CLAUDE_EFFORTS: Choice[] = [
  effortChoice("auto"),
  effortChoice("low"),
  effortChoice("medium"),
  effortChoice("high"),
  effortChoice("xhigh"),
  { ...effortChoice("max"), hint: "max，有的模型没有" },
];

/** 权限模式，顺序照 Shift+Tab 轮换的顺序 */
const CLAUDE_MODES: Choice[] = [
  { id: "default", label: "手动确认", hint: "改文件、跑命令都先问你" },
  { id: "acceptEdits", label: "自动改文件", hint: "改文件不问，跑命令还问" },
  { id: "plan", label: "只做计划", hint: "只看不改，先拿方案给你" },
  { id: "auto", label: "自动模式", hint: "它自己判断哪些要问你" },
  { id: "bypassPermissions", label: "全部放行", hint: "什么都不问；启动时开了才有" },
];
const ZCODE_MODES: Choice[] = [
  { id: "build", label: "正常确认", hint: "只读的直接做，改东西、跑命令先问你" },
  { id: "edit", label: "自动改文件", hint: "改项目里的文件不问，别的还问" },
  { id: "yolo", label: "全部放行", hint: "什么都不问" },
  { id: "plan", label: "只做计划", hint: "只看不改，先拿方案给你" },
];
const MODE_LABELS: Record<string, string> = { dontAsk: "不问就拒" };

export const modeChoices = (kind: string) => (kind === "zcode" ? ZCODE_MODES : CLAUDE_MODES);

export const choiceLabel = (choices: Choice[], id: string | null | undefined) =>
  choices.find((c) => c.id === id)?.label ?? (id ? MODE_LABELS[id] ?? id : null);

/** 设置条直接点就能切的 AI */
export const switchable = (kind: string) => kind === "claude" || kind === "zcode";

export type AgentSetting = "model" | "effort" | "mode";

/** 菜单底下那行：切了以后影响到哪儿，各家规矩不一样 */
const NOTES: Record<string, Record<AgentSetting, string>> = {
  claude: {
    model: "也会成为那台电脑上新开 Claude Code 的默认值",
    effort: "也会成为那台电脑上新开 Claude Code 的默认值",
    mode: "只改这个会话，按 Shift+Tab 轮换过去",
  },
  zcode: {
    model: "只改这个会话，默认模型不变",
    effort: "也会成为那台电脑上新开 ZCode 的默认强度",
    mode: "也会成为这个目录下新开 ZCode 的默认模式",
  },
};

/** 输入框旁边的模型、思考强度、权限模式。Claude Code、ZCode 点了直接切；Codex 的选项随账号变，打开它自己的选择框。
 *  在等你确认时都不让切：这时按的键会被当成选项；模型和强度要等它这一轮干完 */
export function AgentSettingsBar({ agent, switching, onSwitch, onPicker }: {
  agent: AgentInfo;
  /** 正在切哪一项 */
  switching: AgentSetting | null;
  onSwitch: (what: AgentSetting, id: string) => void;
  /** 在终端里打开 AI 自己的选择框，比如 Codex 的 /model */
  onPicker: (command: string) => void;
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
  if (agent.kind === "codex") {
    return (
      <div className="agent-settings">
        <Button variant="ghost" size="xs" className="agent-chip" isDisabled={waiting} title="在终端里打开 Codex 的模型和推理强度选择" onPress={() => onPicker("/model")}>
          模型
        </Button>
        <Button variant="ghost" size="xs" className="agent-chip" isDisabled={waiting} title="在终端里打开 Codex 的权限选择" onPress={() => onPicker("/permissions")}>
          权限
        </Button>
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
  const efforts = zcode ? (settings.efforts ?? []).map(effortChoice) : CLAUDE_EFFORTS;
  const notes = NOTES[agent.kind];

  const busy = switching != null;
  const blocked = waiting ? `${agent.name} 在等你确认，先在终端里选` : undefined;
  const lockedByWork = blocked ?? (agent.state === "working" ? "等它这一轮干完再切" : undefined);
  return (
    <div className="agent-settings">
      <Chip
        title="模型"
        value={modelText ?? "模型"}
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
          title="思考强度"
          value={`思考 ${choiceLabel(efforts, settings.effort) ?? "默认"}`}
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
        title="权限模式"
        value={choiceLabel(modeChoices(agent.kind), settings.mode) ?? "权限"}
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

function Chip({ title, value, choices, current, note, spinning, disabled, reason, onPick }: {
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
      {/* 禁用的按钮收不到鼠标事件，提示挂在外面一层 */}
      <span className="agent-chip-wrap" title={reason ?? title}>
        <Dropdown.Trigger className="agent-chip" isDisabled={disabled} aria-label={`${title}：${value}`}>
          {spinning && <LoaderCircle className="size-3 animate-spin" />}
          <span className="truncate">{value}</span>
          <ChevronDown className="size-3 shrink-0 opacity-60" />
        </Dropdown.Trigger>
      </span>
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
