<script lang="ts">
	import { locale, t } from 'svelte-i18n';
	import Ban from '@lucide/svelte/icons/ban';
	import CircleAlert from '@lucide/svelte/icons/circle-alert';
	import Clock from '@lucide/svelte/icons/clock';
	import Inbox from '@lucide/svelte/icons/inbox';
	import Lock from '@lucide/svelte/icons/lock';
	import RefreshCw from '@lucide/svelte/icons/refresh-cw';
	import SearchX from '@lucide/svelte/icons/search-x';
	import WifiOff from '@lucide/svelte/icons/wifi-off';
	import '../foundations/visually-hidden.css';
	import Button from '../button/Button.svelte';
	import { plural } from '../foundations/plural';
	import { capitalised, stateCopy, type StateKind } from './state-block-state';

	/** Data attributes (`data-testid`, ...) are passed to the block element. */
	type DataAttributes = { [K in `data-${string}`]?: string | undefined };

	interface Props extends DataAttributes {
		kind: StateKind;
		/** Plural object name, already translated, lower case ("projects"). */
		thing?: string;
		/** Singular object name, already translated ("project"), for `notFound`. */
		object?: string;
		/** Who can grant access, for `forbidden` ("administrator"). */
		grantor?: string;
		/** Where access is missing (`forbidden`) or where the action leads (`notFound`: "Back to ..."). */
		place?: string;
		/** Number of applied filters, for `noResults`. Without it the block has no body. */
		count?: number;
		/** What was searched for, for `noSearchResults`. */
		query?: string;
		/** Replaces the heading of the kind, already translated (a state the catalogue has no
		 * wording for, such as a live connection that could not start). */
		title?: string;
		/** Replaces the body sentence of the kind (a feature that says more than the catalogue). */
		message?: string;
		/** The one way forward. The label defaults per kind where the wording has one. */
		action?: { label?: string; run: () => void; primary?: boolean };
	}

	let {
		kind,
		thing = '',
		object = '',
		grantor = '',
		place = '',
		count,
		query = '',
		title,
		message,
		action,
		...rest
	}: Props = $props();

	const copy = $derived(
		stateCopy(kind, {
			thing: !!thing,
			object: !!object,
			access: !!grantor && !!place,
			count: count !== undefined,
			place: !!place
		})
	);
	const values = $derived({
		thing,
		Thing: capitalised(thing),
		object,
		role: grantor,
		place,
		query
	});
	const body = $derived(
		message ??
			(copy.bodyKey
				? copy.bodyCounted
					? plural($t, copy.bodyKey, count ?? 0, $locale, values)
					: $t(copy.bodyKey, { values })
				: '')
	);
	const actionLabel = $derived(
		action?.label ?? (copy.defaultActionKey ? $t(copy.defaultActionKey, { values }) : '')
	);
</script>

{#if kind === 'loading'}
	<div {...rest} class="ui-sr-only" role="status" data-ui="state-block" data-kind={kind}>
		{$t(copy.titleKey, { values })}
	</div>
{:else}
	<div {...rest} class="ui-state-block" role={copy.role} data-ui="state-block" data-kind={kind}>
		{#if kind === 'empty'}
			<Inbox size={24} class="glyph" aria-hidden="true" />
		{:else if kind === 'noResults' || kind === 'noSearchResults'}
			<SearchX size={24} class="glyph" aria-hidden="true" />
		{:else if kind === 'offline'}
			<WifiOff size={24} class="glyph" aria-hidden="true" />
		{:else if kind === 'loadFailed'}
			<CircleAlert size={24} class="glyph" aria-hidden="true" />
		{:else if kind === 'notFound'}
			<Ban size={24} class="glyph" aria-hidden="true" />
		{:else if kind === 'expired'}
			<Clock size={24} class="glyph" aria-hidden="true" />
		{:else if kind === 'identityChanged'}
			<RefreshCw size={24} class="glyph" aria-hidden="true" />
		{:else}
			<Lock size={24} class="glyph" aria-hidden="true" />
		{/if}
		<p class="title">{title ?? $t(copy.titleKey, { values })}</p>
		{#if body}<p class="body">{body}</p>{/if}
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
</style>
