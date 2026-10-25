import { Description, Dropdown, Header, Label } from "@heroui/react";
import { ChevronDown, LoaderCircle } from "lucide-react";
import type { AgentInfo } from "../api";
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

export const CLAUDE_EFFORTS: Choice[] = [
  { id: "auto", label: "自动", hint: "按模型的默认强度" },
  { id: "low", label: "低", hint: "low" },
  { id: "medium", label: "中", hint: "medium" },
  { id: "high", label: "高", hint: "high" },
  { id: "xhigh", label: "很高", hint: "xhigh" },
  { id: "max", label: "最高", hint: "max，有的模型没有" },
];

/** 权限模式，顺序照 Shift+Tab 轮换的顺序 */
export const CLAUDE_MODES: Choice[] = [
  { id: "default", label: "手动确认", hint: "改文件、跑命令都先问你" },
  { id: "acceptEdits", label: "自动改文件", hint: "改文件不问，跑命令还问" },
  { id: "plan", label: "只做计划", hint: "只看不改，先拿方案给你" },
  { id: "auto", label: "自动模式", hint: "它自己判断哪些要问你" },
  { id: "bypassPermissions", label: "全部放行", hint: "什么都不问；启动时开了才有" },
];
const MODE_LABELS: Record<string, string> = { dontAsk: "不问就拒" };

export const choiceLabel = (choices: Choice[], id: string | null | undefined) =>
  choices.find((c) => c.id === id)?.label ?? (id ? MODE_LABELS[id] ?? id : null);

export type AgentSetting = "model" | "effort" | "mode";

/** 输入框旁边的模型、思考强度、权限模式。Claude Code 点了直接切；Codex 的选项随账号变，打开它自己的选择框。
 *  在等你确认时都不让切：这时按的键会被当成选项；模型和强度要等它这一轮干完 */
export function AgentSettingsBar({ agent, switching, onSwitch, onPicker }: {
  agent: AgentInfo;
  /** 正在切哪一项 */
  switching: AgentSetting | null;
  onSwitch: (what: AgentSetting, id: string) => void;
  /** 在终端里打开 AI 自己的选择框，比如 Codex 的 /model */
  onPicker: (command: string) => void;
}) {
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
  const settings = agent.settings;
  if (agent.kind !== "claude" || !settings) return null;
  const busy = switching != null;
  const blocked = waiting ? `${agent.name} 在等你确认，先在终端里选` : undefined;
  const lockedByWork = blocked ?? (agent.state === "working" ? "等它这一轮干完再切" : undefined);
  const model = CLAUDE_MODELS.find((m) => settings.model?.toLowerCase().startsWith(m.id))?.id ?? null;
  const defaultNote = "也会成为那台电脑上新开 Claude Code 的默认值";
  return (
    <div className="agent-settings">
      <Chip
        title="模型"
        value={settings.model ?? "模型"}
        choices={CLAUDE_MODELS}
        current={model}
        note={defaultNote}
        spinning={switching === "model"}
        disabled={busy || !!lockedByWork}
        reason={lockedByWork}
        onPick={(id) => onSwitch("model", id)}
      />
      <Chip
        title="思考强度"
        value={`思考 ${choiceLabel(CLAUDE_EFFORTS, settings.effort) ?? "默认"}`}
        choices={CLAUDE_EFFORTS}
        current={settings.effort ?? null}
        note={defaultNote}
        spinning={switching === "effort"}
        disabled={busy || !!lockedByWork}
        reason={lockedByWork}
        onPick={(id) => onSwitch("effort", id)}
      />
      <Chip
        title="权限模式"
        value={choiceLabel(CLAUDE_MODES, settings.mode) ?? "权限"}
        choices={CLAUDE_MODES}
        current={settings.mode ?? null}
        note="只改这个会话，按 Shift+Tab 轮换过去"
        spinning={switching === "mode"}
        // 等你确认时底下是确认框，读不到当前模式，也切不了
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
