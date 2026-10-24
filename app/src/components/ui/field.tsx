import { Description, Label } from "@heroui/react";
import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";
export function FieldGroup({ className, ...props }: ComponentProps<"div">) {
  return (
    <div
      {...props}
      data-slot="field-group"
      className={cn("flex flex-col gap-5", className)}
    />
  );
}
export function Field({
  className,
  orientation = "vertical",
  ...props
}: ComponentProps<"div"> & { orientation?: "vertical" | "horizontal" }) {
  return (
    <div
      {...props}
      role="group"
      data-slot="field"
      className={cn(
        "flex gap-2",
        orientation === "horizontal"
          ? "flex-row items-start gap-3"
          : "flex-col",
        className,
      )}
    />
  );
}
export function FieldLabel({
  className,
  ...props
}: ComponentProps<typeof Label>) {
  return (
    <Label
      {...props}
      data-slot="field-label"
      className={cn("w-fit text-xs font-medium", className)}
    />
  );
}
export function FieldDescription({
  className,
  ...props
}: ComponentProps<typeof Description>) {
  return (
    <Description
      {...props}
      data-slot="field-description"
      className={cn("text-xs leading-relaxed text-muted-foreground", className)}
    />
  );
}
