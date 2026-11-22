/**
 * Copies text to the clipboard. The one place the app does it: the Members page's created token
 * and the configuration block of a bot connection use the same helper.
 *
 * It copies exactly what it is given and nothing else: it neither reads the clipboard, nor clears
 * it, nor keeps the text, nor reports it anywhere. The caller decides what to tell the person; the
 * text is never part of that message.
 */

/** The slice of the browser the helper needs; tests pass fakes. */
export interface ClipboardEnv {
	clipboard?: { writeText(text: string): Promise<void> };
	/** The legacy path: a hidden textarea and `document.execCommand('copy')`. */
	legacy?: { copy(text: string): boolean };
}

function browserEnv(): ClipboardEnv {
	if (typeof navigator === 'undefined' || typeof document === 'undefined') return {};
	return {
		clipboard: navigator.clipboard,
		legacy: {
			copy(text: string): boolean {
				const area = document.createElement('textarea');
				area.value = text;
				area.setAttribute('readonly', '');
				area.style.position = 'fixed';
				area.style.opacity = '0';
				document.body.appendChild(area);
				try {
					area.select();
					return document.execCommand('copy');
				} finally {
					document.body.removeChild(area);
				}
			}
		}
	};
}

/** Resolves `true` when the text was copied, `false` when the browser refused or has no way. */
export async function copyText(text: string, env: ClipboardEnv = browserEnv()): Promise<boolean> {
	try {
		if (env.clipboard?.writeText) {
			await env.clipboard.writeText(text);
			return true;
		}
		return env.legacy?.copy(text) ?? false;
	} catch {
		return false;
	}
}
