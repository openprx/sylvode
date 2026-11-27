/**
 * Read-only presentation of a record value, by field type.
 *
 * `fieldDisplay()` turns a stored value into a small display model that `FieldValue.svelte`
 * renders; it holds no markup and loads nothing. Every `FormFieldType` has an entry in
 * `RENDERERS` (`satisfies Record<FormFieldType, …>`), so adding a type to `forms.ts` fails type
 * checking until it has a presentation, the same guard `FIELD_CODECS` gives record values.
 *
 * Rules that hold for every type: record content is text, never HTML; unsafe URLs are reported
 * as blocked rather than linked; image and signature data are previews, never printed as text;
 * numbers keep every stored digit (decimal strings are formatted without a float round trip);
 * calendar dates do not shift with the viewer's timezone.
 */

import type { FormAttachment, FormField, FormFieldType } from '#lib/api/forms.js';
import { safeImageUrl, safeLinkUrl } from '#lib/utils/safe-url.js';
import { recordValueIsEmpty } from './detail-layout.js';
import { richTextToPlainText } from './rich-text.js';
import { fieldRuleValue } from './field-rules.js';

export interface FileItem {
	name: string;
	href?: string;
	download?: string;
	sizeLabel?: string;
	typeLabel?: string;
	blocked?: boolean;
	thumb?: string;
	isImage?: boolean;
}

export interface ImageItem {
	src: string;
	alt: string;
	full?: string;
}

export type FieldDisplay =
	| { kind: 'empty' }
	| { kind: 'text'; text: string; multiline: boolean; mono?: boolean; clampLines: number }
	| { kind: 'number'; text: string; raw: string }
	| { kind: 'rating'; value: number; max: number }
	| { kind: 'progress'; value: number; text: string }
	| { kind: 'date'; text: string; iso: string }
	| { kind: 'options'; field: FormField; value: unknown; labels: string[] }
	| { kind: 'boolean'; value: boolean }
	| { kind: 'member'; userId: string; name: string; email?: string; known: boolean }
	| { kind: 'link'; href: string; text: string; scheme: 'mailto' | 'tel' | 'https' }
	| { kind: 'location'; label?: string; lat: number; lng: number; coordinates: string }
	| { kind: 'images'; items: ImageItem[]; blocked: number }
	| { kind: 'files'; items: FileItem[] }
	| { kind: 'signature'; src: string; blocked: boolean }
	| {
			kind: 'relation';
			recordId: string;
			title: string;
			formKey?: string;
			sameForm: boolean;
			available: boolean;
	  }
	| { kind: 'children'; count: number }
	| { kind: 'json'; text: string };

export interface FieldDisplayContext {
	/** Active app locale (`en`, `zh`, or a BCP 47 tag). */
	locale: string;
	/** Key of the form the record belongs to (decides same-form relations). */
	formKey: string;
	members: Map<string, { name?: string; email?: string }>;
	attachmentsByField: Record<string, FormAttachment[]>;
	/** Signed download URLs by attachment id. */
	signedUrls: Record<string, string>;
	/** Title of a relation target when the value carries only its id. */
	relationTitle?: (field: FormField, recordId: string) => string | undefined;
	/** Viewer timezone for datetimes; defaults to the runtime's. Tests pin it. */
	timeZone?: string;
}

const EMPTY: FieldDisplay = { kind: 'empty' };

function isRecord(input: unknown): input is Record<string, unknown> {
	return typeof input === 'object' && input !== null && !Array.isArray(input);
}

/** The `Intl` locale for the app's locale codes. */
export function intlLocale(locale: string): string {
	const value = (locale || 'en').trim();
	if (value === 'zh') return 'zh-CN';
	if (value === 'en') return 'en-US';
	try {
		return Intl.getCanonicalLocales(value)[0] ?? 'en-US';
	} catch {
		return 'en-US';
	}
}

function compactJson(value: unknown): string {
	try {
		return JSON.stringify(value, null, 1)?.replace(/\n\s*/g, ' ') ?? String(value);
	} catch {
		return String(value);
	}
}

/** Text for a value whose shape does not match its type: still readable, never an object dump
 * when a common display key exists. */
