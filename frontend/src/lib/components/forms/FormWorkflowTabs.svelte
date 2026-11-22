<script lang="ts">
	import { FileText, LayoutPanelTop, Shield, Upload } from '@lucide/svelte';
	import { t } from 'svelte-i18n';

	export type FormWorkflowMode =
		| 'data'
		| 'detail'
		| 'design'
		| 'layout'
		| 'permissions'
		| 'importExport';

	interface Props {
		activeMode: FormWorkflowMode;
		onModeChange?: (mode: FormWorkflowMode) => void;
	}

	let { activeMode, onModeChange }: Props = $props();

	const modes: Array<{ key: FormWorkflowMode; icon: typeof LayoutPanelTop }> = [
		{ key: 'design', icon: LayoutPanelTop },
		{ key: 'layout', icon: FileText },
		{ key: 'permissions', icon: Shield },
		{ key: 'importExport', icon: Upload }
	];
</script>

<div class="border-b border-line">
	<div class="flex flex-wrap gap-1 px-1">
		{#each modes as mode, index (index)}
			{@const Icon = mode.icon}
			<button
				type="button"
				class="inline-flex h-10 items-center gap-2 border-b-2 px-3 text-sm font-medium transition-colors disabled:cursor-not-allowed disabled:text-fg-muted {activeMode ===
				mode.key
					? 'border-fg text-fg'
					: 'border-transparent text-fg-muted hover:border-line-strong hover:text-fg'}"
				aria-current={activeMode === mode.key ? 'page' : undefined}
				onclick={() => onModeChange?.(mode.key)}
			>
				<Icon class="h-4 w-4" aria-hidden="true" />
				<span>{$t(`forms.modes.${mode.key}`)}</span>
			</button>
		{/each}
	</div>
</div>
