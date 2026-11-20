import { EMBED_PROTOCOL, EMBED_SOURCE, FRAME_ID_PATTERN } from './embed-config';
import type {
	SurfaceDensity,
	SurfaceOpenMode,
	SurfaceSlot,
	SurfaceTarget,
	SurfaceTargetKind,
	SurfaceTheme
} from './context';

/** Every message, both ways, has this envelope. Payloads hold plain data only: no functions. */
export interface SurfaceMessage<T extends string, P> {
	source: typeof EMBED_SOURCE;
	protocol: typeof EMBED_PROTOCOL;
	frameId: string;
	type: T;
	payload: P;
}

/** What the host sends and the frame decodes. */
export interface InitPayload {
	slot: SurfaceSlot;
	width: number;
	theme: SurfaceTheme;
	density: SurfaceDensity;
	/** `en` or `zh`; absent keeps the frame's own language. */
	locale?: 'en' | 'zh';
}
export interface ThemePayload {
	theme: SurfaceTheme;
	density: SurfaceDensity;
}
export interface ResizePayload {
	width: number;
}
export interface IdPayload {
	id: string;
}
export type HostMessage =
	| SurfaceMessage<'sv:init', InitPayload>
	| SurfaceMessage<'sv:theme', ThemePayload>
	| SurfaceMessage<'sv:resize', ResizePayload>
	| SurfaceMessage<'sv:action', IdPayload>
	| SurfaceMessage<'sv:tip-action', IdPayload>;

/** What the frame sends and the host decodes. */
export interface ActionDescriptor {
	id: string;
	label: string;
	kind: 'primary' | 'secondary' | 'danger-quiet';
	disabledReason?: string;
}
export interface TipDescriptor {
	id: string;
	tone: 'neutral' | 'success' | 'warning' | 'danger';
	message: string;
	action?: { label: string };
	dismissible: boolean;
}
export interface TitlePayload {
	title: string;
	identifier?: string;
	/** "A / B / C": ancestors and current label as one string. */
	context?: string;
	/** Source label for the host's panel header ("Sylvode · HOME-12"). */
	origin: string;
}
export interface ActionsPayload {
	primary?: ActionDescriptor;
	secondary: ActionDescriptor[];
}
export interface StatePayload {
	query: Record<string, string | null>;
	mode: 'replace' | 'push';
}
export interface TargetDescriptor {
	kind: SurfaceTargetKind;
	id: string;
	workspaceId?: string;
	projectId?: string;
	formId?: string;
	href: string;
	panelHref?: string;
	label?: string;
}
export interface ReportPayload {
	object: string;
	written: string[];
	unchanged: string[];
}
export type FrameMessage =
	| SurfaceMessage<'sv:ready', { kind: string }>
	| SurfaceMessage<'sv:title', TitlePayload>
	| SurfaceMessage<'sv:actions', ActionsPayload>
	| SurfaceMessage<'sv:tip', { tip: TipDescriptor | null }>
	| SurfaceMessage<'sv:dirty', { dirty: boolean }>
	| SurfaceMessage<'sv:state', StatePayload>
	| SurfaceMessage<'sv:open', { target: TargetDescriptor; how: SurfaceOpenMode }>
	| SurfaceMessage<'sv:close', Record<string, never>>
	| SurfaceMessage<'sv:report', ReportPayload>
	| SurfaceMessage<'sv:height', { px: number }>;

export type HostMessageType = HostMessage['type'];
export type FrameMessageType = FrameMessage['type'];

/** Builds an envelope; the one place that fixes `source` and `protocol`. */
export function envelope<T extends string, P>(
	frameId: string,
	type: T,
	payload: P
): SurfaceMessage<T, P> {
	return { source: EMBED_SOURCE, protocol: EMBED_PROTOCOL, frameId, type, payload };
}

// Field-by-field validation: every decoder builds a fresh object from the fields it knows and
// drops the rest, so nothing from the sender is passed on unchecked.

type Plain = Record<string, unknown>;

function plain(value: unknown): Plain | null {
	if (typeof value !== 'object' || value === null || Array.isArray(value)) return null;
	const proto = Object.getPrototypeOf(value);
	return proto === Object.prototype || proto === null ? (value as Plain) : null;
}

function text(value: unknown, max = 512): string | null {
	return typeof value === 'string' && value.length <= max ? value : null;
}

function nonEmpty(value: unknown, max = 512): string | null {
	const found = text(value, max);
	return found && found.trim() ? found : null;
}

