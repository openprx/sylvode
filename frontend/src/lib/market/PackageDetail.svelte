<svelte:options runes />

<script lang="ts">
	import { get } from 'svelte/store';
	import { locale, t } from 'svelte-i18n';
	import Button from '#lib/ui/button/Button.svelte';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import type { PackageDetail, Risk } from './api';
	import { riskTone } from './logic';

	/**
	 * The detail of one package: manifest summary, contents, the capabilities it asks for in words
	 * and the forms it binds. Read-only; the install button is shown to administrators only (the
	 * server refuses everybody else anyway).
	 */
	let {
		detail,
		canInstall,
		oninstall
	}: { detail: PackageDetail; canInstall: boolean; oninstall: () => void } = $props();

	function words(capability: string): string {
		const key = `market.cap.${capability}`;
		const text = get(t)(key);
		return text === key ? capability : text;
	}

	function lookup(prefix: string, value: string): string {
		const key = `${prefix}.${value}`;
		const text = get(t)(key);
		return text === key ? value : text;
	}

	const names = (values: string[]) => values.join(', ');
	const forms = $derived(detail.analysis.forms.map((form) => form.bind));
	const layouts = $derived(
		detail.analysis.layouts.map((layout) =>
			layout.slots.length
				? $t('market.slotsOf', { values: { key: layout.key, slots: names(layout.slots) } })
				: layout.key
		)
	);
	const pages = $derived(detail.analysis.pages);
	const pageLabel = (nav: Record<string, string> | null): string => {
		if (!nav) return '';
		const lang = ($locale ?? 'en').split('-')[0] ?? 'en';
		return nav[lang] ?? nav.en ?? Object.values(nav)[0] ?? '';
	};
	const actions = $derived(detail.analysis.actions.map((action) => action.name));
	const rules = $derived(detail.analysis.rules.map((rule) => rule.name));
	const summary = $derived(
		detail.summary?.[$locale ?? 'en'] ??
			detail.summary?.[($locale ?? 'en').split('-')[0] ?? 'en'] ??
			detail.summary?.en ??
			''
	);
</script>

<div class="space-y-4" data-market-detail={detail.package_key}>
	{#if summary}
		<p class="text-sm text-fg">{summary}</p>
	{/if}

	<div class="flex flex-wrap items-center gap-2 text-sm text-fg-muted" data-market-trust>
		<StatusChip>{lookup('market.kindOne', detail.kind)}</StatusChip>
		<span>{$t('market.version', { values: { version: detail.version } })}</span>
		<StatusChip>{lookup('market.tier', detail.tier)}</StatusChip>
		{#if detail.publisher}
			<span>{$t('market.publisher', { values: { name: detail.publisher.id } })}</span>
		{/if}
		<span class={detail.custom_code ? 'text-warning' : ''}>
			{detail.custom_code ? $t('market.customCode') : $t('market.customCodeNo')}
		</span>
	</div>

	<div class="text-sm text-fg">
		<div class="font-medium">{$t('market.contents')}</div>
		<ul class="mt-1 space-y-1 text-fg-muted">
			{#if forms.length}<li>
					{$t('market.contentsForms', { values: { list: names(forms) } })}
				</li>{/if}
			{#if layouts.length}<li>
					{$t('market.contentsLayouts', { values: { list: names(layouts) } })}
				</li>{/if}
			{#if actions.length}<li>
					{$t('market.contentsActions', { values: { list: names(actions) } })}
				</li>{/if}
			{#if rules.length}<li>
					{$t('market.contentsRules', { values: { list: names(rules) } })}
				</li>{/if}
			{#if !forms.length && !layouts.length && !pages.length && !actions.length && !rules.length}
				<li>{$t('market.contentsNothing')}</li>
			{/if}
		</ul>
	</div>

	{#if pages.length}
		<div class="text-sm text-fg" data-market-site-pages>
			<div class="font-medium">{$t('market.sitePages')}</div>
			<ul class="mt-2 space-y-1">
				{#each pages as page (page.route)}
					<li class="flex flex-wrap items-center gap-2" data-market-site-page={page.route}>
						<span class="text-fg">{page.route}</span>
						<StatusChip tone={page.audience === 'visitor' ? 'neutral' : 'warning'}>
							{$t(`market.pageAudience.${page.audience}`)}
						</StatusChip>
						<span class="text-fg-muted">
							{pageLabel(page.nav) || $t('market.pageNoMenu')}
						</span>
					</li>
				{/each}
			</ul>
			<p class="mt-2 text-xs text-fg-muted">{$t('market.sitePagesNote')}</p>
		</div>
	{/if}

	<div class="text-sm text-fg" data-market-requests>
		<div class="font-medium">{$t('market.asksDetail')}</div>
		{#if detail.analysis.requests.length === 0}
			<p class="mt-1 text-fg-muted">{$t('market.asksNothing')}</p>
		{:else}
			<ul class="mt-2 space-y-2">
				{#each detail.analysis.requests as request, index (index)}
					<li class="rounded-card border border-line p-3">
						<div class="text-fg">{words(request.id)}</div>
						<div class="mt-1 flex flex-wrap items-center gap-2 text-xs text-fg-muted">
							{#if request.risk}
								<StatusChip tone={riskTone(request.risk as Risk)}
									>{$t(`market.risk.${request.risk}`)}</StatusChip
								>
							{/if}
							{#if request.form_hint}
								<span>{$t('market.onForm', { values: { form: request.form_hint } })}</span>
							{/if}
						</div>
					</li>
				{/each}
			</ul>
		{/if}
	</div>

	<div class="flex items-center justify-end gap-3">
		{#if canInstall}
			<Button variant="primary" onclick={oninstall} data-market-install
				>{$t('market.install')}</Button
			>
		{:else}
			<span class="text-xs text-fg-muted" data-market-member-note>{$t('market.memberNote')}</span>
		{/if}
	</div>
</div>
