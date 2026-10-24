import { Tooltip as HeroTooltip } from "@heroui/react";
import type { ReactNode } from "react";

export function Tooltip({
  content,
  children,
}: {
  content: string;
  children: ReactNode;
}) {
  return (
    <HeroTooltip delay={400} closeDelay={0}>
      {children}
      <HeroTooltip.Content placement="bottom">{content}</HeroTooltip.Content>
    </HeroTooltip>
  );
}

export const TooltipTrigger = HeroTooltip.Trigger;