function pick<T extends string>(value: unknown, allowed: readonly T[]): T | null {
	return typeof value === 'string' && (allowed as readonly string[]).includes(value)
		? (value as T)
		: null;
}

function length(value: unknown, max = 100_000): number | null {
	return typeof value === 'number' && Number.isFinite(value) && value >= 0 && value <= max
		? value
		: null;
}

/** An in-app path: one leading slash, no protocol-relative or backslash form. */
function appPath(value: unknown): string | null {
	const found = text(value, 2048);
	return found && found.startsWith('/') && !found.startsWith('//') && !found.includes('\\')
		? found
		: null;
}

const SLOTS = ['main', 'panel', 'inline'] as const;
const THEMES = ['light', 'dark'] as const;
const DENSITIES = ['comfortable', 'compact'] as const;
const LOCALES = ['en', 'zh'] as const;
const TONES = ['neutral', 'success', 'warning', 'danger'] as const;
const ACTION_KINDS = ['primary', 'secondary', 'danger-quiet'] as const;
const TARGET_KINDS = ['record', 'issue', 'task', 'page', 'form', 'project'] as const;
const OPEN_MODES = ['main', 'panel', 'replace'] as const;
const STATE_MODES = ['replace', 'push'] as const;

type Decoder<P> = (payload: Plain) => P | null;

function decodeEnvelope<T extends string, P>(
	data: unknown,
	decoders: { [K in T]: Decoder<P> }
): { frameId: string; type: T; payload: P } | null {
	const message = plain(data);
	if (!message) return null;
	if (message.source !== EMBED_SOURCE || message.protocol !== EMBED_PROTOCOL) return null;
	const frameId = typeof message.frameId === 'string' ? message.frameId : '';
	if (!FRAME_ID_PATTERN.test(frameId)) return null;
	const type = typeof message.type === 'string' ? message.type : '';
	if (!Object.prototype.hasOwnProperty.call(decoders, type)) return null;
	const body = plain(message.payload);
	if (!body) return null;
	const payload = decoders[type as T](body);
	return payload === null ? null : { frameId, type: type as T, payload };
}

const hostDecoders: { [K in HostMessageType]: Decoder<HostMessage['payload']> } = {
	'sv:init': (p) => {
		const slot = pick(p.slot, SLOTS);
		const width = length(p.width);
		const theme = pick(p.theme, THEMES);
		const density = pick(p.density, DENSITIES);
		if (!slot || width === null || !theme || !density) return null;
		const locale = p.locale === undefined ? undefined : (pick(p.locale, LOCALES) ?? undefined);
		return locale ? { slot, width, theme, density, locale } : { slot, width, theme, density };
	},
	'sv:theme': (p) => {
		const theme = pick(p.theme, THEMES);
		const density = pick(p.density, DENSITIES);
		return theme && density ? { theme, density } : null;
	},
	'sv:resize': (p) => {
		const width = length(p.width);
		return width === null ? null : { width };
	},
	'sv:action': (p) => {
		const id = nonEmpty(p.id, 128);
		return id ? { id } : null;
	},
	'sv:tip-action': (p) => {
		const id = nonEmpty(p.id, 128);
		return id ? { id } : null;
	}
};

/** Validates a message received by a hosted frame. Anything unexpected returns null. */
export function decodeHostMessage(data: unknown): HostMessage | null {
	const found = decodeEnvelope(data, hostDecoders);
	return found
		? ({
				source: EMBED_SOURCE,
				protocol: EMBED_PROTOCOL,
				frameId: found.frameId,
				type: found.type,
				payload: found.payload
			} as HostMessage)
		: null;
}

function actionDescriptor(value: unknown): ActionDescriptor | null {
	const p = plain(value);
	if (!p) return null;
	const id = nonEmpty(p.id, 128);
	const label = nonEmpty(p.label, 256);
	const kind = pick(p.kind, ACTION_KINDS);
	if (!id || !label || !kind) return null;
	const reason = text(p.disabledReason, 512);
	return reason ? { id, label, kind, disabledReason: reason } : { id, label, kind };
}

function targetDescriptor(value: unknown): TargetDescriptor | null {
	const p = plain(value);
	if (!p) return null;
	const kind = pick(p.kind, TARGET_KINDS);
	const id = nonEmpty(p.id, 128);
	const href = appPath(p.href);
	if (!kind || !id || !href) return null;
	const target: TargetDescriptor = { kind, id, href };
	for (const key of ['workspaceId', 'projectId', 'formId', 'label'] as const) {
		const found = text(p[key], 256);
		if (found) target[key] = found;
	}
	const panelHref = appPath(p.panelHref);
	if (panelHref) target.panelHref = panelHref;
	return target;
}

