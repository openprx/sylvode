import { Dialog } from "@kobalte/core/dialog";
import type { JSX } from "@solidjs/web";
import { X } from "lucide";
import { Show } from "solid-js";
import { cn } from "./cn";
import { Icon } from "./icon";

export interface ModalProps {
  /** Element that opens the dialog; rendered inside a Kobalte trigger. */
  trigger: JSX.Element;
  title: string;
  description?: string;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  class?: string;
  children?: JSX.Element;
  footer?: JSX.Element;
}

export function Modal(props: ModalProps): JSX.Element {
  return (
    <Dialog open={props.open} onOpenChange={props.onOpenChange}>
      <Dialog.Trigger as="span" class="inline-flex">
        {props.trigger}
      </Dialog.Trigger>
      <Dialog.Portal>
        <Dialog.Overlay class="fixed inset-0 z-40 bg-scrim animate-[site-fade-in_150ms_ease-out]" />
        <div class="fixed inset-0 z-50 flex items-center justify-center p-4">
          <Dialog.Content
            class={cn(
              "w-full max-w-md bg-overlay border border-overlay-border rounded-dialog shadow-modal p-6 flex flex-col gap-4 animate-[site-slide-in_150ms_ease-out]",
              props.class,
            )}
          >
            <div class="flex items-start justify-between gap-4">
              <div class="flex flex-col gap-1">
                <Dialog.Title class="text-subheading font-semibold m-0">{props.title}</Dialog.Title>
                <Show when={props.description}>
                  <Dialog.Description class="text-secondary text-small">
                    {props.description}
                  </Dialog.Description>
                </Show>
              </div>
              <Dialog.CloseButton
                class="inline-flex size-8 items-center justify-center rounded-button hover:bg-hover cursor-pointer"
                aria-label="Close"
              >
                <Icon icon={X} />
              </Dialog.CloseButton>
            </div>
            {props.children}
            <Show when={props.footer}>
              <div class="flex justify-end gap-2">{props.footer}</div>
            </Show>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog>
  );
}
