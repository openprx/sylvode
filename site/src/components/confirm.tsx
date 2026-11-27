import { Dialog } from "@kobalte/core/dialog";
import type { JSX } from "@solidjs/web";
import { createSignal } from "solid-js";
import { Button } from "./ui";

interface Pending {
  message: string;
  resolve: (ok: boolean) => void;
}

const [pending, setPending] = createSignal<Pending | null>(null);

/** Asks the visitor to confirm an action; resolves false on cancel. */
export function confirmAction(message: string): Promise<boolean> {
  return new Promise((resolve) => {
    setPending(() => ({ message, resolve }));
  });
}

function settle(ok: boolean): void {
  const current = pending();
  setPending(() => null);
  current?.resolve(ok);
}

/** Mount once near the app root. */
export function ConfirmHost(props: { confirmLabel: string; cancelLabel: string }): JSX.Element {
  return (
    <Dialog open={pending() !== null} onOpenChange={(open) => !open && settle(false)}>
      <Dialog.Portal>
        <Dialog.Overlay class="fixed inset-0 z-40 bg-scrim" />
        <div class="fixed inset-0 z-50 flex items-center justify-center p-4">
          <Dialog.Content class="w-full max-w-sm bg-overlay border border-overlay-border rounded-dialog shadow-modal p-6 flex flex-col gap-4">
            <Dialog.Title class="text-subheading font-semibold m-0">
              {pending()?.message}
            </Dialog.Title>
            <div class="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => settle(false)}>
                {props.cancelLabel}
              </Button>
              <Button onClick={() => settle(true)}>{props.confirmLabel}</Button>
            </div>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog>
  );
}
