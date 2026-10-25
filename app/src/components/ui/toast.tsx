import { Toast, toast as heroToast } from "@heroui/react";
import { Check, CircleAlert, Info, X } from "lucide-react";

export const toast = {
  success: (message: string) => heroToast.success(message),
  error: (message: string) => heroToast.danger(message),
  /** 提醒：可以带一行说明和一个按钮，warning 时用黄色 */
  notice: (message: string, options: { description?: string; action?: { label: string; onPress: () => void }; warning?: boolean } = {}) =>
    (options.warning ? heroToast.warning : heroToast.info)(message, {
      description: options.description,
      actionProps: options.action && { children: options.action.label, onPress: options.action.onPress },
    }),
};

export function Toaster() {
  return (
    <Toast.Provider placement="top" width={360}>
      {({ toast: item }) => (
        <Toast toast={item} variant={item.content.variant}>
          <Toast.Indicator>
            {item.content.variant === "success" ? (
              <Check />
            ) : item.content.variant === "danger" || item.content.variant === "warning" ? (
              <CircleAlert />
            ) : (
              <Info />
            )}
          </Toast.Indicator>
          <Toast.Content>
            <Toast.Title>{item.content.title}</Toast.Title>
            {item.content.description && <Toast.Description>{item.content.description}</Toast.Description>}
          </Toast.Content>
          {item.content.actionProps && <Toast.ActionButton {...item.content.actionProps} />}
          <Toast.CloseButton aria-label="关闭通知">
            <X />
          </Toast.CloseButton>
        </Toast>
      )}
    </Toast.Provider>
  );
}
