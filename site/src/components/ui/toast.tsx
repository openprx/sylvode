import { Toast, toaster } from "@kobalte/core/toast";
import type { JSX } from "@solidjs/web";
import { Portal } from "@solidjs/web";
import { X } from "lucide";
import { Icon } from "./icon";

export type ToastTone = "neutral" | "success" | "danger";

const toneBorder: Record<ToastTone, string> = {
  neutral: "border-overlay-border",
  success: "border-success",
  danger: "border-danger",
};

/** Show a toast. Needs a mounted <Toaster /> to be visible. */
export function showToast(options: {
  title: string;
  description?: string;
  tone?: ToastTone;
}): number {
  const tone = options.tone ?? "neutral";
  return toaster.show((props) => (
    <Toast
      toastId={props.toastId}
      class={`flex items-start gap-3 w-80 bg-overlay border ${toneBorder[tone]} rounded-md shadow-overlay p-4 animate-[site-slide-in_150ms_ease-out]`}
    >
      <div class="flex-1 flex flex-col gap-1">
        <Toast.Title class="font-semibold text-body">{options.title}</Toast.Title>
        {options.description ? (
          <Toast.Description class="text-secondary text-small">
            {options.description}
          </Toast.Description>
        ) : null}
      </div>
      <Toast.CloseButton
        class="inline-flex size-6 items-center justify-center rounded-button hover:bg-hover cursor-pointer"
        aria-label="Dismiss"
      >
        <Icon icon={X} size={14} />
      </Toast.CloseButton>
    </Toast>
  ));
}

/** Mount once near the app root. */
export function Toaster(): JSX.Element {
  return (
    <Portal>
      <Toast.Region>
        <Toast.List class="fixed bottom-4 right-4 z-[60] flex flex-col gap-2 m-0 p-0 list-none outline-none" />
      </Toast.Region>
    </Portal>
  );
}
