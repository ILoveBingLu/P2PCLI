import { Input as HeroInput } from "@heroui/react";
import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";
export function Input({ className, ...props }: ComponentProps<"input">) {
  return (
    <HeroInput
      {...props}
      data-slot="input"
      className={cn(
        "h-9 w-full min-w-0 rounded-md border border-input px-3 py-1 text-sm",
        className,
      )}
    />
  );
}
