/**
 * State of one project view (`project.view` contribution page), kept outside the component so the
 * transitions can be tested without a DOM.
 *
 * The view is always in exactly one of three visible situations: loading (an entry request is in
 * flight or discovery is still loading), showing a frame (`opened` with phase `ready`), or showing
 * an explicit notice (`failed` with a retry, or `unavailable`). A frame that went away without the
 * view asking for it (`retireFrame`) puts the view back to loading, and the next `reconcileView`
 * opens a fresh entry for it.
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

export type ProjectViewPhase = 'loading' | 'ready' | 'failed' | 'unavailable';

/** One opened entry. `token` is unique per open, so every open mounts its own frame. */
export interface OpenedFrame {
	token: number;
	item: Item;
	html: string;
}

export interface ProjectViewState {
	phase: ProjectViewPhase;
	/** i18n key shown while no frame is shown, or ''. */
	notice: string;
	opened: OpenedFrame | null;
	name: string;
	/** Incremented by every open and every cancellation; a finished open only applies if unchanged. */
	run: number;
	inflight: boolean;
}

export interface ProjectViewDeps {
	loadEntry: (target: Item) => Promise<EntryOutcome>;
	projectName: () => Promise<string>;
	refreshDiscovery: () => Promise<void>;
	/** The contribution for this view in the current discovery. */
	latest: () => Item | undefined;
}

export function initialProjectView(): ProjectViewState {
	return { phase: 'loading', notice: '', opened: null, name: '', run: 0, inflight: false };
}

function cancel(state: ProjectViewState) {
	state.run++;
	state.inflight = false;
}

/** Brings the view in line with the latest discovery. Returns the contribution to open, if any. */
export function reconcileView(
	state: ProjectViewState,
	current: DiscoveryState,
	target: Item | undefined
): Item | undefined {
	if (state.opened) {
		const verdict = frameVerdict(current, state.opened.item);
		if (verdict === 'ok') return;
		cancel(state);
		state.opened = null;
		state.notice = verdictReasons[verdict];
		if (verdict === 'error') {
			state.phase = 'failed';
			return;
		}
		state.phase = 'loading';
	}
	if (state.phase === 'failed' && target && current.status !== 'off' && current.status !== 'denied')
		return;
	if (current.status === 'loading') {
		state.phase = 'loading';
		return;
	}
	if (current.status === 'off' || current.status === 'denied' || !target) {
		cancel(state);
		state.phase = 'unavailable';
		return;
	}
	if (!state.inflight) return target;
}

/** Loads the entry for `target` and opens it, unless a later open or cancellation superseded it. */
export async function openView(
	state: ProjectViewState,
	target: Item,
	deps: ProjectViewDeps
): Promise<void> {
	const mine = ++state.run;
	state.inflight = true;
	state.phase = 'loading';
	const [entry, label] = await Promise.all([deps.loadEntry(target), deps.projectName()]);
	if (mine !== state.run) return;
	state.inflight = false;
	state.name = label;
	if (!entry.ok) {
		state.phase = entry.reason === 'unavailable' ? 'unavailable' : 'failed';
		state.notice = entry.reason === 'unavailable' ? '' : 'plugins.host.loadFailed';
		return;
	}
	if (!sameFrameIdentity(entry.item, target)) {
		// Discovery was stale. Only continue when a fresh discovery agrees with the entry.
		await deps.refreshDiscovery();
		if (mine !== state.run) return;
		const latest = deps.latest();
		if (!latest || !sameFrameIdentity(latest, entry.item)) {
			state.phase = 'failed';
			state.notice = 'plugins.versionChanged';
			return;
		}
	}
	state.notice = '';
	state.opened = { token: mine, item: entry.item, html: entry.html };
	state.phase = 'ready';
}

/**
 * The frame asked to close (`reason === ''`) or was closed by the host with a reason. Returns
 * `'back'` when the caller should leave the view, `'failed'` when it now shows the reason.
 */
export function closeView(state: ProjectViewState, reason: string): 'back' | 'failed' {
	state.opened = null;
	if (reason === '') return 'back';
	cancel(state);
	state.phase = 'failed';
	state.notice = reason;
	return 'failed';
}

/**
 * The frame of the open with `token` is gone although the view is still showing it: its host was
 * retired (suspended and removed) while the view was being left, and the view came back before it
 * was destroyed. Returns false when that frame is no longer the one shown.
 */
export function retireFrame(state: ProjectViewState, token: number): boolean {
	if (state.opened?.token !== token) return false;
	cancel(state);
	state.opened = null;
	state.notice = '';
	state.phase = 'loading';
	return true;
}

/** The user asked to try again after a failure. Returns the contribution to open, if any. */
export function retryView(
	state: ProjectViewState,
	current: DiscoveryState,
	target: Item | undefined
): Item | undefined {
	state.notice = '';
	state.phase = 'loading';
	return reconcileView(state, current, target);
}
