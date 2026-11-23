<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { inboundApi, type RuleBody } from '#lib/api/inbound.js';
	import { relativeTime } from '#lib/project/agent-tasks/time.js';
	import { randomUuid } from '#lib/utils/uuid.js';
	import Button from '#lib/ui/button/Button.svelte';
	import Dialog from '#lib/ui/dialog/Dialog.svelte';
	import Segmented from '#lib/ui/segmented/Segmented.svelte';
	import Select from '#lib/ui/select/Select.svelte';
	import TipBar from '#lib/ui/tip-bar/TipBar.svelte';
	import Textarea from '#lib/ui/textarea/Textarea.svelte';
	import RuleTestOutcome from './RuleTestOutcome.svelte';
	import { severityKey, severityOfRank } from './status';
	import {
		SAMPLE_MAX_BYTES,
		begin,
		buildRequest,
		defaultEvent,
		defaultMode,
		defaultSource,
		IDLE,
		parseSample,
		sampleFieldError,
		sampleText,
		settle,
		type SampleError,
		type TestMode,
		type TestState
	} from './test-view';
	import type { InboundEvent, Source } from './wire';

	interface Props {
		open: boolean;
		onclose: () => void;
		/** The project whose rules are tested. */
		projectId: string;
		/** Source to start on (the entry point's), else the first source. */
		sourceId?: string | null;
		/** Stored event to start on (the "Test rules with this event" entry), else the newest. */
		eventId?: string | null;
		/** The rule being edited, unsaved: included in its place in the order (`draft_rule`). */
		draftRule?: RuleBody | null;
		/** The saved rule the draft replaces (`replaces`); without it the draft goes last. */
		replaces?: string | null;
		/** The editor's first exact kind, for the sample event. */
		sampleKind?: string | null;
	}

	let {
		open,
		onclose,
		projectId,
		sourceId = null,
		eventId = null,
		draftRule = null,
		replaces = null,
		sampleKind = null
	}: Props = $props();

	type Load = 'loading' | 'error' | 'ready';

	let sourcesLoad = $state<Load>('loading');
	let sources = $state<Source[]>([]);
	let chosenSource = $state<string | null>(null);
	let eventsLoad = $state<Load>('loading');
	let events = $state<InboundEvent[]>([]);
	let mode = $state<TestMode>('recent');
	let chosenEvent = $state<string | null>(null);
	let sample = $state('');
	let sampleClientError = $state<SampleError | null>(null);
	let run = $state<TestState>(IDLE);

	// Late answers of a source or event load, or of a run, that the person has moved past.
	let loadSeq = 0;
	let runSeq = 0;

	function reset() {
		loadSeq += 1;
		runSeq += 1;
		sourcesLoad = 'loading';
		sources = [];
		chosenSource = null;
		eventsLoad = 'loading';
		events = [];
		chosenEvent = null;
		mode = 'recent';
		sample = '';
		sampleClientError = null;
		run = IDLE;
	}

	async function loadEvents(id: string, preselectedEvent: string | null) {
		const mine = ++loadSeq;
		eventsLoad = 'loading';
		const answer = await inboundApi.listEvents(id, { limit: 50 });
		if (mine !== loadSeq) return;
		if (!answer.ok) {
			eventsLoad = 'error';
			events = [];
			chosenEvent = null;
			return;
		}
		events = answer.value;
		eventsLoad = 'ready';
		chosenEvent = defaultEvent(events, preselectedEvent);
		mode = defaultMode(events.length);
	}

	async function start() {
		reset();
		sample = sampleText(randomUuid(), new Date(), sampleKind);
		const mine = loadSeq;
		const answer = await inboundApi.listSources(projectId);
		if (mine !== loadSeq) return;
		if (!answer.ok) {
			sourcesLoad = 'error';
			return;
		}
		sources = answer.value.sources;
		sourcesLoad = 'ready';
		chosenSource = defaultSource(sources, sourceId);
		if (chosenSource === null) return;
		await loadEvents(chosenSource, eventId);
	}

	let wasOpen = false;
	$effect(() => {
		if (open && !wasOpen) void start();
		if (!open && wasOpen) reset();
		wasOpen = open;
	});

	function chooseSource(id: string | null) {
		if (id === null || id === chosenSource) return;
		chosenSource = id;
		// The answer shown belongs to the other source.
		run = IDLE;
		runSeq += 1;
		void loadEvents(id, null);
	}

	function chooseMode(next: TestMode) {
		mode = next;
	}

	const severityWord = (rank: number): string => $t(severityKey(severityOfRank(rank)));

	const eventOptions = $derived(
		events.map((e) => ({
			value: e.id,
			label: $t('inbound.test.more.recentOption', {
				values: {
					time: relativeTime(e.received_at),
					kind: e.kind,
					severity: severityWord(e.severity_rank)
				}
			})
		}))
	);

	const noEvents = $derived(eventsLoad === 'ready' && events.length === 0);
	const modeOptions = $derived([
		{
			value: 'recent' as TestMode,
			label: $t('inbound.test.mode.recent'),
			disabled: noEvents,
			disabledReason: noEvents ? $t('inbound.test.noEvents') : undefined
		},
		{ value: 'sample' as TestMode, label: $t('inbound.test.mode.sample') }
	]);

	const sourceOptions = $derived(sources.map((s) => ({ value: s.id, label: s.name })));

	const sampleError = $derived(
		sampleClientError
			? $t(`inbound.test.more.sampleError.${sampleClientError}`, {
					values: { max: SAMPLE_MAX_BYTES / 1024 }
				})
			: sampleFieldError(run)
	);

	const canRun = $derived(
		chosenSource !== null && eventsLoad === 'ready' && (mode === 'sample' || chosenEvent !== null)
	);

	async function runTest() {
		if (chosenSource === null) return;
		sampleClientError = null;
		let envelope: Record<string, unknown> | null = null;
		if (mode === 'sample') {
			const parsed = parseSample(sample);
			if (!parsed.ok) {
				// Nothing is sent, and no earlier result stays next to the broken input.
				sampleClientError = parsed.error;
				run = IDLE;
				return;
			}
			envelope = parsed.envelope;
		}
		const request = buildRequest({
			sourceId: chosenSource,
			mode,
			eventId: chosenEvent,
			envelope,
			draftRule,
			replaces
		});
		if (request === null) return;
		const mine = ++runSeq;
		const askedMode = mode;
		run = begin();
		const answer = await inboundApi.testRules(projectId, request);
		if (mine !== runSeq) return;
		run = settle(answer, askedMode);
	}

	const running = $derived(run.phase === 'running');
