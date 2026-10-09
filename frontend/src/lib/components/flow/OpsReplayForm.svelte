<script lang="ts">
	// Delivery replay (`POST /admin/workspaces/{id}/flow/deliveries/replay`). `DeliveryReplay`
	// owns validation, the dry-run -> acknowledgement -> execute gate and the keys; this component
	// renders its snapshot. The acknowledgement box starts unticked and is cleared by any form
	// change. Results are rendered per mode from their own fields only.
	import { t } from 'svelte-i18n';
	import {
		REPLAY_MAX_WINDOW_DAYS,
		REPLAY_MODES,
		REPLAY_MODE_KEYS,
		type DeliveryReplay,
		type ReplayForm,
		type ReplayOutcomeView,
		type ReplaySnapshot
	} from '$lib/flow/operations-service';
	import type { FlowReplayMode } from '$lib/api/flow';

	interface Props {
		replay: DeliveryReplay;
	}

	let { replay }: Props = $props();

	// Bumped after every state-machine call; the snapshot is re-read from `replay` on each bump.
	let version = $state(0);
	const snap = $derived.by<ReplaySnapshot>(() => {
		void version;
		return replay.snapshot();
	});
	let announcement = $state('');

	function sync() {
		version += 1;
	}

	function update(patch: Partial<ReplayForm>) {
		replay.setForm({ ...snap.form, ...patch });
		sync();
	}

	function inputValue(event: Event): string {
		return (event.currentTarget as HTMLInputElement | HTMLSelectElement).value;
	}

	function setMode(event: Event) {
		const value = inputValue(event);
		const mode = REPLAY_MODES.find((candidate) => candidate === value);
		if (mode) update({ mode: mode as FlowReplayMode });
	}

	function setKind(event: Event) {
		update({ subscriberKind: inputValue(event) === 'webhook' ? 'webhook' : '' });
	}

	function acknowledge(event: Event) {
		replay.acknowledge((event.currentTarget as HTMLInputElement).checked);
		sync();
	}

	async function dryRun() {
		const pending = replay.dryRun();
		sync();
		const outcome = await pending;
		sync();
		announcement =
			outcome.status === 'ok'
				? $t('flow.operations.replay.previewNote')
				: outcome.status === 'invalid'
					? $t(outcome.errorKey, { values: { days: REPLAY_MAX_WINDOW_DAYS } })
					: outcome.status === 'failed'
						? $t(outcome.failure.messageKey)
						: '';
	}

	async function execute() {
		const pending = replay.execute();
		sync();
		const outcome = await pending;
		sync();
		announcement =
			outcome.status === 'ok'
				? $t('flow.operations.replay.executedNote')
				: outcome.status === 'failed'
					? $t(outcome.failure.messageKey)
					: '';
	}

	function formatWindow(view: ReplayOutcomeView): string {
		return $t('flow.operations.replay.windowValue', {
			values: {
				from: new Date(view.window.from).toLocaleString(),
				to: new Date(view.window.to).toLocaleString()
			}
		});
	}

	const fieldClass =
		'mt-1 block min-h-11 w-full rounded-md border border-slate-300 bg-white px-3 text-sm focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 disabled:opacity-60 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100';
	const locked = $derived(snap.busy);
</script>

