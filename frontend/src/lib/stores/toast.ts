// The old toast store, kept as a thin router so its ~500 call sites do not change.
//
// Errors and warnings are inline messages in the current page's tip slot; successes and plain
// information become toasts in the app toast region (`lib/ui/toast`). The display duration passed
// by old callers is ignored: the toast region owns timing.
/* eslint-disable @typescript-eslint/no-unused-vars -- `_duration` keeps old call sites compiling */
import { appToasts } from '#lib/ui/toast/app-toasts.js';
import { showError, showWarning } from '#lib/surface/feedback.svelte.js';

export const toast = {
	success: (message: string, _duration?: number) =>
		appToasts.show({ message, tone: 'success', key: `success:${message}` }),
	info: (message: string, _duration?: number) =>
		appToasts.show({ message, tone: 'neutral', key: `info:${message}` }),
	error: (message: string, _duration?: number) => showError(message),
	warning: (message: string, _duration?: number) => showWarning(message)
};
