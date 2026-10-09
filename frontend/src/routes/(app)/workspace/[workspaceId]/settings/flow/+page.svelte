<script lang="ts">
	// Workspace Flow settings. All decisions (request shape, idempotency keys, confirmation
	// gating, error classification) live in `FlowSettingsController`; this component only holds
	// ephemeral UI state (which dialog is open, whether its acknowledgement box is ticked).
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { page } from '$app/stores';
	import { resolve } from '$app/paths';
	import Card from '$lib/components/Card.svelte';
	import EmptyState from '$lib/components/EmptyState.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import { toast } from '$lib/stores/toast';
	import type { FlowFeatureFlags } from '$lib/api/flow';
	import {
		FLOW_MEMBER_LEVELS,
		FLOW_MEMBER_LEVEL_KEYS,
		FlowSettingsController,
		isFlowMemberLevel,
		type FlowMemberLevel,
		type FlowSettingsIntent,
		type FlowSettingsSubmitOutcome
	} from '$lib/flow/flow-settings';
	import { requireRouteParam } from '$lib/utils/route-params';

	const workspaceId = requireRouteParam($page.params.workspaceId, 'workspaceId');
	const controller = new FlowSettingsController(workspaceId);

	let view = $state<'loading' | 'ready' | 'forbidden' | 'failed'>('loading');
	let loadErrorKey = $state('');
	let feature = $state<FlowFeatureFlags | null>(null);
	let selectedLevel = $state<string>('edit');
	let saving = $state(false);
	let statusKey = $state('');
	let statusValues = $state<Record<string, string>>({});
	let errorKey = $state('');
	let retryIntent = $state.raw<FlowSettingsIntent | null>(null);

	// The intent waiting for confirmation, and the user's acknowledgement for it. The box always
	// starts unticked: the confirmation is produced by the user, never prefilled. Intents are held
	// in `$state.raw` because the controller checks them by identity, and `$state` would hand back
	// a deep proxy instead of the object it issued.
	let confirmIntent = $state.raw<FlowSettingsIntent | null>(null);
	let acknowledged = $state(false);
	let disableDialogOpen = $state(false);
	let levelDialogOpen = $state(false);

	const levelLabel = (level: string) =>
		isFlowMemberLevel(level) ? $t(FLOW_MEMBER_LEVEL_KEYS[level]) : level;

	const levelOptions = $derived(
		FLOW_MEMBER_LEVELS.map((level) => ({ value: level, label: $t(FLOW_MEMBER_LEVEL_KEYS[level]) }))
	);

	onMount(async () => {
		const outcome = await controller.load();
		if (outcome.status === 'ready') {
			feature = outcome.feature;
			selectedLevel = outcome.feature.default_member_level;
			view = 'ready';
		} else if (outcome.status === 'forbidden') {
			view = 'forbidden';
		} else {
			loadErrorKey = outcome.messageKey;
			view = 'failed';
		}
	});

	function setStatus(key: string, values: Record<string, string> = {}) {
		statusKey = key;
		statusValues = values;
	}

	function closeDialogs() {
		disableDialogOpen = false;
		levelDialogOpen = false;
		confirmIntent = null;
		acknowledged = false;
	}

	function cancelDialogs() {
		closeDialogs();
		controller.cancel();
		if (feature) selectedLevel = feature.default_member_level;
	}

	async function run(intent: FlowSettingsIntent) {
		saving = true;
		retryIntent = null;
		errorKey = '';
		setStatus('flow.settings.saving');
		const outcome: FlowSettingsSubmitOutcome = await controller.submit(intent);
		saving = false;
		applyOutcome(intent, outcome);
	}

	function applyOutcome(intent: FlowSettingsIntent, outcome: FlowSettingsSubmitOutcome) {
		switch (outcome.status) {
			case 'saved': {
				feature = outcome.feature;
				selectedLevel = outcome.feature.default_member_level;
				const message = outcome.eventId
					? get(t)('flow.settings.saved', { values: { eventId: outcome.eventId } })
					: get(t)('flow.settings.savedNoEvent');
				if (outcome.eventId) setStatus('flow.settings.saved', { eventId: outcome.eventId });
				else setStatus('flow.settings.savedNoEvent');
				toast.success(message, 6000);
				return;
			}
			case 'forbidden':
				view = 'forbidden';
				setStatus('');
				return;
			case 'confirmation_required':
				setStatus('');
				return;
			case 'failed':
				// Announced once, by the `role="alert"` banner below.
				setStatus('');
				errorKey = outcome.messageKey;
				retryIntent = outcome.retryable ? intent : null;
				if (feature) selectedLevel = feature.default_member_level;
				return;
		}
	}

	function requestEnable() {
		const intent = controller.propose({ field: 'enabled', value: true });
		if (intent) void run(intent);
	}

	function requestDisable() {
		const intent = controller.propose({ field: 'enabled', value: false });
		if (!intent) return;
		confirmIntent = intent;
		acknowledged = false;
		disableDialogOpen = true;
	}

	function requestLevelChange() {
		if (!isFlowMemberLevel(selectedLevel)) return;
		const intent = controller.propose({ field: 'default_member_level', value: selectedLevel });
		if (!intent) return;
		confirmIntent = intent;
		acknowledged = false;
		levelDialogOpen = true;
	}

	function confirmPending() {
		const intent = confirmIntent;
		if (!intent || !acknowledged) return;
		controller.confirm(intent);
		closeDialogs();
		void run(intent);
	}

	function formatUpdatedAt(value: string | null): string {
		if (!value) return $t('flow.settings.status.never');
		const date = new Date(value);
		return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
	}

	const pendingLevel = $derived(
		confirmIntent?.change.field === 'default_member_level'
			? (confirmIntent.change.value as FlowMemberLevel)
			: null
	);