function fallback(value: unknown): FieldDisplay {
	if (value === null || value === undefined || value === '') return EMPTY;
	if (typeof value === 'string' || typeof value === 'number') {
		return { kind: 'text', text: String(value), multiline: false, clampLines: 3 };
	}
	if (typeof value === 'boolean') return { kind: 'boolean', value };
	if (isRecord(value)) {
		for (const key of ['display', 'title', 'name', 'label']) {
			if (typeof value[key] === 'string' && (value[key] as string).trim()) {
				return {
					kind: 'text',
					text: (value[key] as string).trim(),
					multiline: false,
					clampLines: 3
				};
			}
		}
	}
	return { kind: 'json', text: compactJson(value) };
}

// Numbers.

const DECIMAL_PATTERN = /^([+-])?(\d+)(?:\.(\d+))?$/;

interface DecimalParts {
	negative: boolean;
	integer: bigint;
	fraction: string;
	raw: string;
}

function decimalParts(value: unknown): DecimalParts | null {
	let raw: string;
	if (typeof value === 'number') {
		if (!Number.isFinite(value)) return null;
		raw = String(value);
		if (/e/i.test(raw))
			raw = value.toFixed(
				Math.min(20, Math.max(0, -Math.floor(Math.log10(Math.abs(value) || 1)) + 15))
			);
	} else if (typeof value === 'string') {
		raw = value.trim();
	} else {
		return null;
	}
	const match = DECIMAL_PATTERN.exec(raw);
	if (!match) return null;
	return {
		negative: match[1] === '-' && /[1-9]/.test(`${match[2]}${match[3] ?? ''}`),
		integer: BigInt(match[2]),
		fraction: match[3] ?? '',
		raw
	};
}

function decimalSeparator(locale: string): string {
	return (
		new Intl.NumberFormat(locale).formatToParts(1.5).find((part) => part.type === 'decimal')
			?.value ?? '.'
	);
}

/**
 * Formats a decimal with grouping in `locale`, keeping every digit: the integer part goes through
 * `Intl` as a `bigint`, the fraction digits are appended unchanged (padded to `minFraction`).
 */
export function formatDecimal(
	value: unknown,
	locale: string,
	options: { currency?: string; minFraction?: number } = {}
): string | null {
	const parts = decimalParts(value);
	if (!parts) return null;
	const intl = intlLocale(locale);
	let fraction = parts.fraction;
	if (options.minFraction && fraction.length < options.minFraction)
		fraction = fraction.padEnd(options.minFraction, '0');
	const style: Intl.NumberFormatOptions = options.currency
		? {
				style: 'currency',
				currency: options.currency,
				minimumFractionDigits: 0,
				maximumFractionDigits: 0
			}
		: { maximumFractionDigits: 0 };
	const formatter = new Intl.NumberFormat(intl, style);
	// A negative zero integer part ("-0.50") has no negative bigint: format -1 and swap the digit.
	const integerPartsSource =
		parts.negative && parts.integer === 0n
			? formatter.formatToParts(-1n)
			: formatter.formatToParts(parts.negative ? -parts.integer : parts.integer);
	const out: string[] = [];
	let lastIntegerIndex = -1;
	integerPartsSource.forEach((part, index) => {
		if (part.type === 'integer') lastIntegerIndex = index;
	});
	integerPartsSource.forEach((part, index) => {
		out.push(part.type === 'integer' && parts.negative && parts.integer === 0n ? '0' : part.value);
		if (index === lastIntegerIndex && fraction) out.push(decimalSeparator(intl), fraction);
	});
	return out.join('');
}

function numericValue(value: unknown): unknown {
	if (isRecord(value)) {
		if (typeof value.decimal === 'string') return value.decimal;
		if (typeof value.value === 'number' || typeof value.value === 'string') return value.value;
	}
	return value;
}

function numberDisplay(value: unknown, ctx: FieldDisplayContext): FieldDisplay {
	if (recordValueIsEmpty(value)) return EMPTY;
	const raw = numericValue(value);
	if (raw === null || raw === undefined || raw === '') return EMPTY;
	const text = formatDecimal(raw, ctx.locale);
	return text === null ? fallback(value) : { kind: 'number', text, raw: String(raw).trim() };
}

function integerDisplay(value: unknown, ctx: FieldDisplayContext): FieldDisplay {
	if (recordValueIsEmpty(value)) return EMPTY;
	const raw = numericValue(value);
	if (raw === null || raw === undefined || raw === '') return EMPTY;
	const parts = decimalParts(raw);
	if (!parts || parts.fraction.replace(/0+$/, '')) return fallback(value);
	const text = formatDecimal(`${parts.negative ? '-' : ''}${parts.integer}`, ctx.locale);
	return text === null ? fallback(value) : { kind: 'number', text, raw: String(raw).trim() };
}

