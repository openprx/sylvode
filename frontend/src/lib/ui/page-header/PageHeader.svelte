<script lang="ts">
	import { t } from 'svelte-i18n';
	import '../foundations/visually-hidden.css';
	import type { Snippet } from 'svelte';
	import Ellipsis from '@lucide/svelte/icons/ellipsis';
	import Plus from '@lucide/svelte/icons/plus';
	import Button from '../button/Button.svelte';
	import IconButton from '../icon-button/IconButton.svelte';
	import Menu from '../menu/Menu.svelte';
	import MenuItem from '../menu/MenuItem.svelte';
	import ContextLine from '../context-line/ContextLine.svelte';
	import { isTruncated, type ContextSegment } from '../context-line/context-line';
	import { tooltip } from '../tooltip/tooltip';
	import { uid } from '../foundations/ids';
	import { headerLayout, type SurfaceAction } from './page-header';

	interface Props {
		title: string;
		/** 88 (standard, pages) or 56 (compact, data surfaces). */
		variant?: 'standard' | 'compact';
		context?: { segments: ContextSegment[]; current: string };
		loading?: boolean;
		primary?: SurfaceAction;
		secondary?: SurfaceAction[];
		titleId?: string;
		/** Width the header lays out for; measured from the element when absent. */
		width?: number;
		/** Rare custom tools, drawn before the buttons. */
		children?: Snippet;
	}

	let {
		title,
		variant = 'standard',
		context,
		loading = false,
		primary,
		secondary = [],
		titleId,
		width,
		children
	}: Props = $props();

	const ownId = uid('page-title');
	const contextId = uid('page-context');
	const heading = $derived(titleId ?? ownId);
	let measured = $state(0);
	let titleEl = $state<HTMLElement | null>(null);
	let menuAnchor = $state<HTMLElement | null>(null);
	let menuOpen = $state(false);
	let titleCut = $state(false);

	const effective = $derived(width ?? (measured > 0 ? measured : 1000));
	const layout = $derived(headerLayout(effective, secondary.length));
	const inline = $derived(secondary.slice(0, layout.inlineSecondary));
	const overflowed = $derived(secondary.slice(layout.inlineSecondary));
	const buttonVariant = (action: SurfaceAction) =>
		action.kind === 'danger-quiet' ? 'danger-quiet' : 'secondary';

	/** The title takes focus after client navigation; it is not a Tab stop. */
	export function focusTitle(): void {
		titleEl?.focus();
	}
</script>

<header
	class="ui-page-header v-{variant}"
	bind:clientWidth={measured}
	data-ui="page-header"
	data-variant={variant}
