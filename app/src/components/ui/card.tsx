import { Card as HeroCard } from "@heroui/react";
import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";
export function Card({ className, ...props }: ComponentProps<typeof HeroCard>) {
  return (
    <HeroCard
      {...props}
      data-slot="card"
      className={cn(
        "gap-6 rounded-xl border bg-card p-0 py-6 text-card-foreground",
        className,
      )}
    />
  );
}
export function CardHeader({
  className,
  ...props
}: ComponentProps<typeof HeroCard.Header>) {
  return (
    <HeroCard.Header
      {...props}
      data-slot="card-header"
      className={cn("gap-2 px-6", className)}
    />
  );
}
export function CardTitle({
  className,
  ...props
}: ComponentProps<typeof HeroCard.Title>) {
  return (
    <HeroCard.Title
      {...props}
      data-slot="card-title"
      className={cn("leading-none font-semibold", className)}
    />
  );
}
export function CardContent({
  className,
  ...props
}: ComponentProps<typeof HeroCard.Content>) {
  return (
    <HeroCard.Content
      {...props}
      data-slot="card-content"
      className={cn("px-6", className)}
    />
  );
}
export function CardFooter({
  className,
  ...props
}: ComponentProps<typeof HeroCard.Footer>) {
  return (
    <HeroCard.Footer
      {...props}
      data-slot="card-footer"
      className={cn("flex items-center px-6", className)}
    />
  );
}