function validCurrency(code: unknown): string | null {
	if (typeof code !== 'string' || !/^[A-Za-z]{3}$/.test(code.trim())) return null;
	try {
		new Intl.NumberFormat('en-US', { style: 'currency', currency: code.trim().toUpperCase() });
		return code.trim().toUpperCase();
	} catch {
		return null;
	}
}

function amountDisplay(field: FormField, value: unknown, ctx: FieldDisplayContext): FieldDisplay {
	if (recordValueIsEmpty(value)) return EMPTY;
	const decimal = isRecord(value) ? value.decimal : value;
	if (decimal === null || decimal === undefined || decimal === '') return EMPTY;
	const rawCurrency =
		isRecord(value) && typeof value.currency === 'string' ? value.currency : field.amount?.currency;
	const scaleSource =
		isRecord(value) && typeof value.scale === 'number' ? value.scale : field.amount?.scale;
	const scale =
		typeof scaleSource === 'number' && scaleSource >= 0 && scaleSource <= 12
			? scaleSource
			: undefined;
	const currency = validCurrency(rawCurrency);
	const raw = `${String(decimal).trim()}${rawCurrency ? ` ${rawCurrency}` : ''}`;
	if (!decimalParts(decimal)) return fallback(value);
	if (!currency) {
		const number = formatDecimal(decimal, ctx.locale, { minFraction: scale });
		return {
			kind: 'number',
			text: rawCurrency ? `${number} ${rawCurrency}` : (number ?? raw),
			raw
		};
	}
	const text = formatDecimal(decimal, ctx.locale, { currency, minFraction: scale });
	return text === null ? fallback(value) : { kind: 'number', text, raw };
}

function boundedNumber(value: unknown): number | null {
	const raw = numericValue(value);
	const number =
		typeof raw === 'number' ? raw : typeof raw === 'string' && raw.trim() ? Number(raw) : NaN;
	return Number.isFinite(number) ? number : null;
}

function ratingDisplay(field: FormField, value: unknown): FieldDisplay {
	const number = boundedNumber(value);
	if (number === null) return recordValueIsEmpty(value) ? EMPTY : fallback(value);
	const maxSource = Number(fieldRuleValue(field, 'max'));
	const max = Number.isInteger(maxSource) && maxSource >= 1 && maxSource <= 10 ? maxSource : 5;
	return { kind: 'rating', value: Math.min(Math.max(Math.round(number), 0), max), max };
}

function progressDisplay(value: unknown, ctx: FieldDisplayContext): FieldDisplay {
	const number = boundedNumber(value);
	if (number === null) return recordValueIsEmpty(value) ? EMPTY : fallback(value);
	const clamped = Math.min(Math.max(number, 0), 100);
	const text = new Intl.NumberFormat(intlLocale(ctx.locale), {
		style: 'percent',
		maximumFractionDigits: 1
	}).format(clamped / 100);
	return { kind: 'progress', value: clamped, text };
}

function formulaDisplay(field: FormField, value: unknown, ctx: FieldDisplayContext): FieldDisplay {
	if (recordValueIsEmpty(value)) return EMPTY;
	if (isRecord(value) && typeof value.decimal === 'string') {
		return typeof value.currency === 'string'
			? amountDisplay(field, value, ctx)
			: numberDisplay(value, ctx);
	}
	if (isRecord(value) && value.type === 'integer') return integerDisplay(value, ctx);
	if (typeof value === 'number') return numberDisplay(value, ctx);
	if (typeof value === 'string')
		return { kind: 'text', text: value, multiline: false, clampLines: 3 };
	return fallback(value);
}

// Dates.

const DATE_PATTERN = /^(\d{4})-(\d{2})-(\d{2})$/;
const LOCAL_DATETIME_PATTERN = /^(\d{4})-(\d{2})-(\d{2})[T ](\d{2}):(\d{2})(?::(\d{2}))?$/;

