import { Chip } from "@heroui/react";
import type { ComponentProps } from "react";
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
      className={className}
    />
  );
}
