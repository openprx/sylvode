<svelte:options runes />

<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import { botConnectionApi, type ConnectInput } from '#lib/api/bot-connection.js';
	import Button from '#lib/ui/button/Button.svelte';
	import Segmented from '#lib/ui/segmented/Segmented.svelte';
	import Select from '#lib/ui/select/Select.svelte';
	import TextField from '#lib/ui/text-field/TextField.svelte';
	import TipBar from '#lib/ui/tip-bar/TipBar.svelte';
	import AdvancedFields from './AdvancedFields.svelte';
	import { connectFields, defaultDraft, draftValid, validateDraft, type Draft } from './advanced';
	import {
		loadConnectedBots,
		loadDomains,
		loadLabels,
		loadStates,
		type Choice,
		type ConnectedBotOption
	} from './options';
	import { routeServerMessage } from './server-errors';
	import {
		normalizeDirectory,
		normalizeRunnerUrl,
		validateBotChoice,
		validateBotName,
		validateRunnerUrl,
		validateWorkingDirectory
	} from './validate';
	import { channelErrorText, tmuxBlock as tmuxBlockOf } from './tmux';
	import { CLIS, type Cli, type Connection, type WriteAnswer } from './wire';

	interface Props {
		workspaceId: string;
		projectId: string;
		/** The project's connections: a CLI that has a tmux connection cannot get another. */
		connections: Connection[];
		oncancel: () => void;
		/** The answer of a successful connect. The block in it is the caller's from here on. */
		onconnected: (answer: WriteAnswer) => void;
		/** A business 409 asks the section to read the connection again. */
		onreload: () => void;
	}
	let { workspaceId, projectId, connections, oncancel, onconnected, onreload }: Props = $props();

	type Mode = 'new' | 'existing';
	let mode = $state<Mode>('new');
	let name = $state('');
	let botId = $state<string | null>(null);
	let url = $state('');
	let cli = $state<Cli>('claude');
	let directory = $state('');
	let draft = $state<Draft>(defaultDraft());
	let advancedOpen = $state(false);

	let connectedBots = $state<ConnectedBotOption[]>([]);
	let states = $state<Choice[]>([]);
	let labels = $state<Choice[]>([]);
	let domains = $state<Choice[]>([]);

	let submitting = $state(false);
	let submitted = $state(false);
	let serverField = $state<{ field: string; text: string } | null>(null);
	let conflict = $state<string | null>(null);

	onMount(async () => {
		[connectedBots, states, labels, domains] = await Promise.all([
			loadConnectedBots(workspaceId, projectId),
			loadStates(projectId),
			loadLabels(workspaceId),
			loadDomains(projectId)
		]);
	});

	const tmuxBlock = $derived(tmuxBlockOf(connections, cli, null));
	$effect(() => {
		// The server defaults to tmux; a CLI that cannot have it gets the one-shot channel instead.
		if (tmuxBlock !== null && draft.channel === 'tmux') draft.channel = 'headless';
	});
	const channelError = $derived(
		serverField?.field === 'channel'
			? channelErrorText(serverField.text, connections, (key, values) => $t(key, { values }))
			: undefined
	);

	const chosen = $derived(connectedBots.find((bot) => bot.id === botId) ?? null);
	const botOptions = $derived(
		connectedBots.map((bot) => ({
			value: bot.id,
			label: $t('execution.connection.bot.option', {
				values: { name: bot.name, address: bot.runnerUrl, count: bot.projects }
			})
		}))
	);
	const cliOptions = $derived(
		CLIS.map((value) => ({ value, label: $t(`execution.connection.cli.${value}`) }))
	);
	const modeOptions = $derived([
		{ value: 'new' as Mode, label: $t('execution.connection.bot.new') },
		{
			value: 'existing' as Mode,
			label: $t('execution.connection.bot.connected'),
			disabled: connectedBots.length === 0
		}
	]);

	const effectiveUrl = $derived(mode === 'existing' ? (chosen?.runnerUrl ?? '') : url);
	const errors = $derived({
		bot: mode === 'existing' ? validateBotChoice(botId) : validateBotName(name),
		url: validateRunnerUrl(effectiveUrl),
		directory: validateWorkingDirectory(directory),
		advanced: validateDraft(draft)
	});
	const valid = $derived(
		errors.bot === null &&
			errors.url === null &&
			errors.directory === null &&
			draftValid(errors.advanced)
	);
	/** A message shows once the form was submitted or the server named the field. */
	const show = (message: string | null, field: string): string | undefined => {
		if (serverField?.field === field) return serverField.text;
		return submitted && message !== null ? $t(message) : undefined;
	};

	async function submit() {
		submitted = true;
		serverField = null;
		conflict = null;
		if (!valid) {
			if (errors.advanced && !draftValid(errors.advanced)) advancedOpen = true;
			return;
		}
		submitting = true;
		const input: ConnectInput = {
			bot: mode === 'existing' && botId !== null ? { id: botId } : { name: name.trim() },
			runner_url: normalizeRunnerUrl(effectiveUrl),
			cli,
			working_directory: normalizeDirectory(directory),
			...connectFields(draft)
		};
		const result = await botConnectionApi.connect(projectId, input);
		submitting = false;
		if (result.ok) {
			onconnected(result.value);
			return;
		}
		const routed = routeServerMessage(result.message);
		if (routed.field === 'channel') {
			// Keep the server's whole message: the text is re-read when it names a known holder.
			serverField = { field: 'channel', text: result.message };
			advancedOpen = true;
		} else if (routed.field !== 'other') serverField = { field: routed.field, text: routed.text };
		else conflict = result.message;
	}