function dateDisplay(value: unknown, ctx: FieldDisplayContext): FieldDisplay {
	if (typeof value !== 'string' || !value.trim())
		return recordValueIsEmpty(value) ? EMPTY : fallback(value);
	const iso = value.trim();
	const match = DATE_PATTERN.exec(iso.slice(0, 10));
	if (!match || (iso.length > 10 && !/^[T ]/.test(iso[10]))) return fallback(value);
	const [year, month, day] = [Number(match[1]), Number(match[2]), Number(match[3])];
	const date = new Date(Date.UTC(year, month - 1, day));
	if (date.getUTCMonth() !== month - 1 || date.getUTCDate() !== day) return fallback(value);
	// A calendar date has no instant: format it in UTC so no timezone moves it to another day.
	const text = new Intl.DateTimeFormat(intlLocale(ctx.locale), {
		dateStyle: 'medium',
		timeZone: 'UTC'
	}).format(date);
	return { kind: 'date', text, iso: iso.slice(0, 10) };
}

function datetimeDisplay(value: unknown, ctx: FieldDisplayContext): FieldDisplay {
	if (typeof value !== 'string' || !value.trim())
		return recordValueIsEmpty(value) ? EMPTY : fallback(value);
	const iso = value.trim();
	const local = LOCAL_DATETIME_PATTERN.exec(iso);
	let date: Date;
	const options: Intl.DateTimeFormatOptions = { dateStyle: 'medium', timeStyle: 'short' };
	if (local) {
		// No offset stored: the wall-clock time the author entered, shown as entered.
		date = new Date(
			Date.UTC(
				Number(local[1]),
				Number(local[2]) - 1,
				Number(local[3]),
				Number(local[4]),
				Number(local[5]),
				Number(local[6] ?? 0)
			)
		);
		options.timeZone = 'UTC';
	} else {
		date = new Date(iso);
		if (ctx.timeZone) options.timeZone = ctx.timeZone;
	}
	if (Number.isNaN(date.getTime())) return fallback(value);
	return {
		kind: 'date',
		text: new Intl.DateTimeFormat(intlLocale(ctx.locale), options).format(date),
		iso
	};
}

// People, links, places.

function memberDisplay(value: unknown, ctx: FieldDisplayContext): FieldDisplay {
	const userId =
		typeof value === 'string'
			? value.trim()
			: isRecord(value) && typeof value.user_id === 'string'
				? value.user_id.trim()
				: '';
	if (!userId) return recordValueIsEmpty(value) ? EMPTY : fallback(value);
	const known = ctx.members.get(userId);
	const ownName = isRecord(value) && typeof value.name === 'string' ? value.name.trim() : '';
	const ownEmail = isRecord(value) && typeof value.email === 'string' ? value.email.trim() : '';
	const name = ownName || known?.name?.trim() || '';
	const email = ownEmail || known?.email?.trim() || undefined;
	return {
		kind: 'member',
		userId,
		name: name || email || '',
		email,
		known: Boolean(name || email)
	};
}

const EMAIL_PATTERN = /^[^\s@<>()]+@[^\s@<>()]+\.[^\s@<>()]+$/;

function emailDisplay(value: unknown): FieldDisplay {
	if (typeof value !== 'string' || !value.trim())
		return recordValueIsEmpty(value) ? EMPTY : fallback(value);
	const text = value.trim();
	const href = EMAIL_PATTERN.test(text) ? safeLinkUrl(`mailto:${text}`) : '';
	return href
		? { kind: 'link', href, text, scheme: 'mailto' }
		: { kind: 'text', text, multiline: false, clampLines: 1 };
}

function phoneDisplay(value: unknown): FieldDisplay {
	if (typeof value !== 'string' && typeof value !== 'number')
		return recordValueIsEmpty(value) ? EMPTY : fallback(value);
	const text = String(value).trim();
	if (!text) return EMPTY;
	// The href is built from digits only, so nothing the author typed reaches the URL scheme.
	const dial = text.replace(/[^\d+]/g, '').replace(/(?!^)\+/g, '');
	return /\d{3,}/.test(dial)
		? { kind: 'link', href: `tel:${dial}`, text, scheme: 'tel' }
		: { kind: 'text', text, multiline: false, clampLines: 1 };
}

function coordinate(value: unknown, limit: number): number | null {
	const number =
		typeof value === 'number'
			? value
			: typeof value === 'string' && value.trim()
				? Number(value)
				: NaN;
	return Number.isFinite(number) && Math.abs(number) <= limit ? number : null;
}

