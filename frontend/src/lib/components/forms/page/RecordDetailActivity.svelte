<svelte:options runes />

<script lang="ts">
	import { Link, FileText, FileDown } from '@lucide/svelte';
	import Button from '#lib/components/Button.svelte';
	import { t } from 'svelte-i18n';
	import { formatDate } from '#lib/forms/record-format.js';
	import { fieldLabel } from '#lib/forms/field-options.js';
	import {
		attachmentIsPreviewableImage,
		attachmentThumbnailUrl
	} from '#lib/forms/attachment-files.js';
	import { safeLinkUrl } from '#lib/utils/safe-url.js';
	import {
		timelineEventTitle,
		timelineEventAggregateKey,
		timelineEventJson
	} from '#lib/forms/views/timeline-events.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const {
		handleCreateLink,
		handleCreateRecordComment,
		selectedRecordAttachmentFields,
		selectedRecordAttachmentValueUrls
	} = fp.recordDetail;
	const { fieldAttachments, attachmentSignedUrl } = fp.attachments;
</script>

{#if s.selectedRecord}
	<div class="mb-3 flex items-center gap-2">
		<Link class="h-4 w-4 text-fg-muted" aria-hidden="true" />

		<h4 class="text-sm font-semibold text-fg">
			{$t('forms.recordLinks')}
		</h4>
	</div>

	<div class="space-y-2">
		{#each s.selectedRecordLinks as item}
			<div class="rounded-card border border-line px-3 py-2 text-xs">
				<div class="font-medium text-fg">
					{item.relation_key} / {item.relation_type}
				</div>

				<div class="mt-1 truncate font-mono text-fg-muted">
					{item.target_type}:{item.target_id}
				</div>
			</div>
		{/each}

		{#if s.selectedRecordLinks.length === 0}
			<p class="text-sm text-fg-muted">
				{$t('forms.noLinkedData')}
			</p>
		{/if}
	</div>

	<div class="mt-3 space-y-2">
		<input
			class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
			bind:value={s.linkDraft.target_id}
			placeholder={$t('forms.targetPlaceholder')}
		/>

		<div class="grid grid-cols-2 gap-2">
			<input
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				bind:value={s.linkDraft.relation_key}
				placeholder={$t('forms.relationKeyPlaceholder')}
			/>

			<input
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				bind:value={s.linkDraft.relation_type}
				placeholder={$t('forms.relationTypePlaceholder')}
			/>
		</div>

		<Button
			size="sm"
			variant="secondary"
			loading={s.linkSubmitting}
			disabled={!s.linkDraft.target_id.trim()}
			onclick={handleCreateLink}
		>
			<Link class="mr-2 h-4 w-4" aria-hidden="true" />
			{$t('forms.addLink')}
		</Button>
	</div>

	<div
		class="mt-4 border-t border-line pt-4"
		data-record-detail-comments={s.selectedRecord.id}
		data-record-detail-comment-count={s.selectedRecordComments.length}
	>
		<div class="mb-3 flex items-center gap-2">
			<FileText class="h-4 w-4 text-fg-muted" aria-hidden="true" />

			<h4 class="text-sm font-semibold text-fg">
				{$t('forms.recordComments')}
			</h4>
		</div>

		<div class="space-y-2">
			{#each s.selectedRecordComments.slice(0, 5) as comment}
				<div
					class="rounded-card border border-line px-3 py-2 text-xs"
					data-record-detail-comment={comment.id}
				>
					<div class="flex flex-wrap items-center justify-between gap-2">
						<span class="font-medium text-fg" data-record-detail-comment-author={comment.id}
							>{comment.author_name ||
								comment.author_email ||
								comment.author_id?.slice(0, 8) ||
								'system'}</span
						>

						<span
							class="font-mono text-[11px] text-fg-muted"
							data-record-detail-comment-created={comment.id}>{formatDate(comment.created_at)}</span
						>
					</div>

					<p
						class="mt-2 whitespace-pre-wrap text-sm leading-relaxed text-fg"
						data-record-detail-comment-body={comment.id}
					>
						{comment.body}
					</p>
				</div>
			{/each}

			{#if s.recordCommentsLoading && s.selectedRecordComments.length === 0}
				<p class="text-sm text-fg-muted">
					{$t('common.loading')}
				</p>
			{:else if s.selectedRecordComments.length === 0}
				<p class="text-sm text-fg-muted" data-record-detail-comments-empty={s.selectedRecord.id}>
					{$t('forms.noRecordComments')}
				</p>
			{/if}

			<div class="space-y-2 pt-1">
				<textarea
					class="min-h-20 w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
					bind:value={s.recordCommentDraft}
					placeholder={$t('forms.recordCommentPlaceholder')}
					data-record-detail-comment-input={s.selectedRecord.id}></textarea>

				<span data-record-detail-comment-submit={s.selectedRecord.id}>
					<Button
						size="sm"
						variant="secondary"
						loading={s.recordCommentSubmitting}
						disabled={!s.recordCommentDraft.trim()}
						onclick={handleCreateRecordComment}>{$t('forms.addComment')}</Button
					>
				</span>
			</div>
		</div>
	</div>

	{#if selectedRecordAttachmentFields().length > 0}
		<div
			class="mt-4 border-t border-line pt-4"
			data-record-detail-attachments={s.selectedRecord.id}
			data-record-detail-attachment-count={s.recordAttachments.filter(
				(attachment) => !attachment.archived_at
			).length}
		>
			<div class="mb-3 flex items-center gap-2">
				<FileDown class="h-4 w-4 text-fg-muted" aria-hidden="true" />

				<h4 class="text-sm font-semibold text-fg">
					{$t('forms.attachmentConfig')}
				</h4>
			</div>

			<div class="space-y-3">
				{#each selectedRecordAttachmentFields() as field}
					{@const metadataAttachments = fieldAttachments(field.key)}
					{@const valueUrls = selectedRecordAttachmentValueUrls(field)}

					<section
						class="rounded-card border border-line px-3 py-2 text-xs"
						data-record-detail-attachment-field={field.key}
						data-record-detail-attachment-field-count={`${field.key}:${metadataAttachments.length + valueUrls.length}`}
					>
						<div class="mb-2 font-medium text-fg">
							{fieldLabel(field)}
						</div>

						<div class="space-y-2">
							{#each metadataAttachments as attachment}
								<div
									class="flex flex-wrap items-center gap-2 rounded bg-nav px-2 py-1.5"
									data-record-detail-attachment-item={attachment.id}
								>
									{#if attachmentIsPreviewableImage(attachment)}
										<img
											src={attachmentThumbnailUrl(attachment)}
											alt={attachment.file_name}
											class="h-10 w-10 rounded object-cover ring-1 ring-line"
											data-record-detail-attachment-thumbnail={attachment.id}
										/>
									{/if}

									<div class="min-w-0 flex-1">
										<div class="truncate font-medium text-fg">
											{attachment.file_name}
										</div>

										<div
											class="truncate font-mono text-[11px] text-fg-muted"
											data-record-detail-attachment-url={attachment.id}
										>
											{attachment.url}
										</div>
									</div>

									<a
										href={attachmentSignedUrl(attachment) || undefined}
										target="_blank"
										rel="noreferrer"
										class="rounded px-2 py-1 font-medium text-fg underline hover:bg-surface"
										data-record-detail-attachment-open={attachment.id}
										data-record-detail-attachment-signed-url={attachmentSignedUrl(attachment)}
										>{$t('forms.attachmentOpen')}</a
									>

									<a
										href={attachmentSignedUrl(attachment) || undefined}
										download={attachment.file_name}
										class="rounded px-2 py-1 font-medium text-fg hover:bg-surface"
										data-record-detail-attachment-download={attachment.id}
										data-record-detail-attachment-signed-download={attachmentSignedUrl(attachment)}
										>{$t('forms.attachmentDownload')}</a
									>
								</div>
							{/each}

							{#each valueUrls as url}
								<div
									class="flex flex-wrap items-center gap-2 rounded bg-nav px-2 py-1.5"
									data-record-detail-attachment-value={`${field.key}:${url}`}
								>
									<div class="min-w-0 flex-1 truncate font-mono text-[11px] text-fg-muted">
										{url}
									</div>

									{#if safeLinkUrl(url)}
										<a
											href={safeLinkUrl(url)}
											target="_blank"
											rel="noreferrer"
											class="rounded px-2 py-1 font-medium text-fg underline hover:bg-surface"
											data-record-detail-attachment-value-open={field.key}
											>{$t('forms.attachmentOpen')}</a
										>
									{:else}
										<span
											class="rounded px-2 py-1 text-[11px] font-medium text-warning"
											data-record-detail-attachment-value-blocked={field.key}
											>{$t('forms.attachmentUrlUnsafe')}</span
										>
									{/if}
								</div>
							{/each}

							{#if metadataAttachments.length === 0 && valueUrls.length === 0}
								<p class="text-sm text-fg-muted" data-record-detail-attachment-empty={field.key}>
									{$t('forms.noRecordAttachments')}
								</p>
							{/if}
						</div>
					</section>
				{/each}
			</div>
		</div>
	{/if}

	<div
		class="mt-4 border-t border-line pt-4"
		data-record-detail-event-history={s.selectedRecord.id}
		data-record-detail-event-count={s.selectedRecordEvents.length}
	>
		<div class="mb-3 flex items-center gap-2">
			<FileText class="h-4 w-4 text-fg-muted" aria-hidden="true" />

			<h4 class="text-sm font-semibold text-fg">
				{$t('forms.timelineSourceEvents')}
			</h4>
		</div>

		<div class="space-y-2">
			{#each s.selectedRecordEvents.slice(0, 5) as event}
				<details
					class="rounded-card border border-line px-3 py-2 text-xs"
					data-record-detail-event={event.id}
					data-record-detail-event-type={event.event_type}
				>
					<summary class="cursor-pointer list-none">
						<div class="flex flex-wrap items-start justify-between gap-2">
							<div class="min-w-0">
								<div class="truncate font-medium text-fg" data-record-detail-event-title={event.id}>
									{timelineEventTitle(event)}
								</div>

								<div
									class="mt-1 font-mono text-[11px] text-fg-muted"
									data-record-detail-event-aggregate={event.id}
								>
									{timelineEventAggregateKey(event)}
								</div>
							</div>

							<span
								class="rounded bg-surface px-2 py-0.5 font-mono text-[11px] text-fg-muted"
								data-record-detail-event-created={event.id}>{formatDate(event.created_at)}</span
							>
						</div>

						<div class="mt-2 flex flex-wrap items-center gap-2 text-[11px] text-fg-muted">
							<span data-record-detail-event-kind={event.id}>{event.event_type}</span>

							{#if event.actor_id}
								<span data-record-detail-event-actor={event.id}
									>{$t('forms.eventActor')} {event.actor_id}</span
								>
							{/if}
						</div>
					</summary>

					<div class="mt-3 grid gap-2 lg:grid-cols-2">
						<pre
							class="max-h-40 overflow-auto rounded bg-nav p-2 font-mono text-[11px] text-fg-muted"
							data-record-detail-event-payload={event.id}>{timelineEventJson(event.payload)}</pre>

						<pre
							class="max-h-40 overflow-auto rounded bg-nav p-2 font-mono text-[11px] text-fg-muted"
							data-record-detail-event-source={event.id}>{timelineEventJson(event.source)}</pre>
					</div>
				</details>
			{/each}

			{#if s.selectedRecordEvents.length === 0}
				<p class="text-sm text-fg-muted" data-record-detail-event-empty={s.selectedRecord.id}>
					{$t('forms.noRecordEvents')}
				</p>
			{/if}
		</div>
	</div>
{/if}