{#snippet outcomeView(view: ReplayOutcomeView, testid: string)}
	<dl
		class="mt-2 grid grid-cols-1 gap-x-4 gap-y-1 text-sm sm:grid-cols-[max-content_1fr]"
		data-testid={testid}
		data-mode={view.mode}
	>
		{#if view.mode === 'rebuild'}
			<dt class="text-slate-500 dark:text-slate-400">{$t('flow.operations.replay.replayed')}</dt>
			<dd class="font-mono" data-testid={`${testid}-count`}>{view.replayed}</dd>
			<dt class="text-slate-500 dark:text-slate-400">
				{$t('flow.operations.replay.skippedAlreadyDelivered')}
			</dt>
			<dd class="font-mono">{view.skippedAlreadyDelivered}</dd>
		{:else}
			<dt class="text-slate-500 dark:text-slate-400">{$t('flow.operations.replay.requeued')}</dt>
			<dd class="font-mono" data-testid={`${testid}-count`}>{view.requeued}</dd>
			<dt class="text-slate-500 dark:text-slate-400">
				{$t('flow.operations.replay.skippedNotFailed')}
			</dt>
			<dd class="font-mono">{view.skippedNotFailed}</dd>
		{/if}
		<dt class="text-slate-500 dark:text-slate-400">{$t('flow.operations.replay.deliveryIds')}</dt>
		<dd class="break-all font-mono text-xs">
			{view.deliveryIds.length === 0 ? $t('flow.operations.none') : view.deliveryIds.join(', ')}
		</dd>
		<dt class="text-slate-500 dark:text-slate-400">{$t('flow.operations.replay.window')}</dt>
		<dd>{formatWindow(view)}</dd>
	</dl>
{/snippet}

<section
	class="rounded-lg border border-slate-200 bg-white p-4 shadow-sm sm:p-6 dark:border-slate-700 dark:bg-slate-900"
	aria-labelledby="flow-ops-replay-heading"
	data-testid="flow-ops-replay"
>
	<h2 id="flow-ops-replay-heading" class="text-lg font-semibold text-slate-900 dark:text-slate-100">
		{$t('flow.operations.replay.heading')}
	</h2>
	<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">
		{$t('flow.operations.replay.description', { values: { days: REPLAY_MAX_WINDOW_DAYS } })}
	</p>
	<p class="sr-only" role="status" aria-live="polite">{announcement}</p>

	<div class="mt-4 grid grid-cols-1 gap-4 md:grid-cols-2">
		<div>
			<label
				for="flow-ops-replay-mode"
				class="block text-sm font-medium text-slate-700 dark:text-slate-300"
			>
				{$t('flow.operations.replay.modeLabel')}
			</label>
			<select
				id="flow-ops-replay-mode"
				class={fieldClass}
				value={snap.form.mode}
				disabled={locked}
				onchange={setMode}
			>
				{#each REPLAY_MODES as mode (mode)}
					<option value={mode}>{$t(REPLAY_MODE_KEYS[mode])}</option>
				{/each}
			</select>
		</div>
		<div>
			<label
				for="flow-ops-replay-event"
				class="block text-sm font-medium text-slate-700 dark:text-slate-300"
			>
				{$t('flow.operations.replay.eventType')}
			</label>
			<input
				id="flow-ops-replay-event"
				class={fieldClass}
				autocomplete="off"
				value={snap.form.eventType}
				disabled={locked}
				oninput={(event) => update({ eventType: inputValue(event) })}
			/>
		</div>
		<div>
			<label
				for="flow-ops-replay-kind"
				class="block text-sm font-medium text-slate-700 dark:text-slate-300"
			>
				{$t('flow.operations.replay.subscriberKind')}
			</label>
			<select
				id="flow-ops-replay-kind"
				class={fieldClass}
				value={snap.form.subscriberKind}
				disabled={locked}
				onchange={setKind}
			>
				<option value="">{$t('flow.operations.replay.subscriberAny')}</option>
				<option value="webhook">{$t('flow.operations.replay.subscriberWebhook')}</option>
			</select>
		</div>
		<div>
			<label
				for="flow-ops-replay-subscriber"
				class="block text-sm font-medium text-slate-700 dark:text-slate-300"
			>
				{$t('flow.operations.replay.subscriberId')}
			</label>
			<input
				id="flow-ops-replay-subscriber"
				class={`${fieldClass} font-mono`}
				autocomplete="off"
				spellcheck="false"
				value={snap.form.subscriberId}
				disabled={locked}
				oninput={(event) => update({ subscriberId: inputValue(event) })}
			/>
		</div>
		<div>
			<label
				for="flow-ops-replay-from"
				class="block text-sm font-medium text-slate-700 dark:text-slate-300"
			>
				{$t('flow.operations.replay.from')}
			</label>
			<input
				id="flow-ops-replay-from"
				type="datetime-local"
				class={fieldClass}
				value={snap.form.from}
				disabled={locked}
				oninput={(event) => update({ from: inputValue(event) })}
			/>
		</div>
		<div>
			<label
				for="flow-ops-replay-to"
				class="block text-sm font-medium text-slate-700 dark:text-slate-300"
			>
				{$t('flow.operations.replay.to')}
			</label>
			<input
				id="flow-ops-replay-to"
				type="datetime-local"
				class={fieldClass}
				value={snap.form.to}
				disabled={locked}
				oninput={(event) => update({ to: inputValue(event) })}
			/>
		</div>
	</div>

	{#if snap.validationKey}
		<p
			class="mt-3 text-sm text-red-700 dark:text-red-300"
			role="alert"
			data-testid="flow-ops-replay-invalid"
		>
			{$t(snap.validationKey, { values: { days: REPLAY_MAX_WINDOW_DAYS } })}
		</p>
	{/if}

	{#if snap.executed === null}
		<button
			type="button"
			class="mt-4 min-h-11 rounded-md bg-blue-600 px-4 text-sm font-medium text-white hover:bg-blue-700 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 focus-visible:ring-offset-2 disabled:opacity-50 dark:focus-visible:ring-offset-slate-900"
			disabled={snap.busy || snap.closed}
			onclick={dryRun}
			data-testid="flow-ops-replay-dry-run"
		>
			{snap.busy ? $t('flow.operations.drawer.running') : $t('flow.operations.replay.dryRun')}
		</button>
	{/if}

	{#if snap.failure}
		<div
			class="mt-3 flex flex-col gap-3 rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-700 sm:flex-row sm:items-center sm:justify-between dark:border-red-900 dark:bg-red-950/30 dark:text-red-300"
			role="alert"
			data-testid="flow-ops-replay-error"
		>
			<span>
				{$t(snap.failure.messageKey)}
				{#if snap.closed}
					{$t('flow.operations.drawer.permanent')}
				{/if}
			</span>
			{#if snap.retry}
				<button
					type="button"
					class="min-h-11 rounded-md border border-red-300 px-4 text-sm font-medium focus:outline-none focus-visible:ring-2 focus-visible:ring-red-500 dark:border-red-700"
					onclick={snap.retry === 'execute' ? execute : dryRun}
				>
					{$t('flow.operations.retry')}
				</button>
			{/if}
		</div>
	{/if}

	{#if snap.preview && snap.executed === null}
		<div
			class="mt-4 rounded-md border border-slate-200 bg-slate-50 p-3 dark:border-slate-700 dark:bg-slate-800/50"
		>
			<h3 class="text-sm font-semibold text-slate-900 dark:text-slate-100">
				{$t('flow.operations.replay.previewHeading')}
			</h3>
			<p class="mt-1 text-xs font-medium text-amber-800 dark:text-amber-300">
				{$t('flow.operations.replay.previewNote')}
			</p>
			{@render outcomeView(snap.preview, 'flow-ops-replay-preview')}
			<label
				class="mt-3 flex min-h-11 cursor-pointer items-center gap-3 text-sm text-slate-900 dark:text-slate-100"
			>
				<input
					type="checkbox"
					class="h-5 w-5 focus:outline-none focus-visible:ring-2 focus-visible:ring-red-500"
					checked={snap.acknowledged}
					disabled={snap.busy}
					onchange={acknowledge}
				/>
				<span>{$t('flow.operations.replay.acknowledge')}</span>
			</label>
			<button
				type="button"
				class="mt-2 min-h-11 rounded-md bg-red-600 px-4 text-sm font-medium text-white hover:bg-red-700 focus:outline-none focus-visible:ring-2 focus-visible:ring-red-500 focus-visible:ring-offset-2 disabled:opacity-50 dark:focus-visible:ring-offset-slate-900"
				disabled={!snap.canExecute}
				onclick={execute}
				data-testid="flow-ops-replay-execute"
			>
				{$t('flow.operations.replay.execute')}
			</button>
		</div>
	{/if}

	{#if snap.executed}
		<div
			class="mt-4 rounded-md border border-emerald-300 bg-emerald-50 p-3 dark:border-emerald-800 dark:bg-emerald-950/30"
		>
			<h3 class="text-sm font-semibold text-emerald-900 dark:text-emerald-200">
				{$t('flow.operations.replay.executedHeading')}
			</h3>
			<p class="mt-1 text-xs text-emerald-800 dark:text-emerald-300">
				{$t('flow.operations.replay.executedNote')}
			</p>
			{@render outcomeView(snap.executed, 'flow-ops-replay-result')}
		</div>
	{/if}
</section>