function locationDisplay(value: unknown): FieldDisplay {
	let source: Record<string, unknown> | null = isRecord(value) ? value : null;
	if (typeof value === 'string' && value.trim()) {
		const [lat, lng, ...label] = value.split(',');
		source = { lat, lng, label: label.join(',').trim() };
	}
	if (!source) return recordValueIsEmpty(value) ? EMPTY : fallback(value);
	const lat = coordinate(source.lat ?? source.latitude, 90);
	const lng = coordinate(source.lng ?? source.lon ?? source.longitude, 180);
	const label =
		[source.label, source.address, source.name]
			.find((item): item is string => typeof item === 'string' && item.trim() !== '')
			?.trim() ?? undefined;
	if (lat === null || lng === null) {
		return label
			? { kind: 'text', text: label, multiline: true, clampLines: 3 }
			: recordValueIsEmpty(value)
				? EMPTY
				: fallback(value);
	}
	return { kind: 'location', label, lat, lng, coordinates: `${lat.toFixed(5)}, ${lng.toFixed(5)}` };
}

// Media and files.

function urlsOf(value: unknown): string[] {
	if (!value) return [];
	if (typeof value === 'string') return value.trim() ? [value.trim()] : [];
	if (Array.isArray(value)) return value.flatMap(urlsOf);
	if (isRecord(value)) {
		for (const key of ['url', 'href', 'src']) {
			if (typeof value[key] === 'string') return urlsOf(value[key]);
		}
	}
	return [];
}

function fileNameFromUrl(url: string): string {
	if (url.startsWith('data:')) return 'file';
	try {
		const path = new URL(url, 'https://sylvode.invalid/').pathname;
		const last = decodeURIComponent(path.split('/').filter(Boolean).pop() ?? '');
		return last || url;
	} catch {
		return url;
	}
}

export function formatByteSize(bytes: number, locale: string): string {
	const intl = intlLocale(locale);
	const units = ['byte', 'kilobyte', 'megabyte', 'gigabyte'] as const;
	let value = Math.max(0, bytes);
	let unit = 0;
	while (value >= 1024 && unit < units.length - 1) {
		value /= 1024;
		unit += 1;
	}
	return new Intl.NumberFormat(intl, {
		style: 'unit',
		unit: units[unit],
		unitDisplay: 'short',
		maximumFractionDigits: unit === 0 ? 0 : 1
	}).format(value);
}

function typeLabel(contentType: string | undefined, name: string): string | undefined {
	const extension = /\.([a-z0-9]{1,8})$/i.exec(name)?.[1];
	if (extension) return extension.toUpperCase();
	const subtype = contentType?.split('/')[1]?.split(';')[0];
	return subtype ? subtype.toUpperCase() : undefined;
}

