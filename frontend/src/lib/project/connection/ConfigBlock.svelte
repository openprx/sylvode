<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { copyText } from '#lib/clipboard.js';
	import { toast } from '#lib/stores/toast.js';
	import Button from '#lib/ui/button/Button.svelte';
	import Checkbox from '#lib/ui/checkbox/Checkbox.svelte';
	import SecurityWarning from '#lib/ui/security-warning/SecurityWarning.svelte';
	import Textarea from '#lib/ui/textarea/Textarea.svelte';
	import { blockFile, canFinish, textareaRows, visibleText, type BlockState } from './block-state';

	interface Props {
		/** The block as the section holds it; this component keeps no copy of the text. */
		block: BlockState;
		onstored: (stored: boolean) => void;
		ondone: () => void;
	}
	let { block, onstored, ondone }: Props = $props();

	const text = $derived(visibleText(block));
	const finishable = $derived(canFinish(block));

	async function copy() {
		// The message never contains the text.
		if (await copyText(text)) toast.success($t('execution.connection.block.copied'));
		else toast.error($t('execution.connection.block.copyFailed'));
	}

	/** Saves the file from memory when the button is pressed. Nothing is requested. */
	function download() {
		const file = blockFile(text);
		const url = URL.createObjectURL(file.blob);
		const anchor = document.createElement('a');
		anchor.href = url;
		anchor.download = file.name;
		document.body.append(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
	}
</script>

<section
	class="block"
	aria-label={$t('execution.connection.block.title')}
	data-testid="config-block"
>
	<SecurityWarning tone="warning" title={$t('execution.connection.block.title')}>
		{$t('execution.connection.block.body')}
	</SecurityWarning>
	<p class="scope">{$t('execution.connection.block.scope')}</p>
	<div class="text">
		<Textarea
			label={$t('execution.connection.block.label')}
			value={text}
			readonly
			rows={textareaRows(text)}
			spellcheck={false}
			autocomplete="off"
			data-testid="config-block-text"
		/>
	</div>
	<div class="buttons">
		<Button variant="primary" onclick={copy}>{$t('execution.connection.block.copy')}</Button>
		<Button variant="secondary" onclick={download}>
			{$t('execution.connection.block.download')}
		</Button>
	</div>
	<Checkbox
		checked={block.phase === 'shown' && block.stored}
		label={$t('execution.connection.block.stored')}
		onchange={(event) => onstored(event.currentTarget.checked)}
	/>
	<div class="buttons">
		<Button
			variant="primary"
			disabled={!finishable}
			disabledReason={finishable ? undefined : $t('execution.connection.block.doneReason')}
			onclick={ondone}>{$t('execution.connection.block.done')}</Button
		>
	</div>
</section>

<style>
	.block {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
	}
	.text :global(textarea) {
		font-family: var(--rf-font-mono-stack);
		overflow-y: auto;
	}
	.scope {
		margin: 0;
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
	.buttons {
		display: flex;
		flex-wrap: wrap;
		gap: var(--rf-space-4);
	}
</style>
