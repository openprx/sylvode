<script lang="ts">
	import { t } from 'svelte-i18n';
	import UiProgressBar from '#lib/ui/progress-bar/ProgressBar.svelte';

	interface Props {
		value: number;
		label?: string;
		showPercentage?: boolean;
	}

	let { value, label, showPercentage = true }: Props = $props();

	const shownLabel = $derived(label ?? (showPercentage ? $t('common.status') : undefined));
</script>

<UiProgressBar
	value={Math.round(value)}
	label={shownLabel}
	valueText={showPercentage ? undefined : ''}
/>
