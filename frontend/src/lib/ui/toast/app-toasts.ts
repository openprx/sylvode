import { Toasts } from './toast-store';

/**
 * The app's one toast store. `lib/components/Toast.svelte` renders it in a `ToastRegion`; the
 * toast routing (`lib/stores/toast.ts`, `lib/surface/feedback.svelte.ts`) writes to it.
 */
export const appToasts = new Toasts();
