import { Chip } from "@heroui/react";
import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";
type Props = ComponentProps<"span"> & {
  children: React.ReactNode;
  variant?:
    "default" | "secondary" | "outline" | "success" | "warning" | "destructive";
};
export function Badge({ variant = "default", className, ...props }: Props) {
  const color =
    variant === "destructive"
      ? "danger"
      : variant === "success" || variant === "warning"
        ? variant
        : "default";
  return (
    <Chip
      {...props}
      color={color}
      size="sm"
      variant={variant === "outline" ? "tertiary" : "soft"}
      className={cn(
        "h-auto min-h-5 gap-1 rounded-full px-2 py-0.5 text-[10px] [&_svg]:size-3",
        variant === "outline" &&
          "border border-border bg-transparent text-muted-foreground",
        className,
      )}
    />
  );
}
