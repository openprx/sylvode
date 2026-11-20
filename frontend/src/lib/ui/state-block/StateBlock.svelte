<script lang="ts">
	import { t } from 'svelte-i18n';
	import Inbox from '@lucide/svelte/icons/inbox';
	import CircleAlert from '@lucide/svelte/icons/circle-alert';
	import Lock from '@lucide/svelte/icons/lock';
	import Button from '../button/Button.svelte';
	import { capitalised, stateCopy, type StateKind } from './state-block-state';

	interface Props {
		kind: StateKind;
		/** Plural object name, already translated, lower case ("projects"). */
		thing?: string;
		/** Who can grant access, for `forbidden` ("administrator"). */
		grantor?: string;
		/** Where access is missing, for `forbidden` ("Northwind workspace"). */
		place?: string;
		/** The one way forward. The label defaults per kind where the wording has one. */
		action?: { label?: string; run: () => void; primary?: boolean };
	}

	let { kind, thing = '', grantor = '', place = '', action }: Props = $props();

	const copy = $derived(stateCopy(kind));
	const values = $derived({
		thing,
		Thing: capitalised(thing),
		role: grantor,
		place
	});
	const actionLabel = $derived(
		action?.label ?? (copy.defaultActionKey ? $t(copy.defaultActionKey) : '')
	);
</script>

{#if kind === 'loading'}
	<div class="ui-state-visually-hidden" role="status" data-ui="state-block" data-kind={kind}>
		{$t(copy.titleKey, { values })}
	</div>
{:else}
	<div class="ui-state-block" role={copy.role} data-ui="state-block" data-kind={kind}>
		{#if kind === 'empty'}
			<Inbox size={24} class="glyph" aria-hidden="true" />
		{:else if kind === 'loadFailed'}
			<CircleAlert size={24} class="glyph" aria-hidden="true" />
		{:else}
			<Lock size={24} class="glyph" aria-hidden="true" />
		{/if}
		<p class="title">{$t(copy.titleKey, { values })}</p>
		{#if copy.bodyKey}<p class="body">{$t(copy.bodyKey, { values })}</p>{/if}
		{#if action && actionLabel}
			<div class="action">
				<Button variant={action.primary ? 'primary' : 'secondary'} onclick={() => action.run()}>
					{actionLabel}
				</Button>
			</div>
		{/if}
	</div>
{/if}

<style>
	.ui-state-block {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: var(--rf-space-4);
		padding: var(--rf-space-9) var(--rf-space-6);
		text-align: center;
		font-family: var(--rf-font-sans-stack);
	}
	.ui-state-block :global(.glyph) {
		color: var(--rf-secondary);
	}
	.title {
		margin: 0;
		font-size: var(--rf-size-body);
		line-height: var(--rf-leading-body-tight);
		font-weight: var(--rf-weight-semibold);
		color: var(--rf-text);
	}
	.body {
		margin: 0;
		max-width: 360px;
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
		color: var(--rf-secondary);
	}
	.action {
		margin-top: var(--rf-space-2);
	}
	/* Loading shows no block; only the status is announced (skeletons carry the visual). */
	.ui-state-visually-hidden {
		position: absolute;
		width: 1px;
		height: 1px;
		margin: -1px;
		padding: 0;
		overflow: hidden;
		clip: rect(0 0 0 0);
		white-space: nowrap;
		border: 0;
	}
</style>
