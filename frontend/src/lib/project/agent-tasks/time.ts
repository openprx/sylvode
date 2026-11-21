import { get } from 'svelte/store';
import { locale } from 'svelte-i18n';
import { timeAgo } from '../../utils/timeago';

/** Relative time ("5 minutes ago") through the app's shared formatter; empty for a bad date. */
export function relativeTime(iso: string): string {
	return Number.isNaN(Date.parse(iso)) ? '' : timeAgo(iso);
}

/** Absolute local date and time in the app language, for the hover text of a relative time. */
export function absoluteTime(iso: string): string {
	const at = new Date(iso);
	if (Number.isNaN(at.getTime())) return '';
	return at.toLocaleString(get(locale) === 'en' ? 'en-US' : 'zh-CN', {
		year: 'numeric',
		month: '2-digit',
		day: '2-digit',
		hour: '2-digit',
		minute: '2-digit',
		second: '2-digit'
	});
}
