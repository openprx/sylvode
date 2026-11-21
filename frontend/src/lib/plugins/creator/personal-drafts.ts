import { creatorTemplates, draftStorageKey, type CreatorTemplateId } from './registry';
import type { CreatorDraft } from './template';
import type { UiViewDraft } from './ui-view-template';

const draftPrefix = (actor: string) =>
	`sv.plugin.creator.v2:personal:${encodeURIComponent(actor)}:`;
const valid = (id: string) => id === 'default' || /^[a-f0-9-]{36}$/.test(id);

export function personalDraftKey(
	actor: string,
	id: string,
	templateId: CreatorTemplateId = 'order_amount'
): string {
	return draftStorageKey('personal', actor, templateId, valid(id) ? id : 'default');
}

export interface PersonalDraftEntry {
	id: string;
	templateId: CreatorTemplateId;
	draft: CreatorDraft | UiViewDraft;
}

export function listPersonalDrafts(actor: string): PersonalDraftEntry[] {
	const entries: PersonalDraftEntry[] = [];
	const origin = { slot: 'form.record.detail' as const, formKey: null };
	try {
		const keys = Object.keys(localStorage);
		for (const key of keys.filter((entry) => entry.startsWith(draftPrefix(actor)))) {
			const [templateId, id, ...rest] = key.slice(draftPrefix(actor).length).split(':');
			const value = localStorage.getItem(key);
			if (rest.length || !value || !valid(id)) continue;
			if (templateId === 'order_amount') {
				entries.push({ id, templateId, draft: creatorTemplates.order_amount.read(value, origin) });
			} else if (templateId === 'ui_view') {
				entries.push({
					id,
					templateId,
					draft: creatorTemplates.ui_view.read(value, { slot: 'project.view', formKey: null })
				});
			} else continue;
		}
	} catch {
		return entries;
	}
	return entries;
}
