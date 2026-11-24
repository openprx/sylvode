<script lang="ts">
	/**
	 * Read-only value of one record field, by type. `variant="cell"` is a single line for the
	 * record table; `variant="panel"` wraps and clamps long content behind "Show more".
	 * Record content is always text: nothing here renders record data as HTML.
	 */
	import { Check, Minus, Paperclip, PenLine, Link as LinkIcon, FileText } from '@lucide/svelte';
	import { t } from 'svelte-i18n';
	import type { FormField } from '#lib/api/forms.js';
	import Avatar from '#lib/components/Avatar.svelte';
	import OptionValueBadges from './OptionValueBadges.svelte';
	import {
		fieldDisplay,
		type FieldDisplayContext,
		type ImageItem
	} from '#lib/forms/field-display.js';

	interface Props {
		field: FormField;
		value: unknown;
		variant: 'cell' | 'panel';
		ctx: FieldDisplayContext;
		/** Full record page URL for a relation target. */
		recordHref?: (recordId: string) => string;
		onopenrelation?: (recordId: string, sameForm: boolean, event: MouseEvent) => void;
		onpreviewimage?: (items: ImageItem[], index: number, trigger: HTMLElement) => void;
	}

	let { field, value, variant, ctx, recordHref, onopenrelation, onpreviewimage }: Props = $props();

	const display = $derived(fieldDisplay(field, value, ctx));
	const panel = $derived(variant === 'panel');
	const label = $derived(field.label?.trim() || field.key.replaceAll('_', ' '));

	let expanded = $state(false);
	let overflowing = $state(false);
	let clampBox = $state<HTMLElement | null>(null);

	$effect(() => {
		const box = clampBox;
		if (!box || typeof ResizeObserver === 'undefined') return;
		const measure = () => {
			if (!expanded) overflowing = box.scrollHeight > box.clientHeight + 1;
		};
		const observer = new ResizeObserver(measure);
		observer.observe(box);
		measure();
		return () => observer.disconnect();
	});

	function clampStyle(lines: number): string {
		return expanded
			? ''
			: `display:-webkit-box;-webkit-box-orient:vertical;-webkit-line-clamp:${lines};overflow:hidden;`;
	}

	function openRelation(event: MouseEvent, recordId: string, sameForm: boolean) {
		if (!onopenrelation) return;
		if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || event.button !== 0)
			return;
		onopenrelation(recordId, sameForm, event);
	}

	const stars = (count: number, max: number) =>
		Array.from({ length: max }, (_, index) => index < count);
</script>

<!-- eslint-disable svelte/no-navigation-without-resolve -- links here are record values (sanitised mailto/tel/http URLs, attachment downloads) or full record page paths built by the caller, not app routes to resolve -->