</script>

<Dialog {open} {onclose} size="wide" title={$t('inbound.test.title')} testId="rule-test-dialog">
	<div class="body">
		<p class="notice" data-testid="rule-test-nothing-created">
			{$t('inbound.test.more.nothingCreated')}
		</p>

		{#if draftRule}
			<TipBar tone="neutral" data-testid="rule-test-draft">
				{$t('inbound.test.more.draftIncluded')}
			</TipBar>
		{/if}

		{#if sourcesLoad === 'loading'}
			<p class="muted">{$t('inbound.test.more.loadingEvents')}</p>
		{:else if sourcesLoad === 'error'}
			<TipBar
				tone="danger"
				action={{ label: $t('inbound.test.more.retry'), run: () => void start() }}
			>
				{$t('inbound.test.more.loadFailed')}
			</TipBar>
		{:else if sources.length === 0}
			<TipBar tone="warning">{$t('inbound.test.noSource')}</TipBar>
		{:else}
			<Select
				label={$t('inbound.test.source')}
				required
				value={chosenSource}
				options={sourceOptions}
				onchange={chooseSource}
				disabled={running}
			/>

			<div class="mode">
				<Segmented
					label={$t('inbound.test.event')}
					options={modeOptions}
					value={mode}
					onchange={chooseMode}
					width="fill"
					disabled={running}
				/>
			</div>

			{#if eventsLoad === 'error'}
				<TipBar
					tone="danger"
					action={{
						label: $t('inbound.test.more.retry'),
						run: () => chosenSource && void loadEvents(chosenSource, null)
					}}
				>
					{$t('inbound.test.more.loadFailed')}
				</TipBar>
			{/if}

			{#if mode === 'recent'}
				<Select
					label={$t('inbound.test.mode.recent')}
					required
					value={chosenEvent}
					options={eventOptions}
					onchange={(v) => (chosenEvent = v)}
					disabled={running || noEvents || eventsLoad !== 'ready'}
					disabledReason={noEvents ? $t('inbound.test.noEvents') : undefined}
				/>
			{:else}
				<div class="sample">
					<Textarea
						label={$t('inbound.test.sample')}
						bind:value={sample}
						rows={14}
						spellcheck={false}
						helper={$t('inbound.test.more.sampleHelp')}
						error={sampleError ?? undefined}
						disabled={running}
					/>
				</div>
			{/if}
		{/if}

		<RuleTestOutcome state={run} />
	</div>

	{#snippet actions()}
		<Button variant="secondary" onclick={onclose}>{$t('ui.dialog.close')}</Button>
		<Button
			variant="primary"
			onclick={() => void runTest()}
			disabled={!canRun}
			loading={running}
			loadingLabel={$t('inbound.test.running')}
		>
			{$t('inbound.test.run')}
		</Button>
	{/snippet}
</Dialog>

<style>
	.body {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
		font-family: var(--rf-font-sans-stack);
	}
	.notice {
		margin: 0;
		color: var(--rf-text);
		font-size: var(--rf-size-secondary);
		font-weight: 600;
	}
	.muted {
		margin: 0;
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
	.sample :global(textarea) {
		font-family: var(--rf-font-mono-stack);
	}
</style>
