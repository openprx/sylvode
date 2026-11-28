<svelte:options runes />

<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { locale, t } from 'svelte-i18n';
	import ArrowLeft from '@lucide/svelte/icons/arrow-left';
	import BadgeCheck from '@lucide/svelte/icons/badge-check';
	import { describeResult } from '#lib/i18n/describe.js';
	import { toast } from '#lib/stores/toast.js';
	import Button from '#lib/ui/button/Button.svelte';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import { catalogueApi, type Risk } from './api';
	import ImportModal from './ImportModal.svelte';
	import IndexImage from './IndexImage.svelte';
	import { indexApi, type IndexPluginDetail, type IndexVersion } from './index-api';
	import {
		defaultVersion,
		descriptionHtml,
		fingerprint,
		formatSize,
		shortDate,
		trustPlan,
		type Role
	} from './index-logic';
	import { pickText, riskTone } from './logic';

	/**
	 * One plugin of a public index: what it is, its pictures, what each version asks for and
	 * carries, who publishes it and whether this workspace trusts the key. Reading is for every
	 * member; Import is offered to owners and administrators and opens the confirmation.
	 */
	let {
		workspaceId,
		role,
		source,
		pluginId,
		onback,
		onimported
	}: {
		workspaceId: string;
		role: Role;
		source: string;
		pluginId: string;
		onback: () => void;
		onimported: (digest: string) => void;
	} = $props();

	let detail = $state.raw<IndexPluginDetail | null>(null);
	let loading = $state(true);
	let failed = $state(false);
	let shown = $state<string | null>(null);
	let importing = $state<IndexVersion | null>(null);
	let importOpen = $state(false);
	let risks = $state.raw<Map<string, Risk> | null>(null);

	const lang = $derived($locale ?? 'en');
	const admin = $derived(role === 'owner' || role === 'admin');

	async function load() {
		loading = true;
		const result = await indexApi.plugin(workspaceId, source, pluginId);
		loading = false;
		failed = result.code !== 0 || !result.data;
		if (failed) {
			toast.error(describeResult(result, get(t)('market.index.detailFailed')));
			return;
		}
		detail = result.data;
		if (result.data && shown === null) shown = defaultVersion(result.data)?.version ?? null;
	}

	onMount(() => {
		void load();
		void catalogueApi.plugin().then((result) => {
			if (result.code === 0 && result.data) {
				risks = new Map(result.data.capabilities.map((entry) => [entry.id, entry.risk]));
			}
		});
	});

	const version = $derived(detail?.versions.find((entry) => entry.version === shown) ?? null);
	const latest = $derived(
		detail?.versions.find((entry) => entry.version === detail?.latest_compatible) ?? null
	);
	const title = $derived(detail ? pickText(detail.title, lang) || detail.id : '');
	const publisherName = $derived(
		detail ? pickText(detail.publisher.name, lang) || detail.publisher.id : ''
	);
	const html = $derived(detail ? descriptionHtml(pickText(detail.description, lang)) : '');

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

	/** Only an https address is shown as a link; anything else is plain text. */
	function safeLink(url: string | null | undefined): string | null {
		return url && /^https:\/\/[^\s]+$/i.test(url) ? url : null;
	}

	function openImport(entry: IndexVersion) {
		importing = entry;
		importOpen = true;
	}

	function importableNow(entry: IndexVersion): boolean {
		return entry.status === 'listed' && entry.compatible;
	}

	function versionNote(entry: IndexVersion): string {
		if (entry.status !== 'listed') return lookup('market.index.versionStatus', entry.status);
		if (!entry.compatible)
			return $t('market.index.needsSylvode', { values: { range: entry.sylvode } });
		return '';
	}
</script>

