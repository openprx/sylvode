import { readDraft, type CreatorDraft } from './template';

const prefix = (actor: string) => `sv.plugin.creator.v1:personal:${encodeURIComponent(actor)}:`;
const valid = (id: string) => id === 'default' || /^[a-f0-9-]{36}$/.test(id);

export function personalDraftKey(actor: string, id: string): string {
	return prefix(actor) + (valid(id) ? id : 'default');
}

export function listPersonalDrafts(actor: string): { id: string; draft: CreatorDraft }[] {
	const start = prefix(actor);
	try {
		return Object.keys(localStorage).flatMap((key) => {
			const id = key.slice(start.length);
			const value = key.startsWith(start) && valid(id) ? localStorage.getItem(key) : null;
			return value ? [{ id, draft: readDraft(value) }] : [];
		});
	} catch {
		return [];
	}
}
