import { Modal } from "@heroui/react";
import { X } from "lucide-react";
import {
  createContext,
  useContext,
  useId,
  type ComponentProps,
  type ReactNode,
} from "react";
import { cn } from "@/lib/utils";

const DescriptionId = createContext<string | undefined>(undefined);
export function Dialog({
  open,
  onOpenChange,
  children,
}: {
  open: boolean;
  onOpenChange?: (open: boolean) => void;
  children: ReactNode;
}) {
  const id = useId();
  return (
    <DescriptionId.Provider value={id}>
      <Modal.Backdrop
        isOpen={open}
        onOpenChange={onOpenChange}
        isDismissable
        className="z-50 bg-black/60"
      >
        {children}
      </Modal.Backdrop>
    </DescriptionId.Provider>
  );
}
export function DialogContent({
  className,
  children,
}: {
  className?: string;
  children: ReactNode;
}) {
  const descriptionId = useContext(DescriptionId);
  return (
    <Modal.Container placement="center" size="lg">
      <Modal.Dialog
        aria-describedby={descriptionId}
        className={cn(
          "relative grid gap-4 rounded-xl border bg-background p-6 text-foreground",
          className,
        )}
      >
        {children}
        <Modal.CloseTrigger
          aria-label="关闭"
          className="absolute right-4 top-4"
        >
          <X className="size-4" />
        </Modal.CloseTrigger>
      </Modal.Dialog>
    </Modal.Container>
  );
}
export const DialogHeader = Modal.Header;
export const DialogTitle = Modal.Heading;
export function DialogFooter({
  className,
  ...props
}: ComponentProps<typeof Modal.Footer>) {
  return (
    <Modal.Footer
      {...props}
      className={cn("flex justify-end gap-2", className)}
    />
  );
}
export function DialogDescription(props: ComponentProps<"p">) {
  const id = useContext(DescriptionId);
  return (
    <p
      {...props}
      id={id}
      className={cn("text-sm text-muted-foreground", props.className)}
    />
  );
}
