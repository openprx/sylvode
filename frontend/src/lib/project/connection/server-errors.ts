/**
 * The server refuses with `field: reason` (a 400) or a business 409. This maps a message to the
 * field that shows it, so the reason appears under the input it is about; anything else is a
 * message for the tip bar.
 */

export type ErrorField = 'name' | 'bot' | 'url' | 'directory' | 'other';

const FIELDS: Array<[RegExp, ErrorField]> = [
	[/^bot\.name:\s*/, 'name'],
	[/^bot\.id:\s*/, 'bot'],
	[/^runner_url:\s*/, 'url'],
	[/^working_directory:\s*/, 'directory']
];

export function routeServerMessage(message: string): { field: ErrorField; text: string } {
	for (const [pattern, field] of FIELDS) {
		if (pattern.test(message)) return { field, text: message.replace(pattern, '') };
	}
	return { field: 'other', text: message };
}