<div class="space-y-6" data-market-index-detail={pluginId}>
	<div>
		<Button variant="text" size="sm" onclick={onback} data-market-index-back>
			<span class="back"><ArrowLeft size={16} aria-hidden="true" />{$t('market.index.back')}</span>
		</Button>
	</div>

	{#if loading && !detail}
		<StateBlock kind="loading" />
	{:else if failed || !detail}
		<StateBlock
			kind="loadFailed"
			thing={$t('market.index.thing')}
			action={{ run: () => void load() }}
		/>
	{:else}
		<header class="top">
			<IndexImage {workspaceId} {source} {pluginId} path={detail.icon} alt="" variant="icon" />
			<div class="titles">
				<h2>{title}</h2>
				<p class="by">
					{publisherName}
					<span class="mono">{detail.publisher.id}</span>
					{#if detail.publisher.trusted}
						<span class="trusted"
							><BadgeCheck size={14} aria-hidden="true" />{$t(
								'market.index.trustedPublisher'
							)}</span
						>
					{/if}
				</p>
				<p class="summary">{pickText(detail.summary, lang)}</p>
				<div class="chips">
					<StatusChip>{lookup('market.kindOne', detail.kind)}</StatusChip>
					{#if detail.metadata.category}
						<StatusChip>{lookup('market.index.category', detail.metadata.category)}</StatusChip>
					{/if}
					{#each detail.metadata.labels as labelId (labelId)}
						<StatusChip>{lookup('market.index.label', labelId)}</StatusChip>
					{/each}
					{#if detail.installed}
						<StatusChip tone="success">{$t('market.installedBadge')}</StatusChip>
					{:else if detail.imported}
						<StatusChip tone="success">{$t('market.index.imported')}</StatusChip>
					{/if}
				</div>
			</div>
			<div class="cta">
				{#if latest}
					{#if latest.imported_digest}
						<Button
							variant="primary"
							onclick={() => onimported(latest.imported_digest ?? '')}
							data-market-index-open-imported>{$t('market.index.openImported')}</Button
						>
					{:else if admin}
						<Button variant="primary" onclick={() => openImport(latest)} data-market-index-import
							>{$t('market.index.importVersion', { values: { version: latest.version } })}</Button
						>
					{:else}
						<span class="note" data-market-index-member-note>{$t('market.index.memberNote')}</span>
					{/if}
				{:else}
					<span class="note">{$t('market.index.noCompatibleNote')}</span>
				{/if}
			</div>
		</header>

		<dl class="facts">
			<div>
				<dt>{$t('market.index.source')}</dt>
				<dd>{detail.source} <span class="mono">{detail.source_url}</span></dd>
			</div>
			{#if detail.license}
				<div>
					<dt>{$t('market.index.license')}</dt>
					<dd>{detail.license}</dd>
				</div>
			{/if}
			{#if safeLink(detail.homepage)}
				<div>
					<dt>{$t('market.index.homepage')}</dt>
					<dd>
						<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -- external https address -->
						<a href={safeLink(detail.homepage)} target="_blank" rel="noopener noreferrer"
							>{detail.homepage}</a
						>
					</dd>
				</div>
			{/if}
			{#if safeLink(detail.repository)}
				<div>
					<dt>{$t('market.index.repository')}</dt>
					<dd>
						<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -- external https address -->
						<a href={safeLink(detail.repository)} target="_blank" rel="noopener noreferrer"
							>{detail.repository}</a
						>
					</dd>
				</div>
			{/if}
		</dl>

		{#if detail.screenshots.length > 0}
			<section aria-label={$t('market.index.screenshots')}>
				<h3>{$t('market.index.screenshots')}</h3>
				<ul class="shots">
					{#each detail.screenshots as shot (shot.path)}
						<li>
							<figure>
								<IndexImage
									{workspaceId}
									{source}
									{pluginId}
									path={shot.path}
									alt={pickText(shot.caption, lang)}
									variant="screenshot"
									fit="contain"
								/>
								<figcaption>{pickText(shot.caption, lang)}</figcaption>
							</figure>
						</li>
					{/each}
				</ul>
			</section>
		{/if}

		{#if html}
			<section aria-label={$t('market.index.description')}>
				<h3>{$t('market.index.description')}</h3>
				<div class="desc" data-market-index-description>
					<!-- eslint-disable-next-line svelte/no-at-html-tags -- descriptionHtml escapes raw HTML and drops images and non-https links (index-logic.ts) -->
					{@html html}
				</div>
			</section>
		{/if}

		<section aria-label={$t('market.index.versions')}>
			<h3>{$t('market.index.versions')}</h3>
			<div class="table-wrap">
				<table data-market-index-versions>
					<thead>
						<tr>
							<th>{$t('market.colVersion')}</th>
							<th>{$t('market.colStatus')}</th>
							<th>{$t('market.index.published')}</th>
							<th>{$t('market.index.size')}</th>
							<th></th>
						</tr>
					</thead>
					<tbody>
						{#each detail.versions as entry (entry.version)}
							<tr class:current={entry.version === shown} data-market-index-version={entry.version}>
								<td class="mono">{entry.version}</td>
								<td>
									<span class="chips">
										{#if entry.status === 'listed'}
											<StatusChip tone={entry.compatible ? 'success' : 'warning'}
												>{entry.compatible
													? $t('market.index.compatible')
													: $t('market.index.incompatible')}</StatusChip
											>
										{:else}
											<StatusChip tone={entry.status === 'revoked' ? 'danger' : 'warning'}
												>{lookup('market.index.versionStatus', entry.status)}</StatusChip
											>
										{/if}
										{#if entry.imported_digest}
											<StatusChip tone="success">{$t('market.index.imported')}</StatusChip>
										{/if}
									</span>
									{#if versionNote(entry)}
										<div class="meta">{versionNote(entry)}</div>
									{/if}
									{#if entry.status_reason}
										<div class="meta">{entry.status_reason}</div>
									{/if}
								</td>
								<td class="meta">{shortDate(entry.published_at, lang, false)}</td>
								<td class="meta">{formatSize(entry.size, lang)}</td>
								<td class="actions">
									<Button
										variant="text"
										size="sm"
										aria-pressed={entry.version === shown}
										onclick={() => (shown = entry.version)}
										>{$t('market.index.showContents')}</Button
									>
									{#if entry.imported_digest}
										<Button
											variant="secondary"
											size="sm"
											onclick={() => onimported(entry.imported_digest ?? '')}
											>{$t('market.index.openImported')}</Button
										>
									{:else if admin && importableNow(entry)}
										<Button variant="secondary" size="sm" onclick={() => openImport(entry)}
											>{$t('market.index.import')}</Button
										>
									{/if}
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		</section>

		{#if version}
			<section aria-label={$t('market.contents')} data-market-index-contents>
				<h3>{$t('market.index.contentsOf', { values: { version: version.version } })}</h3>
				{#if version.changelog}
					<p class="changelog">{pickText(version.changelog, lang)}</p>
				{/if}
				<div class="contents">
					<div>
						<h4>{$t('market.asksDetail')}</h4>
						{#if version.contents.requests.length === 0}
							<p class="meta">{$t('market.asksNothing')}</p>
						{:else}
							<ul class="plain">
								{#each version.contents.requests as request (request)}
									<li>
										<span>{words(request)}</span>
										{#if risks?.get(request)}
											{@const risk = risks.get(request) as Risk}
											<StatusChip tone={riskTone(risk)}>{$t(`market.risk.${risk}`)}</StatusChip>
										{/if}
									</li>
								{/each}
							</ul>
						{/if}
					</div>
					<div>
						<h4>{$t('market.contents')}</h4>
						<ul class="plain">
							<li>
								<span class:warn={version.contents.custom_code}
									>{version.contents.custom_code
										? $t('market.customCode')
										: $t('market.customCodeNo')}</span
								>
							</li>
							{#if version.contents.forms.length}
								<li>
									{$t('market.contentsForms', {
										values: { list: version.contents.forms.join(', ') }
									})}
								</li>
							{/if}
							{#if version.contents.layouts.length}
								<li>
									{$t('market.contentsLayouts', {
										values: {
											list: version.contents.layouts
												.map((layout) =>
													layout.slots.length
														? $t('market.slotsOf', {
																values: { key: layout.key, slots: layout.slots.join(', ') }
															})
														: layout.key
												)
												.join(', ')
										}
									})}
								</li>
							{/if}
							{#if version.contents.actions.length}
								<li>
									{$t('market.contentsActions', {
										values: { list: version.contents.actions.join(', ') }
									})}
								</li>
							{/if}
							{#if version.contents.rules.length}
								<li>
									{$t('market.contentsRules', {
										values: { list: version.contents.rules.join(', ') }
									})}
								</li>
							{/if}
							{#if version.contents.project_block}
								<li>{$t('market.index.projectBlock')}</li>
							{/if}
						</ul>
						{#if version.contents.pages.length}
							<h4>{$t('market.sitePages')}</h4>
							<ul class="plain">
								{#each version.contents.pages as page (page.route)}
									<li>
										<span class="mono">{page.route}</span>
										<StatusChip tone={page.audience === 'visitor' ? 'neutral' : 'warning'}
											>{lookup('market.pageAudience', page.audience)}</StatusChip
										>
									</li>
								{/each}
							</ul>
						{/if}
					</div>
				</div>
				<p class="meta">{$t('market.index.contentsNote')}</p>
			</section>
		{/if}

		<section aria-label={$t('market.index.publisherKeys')} data-market-index-keys>
			<h3>{$t('market.index.publisherKeys')}</h3>
			<ul class="keys">
				{#each detail.publisher.keys as key (key.key_id)}
					<li>
						<span class="mono"
							>{fingerprint(key.key_id).algorithm} {fingerprint(key.key_id).groups}</span
						>
						<span class="chips">
							{#if key.status === 'revoked'}
								<StatusChip tone="danger">{$t('market.index.keyRevoked')}</StatusChip>
							{/if}
							{#if key.pinned}
								<StatusChip tone="success">{$t('market.index.keyPinned')}</StatusChip>
							{:else}
								<StatusChip>{$t('market.index.keyNotPinned')}</StatusChip>
							{/if}
						</span>
						{#if key.reason}<span class="meta">{key.reason}</span>{/if}
					</li>
				{/each}
			</ul>
			<p class="meta">{$t('market.index.keysNote')}</p>
		</section>
	{/if}
</div>

{#if detail && importing}
	<ImportModal
		bind:open={importOpen}
		{workspaceId}
		{role}
		{detail}
		version={importing}
		plan={trustPlan(detail, importing, role)}
		onclose={() => (importing = null)}
		ondone={(digest) => {
			importOpen = false;
			importing = null;
			onimported(digest);
		}}
		onrefresh={() => void load()}
	/>
{/if}

<style>
	h2 {
		margin: 0;
		font-size: var(--rf-size-title, 1.5rem);
		font-weight: var(--rf-weight-semibold);
		text-wrap: balance;
	}
	h3 {
		margin: 0 0 var(--rf-space-5);
		font-size: var(--rf-size-body);
		font-weight: var(--rf-weight-semibold);
	}
	h4 {
		margin: 0 0 var(--rf-space-3);
		font-size: var(--rf-size-secondary);
		font-weight: var(--rf-weight-medium);
	}
	.back {
		display: inline-flex;
		align-items: center;
		gap: var(--rf-space-3);
	}
	.top {
		display: grid;
		grid-template-columns: auto minmax(0, 1fr) auto;
		gap: var(--rf-space-6);
		align-items: start;
	}
	.top :global(.index-image.v-icon) {
		width: 72px;
		height: 72px;
	}
	.titles {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-3);
		min-width: 0;
	}
	.by {
		margin: 0;
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--rf-space-3);
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
	.trusted {
		display: inline-flex;
		align-items: center;
		gap: var(--rf-space-2);
		color: var(--rf-success);
	}
	.summary {
		margin: 0;
		max-width: 70ch;
		font-size: var(--rf-size-body);
	}
	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: var(--rf-space-3);
	}
	.cta {
		display: flex;
		align-items: flex-start;
	}
	.note,
	.meta {
		font-size: var(--rf-size-meta);
		color: var(--rf-secondary);
	}
	.note {
		max-width: 28ch;
	}
	.mono {
		font-family: var(--rf-font-mono-stack, ui-monospace, monospace);
		font-size: var(--rf-size-meta);
		overflow-wrap: anywhere;
	}
	.facts {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(14rem, 1fr));
		gap: var(--rf-space-5);
		margin: 0;
		font-size: var(--rf-size-secondary);
	}
	.facts dt {
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
	}
	.facts dd {
		margin: 0;
		overflow-wrap: anywhere;
	}
	.shots {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(16rem, 1fr));
		gap: var(--rf-space-5);
		margin: 0;
		padding: 0;
		list-style: none;
	}
	figure {
		margin: 0;
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-3);
	}
	figcaption {
		font-size: var(--rf-size-meta);
		color: var(--rf-secondary);
	}
	.desc {
		max-width: 72ch;
		font-size: var(--rf-size-body);
		line-height: 1.55;
	}
	.desc :global(p) {
		margin: 0 0 var(--rf-space-5);
	}
	.desc :global(h1),
	.desc :global(h2),
	.desc :global(h3) {
		margin: var(--rf-space-7, 1.5rem) 0 var(--rf-space-4);
		font-weight: var(--rf-weight-semibold);
	}
	.desc :global(ul) {
		margin: 0 0 var(--rf-space-5);
		padding-left: 1.25rem;
		list-style: disc;
	}
	.desc :global(code) {
		font-family: var(--rf-font-mono-stack, ui-monospace, monospace);
		background: var(--rf-surface);
		padding: 0 var(--rf-space-2);
		border-radius: var(--rf-radius-xs);
	}
	.desc :global(pre) {
		overflow-x: auto;
		background: var(--rf-surface);
		padding: var(--rf-space-5);
		border-radius: var(--rf-radius-card);
	}
	.desc :global(a) {
		color: var(--rf-primary);
		text-decoration: underline;
		text-underline-offset: 2px;
	}
	.table-wrap {
		overflow-x: auto;
		border: var(--rf-border-width) solid var(--rf-line);
		border-radius: var(--rf-radius-card);
	}
	table {
		width: 100%;
		border-collapse: collapse;
		font-size: var(--rf-size-secondary);
	}
	th {
		text-align: left;
		font-weight: var(--rf-weight-medium);
		font-size: var(--rf-size-meta);
		color: var(--rf-secondary);
		padding: var(--rf-space-3) var(--rf-space-5);
	}
	td {
		padding: var(--rf-space-4) var(--rf-space-5);
		border-top: var(--rf-border-width) solid var(--rf-line);
		vertical-align: top;
	}
	tr.current td {
		background: var(--rf-selected);
	}
	.actions {
		display: flex;
		flex-wrap: wrap;
		justify-content: flex-end;
		gap: var(--rf-space-3);
	}
	.changelog {
		margin: 0 0 var(--rf-space-5);
		max-width: 70ch;
		white-space: pre-wrap;
		font-size: var(--rf-size-secondary);
	}
	.contents {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(18rem, 1fr));
		gap: var(--rf-space-6);
		margin-bottom: var(--rf-space-5);
	}
	.plain {
		margin: 0 0 var(--rf-space-5);
		padding: 0;
		list-style: none;
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-3);
		font-size: var(--rf-size-secondary);
	}
	.plain li {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--rf-space-3);
	}
	.warn {
		color: var(--rf-warning);
	}
	.keys {
		margin: 0 0 var(--rf-space-4);
		padding: 0;
		list-style: none;
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
	}
	.keys li {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--rf-space-4);
	}
	@media (max-width: 720px) {
		.top {
			grid-template-columns: auto minmax(0, 1fr);
		}
		.cta {
			grid-column: 1 / -1;
		}
	}
</style>
