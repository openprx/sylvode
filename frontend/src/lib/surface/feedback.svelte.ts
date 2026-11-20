import type { SurfaceTip } from '#lib/surface/context.js';
import { appToasts } from '#lib/ui/toast/app-toasts.js';

/** Puts a tip into the current page's tip slot; `null` clears it. */
export type TipSink = (tip: SurfaceTip | null) => void;

export const ERROR_TIP_ID = 'feedback:error';
export const WARNING_TIP_ID = 'feedback:warning';

let sink: TipSink | null = null;
let showing: 'danger' | 'warning' | null = null;

/**
 * The join point with the surface host. The shell calls this once with
 * `(tip) => host.setTip(tip)` for the standalone host and calls the returned function when the
 * tip slot goes away. While no sink is registered, feedback falls back to toasts.
 */
export function registerTipSink(next: TipSink): () => void {
	sink = next;
	showing = null;
	return () => {
		if (sink === next) {
			sink = null;
			showing = null;
		}
	};
}

export function hasTipSink(): boolean {
	return sink !== null;
}

function toast(message: string, persistent: boolean): void {
	appToasts.show({ message, tone: 'warning', persistent, key: `feedback:${message}` });
}

/** An error belongs to the page that failed: inline in its tip slot, else a persistent toast. */
export function showError(message: string): void {
	if (!sink) {
		toast(message, true);
		return;
	}
	sink({ id: ERROR_TIP_ID, tone: 'danger', message, dismissible: true });
	showing = 'danger';
}

/** A warning takes the tip slot unless an error is standing there; then it becomes a toast. */
export function showWarning(message: string): void {
	if (!sink || showing === 'danger') {
		toast(message, false);
		return;
	}
	sink({ id: WARNING_TIP_ID, tone: 'warning', message, dismissible: true });
	showing = 'warning';
}

/** Clears the inline message; call it when the route changes. */
export function clearFeedback(): void {
	if (sink && showing) sink(null);
	showing = null;
}
