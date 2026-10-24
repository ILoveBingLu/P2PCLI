import { Input as HeroInput } from "@heroui/react";
import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";
type Props = ComponentProps<"input"> &
  Pick<ComponentProps<typeof HeroInput>, "variant">;

export function Input({ className, ...props }: Props) {
  return (
    <HeroInput
      {...props}
      data-slot="input"
      className={cn("w-full min-w-0", className)}
    />
  );
}
