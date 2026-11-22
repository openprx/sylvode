<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import { severityKey, severityOfRank, severityTone } from './status';
	import { SEVERITIES, type Severity } from './wire';

	interface Props {
		/** A severity word, or the numeric rank (0-4) an event row carries. */
		severity: Severity | number;
	}
	let { severity }: Props = $props();

	const resolved = $derived<Severity | null>(
		typeof severity === 'number'
			? severityOfRank(severity)
			: (SEVERITIES.find((s) => s === severity) ?? null)
	);
</script>

<StatusChip tone={severityTone(resolved)} data-severity={resolved ?? 'unknown'}>
	{$t(severityKey(resolved))}
</StatusChip>
