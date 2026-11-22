<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { beforeNavigate, goto } from '$app/navigation';
	import { inboundApi, type RuleBody } from '#lib/api/inbound.js';
	import { confirmAction } from '#lib/ui/dialog/confirm.js';
	import Button from '#lib/ui/button/Button.svelte';
	import MultiSelect from '#lib/ui/multi-select/MultiSelect.svelte';
	import Radio from '#lib/ui/radio/Radio.svelte';
	import RadioGroup from '#lib/ui/radio/RadioGroup.svelte';
	import Segmented from '#lib/ui/segmented/Segmented.svelte';
	import Select from '#lib/ui/select/Select.svelte';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import Switch from '#lib/ui/switch/Switch.svelte';
	import TextField from '#lib/ui/text-field/TextField.svelte';
	import Textarea from '#lib/ui/textarea/Textarea.svelte';
	import TipBar from '#lib/ui/tip-bar/TipBar.svelte';
	import DurationField from './DurationField.svelte';
	import {
		INSTRUCTIONS_MAX,
		buildBody,
		draftChanged,
		draftFromRule,
		draftValid,
		isLimitsField,
		limitsAreDefault,
		limitsInvalid,
		newDraft,
		routeServerMessage,
		validateDraft,
		withAction,
		withSeverity,
		type FieldError,
		type RuleDraft,
		type ServerField
	} from './rule-form';
	import { severityKey } from './status';
	import {
		PRIORITIES,
		SEVERITIES,
		type Rule,
		type RuleAction,
		type Severity,
		type Source
	} from './wire';

	export interface BotOption {
		id: string;
		name: string;
	}
	export interface PersonOption {
		id: string;
		name: string;
	}

	interface Props {
		projectId: string;
		/** The rule being edited; `null` for a new rule. */
		rule: Rule | null;
		sources: readonly Source[];
		bots: readonly BotOption[];
		/** Human members: the recipients to choose from. */
		people: readonly PersonOption[];
		labels: readonly { id: string; name: string }[];
		currentUserId: string | null;
		/** The bot of the project's enabled connection; `null` when there is none. */
		connectedBotId: string | null;
		/** False when the connection could not be read: no "not connected" warning is made up. */
		connectionKnown: boolean;
		/** The saved rule and the warning codes of the answer. */
		onsaved: (rule: Rule, warnings: string[]) => void;
		oncancel: () => void;
		/** A version conflict: the section reads the rules again. */
		onreload: () => void;
	}
	let {
		projectId,
		rule,
		sources,
		bots,
		people,
		labels,
		currentUserId,
		connectedBotId,
		connectionKnown,
		onsaved,
		oncancel,
		onreload
	}: Props = $props();

	const ANY = '__any__';

	// The initial values only: the editor owns its draft from here on.
	// svelte-ignore state_referenced_locally
	let original = $state<RuleDraft>(
		rule !== null ? draftFromRule(rule) : newDraft({ currentUserId, connectedBotId })
	);
	// svelte-ignore state_referenced_locally
	let draft = $state<RuleDraft>(structuredClone(original));
	let limitsOpen = $state(false);
	let submitting = $state(false);
	let submitted = $state(false);
	let conflict = $state(false);
	let serverField = $state<{ field: ServerField; text: string } | null>(null);
	let formError = $state<string | null>(null);

	const errors = $derived(validateDraft(draft));
	const valid = $derived(draftValid(errors));
	const dirty = $derived(draftChanged(draft, original));

	const fieldChanged = (...keys: (keyof RuleDraft)[]): boolean =>
		keys.some((key) => JSON.stringify(draft[key]) !== JSON.stringify(original[key]));

	/** A message shows once the person changed the field, the form was submitted, or the server named it. */
	function show(
		error: FieldError | null,
		field: ServerField,
		...keys: (keyof RuleDraft)[]
	): string | undefined {
		if (serverField?.field === field) return serverField.text;
		if (error === null || !(submitted || fieldChanged(...keys))) return undefined;
		return $t(error.key, error.values ? { values: error.values } : undefined);
	}
	const clearServer = () => (serverField = null);

	const botOptions = $derived(
		[...bots]
			.sort((a, b) => Number(b.id === connectedBotId) - Number(a.id === connectedBotId))
			.map((bot) => ({
				value: bot.id,
				label:
					bot.id === connectedBotId
						? `${bot.name} ${$t('inbound.rules.botConnectedSuffix')}`
						: bot.name
			}))
	);
	const sourceOptions = $derived([
		{ value: ANY, label: $t('inbound.rules.sourceAny') },
		...sources.map((s) => ({ value: s.id, label: s.name }))
	]);
	const severityOptions = $derived(
		SEVERITIES.map((value) => ({ value, label: $t(severityKey(value)) }))
	);
	const priorityOptions = $derived(
		PRIORITIES.map((value) => ({ value, label: $t(`inbound.rules.priority.${value}`) }))
	);
	const botName = $derived(bots.find((b) => b.id === draft.botId)?.name ?? '');
	const botNotConnectedHere = $derived(
		connectionKnown &&
			draft.action === 'work_item_and_task' &&
			draft.botId !== null &&
			draft.botId !== connectedBotId
	);
	const botHelper = $derived(
		botNotConnectedHere
			? $t('inbound.rules.warnBotNotConnected', { values: { bot: botName } })
			: bots.length === 0
				? $t('inbound.rules.noBots')
				: undefined
	);
	const instructionCount = $derived([...draft.instructions].length);
	const severityOrder = [...SEVERITIES].reverse();

	/**
	 * The unsaved rule in the shape `inboundApi.testRules` takes as `draft_rule` (and `replaces`
	 * when a stored rule is being edited), or `null` while a client rule fails. The rule test
	 * dialog reads it through `bind:this`.
	 */
	export function draftForTest(): { draft_rule: RuleBody; replaces?: string } | null {
		const body = buildBody(draft);
		if (body === null) return null;
		return rule !== null ? { draft_rule: body, replaces: rule.id } : { draft_rule: body };
	}

	async function save() {
		submitted = true;
		clearServer();
		conflict = false;
		formError = null;
		if (!valid) {
			if (limitsInvalid(errors)) limitsOpen = true;
			return;
		}
		const body = buildBody(draft);
		if (body === null) return;
		submitting = true;
		const result =
			rule === null
				? await inboundApi.createRule(projectId, body)
				: await inboundApi.replaceRule(rule.id, { ...body, expected_version: rule.version });
		submitting = false;
		if (result.ok) {
			// The saved values are the new baseline: leaving after a save never asks.
			original = structuredClone($state.snapshot(draft));
			onsaved(result.value.rule, result.value.warnings);
			return;
		}
		if (result.kind === 'conflict') {
			conflict = true;
			return;
		}
		const field = routeServerMessage(result.message);
		if (field === 'other') {
			formError = result.message;
			return;
		}
		serverField = { field, text: result.message };
		if (isLimitsField(field)) limitsOpen = true;
	}

	async function cancel() {
		if (dirty && !(await confirmLeave())) return;
		oncancel();
	}

	function confirmLeave(): Promise<boolean> {
		return confirmAction({
			title: $t('inbound.rules.leave'),
			confirmLabel: $t('inbound.rules.leaveConfirm')
		});
	}

	beforeNavigate((navigation) => {
		if (!dirty || submitting) return;
		navigation.cancel();
		const target = navigation.to?.url ?? null;
		if (navigation.type === 'leave' || target === null) return;
		void confirmLeave().then((leave) => {
			if (!leave) return;
			original = structuredClone($state.snapshot(draft));
			// eslint-disable-next-line svelte/no-navigation-without-resolve -- the target is the URL the person was already going to
			void goto(target);
		});
	});

	$effect(() => {
		if (!dirty) return;
		const warn = (event: BeforeUnloadEvent) => event.preventDefault();
		window.addEventListener('beforeunload', warn);
		return () => window.removeEventListener('beforeunload', warn);
	});
