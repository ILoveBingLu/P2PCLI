import {
  Button as HeroButton,
  type ButtonProps as HeroButtonProps,
} from "@heroui/react";
import { cn } from "@/lib/utils";

type Props = Omit<HeroButtonProps, "variant" | "size" | "className"> & {
  variant?:
    "default" | "destructive" | "outline" | "secondary" | "ghost" | "link";
  size?:
    "default" | "xs" | "sm" | "lg" | "icon" | "icon-xs" | "icon-sm" | "icon-lg";
  disabled?: boolean;
  className?: string;
  title?: string;
};
const variants = {
  default: "primary",
  destructive: "danger-soft",
  outline: "outline",
  secondary: "secondary",
  ghost: "ghost",
  link: "ghost",
} as const;

export function Button({
  variant = "default",
  size = "default",
  disabled,
  className,
  title,
  ...props
}: Props) {
  return (
    <HeroButton
      {...props}
      data-slot="button"
      variant={variants[variant]}
      render={(domProps) => <button {...domProps} title={title} />}
      size={
        size === "lg" || size === "icon-lg"
          ? "lg"
          : size === "default" || size === "icon"
            ? "md"
            : "sm"
      }
      isIconOnly={size.startsWith("icon")}
      isDisabled={disabled ?? props.isDisabled}
      className={cn(
        "shrink-0 gap-2 rounded-md text-sm [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0",
        size === "default" && "h-9 px-4",
        size === "xs" && "h-6 px-2 text-xs",
        size === "sm" && "h-8 px-3 text-xs",
        size === "icon-xs" && "size-6 min-w-0 p-0",
        size === "icon-sm" && "size-8 min-w-0 p-0",
        size === "icon" && "size-9 min-w-0 p-0",
        variant === "link" && "text-primary underline-offset-4 hover:underline",
        className,
      )}
    />
  );
}
