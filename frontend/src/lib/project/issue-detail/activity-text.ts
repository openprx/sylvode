/** The wording of the activity entries about a work item's own fields (the page shows the rest). */

type Translate = (key: string, options?: { values?: Record<string, string> }) => string;

function text(detail: Record<string, unknown>, key: string): string {
	const value = detail[key];
	return typeof value === 'string' ? value : '';
}

/** A due date from its timestamp, as a plain date. */
function day(value: string): string {
	const time = new Date(value);
	return Number.isNaN(time.getTime()) ? value : time.toLocaleDateString();
}

/** The sentence of a `created`, `title_changed`, `description_changed` or `due_date_changed` entry; `null` for any other action. */
export function itemFieldActivityText(
	t: Translate,
	action: string,
	detail: Record<string, unknown>
): string | null {
	switch (action) {
		case 'created':
			return t('activity.created');
		case 'title_changed':
			return t('activity.titleChanged', {
				values: { from: text(detail, 'from'), to: text(detail, 'to') }
			});
		case 'description_changed':
			return t('activity.descriptionChanged');
		case 'due_date_changed': {
			const from = text(detail, 'from');
			const to = text(detail, 'to');
			if (!to) return t('activity.dueDateCleared');
			if (!from) return t('activity.dueDateSet', { values: { to: day(to) } });
			return t('activity.dueDateChanged', { values: { from: day(from), to: day(to) } });
		}
		default:
			return null;
	}
}
