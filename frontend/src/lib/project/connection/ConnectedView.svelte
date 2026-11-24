<svelte:options runes />

<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import { t } from 'svelte-i18n';
	import { botConnectionApi, type CallResult, type UpdateInput } from '#lib/api/bot-connection.js';
	import { toast } from '#lib/stores/toast.js';
	import Button from '#lib/ui/button/Button.svelte';
	import Select from '#lib/ui/select/Select.svelte';
	import SettingRow from '#lib/ui/setting-row/SettingRow.svelte';
	import SecurityWarning from '#lib/ui/security-warning/SecurityWarning.svelte';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import TextField from '#lib/ui/text-field/TextField.svelte';
	import { tooltip } from '#lib/ui/tooltip/tooltip.js';
	import AdvancedFields from './AdvancedFields.svelte';
	import ConfirmDialogs, { type Pending } from './ConfirmDialogs.svelte';
	import StatusRows from './StatusRows.svelte';
	import { changedFields, draftOf, draftValid, validateDraft, type Draft } from './advanced';
	import { loadDomains, loadLabels, loadStates, type Choice } from './options';
	import { routeServerMessage } from './server-errors';
	import { runsWithoutPermissionChecks } from './status';
	import { attachCommand, channelErrorText, tmuxBlock as tmuxBlockOf } from './tmux';
	import {
		normalizeDirectory,
		normalizeRunnerUrl,
		validateRunnerUrl,
		validateWorkingDirectory
	} from './validate';
	import { CLIS, type Cli, type Connection, type Status, type WriteAnswer } from './wire';

	interface Props {
		workspaceId: string;
		projectName: string;
		connection: Connection;
		/** Every connection of the project: a CLI with a tmux connection cannot get a second. */
		connections: Connection[];
		status: Status | null;
		now: Date;
		refreshing: boolean;
		onrefresh: () => void;
		/** A write that returned the connection (and, for a rotation, a block). */
		onchanged: (answer: WriteAnswer) => void;
		onremoved: (botId: string) => void;
		/** A stale version: the connection as it now stands. The edited value stays. */
		onconflict: (current: Connection | null) => void;
	}
	let {
		workspaceId,
		projectName,
		connection,
		connections,
		status,
		now,
		refreshing,
		onrefresh,
		onchanged,
		onremoved,
		onconflict
	}: Props = $props();

	const projectId = $derived(connection.project_id);
	const botId = $derived(connection.bot.id);
	const isTmux = $derived(connection.execution.channel === 'tmux');
	const sessionName = $derived(connection.execution.session_name);
	const tmuxBlock = $derived(tmuxBlockOf(connections, connection.execution.cli, botId));
	const warn = $derived(runsWithoutPermissionChecks(status));
	let channelError = $state<string | undefined>(undefined);
	const shared = $derived(
		connection.runner.shared_with_projects.filter((project) => project.id !== projectId)
	);

	let states = $state<Choice[]>([]);
	let labels = $state<Choice[]>([]);
	let domains = $state<Choice[]>([]);
	onMount(async () => {
		[states, labels, domains] = await Promise.all([
			loadStates(connection.project_id),
			loadLabels(workspaceId),
			loadDomains(connection.project_id)
		]);
	});

	type Editing = 'url' | 'cli' | 'dir' | null;
	let editing = $state<Editing>(null);
	let editValue = $state('');
	let editError = $state<string | undefined>(undefined);
	let busy = $state(false);
	let pending = $state<Pending>(null);
	let advancedOpen = $state(false);
	let draft = $state<Draft>(untrack(() => draftOf(connection)));
	let advancedSubmitted = $state(false);
	let lastVersion = untrack(() => connection.version);
	$effect(() => {
		// A newer connection (after a save or a reload) replaces the draft with its values.
		if (connection.version !== lastVersion) {
			lastVersion = connection.version;
			draft = draftOf(connection);
			advancedSubmitted = false;
		}
	});

	const advancedErrors = $derived(validateDraft(draft));
	const advancedChange = $derived(changedFields(draft, connection));
	const advancedDirty = $derived(Object.keys(advancedChange).length > 0);

	function startEdit(field: Exclude<Editing, null>) {
		editing = field;
		editError = undefined;
		editValue =
			field === 'url'
				? connection.runner.url
				: field === 'cli'
					? connection.execution.cli
					: connection.execution.working_directory;
	}

	/** Runs a write and routes its outcome; returns whether it was applied. */
	async function run(call: () => Promise<CallResult<WriteAnswer>>): Promise<boolean> {
		busy = true;
		channelError = undefined;
		const result = await call();
		busy = false;
		if (result.ok) {
			onchanged(result.value);
			return true;
		}
		if (result.code === 409 && result.current !== null) {
			onconflict(result.current);
			return false;
		}
		const routed = routeServerMessage(result.message);
		if (routed.field === 'channel') {
			channelError = channelErrorText(result.message, connections, (key, values) =>
				$t(key, { values })
			);
			advancedOpen = true;
		} else if (routed.field === 'other') toast.error(result.message);
		else editError = routed.text;
		return false;
	}

	async function saveField() {
		const message =
			editing === 'url'
				? validateRunnerUrl(editValue)
				: editing === 'dir'
					? validateWorkingDirectory(editValue)
					: null;
		if (message !== null) {
			editError = $t(message);
			return;
		}
		const change: Omit<UpdateInput, 'expected_version'> =
			editing === 'url'
				? { runner_url: normalizeRunnerUrl(editValue) }
				: editing === 'dir'
					? { working_directory: normalizeDirectory(editValue) }
					: { cli: CLIS.find((cli) => cli === editValue) ?? connection.execution.cli };
		if (
			await run(() =>
				botConnectionApi.update(projectId, botId, {
					expected_version: connection.version,
					...change
				})
			)
		)
			editing = null;
	}

	async function saveAdvanced() {
		advancedSubmitted = true;
		if (!draftValid(advancedErrors) || !advancedDirty) return;
		await run(() =>
			botConnectionApi.update(projectId, botId, {
				expected_version: connection.version,
				...advancedChange
			})
		);
	}

	async function enable() {
		if (await run(() => botConnectionApi.enable(projectId, botId, connection.version)))
			toast.success($t('execution.connection.enabledToast'));
	}

	async function confirmed(action: 'disable' | 'rotate' | 'remove') {
		if (action === 'remove') {
			busy = true;
			const result = await botConnectionApi.remove(projectId, botId, connection.version);
			busy = false;
			pending = null;
			if (result.ok) {
				toast.success($t('execution.connection.removed'));
				onremoved(botId);
			} else if (result.code === 409 && result.current !== null) onconflict(result.current);
			else toast.error(result.message);
			return;
		}
		const ok = await run(() =>
			action === 'disable'
				? botConnectionApi.disable(projectId, botId, connection.version)
				: botConnectionApi.rotate(projectId, botId, connection.version)
		);
		pending = null;
		void ok;
	}
