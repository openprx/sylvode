/**
 * State of an entry area that the user opens and closes (the record panel area and the form list
 * toolbar drawer), kept outside the components so the transitions can be tested without a DOM.
 *
 * At any time the area is closed (`active` is null, possibly with a notice), loading (`active`
 * without a frame: an entry request is in flight) or showing one frame. It is never open and
 * empty. Every frame gets its own `token`, and the components key the host by it, so an open
 * never reuses a host that is still on its way out.
 *
 * A frame whose host retired silently while the page was being left and came back before it was
 * destroyed (`retireSlot`) is reopened with a fresh entry, but only if the latest discovery still
 * lists the same contribution with the same identity; otherwise the area closes with the existing
 * "access changed" or "version changed" notice. Whatever the user had closed stays closed.
 */
import type { PluginContributionItem } from '../api/plugin-packages';
import {
	frameVerdict,
	sameFrameIdentity,
	verdictReasons,
	type DiscoveryState,
	type EntryOutcome
} from './contributions';

type Item = PluginContributionItem;

/** One shown frame: the entry the user opened, its contribution and the loaded page. */
export interface SlotFrame<E> {
	token: number;
	entry: E;
	item: Item;
	html: string;
}

export interface SlotState<E> {
	/** What the user opened. Null when the area is closed. */
	active: E | null;
	frame: SlotFrame<E> | null;
	/** i18n key shown while the area is closed, or ''. */
	notice: string;
	name: string;
	/** Amount of the last `amount.preview` of the shown frame, for the apply confirmation. */
	preview: number | null;
	/** Incremented by every open and close; a finished open only applies if unchanged. */
	run: number;
	tokens: number;
}

export interface SlotDeps<E> {
	/** The contribution behind an entry. */
	itemOf: (entry: E) => Item;
	loadEntry: (item: Item) => Promise<EntryOutcome>;
	projectName: () => Promise<string>;
	/** The server disagreed with the discovery; let it pick the change up. */
	refresh: () => void;
	/** Settles a pending apply confirmation as not confirmed and hides it. */
	dismissConfirmation?: () => void;
	/** False once the owner is gone (destroyed or the session changed). */
	alive?: () => boolean;
}

export function initialSlot<E>(): SlotState<E> {
	return { active: null, frame: null, notice: '', name: '', preview: null, run: 0, tokens: 0 };
}

/** Closes whatever is open or loading and cancels a pending open. */
export function closeSlot<E>(
	state: SlotState<E>,
	deps: Pick<SlotDeps<E>, 'dismissConfirmation'>,
	notice = ''
) {
	state.run++;
	deps.dismissConfirmation?.();
	state.active = null;
	state.frame = null;
	state.preview = null;
	state.notice = notice;
}

/** Fetches the entry for `target` and shows it, unless a later open or close superseded it. */
async function load<E>(
	state: SlotState<E>,
	entry: E,
	target: Item,
	deps: SlotDeps<E>
): Promise<number> {
	const mine = ++state.run;
	const [result, label] = await Promise.all([deps.loadEntry(target), deps.projectName()]);
	if (mine !== state.run || deps.alive?.() === false) return 0;
	state.name = label;
	if (!result.ok) {
		closeSlot(
			state,
			deps,
			result.reason === 'unavailable' ? 'plugins.accessChanged' : 'plugins.host.loadFailed'
		);
		deps.refresh();
		return 0;
	}
	if (!sameFrameIdentity(result.item, target)) {
		closeSlot(state, deps, 'plugins.versionChanged');
		deps.refresh();
		return 0;
	}
	state.frame = { token: ++state.tokens, entry, item: result.item, html: result.html };
	return state.frame.token;
}

/** Opens `entry`, replacing whatever was open. Resolves to the new frame's token, or 0. */
export async function openSlot<E>(
	state: SlotState<E>,
	entry: E,
	deps: SlotDeps<E>
): Promise<number> {
	closeSlot(state, deps);
	state.active = entry;
	return load(state, entry, deps.itemOf(entry), deps);
}

export type ReopenDecision = { kind: 'reopen'; target: Item } | { kind: 'notice'; reason: string };

/**
 * Whether a frame that ran for `opened` may be reopened given the latest discovery. The verdict
 * is the one that closes a running frame: a contribution that is gone, changed (version, digest,
 * grants) or no longer granted is not reopened.
 */
export function reopenDecision(current: DiscoveryState, opened: Item): ReopenDecision {
	const verdict = frameVerdict(current, opened);
	if (verdict !== 'ok') return { kind: 'notice', reason: verdictReasons[verdict] };
	const latest = current.items.find(
		(item) => item.key === opened.key && item.contribution.id === opened.contribution.id
	);
	// While discovery is still loading there is no verdict yet; the entry's own identity check
	// and the discovery watch of the owner still apply to the reopened frame.
	return { kind: 'reopen', target: latest ?? opened };
}

/**
 * The host of the frame with `token` retired silently and is live again. Resolves to `reopened`
 * when a fresh frame is shown, `closed` when the area closed (with a notice, or because the
 * fresh entry could not be loaded) and `ignored` when that frame is no longer the one shown, for
 * example because the user closed it or opened another.
 *
 * The pending apply confirmation is dismissed: the reopened panel is idle again and the amount
 * has to be previewed and confirmed anew.
 */
export async function retireSlot<E>(
	state: SlotState<E>,
	token: number,
	current: DiscoveryState,
	deps: SlotDeps<E>
): Promise<'reopened' | 'closed' | 'ignored'> {
	const frame = state.frame;
	if (!frame || frame.token !== token) return 'ignored';
	const decision = reopenDecision(current, frame.item);
	if (decision.kind === 'notice') {
		closeSlot(state, deps, decision.reason);
		return 'closed';
	}
	deps.dismissConfirmation?.();
	state.frame = null;
	state.preview = null;
	state.notice = '';
	return (await load(state, frame.entry, decision.target, deps)) ? 'reopened' : 'closed';
}

/** Records the previewed amount, unless the frame that asked is no longer the one shown. */
export function recordPreview<E>(state: SlotState<E>, token: number, cents: number | null) {
	if (state.frame?.token === token) state.preview = cents;
}

/** The apply confirmation dialog: one pending question at a time, dismissable from outside. */
export interface ApplyConfirmation {
	/** Asks; any earlier pending question is answered no. */
	request(): Promise<boolean>;
	/** The user confirmed. */
	confirm(): void;
	/** Answers no and hides the dialog. */
	dismiss(): void;
	/** Answers no without touching the dialog (it closed itself). */
	settle(confirmed: boolean): void;
}

export function createApplyConfirmation(view: {
	show: () => void;
	hide: () => void;
}): ApplyConfirmation {
	let resolve: ((confirmed: boolean) => void) | null = null;
	const settle = (confirmed: boolean) => {
		const pending = resolve;
		resolve = null;
		pending?.(confirmed);
	};
	return {
		request() {
			settle(false);
			return new Promise<boolean>((done) => {
				resolve = done;
				view.show();
			});
		},
		confirm() {
			settle(true);
			view.hide();
		},
		dismiss() {
			settle(false);
			view.hide();
		},
		settle
	};
}
