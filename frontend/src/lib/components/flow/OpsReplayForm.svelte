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
	} from '#lib/flow/operations-service.js';
	import type { FlowReplayMode } from '#lib/api/flow.js';

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
		'mt-1 block min-h-11 w-full rounded-control border border-input bg-field px-3 text-sm disabled:bg-surface disabled:text-control';
	const locked = $derived(snap.busy);
</script>

{#snippet outcomeView(view: ReplayOutcomeView, testid: string)}
	<dl
		class="mt-2 grid grid-cols-1 gap-x-4 gap-y-1 text-sm sm:grid-cols-[max-content_1fr]"
		data-testid={testid}
		data-mode={view.mode}
	>
		{#if view.mode === 'rebuild'}
			<dt class="text-fg-muted">{$t('flow.operations.replay.replayed')}</dt>
			<dd class="font-mono" data-testid={`${testid}-count`}>{view.replayed}</dd>
			<dt class="text-fg-muted">
				{$t('flow.operations.replay.skippedAlreadyDelivered')}
			</dt>
			<dd class="font-mono">{view.skippedAlreadyDelivered}</dd>
		{:else}
			<dt class="text-fg-muted">{$t('flow.operations.replay.requeued')}</dt>
			<dd class="font-mono" data-testid={`${testid}-count`}>{view.requeued}</dd>
			<dt class="text-fg-muted">
				{$t('flow.operations.replay.skippedNotFailed')}
			</dt>
			<dd class="font-mono">{view.skippedNotFailed}</dd>
		{/if}
		<dt class="text-fg-muted">{$t('flow.operations.replay.deliveryIds')}</dt>
		<dd class="break-all font-mono text-xs">
			{view.deliveryIds.length === 0 ? $t('flow.operations.none') : view.deliveryIds.join(', ')}
		</dd>
		<dt class="text-fg-muted">{$t('flow.operations.replay.window')}</dt>
		<dd>{formatWindow(view)}</dd>
	</dl>
{/snippet}

<section
	class="rounded-card border border-line bg-raised p-4 sm:p-6"
	aria-labelledby="flow-ops-replay-heading"
	data-testid="flow-ops-replay"
>
	<h2 id="flow-ops-replay-heading" class="text-lg font-semibold text-fg">
		{$t('flow.operations.replay.heading')}
	</h2>
	<p class="mt-1 text-sm text-fg-muted">
		{$t('flow.operations.replay.description', { values: { days: REPLAY_MAX_WINDOW_DAYS } })}
	</p>
	<p class="sr-only" role="status" aria-live="polite">{announcement}</p>

	<div class="mt-4 grid grid-cols-1 gap-4 md:grid-cols-2">
		<div>
			<label for="flow-ops-replay-mode" class="block text-sm font-medium text-fg">
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
			<label for="flow-ops-replay-event" class="block text-sm font-medium text-fg">
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
			<label for="flow-ops-replay-kind" class="block text-sm font-medium text-fg">
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
			<label for="flow-ops-replay-subscriber" class="block text-sm font-medium text-fg">
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
			<label for="flow-ops-replay-from" class="block text-sm font-medium text-fg">
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
			<label for="flow-ops-replay-to" class="block text-sm font-medium text-fg">
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
		<p class="mt-3 text-sm text-danger" role="alert" data-testid="flow-ops-replay-invalid">
			{$t(snap.validationKey, { values: { days: REPLAY_MAX_WINDOW_DAYS } })}
		</p>
	{/if}

	{#if snap.executed === null}
		<button
			type="button"
			class="mt-4 min-h-11 rounded-control bg-primary rf-filled px-4 text-sm font-medium text-on-primary disabled:bg-surface disabled:text-control"
			disabled={snap.busy || snap.closed}
			onclick={dryRun}
			data-testid="flow-ops-replay-dry-run"
		>
			{snap.busy ? $t('flow.operations.drawer.running') : $t('flow.operations.replay.dryRun')}
		</button>
	{/if}

	{#if snap.failure}
		<div
			class="mt-3 flex flex-col gap-3 rounded-card border border-line bg-danger-soft p-3 text-sm text-danger sm:flex-row sm:items-center sm:justify-between"
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
					class="min-h-11 rounded-control border border-danger px-4 text-sm font-medium"
					onclick={snap.retry === 'execute' ? execute : dryRun}
				>
					{$t('flow.operations.retry')}
				</button>
			{/if}
		</div>
	{/if}

	{#if snap.preview && snap.executed === null}
		<div class="mt-4 rounded-card border border-line bg-surface p-3">
			<h3 class="text-sm font-semibold text-fg">
				{$t('flow.operations.replay.previewHeading')}
			</h3>
			<p class="mt-1 text-xs font-medium text-warning">
				{$t('flow.operations.replay.previewNote')}
			</p>
			{@render outcomeView(snap.preview, 'flow-ops-replay-preview')}
			<label class="mt-3 flex min-h-11 cursor-pointer items-center gap-3 text-sm text-fg">
				<input
					type="checkbox"
					class="h-5 w-5"
					checked={snap.acknowledged}
					disabled={snap.busy}
					onchange={acknowledge}
				/>
				<span>{$t('flow.operations.replay.acknowledge')}</span>
			</label>
			<button
				type="button"
				class="mt-2 min-h-11 rounded-control bg-danger-soft px-4 text-sm font-medium text-danger disabled:bg-surface disabled:text-control"
				disabled={!snap.canExecute}
				onclick={execute}
				data-testid="flow-ops-replay-execute"
			>
				{$t('flow.operations.replay.execute')}
			</button>
		</div>
	{/if}

	{#if snap.executed}
		<div class="mt-4 rounded-card border border-line bg-success-soft p-3">
			<h3 class="text-sm font-semibold text-fg">
				{$t('flow.operations.replay.executedHeading')}
			</h3>
			<p class="mt-1 text-xs text-success">
				{$t('flow.operations.replay.executedNote')}
			</p>
			{@render outcomeView(snap.executed, 'flow-ops-replay-result')}
		</div>
	{/if}
</section>