</script>

<div class="view" data-testid="connected-view" data-bot-id={botId}>
	<header class="head">
		<strong>{connection.bot.name}</strong>
		<StatusChip tone={connection.enabled ? 'success' : 'neutral'}>
			{connection.enabled
				? $t('execution.connection.state.enabled')
				: $t('execution.connection.state.disabled')}
		</StatusChip>
		<span class="channel">
			{$t(`execution.connection.advanced.channel.${connection.execution.channel}`)}
		</span>
		{#if shared.length > 0}
			<button
				type="button"
				class="shared"
				use:tooltip={{ text: shared.map((project) => project.name).join(', ') }}
			>
				{$t('execution.connection.state.sharedWith', { values: { count: shared.length } })}
			</button>
		{/if}
	</header>

	{#if warn}
		<SecurityWarning tone="warning" title={$t('execution.connection.warning.bypassTitle')}>
			<span data-testid="bypass-warning">{$t('execution.connection.warning.bypassBody')}</span>
		</SecurityWarning>
	{/if}

	{#if isTmux}
		<div class="session" data-testid="tmux-session">
			{#if sessionName}
				<p>
					{$t('execution.connection.tmux.session')}
					<code data-testid="tmux-session-name">{sessionName}</code>
				</p>
				<p>
					{$t('execution.connection.tmux.attach')}
					<code data-testid="tmux-attach">{attachCommand(sessionName)}</code>
				</p>
			{/if}
			<p class="hint">{$t(`execution.connection.tmux.behaviour.${connection.execution.cli}`)}</p>
		</div>
	{/if}

	<section class="settings">
		<SettingRow
			label={$t('execution.connection.runner.address')}
			value={editing === 'url' ? undefined : connection.runner.url}
		>
			{#snippet action()}
				{#if editing !== 'url'}
					<Button variant="secondary" onclick={() => startEdit('url')}>
						{$t('execution.connection.change')}
					</Button>
				{/if}
			{/snippet}
		</SettingRow>
		{#if editing === 'url'}
			<div class="edit">
				<TextField
					label={$t('execution.connection.runner.address')}
					bind:value={editValue}
					inputmode="url"
					helper={shared.length > 0
						? $t('execution.connection.runner.affects', { values: { count: shared.length + 1 } })
						: undefined}
					error={editError}
				/>
				{@render editActions()}
			</div>
		{/if}

		<SettingRow
			label={$t('execution.connection.cli.label')}
			value={editing === 'cli'
				? undefined
				: $t(`execution.connection.cli.${connection.execution.cli}`)}
		>
			{#snippet action()}
				{#if isTmux}
					<span class="hint" data-testid="cli-fixed">{$t('execution.connection.cli.tmuxOnly')}</span
					>
				{:else if editing !== 'cli'}
					<Button variant="secondary" onclick={() => startEdit('cli')}>
						{$t('execution.connection.change')}
					</Button>
				{/if}
			{/snippet}
		</SettingRow>
		{#if editing === 'cli'}
			<div class="edit">
				<Select
					label={$t('execution.connection.cli.label')}
					value={editValue}
					options={CLIS.map((value: Cli) => ({
						value,
						label: $t(`execution.connection.cli.${value}`)
					}))}
					onchange={(value) => (editValue = value ?? editValue)}
				/>
				{@render editActions()}
			</div>
		{/if}

		<SettingRow
			label={isTmux
				? $t('execution.connection.workingDirectoryTmux')
				: $t('execution.connection.workingDirectory')}
			value={editing === 'dir' ? undefined : connection.execution.working_directory}
		>
			{#snippet action()}
				{#if editing !== 'dir'}
					<Button variant="secondary" onclick={() => startEdit('dir')}>
						{$t('execution.connection.change')}
					</Button>
				{/if}
			{/snippet}
		</SettingRow>
		{#if editing === 'dir'}
			<div class="edit mono">
				<TextField
					label={$t('execution.connection.workingDirectory')}
					bind:value={editValue}
					error={editError}
				/>
				{@render editActions()}
			</div>
		{/if}
	</section>

	{#snippet editActions()}
		<div class="actions">
			<Button variant="primary" loading={busy} onclick={saveField}>
				{$t('execution.connection.save')}
			</Button>
			<Button variant="secondary" disabled={busy} onclick={() => (editing = null)}>
				{$t('execution.connection.cancel')}
			</Button>
		</div>
	{/snippet}

	<StatusRows
		{status}
		cli={connection.execution.cli}
		showTmux={connection.execution.channel === 'tmux'}
		{now}
		{refreshing}
		{onrefresh}
	/>
	{#if status?.runner.state === 'waiting'}
		<p class="hint">{$t('execution.connection.block.lost')}</p>
	{/if}

	<div class="fold">
		<Button
			variant="text"
			aria-expanded={advancedOpen}
			onclick={() => (advancedOpen = !advancedOpen)}
		>
			{$t('execution.connection.advanced.title')}
		</Button>
		{#if advancedOpen}
			<AdvancedFields
				bind:draft
				errors={advancedSubmitted ? advancedErrors : validateDraft(draftOf(connection))}
				{states}
				{labels}
				{domains}
				scope={`edit-${botId}`}
				{tmuxBlock}
				{channelError}
			/>
			<div class="actions">
				<Button variant="primary" loading={busy} disabled={!advancedDirty} onclick={saveAdvanced}>
					{$t('execution.connection.save')}
				</Button>
			</div>
		{/if}
	</div>

	<footer class="actions end">
		{#if connection.enabled}
			<Button variant="secondary" onclick={() => (pending = 'disable')}>
				{$t('execution.connection.actions.disable')}
			</Button>
		{:else}
			<Button variant="secondary" loading={busy} onclick={enable}>
				{$t('execution.connection.actions.enable')}
			</Button>
		{/if}
		<Button variant="secondary" onclick={() => (pending = 'rotate')}>
			{$t('execution.connection.actions.rotate')}
		</Button>
		<Button variant="danger-quiet" onclick={() => (pending = 'remove')}>
			{$t('execution.connection.actions.remove')}
		</Button>
	</footer>

	<ConfirmDialogs
		{connection}
		{projectName}
		{pending}
		{busy}
		onconfirm={confirmed}
		onclose={() => (pending = null)}
	/>
</div>

<style>
	.view {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
		padding: var(--rf-space-5);
		border: var(--rf-border-width) solid var(--rf-line);
		border-radius: var(--rf-radius-card);
	}
	.head {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--rf-space-4);
	}
	.channel {
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
	}
	.session {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-3);
		font-size: var(--rf-size-secondary);
	}
	.session p {
		margin: 0;
	}
	.session code {
		font-family: var(--rf-font-mono-stack);
	}
	.shared {
		padding: 0;
		border: 0;
		background: none;
		font: inherit;
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
		cursor: default;
	}
	.settings {
		display: flex;
		flex-direction: column;
	}
	.edit {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
		padding: var(--rf-space-4) 0;
		max-width: 560px;
	}
	.mono :global(input) {
		font-family: var(--rf-font-mono-stack);
	}
	.hint {
		margin: 0;
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
	}
	.fold {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
		align-items: flex-start;
	}
	.fold :global(.advanced) {
		align-self: stretch;
		max-width: 560px;
	}
	.actions {
		display: flex;
		flex-wrap: wrap;
		gap: var(--rf-space-4);
	}
	.end {
		padding-top: var(--rf-space-5);
	}
</style>