>
	<div class="lead">
		{#if loading}
			<span class="skeleton title-skeleton" role="status" aria-label={$t('ui.pageHeader.loading')}
			></span>
			<span class="skeleton context-skeleton"></span>
		{:else}
			<h1
				bind:this={titleEl}
				id={heading}
				class="title"
				tabindex="-1"
				aria-describedby={context ? contextId : undefined}
				use:tooltip={{ text: titleCut ? title : '' }}
				onpointerenter={() => titleEl && (titleCut = isTruncated(titleEl))}
			>
				{title}
			</h1>
			{#if context}
				<div class="context" class:ui-sr-only={!layout.contextVisible}>
					<ContextLine
						segments={context.segments}
						current={context.current}
						descriptionId={contextId}
					/>
				</div>
			{/if}
		{/if}
	</div>

	{#if !loading}
		<div class="actions">
			{#if children}{@render children()}{/if}
			{#each inline as action (action.id)}
				{@const reasonId = `${ownId}-${action.id}-reason`}
				<span class="action">
					<Button
						variant={buttonVariant(action)}
						disabled={!!action.disabledReason}
						aria-describedby={action.disabledReason ? reasonId : undefined}
						onclick={() => void action.run()}>{action.label}</Button
					>
					{#if action.disabledReason}<span class="reason" id={reasonId}
							>{action.disabledReason}</span
						>{/if}
				</span>
			{/each}
			{#if overflowed.length > 0}
				<span bind:this={menuAnchor} class="anchor">
					<IconButton
						icon={Ellipsis}
						glyph={20}
						label={$t('ui.pageHeader.more')}
						aria-haspopup="menu"
						aria-expanded={menuOpen}
						onclick={() => (menuOpen = !menuOpen)}
					/>
				</span>
			{/if}
			{#if primary}
				{@const reasonId = `${ownId}-${primary.id}-reason`}
				<span class="action">
					{#if layout.primaryAsIcon}
						<IconButton
							icon={primary.icon ?? Plus}
							tone="text"
							label={primary.label}
							disabled={!!primary.disabledReason}
							aria-describedby={primary.disabledReason ? reasonId : undefined}
							onclick={() => void primary.run()}
						/>
						{#if primary.disabledReason}<span class="ui-sr-only" id={reasonId}
								>{primary.disabledReason}</span
							>{/if}
					{:else}
						<Button
							variant="primary"
							disabled={!!primary.disabledReason}
							aria-describedby={primary.disabledReason ? reasonId : undefined}
							onclick={() => void primary.run()}>{primary.label}</Button
						>
						{#if primary.disabledReason}<span class="reason" id={reasonId}
								>{primary.disabledReason}</span
							>{/if}
					{/if}
				</span>
			{/if}
		</div>
	{/if}
</header>

<Menu
	open={menuOpen}
	anchor={menuAnchor}
	onclose={() => (menuOpen = false)}
	label={$t('ui.pageHeader.more')}
	placement="bottom-end"
>
	{#each overflowed as action (action.id)}
		<MenuItem
			compact
			destructive={action.kind === 'danger-quiet'}
			disabled={!!action.disabledReason}
			onselect={() => void action.run()}
		>
			{action.label}{#if action.disabledReason}<span class="menu-reason"
					>{action.disabledReason}</span
				>{/if}
		</MenuItem>
	{/each}
</Menu>

<style>
	.ui-page-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--rf-space-6);
		box-sizing: border-box;
		min-width: 0;
		border-bottom: var(--rf-border-width) solid var(--rf-line);
		background: var(--rf-bg);
		color: var(--rf-text);
		font-family: var(--rf-font-sans-stack);
	}
	.v-standard {
		height: var(--rf-layout-header);
		padding: 0 var(--rf-space-9);
	}
	.v-compact {
		height: var(--rf-layout-header-compact);
		padding: var(--rf-space-5) var(--rf-space-8);
	}
	.lead {
		display: flex;
		min-width: 0;
		flex: 1 1 auto;
	}
	.v-standard .lead {
		flex-direction: column;
		justify-content: center;
		gap: var(--rf-space-1);
	}
	.v-compact .lead {
		flex-direction: row;
		align-items: baseline;
		gap: var(--rf-space-5);
	}
	.title {
		margin: 0;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: var(--rf-size-subheading);
		line-height: 24px;
		font-weight: var(--rf-weight-semibold);
	}
	.v-compact .title {
		max-width: 60%;
		flex: 0 1 auto;
	}
	.title:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: 2px;
		border-radius: 2px;
	}
	.context {
		min-width: 0;
	}
	.v-compact .context {
		flex: 1 1 0;
	}
	.actions {
		display: flex;
		align-items: center;
		flex: none;
		gap: var(--rf-space-4);
	}
	.action {
		display: inline-flex;
		align-items: center;
		gap: var(--rf-space-4);
	}
	.anchor {
		display: inline-flex;
	}
	.reason,
	.menu-reason {
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
		line-height: var(--rf-leading-meta);
	}
	.menu-reason {
		display: block;
	}
	.skeleton {
		display: block;
		border-radius: var(--rf-radius-xs);
		background: var(--rf-selected);
	}
	.title-skeleton {
		width: 140px;
		height: 16px;
	}
	.context-skeleton {
		width: 180px;
		height: 12px;
	}
</style>
