import { contextString } from '../ui/context-line/context-line';
import { widthClass as classify } from './width';
import { isEmbedOrigin } from './embed-config';
import {
	decodeHostMessage,
	describeTarget,
	envelope,
	type ActionDescriptor,
	type FrameMessage,
	type HostMessage
} from './embed-protocol';
import type {
	SurfaceAction,
	SurfaceActions,
	SurfaceAmbient,
	SurfaceDensity,
	SurfaceHeaderVariant,
	SurfaceHost,
	SurfaceHostProvider,
	SurfaceOpenMode,
	SurfaceReport,
	SurfaceSlot,
	SurfaceTarget,
	SurfaceTheme,
	SurfaceTip,
	SurfaceTitle,
	SurfaceWidthClass
} from './context';

const ORIGIN_LABEL = 'Sylvode';

/** What `receive` needs from a `MessageEvent`; a plain object works in tests. */
export interface IncomingMessage {
	data: unknown;
	origin: string;
	source: unknown;
}

export interface EmbedHostDeps {
	/** The only window whose messages are accepted (`window.parent`). */
	parent: unknown;
	/** Origins whose messages are accepted (`embedOrigins()`). */
	origins: readonly string[];
	/** Delivers a message to the parent. The transport fixes the target origin; it never posts `*`. */
	post(message: FrameMessage): void;
	/** Id of this frame: `?frame=` when the host chose one, else a random id. Messages must carry it. */
	frameId: string;
	/** Theme before the host has said anything: the system preference. */
	systemTheme: SurfaceTheme;
	/** The host asked for another language. */
	onlocale?(locale: 'en' | 'zh'): void;
	/** Writes the frame's own URL for `syncState`; the host also gets `sv:state`. */
	writeUrl?(query: Record<string, string | null>, mode: 'replace' | 'push'): void;
}

/** What each setter forwards to; the owner check lives in `OwnedEmbedHost`. */
class OwnedEmbedHost implements SurfaceHost {
	constructor(
		private readonly core: EmbedSurfaceHost,
		private readonly owner: symbol
	) {}
	get frame() {
		return this.core.frame;
	}
	get slot() {
		return this.core.slot;
	}
	get width() {
		return this.core.width;
	}
	get widthClass() {
		return this.core.widthClass;
	}
	get theme() {
		return this.core.theme;
	}
	get density() {
		return this.core.density;
	}
	get globalShortcuts() {
		return this.core.globalShortcuts;
	}
	setTitle(title: SurfaceTitle) {
		if (this.core.isCurrent(this.owner)) this.core.applyTitle(title);
	}
	setActions(actions: SurfaceActions) {
		if (this.core.isCurrent(this.owner)) this.core.applyActions(actions);
	}
	setHeader(variant: SurfaceHeaderVariant) {
		// The host draws its own header (specification 6, rule 1).
		void variant;
	}
	setDensity(density: SurfaceDensity) {
		// Hosted: the host sets it.
		void density;
	}
	setTip(tip: SurfaceTip | null) {
		if (this.core.isCurrent(this.owner)) this.core.applyTip(tip);
	}
	setDirty(dirty: boolean) {
		if (this.core.isCurrent(this.owner)) this.core.applyDirty(dirty);
	}
	syncState(query: Record<string, string | null>, mode: 'replace' | 'push' = 'replace') {
		if (this.core.isCurrent(this.owner)) this.core.applySyncState(query, mode);
	}
	open(target: SurfaceTarget, how: SurfaceOpenMode) {
		if (this.core.isCurrent(this.owner)) this.core.applyOpen(target, how);
	}
	close(fallbackHref?: string) {
		// A hosted surface never navigates the window, so the fallback is not used.
		void fallbackHref;
		if (this.core.isCurrent(this.owner)) this.core.applyClose();
	}
	report(result: SurfaceReport) {
		if (this.core.isCurrent(this.owner)) this.core.applyReport(result);
	}
}