</script>

<form
	class="form"
	data-testid="connect-form"
	onsubmit={(event) => {
		event.preventDefault();
		void submit();
	}}
>
	{#if conflict}
		<TipBar tone="danger" action={{ label: $t('execution.connection.reload'), run: onreload }}>
			{conflict}
		</TipBar>
	{/if}

	<div class="field">
		<span class="caption" id="connect-bot-mode">{$t('execution.connection.bot.label')}</span>
		<Segmented
			labelledby="connect-bot-mode"
			options={modeOptions}
			value={mode}
			width="fill"
			onchange={(value: Mode) => {
				mode = value;
				serverField = null;
			}}
		/>
		{#if connectedBots.length === 0}
			<span class="hint">{$t('execution.connection.bot.noneConnected')}</span>
		{/if}
	</div>

	{#if mode === 'new'}
		<TextField
			label={$t('execution.connection.bot.name')}
			bind:value={name}
			required
			error={show(errors.bot, 'name')}
			autocomplete="off"
		/>
	{:else}
		<Select
			label={$t('execution.connection.bot.connected')}
			value={botId}
			options={botOptions}
			placeholder={$t('execution.connection.bot.choose')}
			required
			error={show(errors.bot, 'bot')}
			onchange={(value) => {
				botId = value;
				serverField = null;
			}}
		/>
	{/if}

	<TextField
		label={$t('execution.connection.runner.address')}
		value={mode === 'existing' ? effectiveUrl : url}
		oninput={(event) => (url = event.currentTarget.value)}
		required
		readonly={mode === 'existing'}
		type="text"
		inputmode="url"
		placeholder="https://runner.example.net:9090"
		helper={mode === 'existing' ? $t('execution.connection.runner.shared') : undefined}
		error={show(errors.url, 'url')}
		autocomplete="off"
	/>

	<Select
		label={$t('execution.connection.cli.label')}
		value={cli}
		options={cliOptions}
		onchange={(value) => (cli = value ?? 'claude')}
	/>
	{#if draft.channel === 'tmux'}
		<span class="hint">{$t('execution.connection.cli.tmuxOnly')}</span>
	{/if}

	<div class="mono">
		<TextField
			label={draft.channel === 'tmux'
				? $t('execution.connection.workingDirectoryTmux')
				: $t('execution.connection.workingDirectory')}
			bind:value={directory}
			required
			placeholder="/home/you/work/project"
			error={show(errors.directory, 'directory')}
			autocomplete="off"
		/>
	</div>

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
				errors={submitted ? errors.advanced : validateDraft(defaultDraft())}
				{states}
				{labels}
				{domains}
				scope="connect"
				{tmuxBlock}
				{channelError}
			/>
		{/if}
	</div>

	<div class="actions">
		<Button
			variant="primary"
			type="submit"
			loading={submitting}
			loadingLabel={$t('execution.connection.connecting')}
		>
			{$t('execution.connection.connectSubmit')}
		</Button>
		<Button variant="secondary" disabled={submitting} onclick={oncancel}>
			{$t('execution.connection.cancel')}
		</Button>
	</div>
</form>

<style>
	.form {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
		max-width: 560px;
	}
	.field {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-3);
	}
	.caption {
		font-size: var(--rf-size-secondary);
		color: var(--rf-text);
	}
	.hint {
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
	}
	.mono :global(input) {
		font-family: var(--rf-font-mono-stack);
	}
	.fold {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
		align-items: flex-start;
	}
	.fold :global(.advanced) {
		align-self: stretch;
	}
	.actions {
		display: flex;
		gap: var(--rf-space-4);
	}
</style>
