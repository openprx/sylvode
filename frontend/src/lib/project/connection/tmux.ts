/**
 * Which CLI may use the tmux channel: only claude and codex can. A project may hold any number of tmux
 * connections for one CLI, and a bot may hold them in several projects.
 */
import type { Cli } from './wire';

/** The CLIs that can run in a tmux session. */
export const TMUX_CLIS: readonly Cli[] = ['claude', 'codex'];

/** The CLI cannot run in a tmux session at all. */
export type TmuxBlock = { kind: 'cli' };

/** Why the tmux option is not available to a connection that runs `cli`, `null` when it is. */
export function tmuxBlock(cli: Cli): TmuxBlock | null {
	return TMUX_CLIS.includes(cli) ? null : { kind: 'cli' };
}

/** The attach command of a session, as typed on the runner machine. */
export function attachCommand(session: string): string {
	return `tmux attach -t ${session}`;
}

/** The text of the reason a tmux option is off. */
export type Translate = (key: string, values?: Record<string, string>) => string;

export function blockReason(_block: TmuxBlock, translate: Translate): string {
	return translate('execution.connection.advanced.channel.onlyTmuxClis');
}
