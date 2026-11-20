<script module lang="ts">
	export type BadgeTone = 'neutral' | 'success' | 'warning' | 'danger';

	/**
	 * Colour is kept for facts that need attention (specification: status is words first). The old
	 * blue, amber and orange tints that carried no warning map to neutral; the words stay.
	 */
	const PRIORITY_TONES: Record<string, BadgeTone> = {
		low: 'neutral',
		medium: 'neutral',
		high: 'warning',
		urgent: 'danger'
	};
	const STATUS_TONES: Record<string, BadgeTone> = {
		backlog: 'neutral',
		todo: 'neutral',
		in_progress: 'neutral',
		done: 'success'
	};

	export function badgeTone(kind: 'priority' | 'status', value: string): BadgeTone {
		return (kind === 'priority' ? PRIORITY_TONES : STATUS_TONES)[value] ?? 'neutral';
	}

	export const BADGE_LABEL_KEYS: Record<string, string> = {
		backlog: 'issue.backlog',
		todo: 'issue.todo',
		in_progress: 'issue.inProgress',
		done: 'issue.done',
		low: 'issue.low',
		medium: 'issue.medium',
		high: 'issue.high',
		urgent: 'issue.urgent'
	};
</script>

<script lang="ts">
	import { t } from 'svelte-i18n';
	import type { IssuePriority, IssueStatus } from '#lib/api/issues.js';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';

	type BadgeValue = IssuePriority | IssueStatus | string;

	interface Props {
		value: BadgeValue;
		kind?: 'priority' | 'status';
		/** Kept for compatibility: the chip has one size. */
		size?: 'sm' | 'md';
	}

	// eslint-disable-next-line svelte/no-unused-props -- `size` is accepted and ignored
	let { value, kind = 'status' }: Props = $props();

	const key = $derived(BADGE_LABEL_KEYS[value]);
	const text = $derived(key ? $t(key) : value);
</script>

<StatusChip tone={badgeTone(kind, value)}>{text}</StatusChip>
