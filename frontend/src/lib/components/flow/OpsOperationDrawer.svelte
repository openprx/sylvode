<script lang="ts">
	// Shared drawer for verify / compact / rebuild-projection. Every rule (dry-run first, execute
	// gate, keys, error classes) lives in `MaintenanceOperation`; this component only mirrors its
	// snapshot and forwards user input. The confirmation field always starts empty: the
	// confirmation value is produced by the user typing the target id, never prefilled.
	import { t } from 'svelte-i18n';
	import Modal from '$lib/components/Modal.svelte';
	import type { FlowOperationReceipt } from '$lib/api/flow';
	import {
		MAINTENANCE_TITLE_KEYS,
		OPERATION_NAME_KEYS,
		RECEIPT_MODE_KEYS,
		RECEIPT_SCOPE_KEYS,
		receiptChanges,
		receiptMode,
		receiptRows,
		receiptWarningKeys,
		type MaintenanceOperation,
		type MaintenanceSnapshot
	} from '$lib/flow/operations-service';

	interface Props {
		open?: boolean;
		operation: MaintenanceOperation | null;
		onclose?: () => void;
		onexecuted?: () => void;
	}

	let { open = $bindable(false), operation, onclose, onexecuted }: Props = $props();

	let snap = $state.raw<MaintenanceSnapshot | null>(null);
	let announcement = $state('');

	$effect(() => {
		snap = operation ? operation.snapshot() : null;
		announcement = '';
	});

	function sync() {
		if (operation) snap = operation.snapshot();
	}

	function setHead(event: Event) {
		operation?.setExpectedHead((event.currentTarget as HTMLInputElement).value);
		sync();
	}

	function setConfirm(event: Event) {
		operation?.setConfirmText((event.currentTarget as HTMLInputElement).value);
		sync();
	}

	function setDeep(event: Event) {
		operation?.setDeep((event.currentTarget as HTMLInputElement).checked);
		sync();
	}

	async function dryRun() {
		if (!operation) return;
		const pending = operation.dryRun();
		sync();
		const outcome = await pending;
		sync();
		announcement =
			outcome.status === 'ok'
				? $t('flow.operations.drawer.previewNote')
				: outcome.status === 'failed'
					? $t(outcome.failure.messageKey)
					: '';
	}

	async function execute() {
		if (!operation) return;
		const pending = operation.execute();
		sync();
		const outcome = await pending;
		sync();
		if (outcome.status === 'ok') {
			announcement = $t('flow.operations.drawer.executedNote');
			onexecuted?.();
		} else if (outcome.status === 'failed') {
			announcement = $t(outcome.failure.messageKey);
		}
	}

	function close() {
		open = false;
		onclose?.();
	}

	const headLabelHint = $derived(
		snap?.kind === 'verify'
			? $t('flow.operations.drawer.expectedHeadOptional')
			: $t('flow.operations.drawer.expectedHeadRequired')
	);
	const headInvalid = $derived(
		snap !== null && snap.expectedHeadText.trim() !== '' && snap.expectedHead === null
	);

	function formatTime(value: string): string {
		const date = new Date(value);
		return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
	}
</script>

