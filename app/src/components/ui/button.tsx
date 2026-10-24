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
        "shrink-0",
        variant === "link" && "underline-offset-4 hover:underline",
        className,
      )}
    />
  );
}
