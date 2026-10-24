import { Toast, toast as heroToast } from "@heroui/react";
import { Check, CircleAlert, Info, X } from "lucide-react";

export const toast = {
  success: (message: string) => heroToast.success(message),
  error: (message: string) => heroToast.danger(message),
};

export function Toaster() {
  return (
    <Toast.Provider placement="bottom end" width={360}>
      {({ toast: item }) => (
        <Toast toast={item} variant={item.content.variant}>
          <Toast.Indicator>
            {item.content.variant === "success" ? (
              <Check />
            ) : item.content.variant === "danger" ? (
              <CircleAlert />
            ) : (
              <Info />
            )}
          </Toast.Indicator>
          <Toast.Content>
            <Toast.Title>{item.content.title}</Toast.Title>
          </Toast.Content>
          <Toast.CloseButton aria-label="关闭通知">
            <X />
          </Toast.CloseButton>
        </Toast>
      )}
    </Toast.Provider>
  );
}