{#snippet empty()}
	<span class="text-fg-muted" data-field-value-empty
		><span aria-hidden="true">—</span><span class="sr-only">{$t('forms.recordPanel.empty')}</span
		></span
	>
{/snippet}

{#snippet clamped(text: string, lines: number, extraClass: string)}
	{#if panel}
		<div>
			<div
				bind:this={clampBox}
				class="break-words {extraClass}"
				style={clampStyle(lines)}
				data-field-value-text
			>
				{text}
			</div>
			{#if overflowing || expanded}
				<button
					type="button"
					class="mt-1 inline-flex min-h-6 items-center rounded-control text-xs font-medium text-fg hover:underline"
					aria-expanded={expanded}
					onclick={() => (expanded = !expanded)}
					>{expanded ? $t('forms.recordPanel.showLess') : $t('forms.recordPanel.showMore')}</button
				>
			{/if}
		</div>
	{:else}
		<span class="block truncate {extraClass.replace('whitespace-pre-line', '')}" title={text}
			>{text.split('\n')[0]}</span
		>
	{/if}
{/snippet}

{#if display.kind === 'empty'}
	{@render empty()}
{:else if display.kind === 'text'}
	{@render clamped(
		display.text,
		display.clampLines,
		`${display.multiline ? 'whitespace-pre-line' : ''} ${display.mono ? 'font-mono text-xs [unicode-bidi:isolate]' : ''}`
	)}
{:else if display.kind === 'json'}
	{@render clamped(display.text, 4, 'font-mono text-xs [unicode-bidi:isolate]')}
{:else if display.kind === 'number'}
	<span
		class="tabular-nums {panel ? '' : 'block truncate text-end'}"
		title={panel ? undefined : display.text}
		data-field-value-number>{display.text}</span
	>
{:else if display.kind === 'rating'}
	<span
		class="inline-flex items-center gap-0.5 text-warning {panel ? 'text-base' : 'text-xs'}"
		role="img"
		aria-label={$t('forms.fieldValue.ratingOf', {
			values: { value: display.value, max: display.max }
		})}
	>
		{#each stars(display.value, display.max) as filled, index (index)}
			<span aria-hidden="true" class={filled ? '' : 'text-fg-muted'}>{filled ? '★' : '☆'}</span>
		{/each}
	</span>
{:else if display.kind === 'progress'}
	<span
		class="inline-flex items-center gap-2"
		role="img"
		aria-label={$t('forms.fieldValue.percent', { values: { value: Math.round(display.value) } })}
	>
		<span
			class="relative h-1.5 overflow-hidden rounded-full bg-surface {panel ? 'w-30' : 'w-12'}"
			aria-hidden="true"
		>
			<span
				class="absolute inset-y-0 start-0 rounded-full bg-primary"
				style={`width:${display.value}%`}
			></span>
		</span>
		<span class="text-xs tabular-nums text-fg" aria-hidden="true">{display.text}</span>
	</span>
{:else if display.kind === 'date'}
	<time datetime={display.iso} class={panel ? '' : 'block truncate'} title={display.iso}
		>{display.text}</time
	>
{:else if display.kind === 'options'}
	{#if panel || field.type !== 'multi_select' || display.labels.length <= 2}
		<OptionValueBadges {field} value={display.value} />
	{:else}
		<span class="inline-flex max-w-full items-center gap-1">
			<OptionValueBadges {field} value={display.labels.slice(0, 2)} />
			<span class="text-xs text-fg-muted">+{display.labels.length - 2}</span>
		</span>
	{/if}
{:else if display.kind === 'boolean'}
	<span class="inline-flex items-center gap-1.5">
		{#if display.value}
			<Check class="h-4 w-4 text-success" aria-hidden="true" />
			<span>{$t('forms.fieldValue.yes')}</span>
		{:else}
			<Minus class="h-4 w-4 text-fg-muted" aria-hidden="true" />
			<span>{$t('forms.fieldValue.no')}</span>
		{/if}
	</span>
{:else if display.kind === 'member'}
	<span class="inline-flex min-w-0 max-w-full items-center gap-2">
		<Avatar name={display.name || undefined} />
		<span class="min-w-0">
			<span class="block truncate">
				{#if display.known}
					{display.name}
				{:else}
					{$t('forms.fieldValue.unknownMember')}
				{/if}
			</span>
			{#if panel && display.email && display.email !== display.name}
				<span class="block truncate text-xs text-fg-muted">{display.email}</span>
			{/if}
		</span>
	</span>
{:else if display.kind === 'link'}
	<a
		href={display.href}
		class="break-all text-fg underline-offset-2 hover:underline {panel ? '' : 'block truncate'}"
		dir={display.scheme === 'tel' ? 'ltr' : undefined}
		data-row-ignore>{display.text}</a
	>
{:else if display.kind === 'location'}
	<span class={panel ? 'block' : 'block truncate'} data-field-value-location>
		{#if display.label}<span class="me-1">{display.label}</span>{/if}
		{#if panel || !display.label}
			<span class="font-mono text-xs text-fg-muted" dir="ltr">{display.coordinates}</span>
		{/if}
	</span>
{:else if display.kind === 'images'}
	{#if panel}
		<span class="flex flex-wrap gap-2">
			{#each display.items as item, index (item.src + index)}
				<button
					type="button"
					class="h-18 w-18 overflow-hidden rounded-control border border-line bg-surface"
					aria-label={$t('forms.fieldValue.openImage', { values: { index: index + 1 } })}
					data-field-value-image={index}
					onclick={(event) => onpreviewimage?.(display.items, index, event.currentTarget)}
				>
					<img src={item.src} alt={item.alt} class="h-full w-full object-cover" loading="lazy" />
				</button>
			{/each}
			{#if display.blocked > 0}
				<span class="self-center text-xs text-warning">{$t('forms.fieldValue.linkBlocked')}</span>
			{/if}
		</span>
	{:else if display.items.length > 0}
		<span class="inline-flex items-center gap-1.5">
			<img
				src={display.items[0].src}
				alt={display.items[0].alt}
				class="h-6 w-6 rounded-control object-cover"
				loading="lazy"
			/>
			{#if display.items.length > 1}
				<span class="text-xs text-fg-muted">+{display.items.length - 1}</span>
			{/if}
		</span>
	{:else}
		<span class="text-xs text-warning">{$t('forms.fieldValue.linkBlocked')}</span>
	{/if}
{:else if display.kind === 'files'}
	{#if panel}
		<ul class="space-y-1.5">
			{#each display.items as item, index (item.name + index)}
				<li class="flex min-w-0 items-center gap-2" data-field-value-file={item.name}>
					{#if item.thumb && !item.blocked}
						<button
							type="button"
							class="h-8 w-8 shrink-0 overflow-hidden rounded-control border border-line"
							aria-label={$t('forms.fieldValue.openImage', { values: { index: index + 1 } })}
							onclick={(event) =>
								onpreviewimage?.(
									[{ src: item.thumb ?? '', alt: item.name, full: item.href }],
									0,
									event.currentTarget
								)}
						>
							<img src={item.thumb} alt="" class="h-full w-full object-cover" loading="lazy" />
						</button>
					{:else}
						<FileText class="h-4 w-4 shrink-0 text-fg-muted" aria-hidden="true" />
					{/if}
					<span class="min-w-0 flex-1">
						<span class="block truncate" title={item.name}>{item.name}</span>
						{#if item.typeLabel || item.sizeLabel}
							<span class="block text-xs text-fg-muted"
								>{[item.typeLabel, item.sizeLabel].filter(Boolean).join(' · ')}</span
							>
						{/if}
					</span>
					{#if item.blocked}
						<span class="text-xs text-warning" data-record-detail-attachment-value-blocked
							>{$t('forms.fieldValue.linkBlocked')}</span
						>
					{:else}
						<a
							href={item.href}
							target="_blank"
							rel="noopener noreferrer"
							class="inline-flex min-h-6 items-center rounded-control px-1 text-xs font-medium text-fg hover:underline"
							>{$t('forms.fieldValue.open')}</a
						>
						<a
							href={item.download}
							download={item.name}
							class="inline-flex min-h-6 items-center rounded-control px-1 text-xs font-medium text-fg hover:underline"
							>{$t('forms.fieldValue.download')}</a
						>
					{/if}
				</li>
			{/each}
		</ul>
	{:else}
		<span class="inline-flex items-center gap-1 text-fg-muted">
			<Paperclip class="h-3.5 w-3.5" aria-hidden="true" />
			<span class="text-xs tabular-nums">{display.items.length}</span>
			<span class="sr-only"
				>{$t('forms.fieldValue.files', { values: { count: display.items.length } })}</span
			>
		</span>
	{/if}
{:else if display.kind === 'signature'}
	{#if display.blocked}
		<span class="text-xs text-warning">{$t('forms.fieldValue.linkBlocked')}</span>
	{:else if panel}
		<button
			type="button"
			class="rounded-control border border-line bg-raised p-1"
			onclick={(event) =>
				onpreviewimage?.(
					[
						{
							src: display.src,
							alt: $t('forms.fieldValue.signatureAlt', { values: { field: label } })
						}
					],
					0,
					event.currentTarget
				)}
			data-field-value-signature
		>
			<img
				src={display.src}
				alt={$t('forms.fieldValue.signatureAlt', { values: { field: label } })}
				class="h-16 w-40 bg-raised object-contain"
			/>
		</button>
	{:else}
		<span
			class="inline-flex items-center gap-1 rounded-control bg-surface px-1.5 py-0.5 text-xs text-fg"
		>
			<PenLine class="h-3 w-3" aria-hidden="true" />
			{$t('forms.fieldValue.signed')}
		</span>
	{/if}
{:else if display.kind === 'relation'}
	{#if !display.available && !recordHref}
		<span class="text-fg-muted">{$t('forms.fieldValue.unavailableRecord')}</span>
	{:else}
		<a
			href={recordHref?.(display.recordId)}
			class="inline-flex min-h-6 max-w-full items-center gap-1 rounded-control border border-line px-2 py-0.5 text-sm text-fg hover:bg-surface"
			data-row-ignore
			data-field-value-relation={display.recordId}
			onclick={(event) => openRelation(event, display.recordId, display.sameForm)}
		>
			<LinkIcon class="h-3 w-3 shrink-0" aria-hidden="true" />
			<span class="truncate"
				>{display.available ? display.title : $t('forms.fieldValue.unavailableRecord')}</span
			>
		</a>
	{/if}
{:else if display.kind === 'children'}
	<span class="text-fg-muted"
		>{$t('forms.fieldValue.childRows', { values: { count: display.count } })}</span
	>
{/if}
