<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import type { SharedMemberEditor } from '#lib/flow/shared-blocks/editor.js';
	import type {
		SharedViewSession,
		SharedSessionSnapshot
	} from '#lib/flow/shared-blocks/session.js';
	let {
		editor,
		session,
		snapshot
	}: { editor: SharedMemberEditor; session: SharedViewSession; snapshot: SharedSessionSnapshot } =
		$props();
	let host: HTMLDivElement;
	let ready = $state(false),
		failed = $state(false),
		marks = $state<string[]>([]),
		linkOpen = $state(false),
		href = $state(''),
		linkError = $state(false);
	const editable = $derived(
		snapshot.authorized &&
			snapshot.draft?.remote?.effective_permission === 'edit' &&
			snapshot.draft.phase !== 'saving'
	);
	const code = $derived(snapshot.editing?.member.content.kind === 'code');
	onMount(() => {
		let active = true;
		editor.onSelection(() => {
			if (active) marks = editor.activeMarks();
		});
		void editor
			.mount(host)
			.then(() => {
				if (active) {
					ready = true;
					marks = editor.activeMarks();
				}
			})
			.catch(() => {
				if (active) failed = true;
			});
		return () => {
			active = false;
			editor.unmount();
		};
	});
	$effect(() => editor.setEditable(editable));
	function format(mark: 'bold' | 'italic' | 'strike' | 'code') {
		editor.mark(mark);
		marks = editor.activeMarks();
	}
</script>

<section
	class="mt-6 rounded-card border border-line p-4"
	aria-label={$t('flow.shared.localDraft')}
	data-testid="flow-shared-draft"
>
	<h2 class="mb-2 text-sm font-medium">{$t('flow.shared.localDraft')}</h2>
	<p class="mb-3 text-xs text-fg-muted">{$t('flow.shared.editorLimit')}</p>
	{#if !code}<div
			role="toolbar"
			aria-label={$t('flow.shared.formatting')}
			class="mb-2 flex flex-wrap gap-1"
		>
			<button
				type="button"
				class="rounded border px-2 py-1 font-semibold"
				disabled={!ready || !editable}
				aria-label={$t('flow.shared.bold')}
				title={$t('flow.shared.bold')}
				aria-pressed={marks.includes('bold')}
				onmousedown={(event) => event.preventDefault()}
				onclick={() => format('bold')}>B</button
			>
			<button
				type="button"
				class="rounded border px-2 py-1 italic"
				disabled={!ready || !editable}
				aria-label={$t('flow.shared.italic')}
				title={$t('flow.shared.italic')}
				aria-pressed={marks.includes('italic')}
				onmousedown={(event) => event.preventDefault()}
				onclick={() => format('italic')}>I</button
			>
			<button
				type="button"
				class="rounded border px-2 py-1"
				disabled={!ready || !editable}
				aria-label={$t('flow.shared.strike')}
				title={$t('flow.shared.strike')}
				aria-pressed={marks.includes('strike')}
				onmousedown={(event) => event.preventDefault()}
				onclick={() => format('strike')}><s>S</s></button
			>
			<button
				type="button"
				class="rounded border px-2 py-1 font-mono"
				disabled={!ready || !editable}
				aria-label={$t('flow.shared.codeMark')}
				title={$t('flow.shared.codeMark')}
				aria-pressed={marks.includes('code')}
				onmousedown={(event) => event.preventDefault()}
				onclick={() => format('code')}>`</button
			>
			<button
				type="button"
				class="rounded border px-2 py-1 text-sm"
				disabled={!ready || !editable}
				aria-expanded={linkOpen}
				onmousedown={(event) => event.preventDefault()}
				onclick={() => (linkOpen = !linkOpen)}>{$t('flow.shared.link')}</button
			>
		</div>{/if}
	{#if linkOpen}<form
			class="mb-3 flex flex-wrap items-end gap-2"
			onsubmit={(event) => {
				event.preventDefault();
				linkError = !editor.link(href.trim());
				if (!linkError) linkOpen = false;
			}}
		>
			<label class="text-sm"
				>{$t('flow.shared.linkUrl')}<input
					class="block rounded border px-2 py-1"
					type="text"
					bind:value={href}
					disabled={!editable}
				/></label
			>
			<button type="submit" class="rounded border px-2 py-1 text-sm" disabled={!ready || !editable}
				>{$t('common.confirm')}</button
			>
			<button
				type="button"
				class="px-2 py-1 text-sm"
				disabled={!ready || !editable}
				onclick={() => {
					linkError = !editor.link(null);
					if (!linkError) linkOpen = false;
				}}>{$t('flow.shared.removeLink')}</button
			>
			{#if linkError}<p role="alert" class="w-full text-xs text-danger">
					{$t('flow.shared.linkError')}
				</p>{/if}
		</form>{/if}
	<div
		bind:this={host}
		class="shared-member-input rounded border border-line p-3 whitespace-pre-wrap focus-within:ring-2 focus-within:ring-focus"
	></div>
	{#if failed}<p role="alert" class="mt-2 text-sm text-danger">
			{$t('flow.shared.editorFailed')}
		</p>{:else if !ready}<p role="status" class="mt-2 text-sm text-fg-muted">
			{$t('common.loading')}
		</p>{/if}
	{#if snapshot.editing?.member.content.kind === 'todo'}<label class="mt-3 flex gap-2 text-sm"
			><input
				type="checkbox"
				checked={snapshot.editing.checked}
				disabled={!editable}
				onchange={(event) => session.setChecked(event.currentTarget.checked)}
			/>{$t('flow.shared.savedChecked')}</label
		>{/if}
	{#if snapshot.editing?.member.content.kind === 'toggle'}<label class="mt-3 flex gap-2 text-sm"
			><input
				type="checkbox"
				checked={snapshot.editing.collapsed}
				disabled={!editable}
				onchange={(event) => session.setCollapsed(event.currentTarget.checked)}
			/>{$t('flow.shared.savedCollapsed')}</label
		>{/if}
	<div class="mt-4 flex flex-wrap items-center gap-3 text-sm">
		<button
			type="button"
			class="rounded bg-primary rf-filled px-3 py-2 text-on-primary disabled:bg-surface disabled:text-control"
			data-testid="flow-shared-save"
			disabled={!ready ||
				!editable ||
				snapshot.loading ||
				snapshot.draft?.phase === 'conflict' ||
				!snapshot.draft?.commands.length}
			onclick={() => session.save()}
			>{snapshot.draft?.phase === 'saving' ? $t('flow.shared.saving') : $t('common.save')}</button
		>
		<button
			type="button"
			class="rounded border px-2 py-1"
			disabled={!ready || !editable}
			onclick={() => editor.undo()}>{$t('flow.shared.undoDraft')}</button
		>
		<button
			type="button"
			class="rounded border px-2 py-1"
			disabled={!ready || !editable}
			onclick={() => editor.redo()}>{$t('flow.shared.redoDraft')}</button
		>
		<button
			type="button"
			class="text-fg-muted underline"
			disabled={snapshot.draft?.phase === 'saving'}
			onclick={() => session.discard()}>{$t('flow.shared.discardDraft')}</button
		>
	</div>
</section>

<style>
	.shared-member-input :global(.ProseMirror) {
		min-height: 6rem;
		outline: none;
		white-space: pre-wrap;
	}
	.shared-member-input :global(.ProseMirror pre) {
		white-space: pre-wrap;
	}
</style>
