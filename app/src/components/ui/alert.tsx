import { Alert as HeroAlert } from "@heroui/react";
import { Children, isValidElement, type ComponentProps } from "react";
import { cn } from "@/lib/utils";

export function Alert({
  variant = "default",
  children,
  className,
  ...props
}: ComponentProps<"div"> & {
  variant?: "default" | "success" | "warning" | "destructive";
}) {
  const items = Children.toArray(children);
  const indicator = items.find(
    (child) =>
      isValidElement(child) &&
      child.type !== AlertTitle &&
      child.type !== AlertDescription,
  );
  return (
    <HeroAlert
      {...props}
      status={variant === "destructive" ? "danger" : variant}
      role="alert"
      className={cn("w-full text-sm [&_svg]:size-4", className)}
    >
      {indicator && <HeroAlert.Indicator>{indicator}</HeroAlert.Indicator>}
      <HeroAlert.Content>
        {items.filter((child) => child !== indicator)}
      </HeroAlert.Content>
    </HeroAlert>
  );
}
export const AlertTitle = HeroAlert.Title;
export function AlertDescription({
  className,
  ...props
}: ComponentProps<typeof HeroAlert.Description>) {
  return (
    <HeroAlert.Description
      {...props}
      className={cn(
        "grid justify-items-start gap-2 text-xs leading-relaxed",
        className,
      )}
    />
  );
}
