import type { PluginSlot, PluginUpload } from '../../api/plugin-packages';
import template from './order-amount-template.json';

export interface CreatorDraft {
	key: string;
	name: string;
	publisher: string;
	version: string;
	formKey: string;
	detailLabel: string;
	detailTitle: string;
	detailText: string;
	actionLabel: string;
	actionTitle: string;
	actionText: string;
}

export const detailSlot = 'form.record.detail';
export const actionSlot = 'form.record.actions';
export const defaultDraft: Readonly<CreatorDraft> = {
	key: 'custom_order_amount',
	name: 'Custom order amount',
	publisher: 'Project author',
	version: '1.0.0',
	formKey: 'order_line',
	detailLabel: 'Order amount panel',
	detailTitle: 'Order amount breakdown',
	detailText: 'Review the amount for this order line’s quantity and unit price.',
	actionLabel: 'Calculate order amount',
	actionTitle: 'Calculate order amount',
	actionText: 'Preview this order line’s total from the record actions.'
};

export function creatorSlot(value: string | null): PluginSlot {
	return value === actionSlot ? actionSlot : detailSlot;
}

export function creatorPlacement(slot: PluginSlot) {
	return { id: slot === actionSlot ? 'amount_action' : 'detail_panel', slot };
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();
const keyPattern = /^[a-z0-9]+(?:_[a-z0-9]+)*$/;
const releasePattern = /^(0|[1-9]\d{0,8})\.(0|[1-9]\d{0,8})\.(0|[1-9]\d{0,8})$/;

export function invalidDraftFields(draft: CreatorDraft): (keyof CreatorDraft)[] {
	return (Object.keys(defaultDraft) as (keyof CreatorDraft)[]).filter((field) => {
		const value = draft[field].trim();
		if (field === 'key' || field === 'formKey') return value.length > 64 || !keyPattern.test(value);
		if (field === 'version') return !releasePattern.test(value);
		const limit = field.endsWith('Text') ? 320 : 120;
		return (
			!value || encoder.encode(value).length > limit || /[\u0000-\u001f\u007f-\u009f]/.test(value)
		);
	});
}

export function readDraft(
	value: string | null,
	originFormKey = defaultDraft.formKey
): CreatorDraft {
	const defaults = {
		...defaultDraft,
		formKey:
			keyPattern.test(originFormKey) && originFormKey.length <= 64
				? originFormKey
				: defaultDraft.formKey
	};
	if (!value) return defaults;
	try {
		const prior: unknown = JSON.parse(value);
		if (!prior || typeof prior !== 'object') return defaults;
		return Object.fromEntries(
			Object.entries(defaults).map(([key, fallback]) => {
				const value = (prior as Record<string, unknown>)[key];
				return [key, typeof value === 'string' && value.length <= 320 ? value : fallback];
			})
		) as unknown as CreatorDraft;
	} catch {
		return defaults;
	}
}

function decodeBase64(value: string) {
	return Uint8Array.from(atob(value), (byte) => byte.charCodeAt(0));
}

async function sha256(bytes: Uint8Array<ArrayBuffer>): Promise<string> {
	const digest = await crypto.subtle.digest('SHA-256', bytes);
	return [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, '0')).join('');
}

export interface CreatorPackage {
	upload: PluginUpload;
	html: string;
	json: string;
	fixtureCents: number;
}

export async function buildCreatorPackage(draft: CreatorDraft): Promise<CreatorPackage> {
	if (invalidDraftFields(draft).length) throw new Error('Invalid draft');
	const { key, name, publisher, version, formKey, detailLabel, actionLabel, ...text } = draft;
	const authorCopy = JSON.stringify(
		Object.fromEntries(Object.entries(text).map(([field, value]) => [field, value.trim()]))
	).replaceAll('<', '\\u003c');
	const html = decoder
		.decode(decodeBase64(template.web_base64))
		.replace(
			'<main>',
			() => '<script type="application/json" id="author-copy">' + authorCopy + '</script>\n<main>'
		)
		.replace(
			'    const placement = event.data.placement;',
			"    const placement = event.data.placement;\n    const author = JSON.parse(document.getElementById('author-copy').textContent);"
		)
		.replace("'Calculate order amount';", 'author.actionTitle;')
		.replace("'Preview this order line’s total from the record actions.';", 'author.actionText;')
		.replace("'Order amount breakdown';", 'author.detailTitle;')
		.replace(
			"'Review the amount for this order line’s quantity and unit price.';",
			'author.detailText;'
		);
	const web = encoder.encode(html);
	const webSha = await sha256(web);
	const source = template.manifest;
	const manifest = {
		...source,
		key: key.trim(),
		name: name.trim(),
		publisher: publisher.trim(),
		version: version.trim(),
		ui: {
			...source.ui,
			form_key: formKey.trim(),
			entry: { path: 'panel.html', size: web.length, sha256: webSha },
			contributions: [
				{ ...creatorPlacement(detailSlot), label: detailLabel.trim() },
				{ ...creatorPlacement(actionSlot), label: actionLabel.trim() }
			]
		},
		capabilities: source.capabilities.map((capability) => ({
			...capability,
			name: capability.name as 'amount.preview' | 'amount.apply'
		}))
	};
	const upload = {
		manifest,
		wasm_base64: template.wasm_base64,
		web_base64: btoa(String.fromCharCode(...web))
	};
	return {
		upload,
		html,
		json: JSON.stringify(upload, null, 2) + '\n',
		fixtureCents: await runFixedGuest()
	};
}

async function runFixedGuest(): Promise<number> {
	const bytes = decodeBase64(template.wasm_base64);
	if ((await sha256(bytes)) !== template.manifest.wasm.sha256)
		throw new Error('Template integrity error');
	const module = await WebAssembly.compile(bytes);
	if (WebAssembly.Module.imports(module).length)
		throw new Error('Template imports are not allowed');
	const { exports } = await WebAssembly.instantiate(module, {});
	const guest = exports as unknown as {
		memory: WebAssembly.Memory;
		openpr_plugin_abi_version: () => number;
		openpr_alloc: (length: number) => number;
		openpr_invoke: (pointer: number, length: number) => bigint;
	};
	if (guest.openpr_plugin_abi_version() !== 1) throw new Error('Unsupported template ABI');
	const input = encoder.encode(
		JSON.stringify({ capability: 'amount.preview', quantity: 2, unit_price_cents: 1250 })
	);
	const pointer = guest.openpr_alloc(input.length);
	new Uint8Array(guest.memory.buffer, pointer, input.length).set(input);
	const output = guest.openpr_invoke(pointer, input.length);
	const offset = Number(output >> 32n);
	const size = Number(output & 0xffffffffn);
	if (size < 1 || size > 64) throw new Error('Invalid template output');
	const result = JSON.parse(decoder.decode(new Uint8Array(guest.memory.buffer, offset, size))) as {
		amount_cents: number;
	};
	if (result.amount_cents !== 2500) throw new Error('Template output did not match the fixture');
	return result.amount_cents;
}
