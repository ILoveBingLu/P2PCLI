import { AlertDialog as HeroAlertDialog } from "@heroui/react";
import {
  createContext,
  useContext,
  useId,
  type ComponentProps,
  type ReactNode,
} from "react";
import { cn } from "@/lib/utils";

const DescriptionId = createContext<string | undefined>(undefined);
export function AlertDialog({
  open,
  children,
}: {
  open: boolean;
  children: ReactNode;
}) {
  const id = useId();
  return (
    <DescriptionId.Provider value={id}>
      <HeroAlertDialog.Backdrop
        isOpen={open}
        isDismissable={false}
        isKeyboardDismissDisabled
        className="z-50 bg-black/60"
      >
        {children}
      </HeroAlertDialog.Backdrop>
    </DescriptionId.Provider>
  );
}
export function AlertDialogContent({ children }: { children: ReactNode }) {
  const descriptionId = useContext(DescriptionId);
  return (
    <HeroAlertDialog.Container placement="center" size="lg">
      <HeroAlertDialog.Dialog
        aria-describedby={descriptionId}
        className="grid gap-4 rounded-xl border bg-background p-6 text-foreground"
      >
        {children}
      </HeroAlertDialog.Dialog>
    </HeroAlertDialog.Container>
  );
}
export const AlertDialogHeader = HeroAlertDialog.Header;
export const AlertDialogTitle = HeroAlertDialog.Heading;
export function AlertDialogFooter({
  className,
  ...props
}: ComponentProps<typeof HeroAlertDialog.Footer>) {
  return (
    <HeroAlertDialog.Footer
      {...props}
      className={cn("flex justify-end gap-2", className)}
    />
  );
}
export function AlertDialogDescription(props: ComponentProps<"p">) {
  const id = useContext(DescriptionId);
  return (
    <p
      {...props}
      id={id}
      className={cn("text-sm text-muted-foreground", props.className)}
    />
  );
}
