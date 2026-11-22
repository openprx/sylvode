/**
 * The server refuses with `field: reason` (a 400) or a business 409. This maps a message to the
 * field that shows it, so the reason appears under the input it is about; anything else is a
 * message for the tip bar.
 */

export type ErrorField = 'name' | 'bot' | 'url' | 'directory' | 'channel' | 'other';

const FIELDS: Array<[RegExp, ErrorField]> = [
	[/^bot\.name:\s*/, 'name'],
	[/^bot\.id:\s*/, 'bot'],
	[/^runner_url:\s*/, 'url'],
	[/^working_directory:\s*/, 'directory'],
	[/^channel:\s*/, 'channel'],
	[/^cli:\s*/, 'channel']
];

export function routeServerMessage(message: string): { field: ErrorField; text: string } {
	for (const [pattern, field] of FIELDS) {
		if (pattern.test(message)) return { field, text: message.replace(pattern, '') };
	}
	return { field: 'other', text: message };
}

const TMUX_TAKEN = /^project already has a tmux connection for (\S+) \(bot (.+)\)$/;

/**
 * The 409 of a second tmux connection for a CLI: `channel: project already has a tmux connection
 * for <cli> (bot <name>)`. The bot it names, so the form can say "<bot> already uses tmux
 * session <KEY>-<cli>" as it does before submitting; `null` for any other message.
 */
export function tmuxTakenBy(message: string): { cli: string; bot: string } | null {
	const body = routeServerMessage(message);
	const match = body.field === 'channel' ? TMUX_TAKEN.exec(body.text) : null;
	return match?.[1] && match[2] ? { cli: match[1], bot: match[2] } : null;
}