{#snippet receiptView(receipt: FlowOperationReceipt, testid: string)}
	<dl
		class="mt-2 grid grid-cols-1 gap-x-4 gap-y-1 text-sm sm:grid-cols-[max-content_1fr]"
		data-testid={testid}
	>
		<dt class="text-slate-500 dark:text-slate-400">{$t('flow.operations.receipt.operation')}</dt>
		<dd>
			{OPERATION_NAME_KEYS[receipt.operation]
				? $t(OPERATION_NAME_KEYS[receipt.operation])
				: receipt.operation}
		</dd>
		<dt class="text-slate-500 dark:text-slate-400">{$t('flow.operations.receipt.mode')}</dt>
		<dd data-testid={`${testid}-mode`} data-mode={receiptMode(receipt)}>
			{$t(RECEIPT_MODE_KEYS[receiptMode(receipt)])}
		</dd>
		{#if receipt.scope}
			<dt class="text-slate-500 dark:text-slate-400">
				{$t('flow.operations.receipt.scope.label')}
			</dt>
			<dd data-testid={`${testid}-scope`} data-kind={receipt.scope.kind}>
				{RECEIPT_SCOPE_KEYS[receipt.scope.kind]
					? $t(RECEIPT_SCOPE_KEYS[receipt.scope.kind])
					: receipt.scope.kind}
				<span class="block break-all font-mono text-xs"
					>{receipt.scope.kind === 'object'
						? receipt.scope.object_id
						: receipt.scope.document_id}</span
				>
			</dd>
		{/if}
		<dt class="text-slate-500 dark:text-slate-400">{$t('flow.operations.receipt.status')}</dt>
		<dd class="font-mono">{receipt.status}</dd>
		<dt class="text-slate-500 dark:text-slate-400">{$t('flow.operations.receipt.expectedHead')}</dt>
		<dd class="font-mono">{receipt.expected_head_seq}</dd>
		{#if typeof receipt.observed_head_seq === 'number'}
			<dt class="text-slate-500 dark:text-slate-400">
				{$t('flow.operations.receipt.observedHead')}
			</dt>
			<dd class="font-mono">{receipt.observed_head_seq}</dd>
		{/if}
		<dt class="text-slate-500 dark:text-slate-400">{$t('flow.operations.receipt.operationId')}</dt>
		<dd class="break-all font-mono text-xs" data-testid={`${testid}-operation-id`}>
			{receipt.operation_id}
		</dd>
		{#if receipt.started_at}
			<dt class="text-slate-500 dark:text-slate-400">{$t('flow.operations.receipt.startedAt')}</dt>
			<dd class="text-xs">{formatTime(receipt.started_at)}</dd>
			<dt class="text-slate-500 dark:text-slate-400">{$t('flow.operations.receipt.finishedAt')}</dt>
			<dd class="text-xs">
				{receipt.finished_at
					? formatTime(receipt.finished_at)
					: $t('flow.operations.receipt.notFinished')}
			</dd>
		{/if}
		{#if receipt.audit_event_id}
			<dt class="text-slate-500 dark:text-slate-400">
				{$t('flow.operations.receipt.auditEventId')}
			</dt>
			<dd class="break-all font-mono text-xs" data-testid={`${testid}-audit-event`}>
				{receipt.audit_event_id}
			</dd>
		{/if}
		{#each receiptRows(receipt) as row (row.field)}
			<dt
				class={row.labelKey
					? 'text-slate-500 dark:text-slate-400'
					: 'font-mono text-xs text-slate-500 dark:text-slate-400'}
			>
				{row.labelKey ? $t(row.labelKey) : row.field}
			</dt>
			<dd class="break-all font-mono text-xs" data-field={row.field}>{row.value}</dd>
		{/each}
	</dl>
	{@const changes = receiptChanges(receipt)}
	{@const warningKeys = receiptWarningKeys(receipt)}
	<section class="mt-3" data-testid={`${testid}-changes`}>
		<h4 class="text-sm font-medium text-slate-900 dark:text-slate-100">
			{$t('flow.operations.receipt.changesHeading')}
		</h4>
		{#if changes.length === 0}
			<p class="mt-1 text-sm text-slate-500 dark:text-slate-400">
				{$t('flow.operations.receipt.changesNone')}
			</p>
		{/if}
		{#each changes as change, index (index)}
			<div
				class="mt-2 rounded-md border border-slate-200 p-2 dark:border-slate-700"
				data-testid={`${testid}-change`}
				data-kind={change.kind}
				data-changed={String(change.changed)}
			>
				<p class="break-all text-sm">{$t(change.labelKey, { values: { id: change.subjectId } })}</p>
				<p class="text-xs text-slate-500 dark:text-slate-400">
					{change.changed
						? $t('flow.operations.receipt.change.changed')
						: $t('flow.operations.receipt.change.unchanged')}
				</p>
				<table class="mt-1 w-full table-fixed text-left text-xs">
					<thead class="text-slate-500 dark:text-slate-400">
						<tr>
							<th scope="col" class="py-1 pr-2">{$t('flow.operations.receipt.change.field')}</th>
							<th scope="col" class="py-1 pr-2">{$t('flow.operations.receipt.change.before')}</th>
							<th scope="col" class="py-1">{$t('flow.operations.receipt.change.after')}</th>
						</tr>
					</thead>
					<tbody>
						{#each change.rows as row (row.field)}
							<tr data-field={row.field}>
								<th scope="row" class="py-1 pr-2 font-normal">
									{row.labelKey ? $t(row.labelKey) : row.field}
								</th>
								<td class="break-all py-1 pr-2 font-mono" data-testid="flow-ops-change-before"
									>{row.before}</td
								>
								<td class="break-all py-1 font-mono" data-testid="flow-ops-change-after"
									>{row.after}</td
								>
							</tr>
						{/each}
					</tbody>
				</table>
				{#if change.deletedUpdates !== null}
					<p class="mt-1 text-xs">
						{$t('flow.operations.receipt.change.deletedUpdates', {
							values: { n: change.deletedUpdates }
						})}
					</p>
				{/if}
			</div>
		{/each}
	</section>
	{#if warningKeys.length > 0}
		<section class="mt-3" data-testid={`${testid}-warnings`}>
			<h4 class="text-sm font-medium text-amber-900 dark:text-amber-200">
				{$t('flow.operations.receipt.warningsHeading')}
			</h4>
			<ul class="mt-1 list-disc space-y-1 pl-5 text-sm text-amber-900 dark:text-amber-200">
				{#each warningKeys as key, index (index)}
					<li>{$t(key)}</li>
				{/each}
			</ul>
		</section>
	{/if}
{/snippet}

<Modal
	bind:open
	title={snap ? $t(MAINTENANCE_TITLE_KEYS[snap.kind]) : ''}
	maxWidthClass="max-w-2xl"
	onclose={close}
>
	{#if snap}
		<div class="space-y-4" data-testid="flow-ops-drawer" data-kind={snap.kind}>
			<p class="sr-only" role="status" aria-live="polite">{announcement}</p>
			<div class="text-sm">
				<p class="text-slate-500 dark:text-slate-400">
					{snap.kind === 'rebuild'
						? $t('flow.operations.drawer.targetObject')
						: $t('flow.operations.drawer.targetDocument')}
				</p>
				<p
					class="break-all font-mono text-slate-900 dark:text-slate-100"
					data-testid="flow-ops-drawer-target"
				>
					{snap.targetId}
				</p>
			</div>

			{#if snap.kind === 'verify'}
				<label
					class="flex min-h-11 cursor-pointer items-center gap-3 text-sm text-slate-900 dark:text-slate-100"
				>
					<input
						type="checkbox"
						class="h-5 w-5 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500"
						checked={snap.deep}
						disabled={snap.busy || snap.closed}
						onchange={setDeep}
					/>
					<span>{$t('flow.operations.drawer.deep')}</span>
				</label>
			{/if}

			<div>
				<label
					for="flow-ops-expected-head"
					class="block text-sm font-medium text-slate-700 dark:text-slate-300"
				>
					{$t('flow.operations.drawer.expectedHead')}
				</label>
				<input
					id="flow-ops-expected-head"
					inputmode="numeric"
					class="mt-1 block min-h-11 w-full rounded-md border border-slate-300 px-3 font-mono text-sm focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:border-slate-600 dark:bg-slate-800"
					value={snap.expectedHeadText}
					disabled={snap.busy || snap.closed || snap.executed !== null}
					aria-describedby="flow-ops-expected-head-hint"
					aria-invalid={headInvalid}
					oninput={setHead}
				/>
				<p id="flow-ops-expected-head-hint" class="mt-1 text-xs text-slate-500 dark:text-slate-400">
					{headLabelHint}
				</p>
				{#if headInvalid}
					<p class="mt-1 text-xs text-red-700 dark:text-red-300">
						{$t('flow.operations.drawer.invalidHead')}
					</p>
				{/if}
			</div>

			{#if snap.executed === null}
				<button
					type="button"
					class="min-h-11 rounded-md bg-blue-600 px-4 text-sm font-medium text-white hover:bg-blue-700 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 focus-visible:ring-offset-2 disabled:opacity-50 dark:focus-visible:ring-offset-slate-800"
					disabled={!snap.canDryRun}
					onclick={dryRun}
					data-testid="flow-ops-drawer-dry-run"
				>
					{snap.busy
						? $t('flow.operations.drawer.running')
						: snap.kind === 'verify'
							? $t('flow.operations.drawer.runVerify')
							: $t('flow.operations.drawer.dryRun')}
				</button>
			{/if}

			{#if snap.failure}
				<div
					class="flex flex-col gap-3 rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-700 sm:flex-row sm:items-center sm:justify-between dark:border-red-900 dark:bg-red-950/30 dark:text-red-300"
					role="alert"
					data-testid="flow-ops-drawer-error"
				>
					<span>
						{$t(snap.failure.messageKey)}
						{#if snap.closed}
							{$t('flow.operations.drawer.permanent')}
						{/if}
					</span>
					{#if snap.retry === 'dry_run'}
						<button
							type="button"
							class="min-h-11 rounded-md border border-red-300 px-4 text-sm font-medium focus:outline-none focus-visible:ring-2 focus-visible:ring-red-500 dark:border-red-700"
							onclick={dryRun}
						>
							{$t('flow.operations.retry')}
						</button>
					{:else if snap.retry === 'execute'}
						<button
							type="button"
							class="min-h-11 rounded-md border border-red-300 px-4 text-sm font-medium focus:outline-none focus-visible:ring-2 focus-visible:ring-red-500 dark:border-red-700"
							onclick={execute}
						>
							{$t('flow.operations.retry')}
						</button>
					{/if}
				</div>
			{/if}

			{#if snap.preview && snap.executed === null}
				<div
					class="rounded-md border border-slate-200 bg-slate-50 p-3 dark:border-slate-700 dark:bg-slate-800/50"
				>
					<h3 class="text-sm font-semibold text-slate-900 dark:text-slate-100">
						{$t('flow.operations.drawer.previewHeading')}
					</h3>
					<p class="mt-1 text-xs font-medium text-amber-800 dark:text-amber-300">
						{$t('flow.operations.drawer.previewNote')}
					</p>
					{@render receiptView(snap.preview, 'flow-ops-preview')}
				</div>

				{#if snap.kind !== 'verify'}
					<div>
						<label
							for="flow-ops-confirm"
							class="block text-sm font-medium text-slate-700 dark:text-slate-300"
						>
							{snap.kind === 'rebuild'
								? $t('flow.operations.drawer.confirmLabelObject')
								: $t('flow.operations.drawer.confirmLabelDocument')}
						</label>
						<input
							id="flow-ops-confirm"
							class="mt-1 block min-h-11 w-full rounded-md border border-slate-300 px-3 font-mono text-sm focus:outline-none focus-visible:ring-2 focus-visible:ring-red-500 dark:border-slate-600 dark:bg-slate-800"
							autocomplete="off"
							spellcheck="false"
							value={snap.confirmText}
							disabled={snap.busy || snap.closed}
							oninput={setConfirm}
						/>
						{#if snap.confirmText !== '' && !snap.confirmMatches}
							<p class="mt-1 text-xs text-red-700 dark:text-red-300">
								{$t('flow.operations.drawer.confirmMismatch')}
							</p>
						{/if}
					</div>
					<button
						type="button"
						class="min-h-11 rounded-md bg-red-600 px-4 text-sm font-medium text-white hover:bg-red-700 focus:outline-none focus-visible:ring-2 focus-visible:ring-red-500 focus-visible:ring-offset-2 disabled:opacity-50 dark:focus-visible:ring-offset-slate-800"
						disabled={!snap.canExecute}
						onclick={execute}
						data-testid="flow-ops-drawer-execute"
					>
						{snap.busy
							? $t('flow.operations.drawer.running')
							: snap.kind === 'rebuild'
								? $t('flow.operations.drawer.execute.rebuild')
								: $t('flow.operations.drawer.execute.compact')}
					</button>
				{/if}
			{/if}

			{#if snap.executed}
				<div
					class="rounded-md border border-emerald-300 bg-emerald-50 p-3 dark:border-emerald-800 dark:bg-emerald-950/30"
				>
					<h3 class="text-sm font-semibold text-emerald-900 dark:text-emerald-200">
						{$t('flow.operations.drawer.executedHeading')}
					</h3>
					<p class="mt-1 text-xs text-emerald-800 dark:text-emerald-300">
						{$t('flow.operations.drawer.executedNote')}
					</p>
					{@render receiptView(snap.executed, 'flow-ops-executed')}
				</div>
			{/if}
		</div>
	{/if}
</Modal>