</script>

<div class="mx-auto max-w-3xl space-y-6">
	<div>
		<h1 class="text-2xl font-bold text-slate-900 dark:text-slate-100">
			{$t('flow.settings.title')}
		</h1>
		<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">{$t('flow.settings.subtitle')}</p>
	</div>

	{#if view === 'loading'}
		<div
			class="rounded-lg border border-slate-200 bg-white p-6 text-slate-500 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-400"
			role="status"
			aria-live="polite"
		>
			{$t('flow.settings.loading')}
		</div>
	{:else if view === 'forbidden'}
		<div data-testid="flow-settings-forbidden" role="status" aria-live="polite">
			<EmptyState
				icon="office"
				title={$t('flow.settings.forbiddenTitle')}
				description={$t('flow.error.forbidden')}
			/>
		</div>
	{:else if view === 'failed'}
		<div
			class="rounded-lg border border-red-200 bg-red-50 p-6 text-sm text-red-700 dark:border-red-900 dark:bg-red-950/30 dark:text-red-300"
			role="alert"
		>
			{$t(loadErrorKey)}
		</div>
	{:else if feature}
		<p class="sr-only" role="status" aria-live="polite" data-testid="flow-settings-live">
			{statusKey ? $t(statusKey, { values: statusValues }) : ''}
		</p>

		<Card>
			<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">
				{$t('flow.settings.status.heading')}
			</h2>
			<dl
				class="mt-4 grid grid-cols-1 gap-4 text-sm md:grid-cols-2"
				data-testid="flow-settings-status"
			>
				<div>
					<dt class="text-slate-500 dark:text-slate-400">
						{$t('flow.settings.status.flowEnabled')}
					</dt>
					<dd
						class="mt-1 font-medium text-slate-900 dark:text-slate-100"
						data-testid="flow-settings-enabled"
					>
						{feature.flow_enabled ? $t('flow.settings.status.on') : $t('flow.settings.status.off')}
					</dd>
				</div>
				<div>
					<dt class="text-slate-500 dark:text-slate-400">
						{$t('flow.settings.status.defaultMemberLevel')}
					</dt>
					<dd
						class="mt-1 font-medium text-slate-900 dark:text-slate-100"
						data-testid="flow-settings-level"
					>
						{levelLabel(feature.default_member_level)}
					</dd>
				</div>
				<div>
					<dt class="text-slate-500 dark:text-slate-400">
						{$t('flow.settings.status.authzEpoch')}
					</dt>
					<dd
						class="mt-1 font-mono text-slate-900 dark:text-slate-100"
						data-testid="flow-settings-epoch"
					>
						{feature.authz_epoch}
					</dd>
					<dd class="mt-1 text-xs text-slate-500 dark:text-slate-400">
						{$t('flow.settings.status.authzEpochHint')}
					</dd>
				</div>
				<div>
					<dt class="text-slate-500 dark:text-slate-400">{$t('flow.settings.status.updatedAt')}</dt>
					<dd class="mt-1 text-slate-900 dark:text-slate-100">
						{formatUpdatedAt(feature.updated_at)}
					</dd>
				</div>
				<div class="md:col-span-2">
					<dt class="text-slate-500 dark:text-slate-400">{$t('flow.settings.status.updatedBy')}</dt>
					<dd class="mt-1 break-all font-mono text-xs text-slate-900 dark:text-slate-100">
						{feature.updated_by ?? $t('flow.settings.status.never')}
					</dd>
				</div>
			</dl>
		</Card>

		{#if errorKey}
			<div
				class="flex flex-col gap-3 rounded-lg border border-red-200 bg-red-50 p-4 text-sm text-red-700 sm:flex-row sm:items-center sm:justify-between dark:border-red-900 dark:bg-red-950/30 dark:text-red-300"
				role="alert"
			>
				<span>{$t(errorKey)}</span>
				{#if retryIntent}
					{@const intent = retryIntent}
					<button
						type="button"
						class="min-h-11 rounded-md border border-red-300 px-4 text-sm font-medium focus:outline-none focus-visible:ring-2 focus-visible:ring-red-500 dark:border-red-700"
						onclick={() => void run(intent)}
					>
						{$t('flow.settings.retry')}
					</button>
				{/if}
			</div>
		{/if}

		<Card>
			<div class="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
				<div>
					<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">
						{$t('flow.settings.toggle.heading')}
					</h2>
					<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">
						{feature.flow_enabled
							? $t('flow.settings.toggle.descriptionOn')
							: $t('flow.settings.toggle.descriptionOff')}
					</p>
				</div>
				{#if feature.flow_enabled}
					<button
						type="button"
						class="min-h-11 shrink-0 rounded-md bg-red-600 px-4 text-sm font-medium text-white hover:bg-red-700 focus:outline-none focus-visible:ring-2 focus-visible:ring-red-500 focus-visible:ring-offset-2 disabled:opacity-50 motion-safe:transition-colors dark:focus-visible:ring-offset-slate-900"
						disabled={saving}
						onclick={requestDisable}
					>
						{$t('flow.settings.toggle.disable')}
					</button>
				{:else}
					<button
						type="button"
						class="min-h-11 shrink-0 rounded-md bg-blue-600 px-4 text-sm font-medium text-white hover:bg-blue-700 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 focus-visible:ring-offset-2 disabled:opacity-50 motion-safe:transition-colors dark:focus-visible:ring-offset-slate-900"
						disabled={saving}
						onclick={requestEnable}
					>
						{$t('flow.settings.toggle.enable')}
					</button>
				{/if}
			</div>
		</Card>

		<Card>
			<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">
				{$t('flow.settings.level.heading')}
			</h2>
			<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">
				{$t('flow.settings.level.description')}
			</p>
			<div class="mt-4 flex flex-col gap-3 sm:flex-row sm:items-end">
				<div class="flex-1">
					<label
						for="flow-default-member-level"
						class="block text-sm font-medium text-slate-700 dark:text-slate-300"
					>
						{$t('flow.settings.level.label')}
					</label>
					<select
						id="flow-default-member-level"
						class="mt-1 block min-h-11 w-full rounded-md border border-slate-300 bg-white px-3 text-sm focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
						bind:value={selectedLevel}
						disabled={saving}
					>
						{#each levelOptions as option (option.value)}
							<option value={option.value}>{option.label}</option>
						{/each}
					</select>
				</div>
				<button
					type="button"
					class="min-h-11 shrink-0 rounded-md bg-blue-600 px-4 text-sm font-medium text-white hover:bg-blue-700 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 focus-visible:ring-offset-2 disabled:opacity-50 motion-safe:transition-colors dark:focus-visible:ring-offset-slate-900"
					disabled={saving || selectedLevel === feature.default_member_level}
					onclick={requestLevelChange}
				>
					{$t('flow.settings.level.apply')}
				</button>
			</div>
		</Card>

		<Card>
			<div class="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
				<div>
					<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">
						{$t('flow.settings.package.heading')}
					</h2>
					<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">
						{$t('flow.settings.package.description')}
					</p>
				</div>
				<a
					href={resolve('/(app)/workspace/[workspaceId]/settings/flow/package', { workspaceId })}
					class="inline-flex min-h-11 shrink-0 items-center justify-center rounded-md border border-blue-300 px-4 text-sm font-medium text-blue-700 hover:bg-blue-50 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 motion-safe:transition-colors dark:border-blue-700 dark:text-blue-300 dark:hover:bg-blue-950/30"
				>
					{$t('flow.settings.package.open')}
				</a>
			</div>
		</Card>
	{/if}
</div>

<Modal
	bind:open={disableDialogOpen}
	title={$t('flow.settings.disableConfirm.title')}
	onclose={cancelDialogs}
>
	<p class="text-sm text-slate-700 dark:text-slate-300">
		{$t('flow.settings.disableConfirm.intro')}
	</p>
	<ul class="mt-3 list-disc space-y-1 pl-5 text-sm text-slate-700 dark:text-slate-300">
		<li>{$t('flow.settings.disableConfirm.consequenceNav')}</li>
		<li>{$t('flow.settings.disableConfirm.consequenceSessions')}</li>
		<li>{$t('flow.settings.disableConfirm.consequenceDirectUrl')}</li>
	</ul>
	<label
		class="mt-4 flex min-h-11 cursor-pointer items-center gap-3 text-sm text-slate-900 dark:text-slate-100"
	>
		<input
			type="checkbox"
			class="h-5 w-5 focus:outline-none focus-visible:ring-2 focus-visible:ring-red-500"
			bind:checked={acknowledged}
		/>
		<span>{$t('flow.settings.disableConfirm.acknowledge')}</span>
	</label>
	{#snippet footer()}
		<button
			type="button"
			class="min-h-11 rounded-md px-4 text-sm font-medium text-slate-700 hover:bg-slate-100 focus:outline-none focus-visible:ring-2 focus-visible:ring-slate-500 dark:text-slate-200 dark:hover:bg-slate-700"
			onclick={cancelDialogs}
		>
			{$t('common.cancel')}
		</button>
		<button
			type="button"
			class="min-h-11 rounded-md bg-red-600 px-4 text-sm font-medium text-white hover:bg-red-700 focus:outline-none focus-visible:ring-2 focus-visible:ring-red-500 focus-visible:ring-offset-2 disabled:opacity-50 dark:focus-visible:ring-offset-slate-800"
			disabled={!acknowledged}
			onclick={confirmPending}
		>
			{$t('flow.settings.disableConfirm.confirm')}
		</button>
	{/snippet}
</Modal>

<Modal
	bind:open={levelDialogOpen}
	title={$t('flow.settings.levelConfirm.title')}
	onclose={cancelDialogs}
>
	<p class="text-sm text-slate-700 dark:text-slate-300">
		{$t('flow.settings.levelConfirm.body', {
			values: {
				from: feature ? levelLabel(feature.default_member_level) : '',
				to: pendingLevel ? levelLabel(pendingLevel) : ''
			}
		})}
	</p>
	<p class="mt-3 text-sm text-slate-700 dark:text-slate-300">
		{$t('flow.settings.levelConfirm.epoch')}
	</p>
	<label
		class="mt-4 flex min-h-11 cursor-pointer items-center gap-3 text-sm text-slate-900 dark:text-slate-100"
	>
		<input
			type="checkbox"
			class="h-5 w-5 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500"
			bind:checked={acknowledged}
		/>
		<span>{$t('flow.settings.levelConfirm.acknowledge')}</span>
	</label>
	{#snippet footer()}
		<button
			type="button"
			class="min-h-11 rounded-md px-4 text-sm font-medium text-slate-700 hover:bg-slate-100 focus:outline-none focus-visible:ring-2 focus-visible:ring-slate-500 dark:text-slate-200 dark:hover:bg-slate-700"
			onclick={cancelDialogs}
		>
			{$t('common.cancel')}
		</button>
		<button
			type="button"
			class="min-h-11 rounded-md bg-blue-600 px-4 text-sm font-medium text-white hover:bg-blue-700 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 focus-visible:ring-offset-2 disabled:opacity-50 dark:focus-visible:ring-offset-slate-800"
			disabled={!acknowledged}
			onclick={confirmPending}
		>
			{$t('flow.settings.levelConfirm.confirm')}
		</button>
	{/snippet}
</Modal>