function stringList(value: unknown, max = 64): string[] | null {
	if (!Array.isArray(value) || value.length > max) return null;
	const items: string[] = [];
	for (const item of value) {
		const found = text(item, 256);
		if (found === null) return null;
		items.push(found);
	}
	return items;
}

const frameDecoders: { [K in FrameMessageType]: Decoder<FrameMessage['payload']> } = {
	'sv:ready': (p) => {
		const kind = nonEmpty(p.kind, 64);
		return kind ? { kind } : null;
	},
	'sv:title': (p) => {
		const title = text(p.title, 512);
		const origin = text(p.origin, 512);
		if (title === null || origin === null) return null;
		const payload: TitlePayload = { title, origin };
		const identifier = text(p.identifier, 128);
		if (identifier) payload.identifier = identifier;
		const context = text(p.context, 1024);
		if (context) payload.context = context;
		return payload;
	},
	'sv:actions': (p) => {
		if (!Array.isArray(p.secondary) || p.secondary.length > 32) return null;
		const secondary: ActionDescriptor[] = [];
		for (const item of p.secondary) {
			const action = actionDescriptor(item);
			if (!action) return null;
			secondary.push(action);
		}
		if (p.primary === undefined) return { secondary };
		const primary = actionDescriptor(p.primary);
		return primary ? { primary, secondary } : null;
	},
	'sv:tip': (p) => {
		if (p.tip === null) return { tip: null };
		const tip = plain(p.tip);
		if (!tip) return null;
		const id = nonEmpty(tip.id, 128);
		const tone = pick(tip.tone, TONES);
		const message = text(tip.message, 2048);
		if (!id || !tone || message === null || typeof tip.dismissible !== 'boolean') return null;
		const descriptor: TipDescriptor = { id, tone, message, dismissible: tip.dismissible };
		if (tip.action !== undefined) {
			const action = plain(tip.action);
			const label = action ? nonEmpty(action.label, 256) : null;
			if (!label) return null;
			descriptor.action = { label };
		}
		return { tip: descriptor };
	},
	'sv:dirty': (p) => (typeof p.dirty === 'boolean' ? { dirty: p.dirty } : null),
	'sv:state': (p) => {
		const query = plain(p.query);
		const mode = pick(p.mode, STATE_MODES);
		if (!query || !mode) return null;
		const clean: Record<string, string | null> = {};
		for (const [key, value] of Object.entries(query)) {
			if (key.length > 64) return null;
			if (value === null) clean[key] = null;
			else {
				const found = text(value, 1024);
				if (found === null) return null;
				clean[key] = found;
			}
		}
		return { query: clean, mode };
	},
	'sv:open': (p) => {
		const target = targetDescriptor(p.target);
		const how = pick(p.how, OPEN_MODES);
		return target && how ? { target, how } : null;
	},
	'sv:close': () => ({}),
	'sv:report': (p) => {
		const object = text(p.object, 512);
		const written = stringList(p.written);
		const unchanged = stringList(p.unchanged);
		return object !== null && written && unchanged ? { object, written, unchanged } : null;
	},
	'sv:height': (p) => {
		const px = length(p.px, 1_000_000);
		return px === null ? null : { px };
	}
};

/** Validates a message received by the host from a frame. Anything unexpected returns null. */
export function decodeFrameMessage(data: unknown): FrameMessage | null {
	const found = decodeEnvelope(data, frameDecoders);
	return found
		? ({
				source: EMBED_SOURCE,
				protocol: EMBED_PROTOCOL,
				frameId: found.frameId,
				type: found.type,
				payload: found.payload
			} as FrameMessage)
		: null;
}

/** The plain-data form of a target, as `sv:open` carries it. */
export function describeTarget(target: SurfaceTarget): TargetDescriptor {
	const out: TargetDescriptor = { kind: target.kind, id: target.id, href: target.href };
	if (target.workspaceId) out.workspaceId = target.workspaceId;
	if (target.projectId) out.projectId = target.projectId;
	if (target.formId) out.formId = target.formId;
	if (target.panelHref) out.panelHref = target.panelHref;
	if (target.label) out.label = target.label;
	return out;
}
