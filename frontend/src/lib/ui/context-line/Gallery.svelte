<script lang="ts">
	import { t } from 'svelte-i18n';
	import ContextLine from './ContextLine.svelte';
	import type { ContextSegment } from './context-line';

	const segments = $derived<ContextSegment[]>([
		{ id: 'project', label: $t('ui.contextLine.sampleProject'), href: '#project' },
		{ id: 'forms', label: $t('ui.contextLine.sampleForms'), href: '#forms' },
		{ id: 'section', label: $t('ui.contextLine.sampleSection'), href: '#section' },
		{ id: 'group', label: $t('ui.contextLine.sampleGroup'), href: '#group' }
	]);
	const restricted = $derived<ContextSegment[]>([
		{ id: 'project', label: $t('ui.contextLine.sampleProject'), href: '#project' },
		{ id: 'hidden', label: '', state: 'restricted' },
		{ id: 'broken', label: '', state: 'unavailable' }
	]);
</script>

<section data-gallery-section="context-line">
	<h2>context-line</h2>
	<div class="stack">
		<div data-gallery="context-line:default" class="wide">
			<ContextLine {segments} current={$t('ui.contextLine.sampleCurrent')} />
		</div>
		<div data-gallery="context-line:collapsed" class="narrow">
			<ContextLine {segments} current={$t('ui.contextLine.sampleCurrent')} />
		</div>
		<div data-gallery="context-line:restricted" class="wide">
			<ContextLine segments={restricted} current={$t('ui.contextLine.sampleCurrent')} />
		</div>
		<div data-gallery="context-line:loading" class="wide">
			<ContextLine segments={[]} current="" loading />
		</div>
	</div>
</section>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
	}
	.wide {
		width: 520px;
		max-width: 100%;
	}
	.narrow {
		width: 340px;
		max-width: 100%;
	}
</style>
