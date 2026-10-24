import { Separator as HeroSeparator } from "@heroui/react";
import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";
export function Separator({
  className,
  ...props
}: ComponentProps<typeof HeroSeparator>) {
  return (
    <HeroSeparator
      {...props}
      className={cn(
        "shrink-0 bg-border",
        typeof className === "string" && className,
      )}
    />
  );
}