/**
 * The hosted frame's side of the surface contract. Surfaces reach it only through `useSurface`;
 * the embed layout feeds it incoming messages with `receive` and its measured width with
 * `setMeasuredWidth`. Everything the surface declares leaves as a message to the host.
 */
export class EmbedSurfaceHost implements SurfaceHostProvider {
	readonly frame = 'hosted' as const;
	readonly globalShortcuts = false;

	slot = $state<SurfaceSlot>('main');
	theme = $state<SurfaceTheme>('light');
	density = $state<SurfaceDensity>('comfortable');
	/** True once `sv:init` arrived or the wait for it ran out: the frame may render its surface. */
	settled = $state(false);
	/** True once `sv:init` arrived. */
	initialised = $state(false);
	private measured = $state<number | null>(null);
	private hint = $state(0);
	private owner = $state.raw<symbol | null>(null);
	private readonly frameId: string;
	// Plain bookkeeping, not read by any component, so not reactive.
	// eslint-disable-next-line svelte/prefer-svelte-reactivity
	private readonly actionRuns = new Map<string, SurfaceAction>();
	private tip: SurfaceTip | null = null;
	// eslint-disable-next-line svelte/prefer-svelte-reactivity
	private readonly last = new Map<string, string>();

	constructor(private readonly deps: EmbedHostDeps) {
		this.frameId = deps.frameId;
		this.theme = deps.systemTheme;
	}

	/** Width of the frame document: measured when the layout observes it, else the host's hint. */
	get width(): number {
		return this.measured ?? this.hint;
	}
	get widthClass(): SurfaceWidthClass {
		return classify(this.width);
	}
	get hasSurface(): boolean {
		return this.owner !== null;
	}
	get id(): string {
		return this.frameId;
	}

	/** Called by the layout's ResizeObserver on the frame document. */
	setMeasuredWidth(px: number): void {
		this.measured = px;
	}

	/** The wait for `sv:init` ran out: system theme, slot `main`, which are already the defaults. */
	settle(): void {
		this.settled = true;
	}

	/** Tells the host this frame is listening; the host answers with `sv:init`. */
	announce(kind: string): void {
		this.send('sv:ready', { kind }, false);
	}

	/** Escape pressed inside the frame: the host owns close, so it is asked. */
	requestClose(): void {
		this.applyClose();
	}

	/** Inline slot only: the height the host should give the frame. */
	reportHeight(px: number): void {
		if (this.slot === 'inline') this.send('sv:height', { px: Math.ceil(px) });
	}

	/**
	 * Handles one incoming message. Returns true when it was valid, from the parent and from an
	 * allowed origin; everything else is ignored without effect.
	 */
	receive(event: IncomingMessage): boolean {
		if (event.source !== this.deps.parent || !this.deps.parent) return false;
		if (!isEmbedOrigin(event.origin, this.deps.origins)) return false;
		const message = decodeHostMessage(event.data);
		if (!message) return false;
		if (message.frameId !== this.frameId) return false;
		this.dispatch(message);
		return true;
	}

	private dispatch(message: HostMessage): void {
		switch (message.type) {
			case 'sv:init':
				this.slot = message.payload.slot;
				this.hint = message.payload.width;
				this.theme = message.payload.theme;
				this.density = message.payload.density;
				this.initialised = true;
				this.settled = true;
				if (message.payload.locale) this.deps.onlocale?.(message.payload.locale);
				break;
			case 'sv:theme':
				this.theme = message.payload.theme;
				this.density = message.payload.density;
				break;
			case 'sv:resize':
				this.hint = message.payload.width;
				break;
			case 'sv:action': {
				const action = this.actionRuns.get(message.payload.id);
				if (action && !action.disabledReason) this.run(action.run);
				break;
			}
			case 'sv:tip-action': {
				const tip = this.tip;
				if (tip?.action && tip.id === message.payload.id) this.run(tip.action.run);
				break;
			}
		}
	}

