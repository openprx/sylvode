/**
 * The one-time configuration block as a state machine.
 *
 * The block holds the bot's token and the webhook's signing secret, and the API shows it exactly
 * once. It therefore lives in one place only: the `BlockState` value a component keeps in its own
 * state while the block is on screen. It is never put in a store, in storage, in the URL, in a
 * log or in an error report, and leaving the "shown" phase drops the only reference to the text.
 *
 * Plain functions over a plain value (no class, no store, no closure over the text), so the rules
 * are unit-tested directly and a snapshot of any other state can never contain the text.
 */

export type BlockState = { phase: 'none' } | { phase: 'shown'; text: string; stored: boolean };

export const NO_BLOCK: BlockState = { phase: 'none' };

/** The block of a connect or rotate answer, not yet confirmed as stored. */
export function showBlock(text: string): BlockState {
	return text === '' ? NO_BLOCK : { phase: 'shown', text, stored: false };
}

/** The "I have stored this configuration" checkbox. */
export function setStored(state: BlockState, stored: boolean): BlockState {
	return state.phase === 'shown' ? { phase: 'shown', text: state.text, stored } : state;
}

/** "Done" is available only once the person confirmed the block is stored. */
export function canFinish(state: BlockState): boolean {
	return state.phase === 'shown' && state.stored;
}

/** "Done": drops the text, but only after the confirmation. */
export function finish(state: BlockState): BlockState {
	return canFinish(state) ? NO_BLOCK : state;
}

/** Leaving after the person agreed to lose the block: drops the text. */
export function discard(): BlockState {
	return NO_BLOCK;
}

/** Leaving the page (in-app navigation or the browser) asks while the block is unconfirmed. */
export function needsLeaveConfirmation(state: BlockState): boolean {
	return state.phase === 'shown' && !state.stored;
}

/** The text on screen, `''` when there is none. */
export function visibleText(state: BlockState): string {
	return state.phase === 'shown' ? state.text : '';
}

/** Rows of the read-only textarea: fitted to the text, 24 at most (it scrolls beyond). */
export function textareaRows(text: string): number {
	return Math.min(24, Math.max(3, text.split('\n').length));
}

export const BLOCK_FILENAME = 'sylvode-runner.toml';

/**
 * The file the "Download file" button saves, made from the text in memory when the person
 * clicks. Nothing is requested and nothing is downloaded on its own.
 */
export function blockFile(text: string): { name: string; blob: Blob } {
	return { name: BLOCK_FILENAME, blob: new Blob([text], { type: 'application/toml' }) };
}