</script>

<form
	class="form"
	data-testid="rule-editor"
	onsubmit={(event) => {
		event.preventDefault();
		void save();
	}}
>
	<h2 class="heading">
		{rule === null
			? $t('inbound.rules.new')
			: $t('inbound.rules.editTitle', { values: { name: rule.name } })}
	</h2>

	{#if conflict}
		<TipBar tone="warning" action={{ label: $t('inbound.rules.reload'), run: onreload }}>
			{$t('inbound.rules.conflict')}
		</TipBar>
	{/if}
	{#if formError}
		<TipBar tone="danger">{formError}</TipBar>
	{/if}

	<fieldset class="group">
		<legend class="legend">{$t('inbound.rules.groupWhen')}</legend>

		<TextField
			label={$t('inbound.rules.name')}
			bind:value={draft.name}
			required
			error={show(errors.name, 'name', 'name')}
			oninput={clearServer}
			autocomplete="off"
		/>

		<Select
			label={$t('inbound.rules.source')}
			value={draft.sourceId ?? ANY}
			options={sourceOptions}
			error={show(null, 'source')}
			onchange={(value) => {
				draft.sourceId = value === null || value === ANY ? null : value;
				clearServer();
			}}
		/>

		<div class="mono">
			<Textarea
				label={$t('inbound.rules.kinds')}
				bind:value={draft.kindsText}
				rows={3}
				helper={$t('inbound.rules.kindsHelp')}
				error={show(errors.kinds, 'kinds', 'kindsText')}
				oninput={clearServer}
				spellcheck={false}
				autocomplete="off"
			/>
		</div>

		<div class="field">
			<span class="caption" id="rule-min-severity">{$t('inbound.rules.minSeverity')}</span>
			<Segmented
				labelledby="rule-min-severity"
				options={severityOptions}
				value={draft.minSeverity}
				width="fill"
				onchange={(value: Severity) => (draft = withSeverity(draft, value))}
			/>
		</div>
	</fieldset>

	<fieldset class="group">
		<legend class="legend">{$t('inbound.rules.groupThen')}</legend>

		<div class="field">
			<span class="caption" id="rule-action">{$t('inbound.rules.action')}</span>
			<RadioGroup
				labelledby="rule-action"
				value={draft.action}
				onchange={(value) => (draft = withAction(draft, value as RuleAction))}
			>
				<Radio value="work_item" label={$t('inbound.rules.actionOpen')} />
				<Radio value="work_item_and_task" label={$t('inbound.rules.actionHand')} />
			</RadioGroup>
		</div>

		{#if draft.action === 'work_item_and_task'}
			<Select
				label={$t('inbound.rules.bot')}
				value={draft.botId}
				options={botOptions}
				placeholder={$t('inbound.rules.botPlaceholder')}
				required
				helper={botHelper}
				error={show(errors.bot, 'bot', 'botId', 'action')}
				onchange={(value) => {
					draft.botId = value;
					clearServer();
				}}
			/>

			<div class="field">
				<Textarea
					label={$t('inbound.rules.instructions')}
					bind:value={draft.instructions}
					rows={8}
					helper={$t('inbound.rules.instructionsHelp')}
					error={show(errors.instructions, 'instructions', 'instructions')}
					oninput={clearServer}
				/>
				<span class="counter" class:over={instructionCount > INSTRUCTIONS_MAX}>
					{$t('inbound.rules.instructionsCount', { values: { count: instructionCount } })}
				</span>
			</div>
		{/if}

		<MultiSelect
			label={$t('inbound.rules.notify')}
			value={draft.notifyIds}
			options={people.map((p) => ({ value: p.id, label: p.name }))}
			searchable
			helper={draft.notifyIds.length === 0 ? $t('inbound.rules.notifyEmpty') : undefined}
			error={show(errors.notify, 'notify', 'notifyIds')}
			onchange={(value) => {
				draft.notifyIds = value;
				clearServer();
			}}
		/>

		<MultiSelect
			label={$t('inbound.rules.labels')}
			value={draft.labelIds}
			options={labels.map((l) => ({ value: l.id, label: l.name }))}
			searchable
			error={show(errors.labels, 'labels', 'labelIds')}
			onchange={(value) => {
				draft.labelIds = value;
				clearServer();
			}}
		/>
	</fieldset>

	<div class="fold">
		<div class="fold-head">
			<Button variant="text" aria-expanded={limitsOpen} onclick={() => (limitsOpen = !limitsOpen)}>
				{$t('inbound.rules.groupLimits')}
			</Button>
			<StatusChip>
				{$t(
					limitsAreDefault(draft) ? 'inbound.rules.limitsDefault' : 'inbound.rules.limitsChanged'
				)}
			</StatusChip>
		</div>
		<!-- Kept mounted while collapsed: the duration fields own their drafts. -->
		<div class="limits" hidden={!limitsOpen}>
			<TextField
				label={$t('inbound.rules.tasksPerHour')}
				bind:value={draft.tasksPerHour}
				type="number"
				min="1"
				max="120"
				helper={$t('inbound.rules.tasksPerHourHelp')}
				error={show(errors.tasksPerHour, 'tasksPerHour', 'tasksPerHour')}
				oninput={clearServer}
			/>
			<TextField
				label={$t('inbound.rules.tasksAtOnce')}
				bind:value={draft.activeTasks}
				type="number"
				min="1"
				max="20"
				error={show(errors.activeTasks, 'activeTasks', 'activeTasks')}
				oninput={clearServer}
			/>
			<TextField
				label={$t('inbound.rules.attempts')}
				bind:value={draft.attempts}
				type="number"
				min="1"
				max="10"
				error={show(errors.attempts, 'attempts', 'attempts')}
				oninput={clearServer}
			/>
			<DurationField
				label={$t('inbound.rules.dedupWindow')}
				seconds={original.dedupWindowSecs ?? 0}
				helper={$t('inbound.rules.dedupWindowHelp')}
				error={show(errors.dedupWindow, 'dedupWindow', 'dedupWindowSecs')}
				onchange={(seconds) => {
					draft.dedupWindowSecs = seconds;
					clearServer();
				}}
			/>
			<DurationField
				label={$t('inbound.rules.rearm')}
				seconds={original.rearmAfterSecs ?? 0}
				helper={$t('inbound.rules.rearmHelp')}
				error={show(errors.rearm, 'rearm', 'rearmAfterSecs')}
				onchange={(seconds) => {
					draft.rearmAfterSecs = seconds;
					clearServer();
				}}
			/>
			<div class="field">
				<span class="caption">{$t('inbound.rules.priorityMap')}</span>
				<div class="priorities">
					{#each severityOrder as severity (severity)}
						<Select
							label={$t(severityKey(severity))}
							value={draft.priorityMap[severity]}
							options={priorityOptions}
							onchange={(value) => {
								if (value !== null) draft.priorityMap[severity] = value;
							}}
						/>
					{/each}
				</div>
			</div>
		</div>
	</div>

	<Switch bind:checked={draft.enabled} label={$t('inbound.rules.enabledSwitch')} />

	<div class="actions">
		<Button
			variant="primary"
			type="submit"
			loading={submitting}
			loadingLabel={$t('inbound.rules.saving')}
			disabled={!valid}
			disabledReason={$t('inbound.rules.invalid')}
		>
			{$t('inbound.rules.saveRule')}
		</Button>
		<!-- U5 HOOK: the "Test with this rule" button goes here; it reads `draftForTest()`. -->
		<Button variant="secondary" disabled={submitting} onclick={() => void cancel()}>
			{$t('inbound.sources.cancel')}
		</Button>
	</div>
</form>

<style>
	.form {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
	}
	.heading {
		margin: 0;
		font-size: var(--rf-size-title);
		font-weight: 600;
	}
	.group {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
		margin: 0;
		padding: 0;
		border: 0;
		min-width: 0;
	}
	.legend {
		padding: 0;
		margin-bottom: var(--rf-space-3);
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
		font-weight: 600;
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
	.counter {
		align-self: flex-end;
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
	}
	.counter.over {
		color: var(--rf-danger);
	}
	.mono :global(textarea) {
		font-family: var(--rf-font-mono-stack);
	}
	.fold {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
	}
	.fold-head {
		display: flex;
		align-items: center;
		gap: var(--rf-space-3);
	}
	.limits {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
	}
	.limits[hidden] {
		display: none;
	}
	.priorities {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--rf-space-3);
	}
	.actions {
		display: flex;
		gap: var(--rf-space-4);
	}
</style>
