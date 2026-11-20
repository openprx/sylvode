<script lang="ts">
	import { locale, t } from 'svelte-i18n';
	import '../foundations/visually-hidden.css';
	import CircleAlert from '@lucide/svelte/icons/circle-alert';
	import Button from '../button/Button.svelte';
	import {
		announcementFor,
		clampValue,
		percentOf,
		stepOf,
		type Announcement,
		type ProgressState
	} from './progress-state';

	interface Props {
		value: number;
		max?: number;
		/** Visible label above the bar; also the accessible name of the bar. */
		label?: string;
		/** Text shown at the end and spoken as the value, for example "3 of 8". Defaults to a percentage. */
		valueText?: string;
		state?: ProgressState;
		/** What happens next, or the result. Queued has a default. */
		message?: string;
		onretry?: () => void;
	}

	let {
		value,
		max = 100,
		label,
		valueText,
		state: phase = 'running',
		message,
		onretry
	}: Props = $props();

	const clamped = $derived(clampValue(value, max));
	const percent = $derived(percentOf(value, max));
	const shown = $derived(
		valueText ?? `${new Intl.NumberFormat($locale ?? undefined).format(percent)}%`
	);
	const statusMessage = $derived(
		message ?? (phase === 'queued' ? $t('ui.progress.queued') : undefined)
	);

	const announceKey: Record<Announcement, string> = {
		started: 'ui.progress.started',
		milestone: 'ui.progress.milestone',
		done: 'ui.progress.done',
		failed: 'ui.progress.failed'
	};

	let announcement = $state('');
	let previous: { state: ProgressState; step: number } | null = null;

	$effect(() => {
		const next = { state: phase, step: stepOf(percent) };
		const kind = announcementFor(previous, next);
		previous = next;
		if (kind) announcement = $t(announceKey[kind], { values: { percent } });
	});
</script>

<div class="ui-progress state-{phase}" data-ui="progress-bar" data-state={phase}>
	{#if label}<span class="label">{label}</span>{/if}
	<div class="line">
		<div
			class="track"
			role="progressbar"
			aria-label={label}
			aria-valuemin={0}
			aria-valuemax={max}
			aria-valuenow={clamped}
			aria-valuetext={shown}
		>
			<div class="fill" style:width="{percent}%"></div>
		</div>
		<span class="value">{shown}</span>
	</div>
	{#if statusMessage}
		<div class="message" class:failed={phase === 'failed'}>
			{#if phase === 'failed'}<CircleAlert size={16} class="glyph" aria-hidden="true" />{/if}
			<span class="text">{statusMessage}</span>
			{#if phase === 'failed' && onretry}
				<Button variant="text" size="sm" onclick={onretry}>{$t('ui.progress.retry')}</Button>
			{/if}
		</div>
	{/if}
	<span class="ui-sr-only" role="status" aria-live="polite">{announcement}</span>
</div>

<style>
	.ui-progress {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-2);
		min-width: 240px;
		position: relative;
		font-family: var(--rf-font-sans-stack);
	}
	.label {
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
		color: var(--rf-secondary);
	}
	.line {
		display: flex;
		align-items: center;
		gap: var(--rf-space-4);
	}
	/* Track 4 high with radius 2 (the file's literal for progress bars). */
	.track {
		flex: 1;
		height: 4px;
		border-radius: 2px;
		background: var(--rf-surface);
		overflow: hidden;
	}
	.fill {
		height: 100%;
		border-radius: 2px;
		background: var(--rf-primary);
		transition: width var(--rf-duration-base) var(--rf-ease-standard);
	}
	.value {
		min-width: 36px;
		text-align: end;
		font-size: var(--rf-size-meta);
		line-height: var(--rf-leading-meta);
		font-variant-numeric: tabular-nums;
		color: var(--rf-text);
	}
	.message {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: var(--rf-space-2) var(--rf-space-4);
		font-size: var(--rf-size-meta);
		line-height: var(--rf-leading-meta);
		color: var(--rf-secondary);
	}
	.message.failed {
		color: var(--rf-danger);
	}
	.message :global(.glyph) {
		flex: none;
	}
</style>
