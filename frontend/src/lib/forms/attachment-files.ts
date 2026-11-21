import type { FormAttachment, FormField, FormRecord } from '#lib/api/forms.js';
import { safeImageUrl } from '#lib/utils/safe-url.js';
import { isRecord } from '#lib/forms/record-format.js';

export function recordMediaUrl(record: FormRecord, field: FormField | null): string {
	if (!field) return '';

	const raw = record.values[field.key];

	if (typeof raw === 'string') return raw;

	if (isRecord(raw)) {
		if (typeof raw.url === 'string') return raw.url;
		if (typeof raw.href === 'string') return raw.href;
		if (typeof raw.src === 'string') return raw.src;
	}

	if (Array.isArray(raw)) {
		const first = raw.find((item) => typeof item === 'string' || isRecord(item));

		if (typeof first === 'string') return first;

		if (isRecord(first)) {
			if (typeof first.url === 'string') return first.url;
			if (typeof first.href === 'string') return first.href;
			if (typeof first.src === 'string') return first.src;
		}
	}

	return '';
}

export function attachmentFileName(url: string, field: FormField): string {
	const cleaned = url.trim();

	if (!cleaned) return field.type === 'image' ? `${field.key}.png` : `${field.key}.bin`;

	try {
		const parsed = new URL(cleaned);
		const name = parsed.pathname.split('/').filter(Boolean).pop();

		if (name) return decodeURIComponent(name);
	} catch {
		const name = cleaned.split('/').filter(Boolean).pop();

		if (name) return name;
	}

	return field.type === 'image' ? `${field.key}.png` : `${field.key}.bin`;
}

export function attachmentContentType(fileName: string, field: FormField): string {
	const lower = fileName.toLowerCase();

	if (field.type === 'image') {
		if (lower.endsWith('.jpg') || lower.endsWith('.jpeg')) return 'image/jpeg';
		if (lower.endsWith('.webp')) return 'image/webp';
		if (lower.endsWith('.gif')) return 'image/gif';

		return 'image/png';
	}

	if (lower.endsWith('.pdf')) return 'application/pdf';
	if (lower.endsWith('.png')) return 'image/png';
	if (lower.endsWith('.jpg') || lower.endsWith('.jpeg')) return 'image/jpeg';

	return 'application/octet-stream';
}

export function attachmentAcceptAttr(field: FormField): string | undefined {
	const accept = field.attachment?.accept?.filter(Boolean) ?? [];

	if (accept.length > 0) return accept.join(',');

	return field.type === 'image' ? 'image/*' : undefined;
}

export function attachmentStorageName(fileName: string): string {
	return fileName.replace(/[^A-Za-z0-9._-]+/g, '_').replace(/^_+|_+$/g, '') || 'file';
}

export function attachmentThumbnailUrl(attachment: FormAttachment): string {
	return safeImageUrl(attachment.thumbnail_url || attachment.url);
}

export function attachmentIsPreviewableImage(attachment: FormAttachment): boolean {
	return (
		attachment.content_type?.startsWith('image/') ||
		/\.(png|jpe?g|webp|gif)$/i.test(attachment.file_name)
	);
}

export function recordValueUrl(value: unknown): string {
	if (typeof value === 'string') return value;
	if (isRecord(value) && typeof value.url === 'string') return value.url;

	return '';
}

/** SVG can carry scripts, so an SVG data URL is never a signature image. */
export function isSvgDataUrl(value: string): boolean {
	return /^\s*data:\s*image\/svg/i.test(value);
}

export function signatureImageValue(draft: Record<string, unknown>, fieldKey: string): string {
	const value = draft[fieldKey];

	if (typeof value !== 'string') return '';

	return (value.startsWith('data:image/') && !isSvgDataUrl(value)) ||
		value.startsWith('/api/v1/uploads/signatures/') ||
		value.startsWith('/uploads/signatures/')
		? value
		: '';
}

export function signaturePoint(
	event: PointerEvent,
	canvas: HTMLCanvasElement
): { x: number; y: number } {
	const rect = canvas.getBoundingClientRect();

	return { x: event.clientX - rect.left, y: event.clientY - rect.top };
}
