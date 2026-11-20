<script lang="ts">
	import { t } from 'svelte-i18n';
	import Button from './Button.svelte';
	import type { ButtonVariant } from './button-state';

	const variants: ButtonVariant[] = ['primary', 'secondary', 'text', 'danger', 'danger-quiet'];
</script>

<section data-gallery-section="button">
	<h2>button</h2>
	<div class="row">
		{#each variants as variant (variant)}
			<span data-gallery="button:{variant}"><Button {variant}>{variant}</Button></span>
			<span data-gallery="button:{variant}:disabled">
				<Button {variant} disabled>{variant}</Button>
			</span>
		{/each}
		<span data-gallery="button:disabled-reason">
			<Button variant="primary" disabled disabledReason={$t('ui.gallery.reason')}>
				{$t('ui.gallery.sample')}
			</Button>
		</span>
		<span data-gallery="button:loading">
			<Button variant="primary" loading loadingLabel={$t('ui.gallery.busy')}>
				{$t('ui.gallery.confirm')}
			</Button>
		</span>
		<span data-gallery="button:small"><Button size="sm">sm</Button></span>
	</div>
</section>

<style>
	.row {
		display: flex;
		flex-wrap: wrap;
		gap: var(--rf-space-4);
		align-items: flex-start;
	}
</style>
