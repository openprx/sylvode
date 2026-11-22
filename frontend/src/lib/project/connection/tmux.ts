/**
 * Which CLI may use the tmux channel in a project (design 6.0 C3, 6.4 U3): only claude can, and
 * at most one connection per CLI. Pure functions over the project's connections, so the forms
 * can disable the option with its reason before the server has to refuse.
 */
import { routeServerMessage, tmuxTakenBy } from './server-errors';
import type { Cli, Connection } from './wire';

export const TMUX_CLI: Cli = 'claude';

export type TmuxBlock =
	/** Another connection of the project already uses `<KEY>-<cli>`. */
	| { kind: 'taken'; bot: string; session: string | null }
	/** The CLI cannot run in a tmux session at all. */
	| { kind: 'cli' };

/**
 * Why the tmux option is not available to a connection that runs `cli`, `null` when it is.
 * `exceptBotId` is the connection being edited: it does not hold the session against itself.
 */
export function tmuxBlock(
	connections: readonly Connection[],
	cli: Cli,
	exceptBotId: string | null
): TmuxBlock | null {
	if (cli !== TMUX_CLI) return { kind: 'cli' };
	const holder = connections.find(
		(connection) =>
			connection.bot.id !== exceptBotId &&
			connection.execution.channel === 'tmux' &&
			connection.execution.cli === cli
	);
	return holder
		? { kind: 'taken', bot: holder.bot.name, session: holder.execution.session_name }
		: null;
}

/** The attach command of a session, as typed on the runner machine. */
export function attachCommand(session: string): string {
	return `tmux attach -t ${session}`;
}

/** The text of the reason a tmux option is off, or the server's refusal of a channel. */
export type Translate = (key: string, values?: Record<string, string>) => string;

export function blockReason(block: TmuxBlock, translate: Translate): string {
	if (block.kind === 'cli') return translate('execution.connection.advanced.channel.onlyClaude');
	return block.session === null
		? translate('execution.connection.advanced.channel.takenNoSession', { bot: block.bot })
		: translate('execution.connection.advanced.channel.taken', {
				bot: block.bot,
				session: block.session
			});
}

/**
 * The field error for a refused channel. The 409 of a second tmux connection reads like the
 * reason shown before submitting (the session name comes from the holder in `connections`);
 * any other refusal shows the server's own words.
 */
export function channelErrorText(
	message: string,
	connections: readonly Connection[],
	translate: Translate
): string {
	const taken = tmuxTakenBy(message);
	if (taken === null) return routeServerMessage(message).text;
	const holder = connections.find(
		(connection) => connection.bot.name === taken.bot && connection.execution.cli === taken.cli
	);
	return blockReason(
		{ kind: 'taken', bot: taken.bot, session: holder?.execution.session_name ?? null },
		translate
	);
}
