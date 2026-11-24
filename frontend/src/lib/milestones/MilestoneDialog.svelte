<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { milestonesApi, type Milestone } from '#lib/api/milestones.js';
	import Modal from '#lib/components/Modal.svelte';
	import { toast } from '#lib/stores/toast.js';
	import { milestoneFailureMessage, nameProblem } from './rules.js';

	interface Props {
		open: boolean;
		workspaceId: string;
		/** The milestone being edited; null creates one. */
		milestone: Milestone | null;
		/** Called with the milestone the server answered, after the dialog has closed. */
		onsaved: (milestone: Milestone) => void;
	}

	let { open = $bindable(false), workspaceId, milestone, onsaved }: Props = $props();

	let name = $state('');
	let description = $state('');
	let targetDate = $state('');
	let saving = $state(false);
	// A refused name is shown under the field: a toast would sit behind the open dialog.
	let nameError = $state('');
	let nameInput = $state<HTMLInputElement | null>(null);

	// The form starts from the milestone (or empty) each time the dialog opens.
	$effect(() => {
		if (!open) return;
		name = milestone?.name ?? '';
		description = milestone?.description ?? '';
		targetDate = milestone?.target_date ?? '';
		nameError = '';
	});

	function failName(message: string) {
		nameError = message;
		nameInput?.focus();
		nameInput?.select();
	}

	async function submit() {
		if (saving) return;
		const problem = nameProblem(name);
		if (problem) {
			failName($t(problem === 'empty' ? 'milestones.enterName' : 'milestones.nameTooLong'));
			return;
		}
		saving = true;
		const trimmed = name.trim();
		const response = milestone
			? await milestonesApi.update(milestone.id, {
					name: trimmed,
					description: description.trim(),
					target_date: targetDate || null
				})
			: await milestonesApi.create(workspaceId, {
					name: trimmed,
					description: description.trim() || undefined,
					target_date: targetDate || undefined
				});
		saving = false;
		if (response.code !== 0 || !response.data) {
			const text = milestoneFailureMessage($t, response);
			if (response.error_code === 'milestone_name_taken') failName(text);
			else toast.error(text);
			return;
		}
		open = false;
		toast.success($t(milestone ? 'milestones.updatedToast' : 'milestones.createdToast'));
		onsaved(response.data);
	}
</script>

{#snippet footer()}
	<button
		type="button"
		onclick={() => (open = false)}
		class="rounded-control border border-fg px-4 py-2 text-sm text-fg hover:bg-surface"
	>
		{$t('common.cancel')}
	</button>
	<button
		type="button"
		onclick={submit}
		disabled={saving}
		data-testid="milestone-dialog-save"
		class="rounded-control rf-filled bg-primary px-4 py-2 text-sm text-on-primary disabled:bg-surface disabled:text-fg-muted disabled:cursor-not-allowed"
	>
		{#if milestone}
			{saving ? $t('common.saving') : $t('common.save')}
		{:else}
			{saving ? $t('common.creating') : $t('common.create')}
		{/if}
	</button>
{/snippet}

<Modal
	bind:open
	title={$t(milestone ? 'milestones.edit' : 'milestones.create')}
	maxWidthClass="max-w-lg"
	{footer}
>
	<div class="space-y-4">
		<div>
			<label for="milestone-name" class="mb-1 block text-sm font-medium text-fg"
				>{$t('milestones.name')}</label
			>
			<input
				id="milestone-name"
				type="text"
				bind:value={name}
				bind:this={nameInput}
				oninput={() => (nameError = '')}
				aria-invalid={nameError ? 'true' : undefined}
				aria-describedby={nameError ? 'milestone-name-error' : undefined}
				placeholder={$t('milestones.namePlaceholder')}
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg placeholder:text-fg-muted"
			/>
			{#if nameError}
				<p id="milestone-name-error" role="alert" class="mt-1 text-sm text-danger">{nameError}</p>
			{/if}
		</div>
		<div>
			<label for="milestone-description" class="mb-1 block text-sm font-medium text-fg"
				>{$t('milestones.description')}</label
			>
			<textarea
				id="milestone-description"
				bind:value={description}
				rows="3"
				placeholder={$t('milestones.descriptionPlaceholder')}
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg placeholder:text-fg-muted"
			></textarea>
		</div>
		<div>
			<label for="milestone-target-date" class="mb-1 block text-sm font-medium text-fg"
				>{$t('milestones.targetDate')}</label
			>
			<input
				id="milestone-target-date"
				type="date"
				bind:value={targetDate}
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
			/>
		</div>
	</div>
</Modal>
