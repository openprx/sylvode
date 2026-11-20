<script lang="ts">
	/**
	 * "Save view…": one required field, the name, pre-filled and selected. The audience choice is
	 * shown only to people who may share; "Only me" is the default. Enter saves, Esc cancels.
	 */
	import { tick } from 'svelte';
	import { t } from 'svelte-i18n';
	import Popover from './Popover.svelte';

	interface Props {
		open: boolean;
		anchor: HTMLElement | null;
		suggestedName: string;
		canShare: boolean;
		existingNames: string[];
		saving: boolean;
		error: string | null;
		onsave: (input: { name: string; visibility: 'private' | 'shared' }) => void;
		oncancel: () => void;
	}

	let {
		open,
		anchor,
		suggestedName,
		canShare,
		existingNames,
		saving,
		error,
		onsave,
		oncancel
	}: Props = $props();

	let name = $state('');
	let visibility = $state<'private' | 'shared'>('private');
	let input = $state<HTMLInputElement | null>(null);
	let touched = $state(false);

	const trimmed = $derived(name.trim());
	const invalid = $derived(trimmed.length === 0 || trimmed.length > 80);
	const duplicate = $derived(
		existingNames.some((existing) => existing.trim().toLowerCase() === trimmed.toLowerCase())
	);
	const message = $derived(error ?? (touched && invalid ? $t('forms.views.nameRequired') : null));

	let wasOpen = false;
	$effect(() => {
		if (open && !wasOpen) {
			name = suggestedName;
			visibility = 'private';
			touched = false;
			void tick().then(() => {
				input?.focus();
				input?.select();
			});
		}
		wasOpen = open;
	});

	function submit(event: SubmitEvent) {
		event.preventDefault();
		touched = true;
		if (invalid || saving) return;
		onsave({ name: trimmed, visibility: canShare ? visibility : 'private' });
	}
</script>

<Popover
	{open}
	{anchor}
	onclose={oncancel}
	labelledby="save-view-title"
	align="end"
	width={320}
	autofocus={false}
	testId="save-view"
>
	<form class="space-y-3 p-2" onsubmit={submit} data-save-view-form>
		<h3 id="save-view-title" class="text-sm font-semibold text-fg">
			{$t('forms.views.saveTitle')}
		</h3>
		<label class="block space-y-1">
			<span class="text-xs font-medium text-fg-muted"
				>{$t('forms.views.nameLabel')}</span
			>
			<input
				bind:this={input}
				bind:value={name}
				maxlength="80"
				required
				class="h-9 w-full rounded-control border bg-field px-2 text-sm {message
					? 'border-danger'
					: 'border-input'}"
				aria-invalid={message ? 'true' : undefined}
				aria-describedby={message ? 'save-view-error' : duplicate ? 'save-view-hint' : undefined}
				data-save-view-name
			/>
		</label>
		{#if message}
			<p id="save-view-error" class="text-xs text-danger" role="alert">
				{message}
			</p>
		{:else if duplicate}
			<p id="save-view-hint" class="text-xs text-fg-muted">
				{$t('forms.views.duplicateNameHint')}
			</p>
		{/if}
		{#if canShare}
			<fieldset class="space-y-1.5" data-save-view-audience>
				<legend class="mb-1 text-xs font-medium text-fg-muted">
					{$t('forms.views.audienceLabel')}
				</legend>
				<label class="flex min-h-6 items-center gap-2 text-sm">
					<input type="radio" name="save-view-audience" value="private" bind:group={visibility} />
					{$t('forms.views.audiencePrivate')}
				</label>
				<label class="flex min-h-6 items-center gap-2 text-sm">
					<input type="radio" name="save-view-audience" value="shared" bind:group={visibility} />
					{$t('forms.views.audienceShared')}
				</label>
			</fieldset>
		{/if}
		<div class="flex justify-end gap-2">
			<button
				type="button"
				class="h-8 rounded-control px-3 text-sm text-fg hover:bg-surface"
				onclick={() => {
					oncancel();
					anchor?.focus();
				}}>{$t('common.cancel')}</button
			>
			<button
				type="submit"
				class="h-8 rounded-control bg-primary px-3 text-sm font-medium text-on-primary disabled:bg-surface disabled:text-control"
				disabled={saving}
				data-save-view-submit>{$t('common.save')}</button
			>
		</div>
	</form>
</Popover>