const IMAGE_EXTENSION = /\.(png|jpe?g|gif|webp|avif|bmp)(?:[?#].*)?$/i;

function fieldAttachments(field: FormField, ctx: FieldDisplayContext): FormAttachment[] {
	return (ctx.attachmentsByField[field.key] ?? []).filter((attachment) => !attachment.archived_at);
}

function attachmentHref(attachment: FormAttachment, ctx: FieldDisplayContext): string {
	return safeLinkUrl(ctx.signedUrls[attachment.id] || attachment.url);
}

function imagesDisplay(field: FormField, value: unknown, ctx: FieldDisplayContext): FieldDisplay {
	const label = field.label?.trim() || field.key;
	const items: ImageItem[] = [];
	let blocked = 0;
	for (const attachment of fieldAttachments(field, ctx)) {
		const full = safeImageUrl(ctx.signedUrls[attachment.id] || attachment.url);
		const thumb = safeImageUrl(attachment.thumbnail_url) || full;
		if (thumb) items.push({ src: thumb, full: full || thumb, alt: attachment.file_name || label });
		else blocked += 1;
	}
	for (const url of urlsOf(value)) {
		if (items.some((item) => item.full === url || item.src === url)) continue;
		const src = safeImageUrl(url);
		if (src) items.push({ src, full: src, alt: `${label} ${items.length + 1}` });
		else blocked += 1;
	}
	if (items.length === 0 && blocked === 0) return EMPTY;
	return { kind: 'images', items, blocked };
}

function filesDisplay(field: FormField, value: unknown, ctx: FieldDisplayContext): FieldDisplay {
	const items: FileItem[] = [];
	const seen = new Set<string>();
	for (const attachment of fieldAttachments(field, ctx)) {
		const href = attachmentHref(attachment, ctx);
		const isImage = attachment.content_type?.startsWith('image/') ?? false;
		seen.add(attachment.url);
		items.push({
			name: attachment.file_name || fileNameFromUrl(attachment.url),
			href: href || undefined,
			download: href || undefined,
			sizeLabel: Number.isFinite(attachment.byte_size)
				? formatByteSize(attachment.byte_size, ctx.locale)
				: undefined,
			typeLabel: typeLabel(attachment.content_type, attachment.file_name ?? ''),
			blocked: !href,
			isImage,
			thumb: isImage
				? safeImageUrl(attachment.thumbnail_url) || safeImageUrl(href) || undefined
				: undefined
		});
	}
	const refs = Array.isArray(value) ? value : value ? [value] : [];
	for (const ref of refs) {
		for (const url of urlsOf(ref)) {
			if (seen.has(url)) continue;
			seen.add(url);
			const href = safeLinkUrl(url);
			const name =
				isRecord(ref) && typeof ref.name === 'string' && ref.name.trim()
					? ref.name.trim()
					: fileNameFromUrl(url);
			const size =
				isRecord(ref) && typeof ref.size === 'number'
					? ref.size
					: isRecord(ref) && typeof ref.byte_size === 'number'
						? ref.byte_size
						: null;
			const isImage = IMAGE_EXTENSION.test(url) || url.startsWith('data:image/');
			items.push({
				name,
				href: href || undefined,
				download: href || undefined,
				sizeLabel: size !== null ? formatByteSize(size, ctx.locale) : undefined,
				typeLabel: typeLabel(
					isRecord(ref) && typeof ref.content_type === 'string' ? ref.content_type : undefined,
					name
				),
				blocked: !href,
				isImage,
				thumb: isImage ? safeImageUrl(url) || undefined : undefined
			});
		}
	}
	return items.length === 0 ? EMPTY : { kind: 'files', items };
}

function signatureDisplay(value: unknown): FieldDisplay {
	const [url] = urlsOf(value);
	if (!url) return recordValueIsEmpty(value) ? EMPTY : fallback(value);
	const src = safeImageUrl(url);
	return { kind: 'signature', src, blocked: !src };
}

function relationDisplay(field: FormField, value: unknown, ctx: FieldDisplayContext): FieldDisplay {
	const first = Array.isArray(value) ? value[0] : value;
	const recordId =
		typeof first === 'string'
			? first.trim()
			: isRecord(first) && typeof first.record_id === 'string'
				? first.record_id.trim()
				: '';
	if (!recordId) return recordValueIsEmpty(value) ? EMPTY : fallback(value);
	const ownTitle =
		isRecord(first) && typeof first.title === 'string' && first.title.trim()
			? first.title.trim()
			: isRecord(first) && typeof first.display === 'string' && first.display.trim()
				? first.display.trim()
				: '';
	const title = ownTitle || ctx.relationTitle?.(field, recordId)?.trim() || '';
	const formKey =
		(isRecord(first) && typeof first.form_key === 'string'
			? first.form_key
			: field.relation?.form_key) || undefined;
	return {
		kind: 'relation',
		recordId,
		title,
		formKey,
		sameForm: Boolean(formKey) && formKey === ctx.formKey,
		available: Boolean(title)
	};
}

function childrenDisplay(value: unknown): FieldDisplay {
	const count = Array.isArray(value)
		? value.filter((row) => row !== null && row !== undefined).length
		: 0;
	return count === 0
		? recordValueIsEmpty(value)
			? EMPTY
			: fallback(value)
		: { kind: 'children', count };
}

function optionsDisplay(field: FormField, value: unknown): FieldDisplay {
	const labels = (Array.isArray(value) ? value : [value])
		.map((item) =>
			typeof item === 'string' || typeof item === 'number' ? String(item).trim() : ''
		)
		.filter(Boolean);
	if (labels.length === 0) return recordValueIsEmpty(value) ? EMPTY : fallback(value);
	return { kind: 'options', field, value, labels };
}

function booleanDisplay(value: unknown): FieldDisplay {
	if (typeof value === 'boolean') return { kind: 'boolean', value };
	if (value === 'true' || value === 'false') return { kind: 'boolean', value: value === 'true' };
	return recordValueIsEmpty(value) ? EMPTY : fallback(value);
}

function textDisplay(
	value: unknown,
	options: { multiline: boolean; clampLines: number; mono?: boolean }
): FieldDisplay {
	if (typeof value === 'number') return { kind: 'text', text: String(value), ...options };
	if (typeof value !== 'string') return recordValueIsEmpty(value) ? EMPTY : fallback(value);
	const text = options.multiline ? value.replace(/\r\n/g, '\n').trim() : value.trim();
	return text ? { kind: 'text', text, ...options } : EMPTY;
}

type Renderer = (field: FormField, value: unknown, ctx: FieldDisplayContext) => FieldDisplay;

const RENDERERS = {
	text: (_f, v) => textDisplay(v, { multiline: false, clampLines: 3 }),
	textarea: (_f, v) => textDisplay(v, { multiline: true, clampLines: 6 }),
	rich_text: (_f, v) =>
		typeof v === 'string'
			? textDisplay(richTextToPlainText(v), { multiline: true, clampLines: 8 })
			: textDisplay(v, { multiline: true, clampLines: 8 }),
	ai_summary: (_f, v) => textDisplay(v, { multiline: true, clampLines: 6 }),
	address: (_f, v) => textDisplay(v, { multiline: true, clampLines: 3 }),
	scan: (_f, v) => textDisplay(v, { multiline: false, clampLines: 1, mono: true }),
	autonumber: (_f, v) => textDisplay(v, { multiline: false, clampLines: 1, mono: true }),
	number: (_f, v, c) => numberDisplay(v, c),
	integer: (_f, v, c) => integerDisplay(v, c),
	amount: (f, v, c) => amountDisplay(f, v, c),
	rating: (f, v) => ratingDisplay(f, v),
	progress: (_f, v, c) => progressDisplay(v, c),
	formula: (f, v, c) => formulaDisplay(f, v, c),
	date: (_f, v, c) => dateDisplay(v, c),
	datetime: (_f, v, c) => datetimeDisplay(v, c),
	single_select: (f, v) => optionsDisplay(f, v),
	multi_select: (f, v) => optionsDisplay(f, v),
	boolean: (_f, v) => booleanDisplay(v),
	member: (_f, v, c) => memberDisplay(v, c),
	email: (_f, v) => emailDisplay(v),
	phone: (_f, v) => phoneDisplay(v),
	location: (_f, v) => locationDisplay(v),
	signature: (_f, v) => signatureDisplay(v),
	image: (f, v, c) => imagesDisplay(f, v, c),
	attachment: (f, v, c) => filesDisplay(f, v, c),
	relation: (f, v, c) => relationDisplay(f, v, c),
	child_table: (_f, v) => childrenDisplay(v)
} satisfies Record<FormFieldType, Renderer>;

/** The field types with a read-only presentation; equal to the `FormFieldType` union. */
export const DISPLAY_FIELD_TYPES = Object.keys(RENDERERS) as FormFieldType[];

export function fieldDisplay(
	field: FormField,
	value: unknown,
	ctx: FieldDisplayContext
): FieldDisplay {
	const type = (field.type ?? 'text') as string;
	const renderer = (RENDERERS as Record<string, Renderer>)[type];
	try {
		return renderer ? renderer(field, value, ctx) : fallback(value);
	} catch {
		// A malformed value never breaks the list; it is shown as text.
		return fallback(value);
	}
}

export function isEmptyValue(field: FormField, value: unknown, ctx?: FieldDisplayContext): boolean {
	if (!ctx) return recordValueIsEmpty(value);
	return fieldDisplay(field, value, ctx).kind === 'empty';
}

/** What the copy button copies, or `null` when the value offers no copy (binary, previews). */
export function copyText(display: FieldDisplay): string | null {
	switch (display.kind) {
		case 'text':
		case 'json':
			return display.text;
		case 'number':
			return display.raw;
		case 'date':
			return display.iso;
		case 'rating':
			return String(display.value);
		case 'progress':
			return display.text;
		case 'options':
			return display.labels.join(', ');
		case 'member':
			return display.email || display.name || display.userId;
		case 'link':
			return display.text;
		case 'location':
			return `${display.lat},${display.lng}`;
		case 'relation':
			return display.title || display.recordId;
		case 'files':
			return (
				display.items
					.map((item) => item.href)
					.filter(Boolean)
					.join('\n') || null
			);
		case 'images':
			return (
				display.items
					.map((item) => item.full ?? item.src)
					.filter((src) => !src.startsWith('data:'))
					.join('\n') || null
			);
		case 'boolean':
		case 'signature':
		case 'children':
		case 'empty':
			return null;
	}
}