	private run(fn: () => void | Promise<void>): void {
		try {
			void Promise.resolve(fn()).catch(() => {});
		} catch {
			// A failing action must not break the message loop.
		}
	}

	/** Posts a message; a message identical to the last one of its type is not sent again. */
	private send<T extends FrameMessage['type']>(
		type: T,
		payload: Extract<FrameMessage, { type: T }>['payload'],
		dedupe = true
	): void {
		const id = this.frameId;
		const key = JSON.stringify(payload);
		if (dedupe) {
			if (this.last.get(type) === key) return;
			this.last.set(type, key);
		}
		this.deps.post(envelope(id, type, payload) as FrameMessage);
	}

	attach(owner: symbol): SurfaceHost {
		this.owner = owner;
		this.resetDeclared();
		return new OwnedEmbedHost(this, owner);
	}

	detach(owner: symbol): void {
		if (this.owner !== owner) return;
		this.owner = null;
		this.resetDeclared();
	}

	/** Forgets what the previous surface declared and tells the host so. */
	private resetDeclared(): void {
		this.actionRuns.clear();
		this.tip = null;
		const hadSomething = this.last.size > 0;
		this.last.clear();
		if (!hadSomething) return;
		this.send('sv:actions', { secondary: [] });
		this.send('sv:tip', { tip: null });
		this.send('sv:dirty', { dirty: false });
	}

	ambient(): SurfaceAmbient {
		const self = () => this;
		return {
			frame: 'hosted',
			get theme() {
				return self().theme;
			},
			get density() {
				return self().density;
			},
			globalShortcuts: false,
			open: (target, how) => self().applyOpen(target, how),
			close: () => self().applyClose()
		};
	}

	isCurrent(owner: symbol): boolean {
		return this.owner === owner;
	}

	applyTitle(title: SurfaceTitle): void {
		// While the object loads the host keeps its own skeleton; nothing is sent.
		if (title.loading || !title.title) return;
		this.send('sv:title', {
			title: title.title,
			...(title.identifier ? { identifier: title.identifier } : {}),
			...(title.context
				? { context: contextString(title.context.segments, title.context.current) }
				: {}),
			origin: title.origin ?? `${ORIGIN_LABEL} · ${title.identifier ?? title.title}`
		});
	}

	applyActions(actions: SurfaceActions): void {
		this.actionRuns.clear();
		const describe = (action: SurfaceAction): ActionDescriptor => {
			this.actionRuns.set(action.id, action);
			return {
				id: action.id,
				label: action.label,
				kind: action.kind,
				...(action.disabledReason ? { disabledReason: action.disabledReason } : {})
			};
		};
		const primary = actions.primary ? describe(actions.primary) : undefined;
		const secondary = actions.secondary.map(describe);
		this.send('sv:actions', primary ? { primary, secondary } : { secondary });
	}

	applyTip(tip: SurfaceTip | null): void {
		this.tip = tip;
		this.send('sv:tip', {
			tip: tip
				? {
						id: tip.id,
						tone: tip.tone,
						message: tip.message,
						...(tip.action ? { action: { label: tip.action.label } } : {}),
						dismissible: tip.dismissible ?? tip.tone === 'success'
					}
				: null
		});
	}

	applyDirty(dirty: boolean): void {
		this.send('sv:dirty', { dirty });
	}

	applySyncState(query: Record<string, string | null>, mode: 'replace' | 'push'): void {
		this.deps.writeUrl?.(query, mode);
		this.send('sv:state', { query, mode });
	}

	applyOpen(target: SurfaceTarget, how: SurfaceOpenMode): void {
		this.send('sv:open', { target: describeTarget(target), how }, false);
	}

	applyClose(): void {
		this.send('sv:close', {}, false);
	}

	applyReport(result: SurfaceReport): void {
		this.send(
			'sv:report',
			{
				object: result.object,
				written: [...result.written],
				unchanged: [...result.unchanged]
			},
			false
		);
	}
}
