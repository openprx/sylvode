// The notification switches of Settings, as pure functions: the screen shows what these return, and
// the rules of the server (a daily summary needs e-mail; `language` is always sent) live here once.

import type {
	NotificationLanguage,
	NotificationPreferences,
	PreferencesAnswer
} from '#lib/api/auth.js';

export interface PreferencesForm {
	emailNotification: boolean;
	dailyDigest: boolean;
	mentionOnly: boolean;
}

export interface PreferencesView {
	form: PreferencesForm;
	/** False when the server has no e-mail delivery set up. */
	emailAvailable: boolean;
}

/** The e-mail language for an interface locale: Chinese locales write Chinese, all others English. */
export function languageOf(locale: string | null | undefined): NotificationLanguage {
	return locale?.toLowerCase().startsWith('zh') ? 'zh' : 'en';
}

/** What the server answered, as the screen's form. A summary without e-mail reads as off. */
export function fromAnswer(answer: PreferencesAnswer): PreferencesView {
	const prefs = answer.notification_prefs;
	return {
		emailAvailable: answer.email_available === true,
		form: {
			emailNotification: prefs.email_notification,
			dailyDigest: prefs.email_notification && prefs.daily_digest,
			mentionOnly: prefs.mention_only
		}
	};
}

/** The body of a save: `language` is the current interface language, and a summary needs e-mail on. */
export function toApi(
	form: PreferencesForm,
	locale: string | null | undefined
): NotificationPreferences {
	return {
		email_notification: form.emailNotification,
		mention_only: form.mentionOnly,
		daily_digest: form.emailNotification && form.dailyDigest,
		language: languageOf(locale)
	};
}

/** Switching e-mail off switches the summary off with it; the other switches never touch each other. */
export function withEmail(form: PreferencesForm, on: boolean): PreferencesForm {
	return { ...form, emailNotification: on, dailyDigest: on && form.dailyDigest };
}

export function withDigest(form: PreferencesForm, on: boolean): PreferencesForm {
	return { ...form, dailyDigest: form.emailNotification && on };
}

export interface SwitchState {
	disabled: boolean;
	/** Message key of the reason beside a disabled switch, if any. */
	reasonKey: string | undefined;
}

export const EMAIL_UNAVAILABLE_KEY = 'notificationPrefs.emailUnavailable';
export const DIGEST_NEEDS_EMAIL_KEY = 'notificationPrefs.digestNeedsEmail';

/** Both e-mail switches are disabled without server e-mail; the summary also waits for e-mail on. */
export function emailSwitchState(view: PreferencesView): SwitchState {
	return view.emailAvailable
		? { disabled: false, reasonKey: undefined }
		: { disabled: true, reasonKey: EMAIL_UNAVAILABLE_KEY };
}

export function digestSwitchState(view: PreferencesView): SwitchState {
	if (!view.emailAvailable) return { disabled: true, reasonKey: EMAIL_UNAVAILABLE_KEY };
	if (!view.form.emailNotification) return { disabled: true, reasonKey: DIGEST_NEEDS_EMAIL_KEY };
	return { disabled: false, reasonKey: undefined };
}
