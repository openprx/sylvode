<svelte:options runes />

<script lang="ts">
	import { onMount } from 'svelte';
	import { locale, t } from 'svelte-i18n';
	import { authApi } from '#lib/api/auth.js';
	import { describeResult } from '#lib/i18n/describe.js';
	import { toast } from '#lib/stores/toast.js';
	import Card from '#lib/components/Card.svelte';
	import Button from '#lib/ui/button/Button.svelte';
	import Switch from '#lib/ui/switch/Switch.svelte';
	import {
		digestSwitchState,
		emailSwitchState,
		fromAnswer,
		languageOf,
		toApi,
		withDigest,
		withEmail,
		type PreferencesView
	} from './preferences.js';

	let view = $state<PreferencesView>({
		emailAvailable: true,
		form: { emailNotification: false, dailyDigest: false, mentionOnly: false }
	});
	let loadFailed = $state(false);
	let saving = $state(false);

	const emailState = $derived(emailSwitchState(view));
	const digestState = $derived(digestSwitchState(view));
	const language = $derived($t(languageOf($locale) === 'zh' ? 'settings.zh' : 'settings.en'));

	onMount(async () => {
		const response = await authApi.getPreferences();
		if (response.code === 0 && response.data?.notification_prefs) {
			view = fromAnswer(response.data);
		} else {
			loadFailed = true;
			toast.error(describeResult(response));
		}
	});

	async function save() {
		if (saving || loadFailed) return;
		saving = true;
		const response = await authApi.updatePreferences(toApi(view.form, $locale));
		if (response.code === 0 && response.data?.notification_prefs) {
			view = fromAnswer(response.data);
			toast.success($t('notificationPrefs.saved'));
		} else {
			toast.error(describeResult(response));
		}
		saving = false;
	}
</script>

<Card>
	<h2 class="text-lg font-semibold text-fg">{$t('notificationPrefs.title')}</h2>
	{#if !view.emailAvailable}
		<p class="mt-2 text-sm text-fg-muted" data-testid="email-unavailable">
			{$t('notificationPrefs.emailUnavailable')}
		</p>
	{/if}
	<div class="mt-4 space-y-4 text-sm text-fg">
		<div>
			<Switch
				label={$t('notificationPrefs.email')}
				checked={view.form.emailNotification}
				disabled={emailState.disabled || loadFailed}
				data-testid="pref-email"
				onchange={(event) => (view.form = withEmail(view.form, event.currentTarget.checked))}
			/>
		</div>
		<div>
			<Switch
				label={$t('notificationPrefs.digest')}
				checked={view.form.dailyDigest}
				disabled={digestState.disabled || loadFailed}
				data-testid="pref-digest"
				onchange={(event) => (view.form = withDigest(view.form, event.currentTarget.checked))}
			/>
			{#if view.emailAvailable && digestState.reasonKey}
				<p class="mt-1 text-xs text-fg-muted">{$t(digestState.reasonKey)}</p>
			{/if}
		</div>
		<div>
			<Switch
				label={$t('notificationPrefs.mentionOnly')}
				checked={view.form.mentionOnly}
				disabled={loadFailed}
				data-testid="pref-mention-only"
				onchange={(event) =>
					(view.form = { ...view.form, mentionOnly: event.currentTarget.checked })}
			/>
			<p class="mt-1 text-xs text-fg-muted">{$t('notificationPrefs.mentionOnlyHelp')}</p>
		</div>
	</div>
	<p class="mt-4 text-xs text-fg-muted">
		{$t('notificationPrefs.languageNote', { values: { language } })}
	</p>
	<div class="mt-4 flex justify-end">
		<Button variant="primary" loading={saving} disabled={loadFailed} onclick={save}
			>{$t('notificationPrefs.save')}</Button
		>
	</div>
</Card>
