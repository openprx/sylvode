import { randomUuid } from '#lib/utils/uuid.js';
import type { BodyErrorReason } from './labels';
/** Uploads and pending child creation belong to their original pinned Page, not a mounted view. */
import {
	BodyAssetUpload,
	type BodyAssetTransport,
	type BodyAssetKind,
	type BodyUploadState,
	type BodyInsertionBookmark
} from './assets';
import type { BodyAtomIntent } from './doc-intents';
import type { FlowChildReservation } from '#lib/api/flow.js';
import type { ApiResult } from '#lib/api/client.js';
export interface BodyJobHost {
	objectId: string;
	allowed(): boolean;
	ensure(): Promise<boolean>;
	insert(
		bookmark: BodyInsertionBookmark,
		intent: BodyAtomIntent
	): Promise<{ blockId: string } | null>;
	saved(signal: AbortSignal): Promise<boolean>;
	contains(blockId: string): Promise<boolean>;
	failureReason(): string | null;
	prepare(
		title: string,
		key: string,
		signal?: AbortSignal
	): Promise<ApiResult<FlowChildReservation>>;
	cancelReservation(id: string): Promise<ApiResult<unknown>>;
	reservationStatus(id: string, signal?: AbortSignal): Promise<ApiResult<FlowChildReservation>>;
	childReady(id: string): void;
}
export interface ChildBodyJob {
	id: string;
	key: string;
	title: string;
	bookmark: BodyInsertionBookmark;
	phase: 'preparing' | 'pending' | 'inserting' | 'failed';
	reservation: FlowChildReservation | null;
	inserted: string | null;
	error: BodyErrorReason | null;
}
export interface BodyDraft {
	mode: 'upload' | 'replace' | 'details' | 'child' | 'link';
	kind: 'image' | 'attachment';
	bookmark: BodyInsertionBookmark;
	alt: string;
	caption: string;
	title: string;
	targetId: string;
	file: File | null;
	dirty: boolean;
}
export interface BodyJobsSnapshot {
	draft: BodyDraft | null;
	uploads: { job: BodyAssetUpload; state: BodyUploadState }[];
	children: { job: ChildBodyJob; state: ChildBodyJob }[];
	pending: number;
}
export class PageBodyJobs {
	private draft: BodyDraft | null = null;
	private uploads = new Map<
		BodyAssetUpload,
		{
			unsubscribe: () => void;
			inserted: string | null;
			details: { alt: string; caption: string; replace: boolean };
		}
	>();
	private running = new Set<object>();
	private children = new Map<string, ChildBodyJob>();
	private listeners = new Set<(snapshot: BodyJobsSnapshot) => void>();
	private disposed = false;
	private controller = new AbortController();
	constructor(
		private readonly host: BodyJobHost,
		private readonly transport: BodyAssetTransport
	) {}
	private snapshot(): BodyJobsSnapshot {
		return {
			draft: this.draft,
			uploads: [...this.uploads.keys()].map((job) => ({ job, state: { ...job.state } })),
			children: [...this.children.values()].map((job) => ({ job, state: { ...job } })),
			pending:
				[...this.uploads.keys()].filter((job) => job.pending).length +
				this.children.size +
				(this.draft?.dirty ? 1 : 0)
		};
	}
	subscribe(listener: (snapshot: BodyJobsSnapshot) => void): () => void {
		this.listeners.add(listener);
		listener(this.snapshot());
		return () => this.listeners.delete(listener);
	}
	private emit(): void {
		if (!this.disposed) for (const listener of this.listeners) listener(this.snapshot());
	}
	setDraft(draft: BodyDraft | null): void {
		this.draft = draft;
		this.emit();
	}
	async details(
		kind: 'image' | 'attachment',
		bookmark: BodyInsertionBookmark,
		attrs: { alt: string; caption: string }
	): Promise<boolean> {
		if (!this.host.allowed() || !(await this.host.ensure())) return false;
		return !!(await this.host.insert(bookmark, { kind, attrs, replace: true, merge: true }));
	}

	pendingTitle(id: string): string | null {
		return (
			[...this.children.values()].find((job) => job.reservation?.child_page_id === id)?.title ??
			null
		);
	}
	upload(
		kind: BodyAssetKind,
		file: File,
		bookmark: BodyInsertionBookmark,
		details = { alt: '', caption: '', replace: false }
	): BodyAssetUpload {
		const job = new BodyAssetUpload(
			this.host.objectId,
			kind,
			file,
			{ ...bookmark },
			this.transport
		);
		this.uploads.set(job, { unsubscribe: () => {}, inserted: null, details });
		this.uploads.get(job)!.unsubscribe = job.subscribe(() => this.emit());
		void this.retryUpload(job);
		return job;
	}
	async retryUpload(job: BodyAssetUpload): Promise<void> {
		if (this.running.has(job) || this.disposed) return;
		this.running.add(job);
		try {
			await this.runUpload(job);
		} finally {
			this.running.delete(job);
		}
	}
	private async runUpload(job: BodyAssetUpload): Promise<void> {
		const slot = this.uploads.get(job);
		if (!slot || this.disposed) return;
		const receipt = await job.start();
		if (!receipt || this.disposed || !this.uploads.has(job)) return;
		if (!this.host.allowed() || !(await this.host.ensure())) {
			job.insertionFailed('insert');
			return;
		}
		job.beginInsertion();
		if (slot.inserted && !(await this.host.contains(slot.inserted))) slot.inserted = null;
		if (!slot.inserted) {
			const inserted = await this.host.insert(job.bookmark, {
				kind: job.kind,
				replace: slot.details.replace,
				attrs:
					job.kind === 'image'
						? { asset_id: receipt.asset_id, alt: slot.details.alt, caption: slot.details.caption }
						: {
								asset_id: receipt.asset_id,
								name: receipt.filename,
								mime: receipt.content_type,
								caption: slot.details.caption
							}
			});
			if (this.disposed) return;
			if (!inserted) {
				job.insertionFailed('position');
				return;
			}
			slot.inserted = inserted.blockId;
		}
		if (await this.host.saved(this.controller.signal)) {
			if (this.disposed) return;
			job.complete();
			slot.unsubscribe();
			job.dispose();
			this.uploads.delete(job);
			this.emit();
		} else if (!this.disposed) job.insertionFailed('insert');
	}
	repositionUpload(job: BodyAssetUpload, bookmark: BodyInsertionBookmark): void {
		const slot = this.uploads.get(job);
		if (!slot || slot.inserted) return;
		job.bookmark = { ...bookmark };
		void this.retryUpload(job);
	}
	async discardUpload(job: BodyAssetUpload): Promise<void> {
		const slot = this.uploads.get(job);
		if (!slot || slot.inserted) return;
		if (await job.discard()) {
			slot.unsubscribe();
			this.uploads.delete(job);
			this.emit();
		}
	}
	createChild(title: string, bookmark: BodyInsertionBookmark): ChildBodyJob {
		const job: ChildBodyJob = {
			id: randomUuid(),
			key: randomUuid(),
			title,
			bookmark: { ...bookmark },
			phase: 'preparing',
			reservation: null,
			inserted: null,
			error: null
		};
		this.children.set(job.id, job);
		this.emit();
		void this.retryChild(job);
		return job;
	}
	async retryChild(job: ChildBodyJob): Promise<void> {
		if (this.running.has(job) || this.disposed) return;
		this.running.add(job);
		try {
			await this.runChild(job);
		} finally {
			this.running.delete(job);
		}
	}
	private async runChild(job: ChildBodyJob): Promise<void> {
		if (this.disposed || !this.children.has(job.id)) return;
		if (!this.host.allowed() || !(await this.host.ensure())) {
			job.phase = 'failed';
			job.error = 'insert';
			this.emit();
			return;
		}
		if (job.inserted && !(await this.host.contains(job.inserted))) job.inserted = null;
		if (
			job.reservation &&
			!job.inserted &&
			this.host.failureReason() === 'child_reservation_expired'
		) {
			job.reservation = null;
			job.key = randomUuid();
		}
		if (job.reservation && !job.inserted) {
			const status = await this.host.reservationStatus(
				job.reservation.reservation_id,
				this.controller.signal
			);
			if (this.disposed || !this.children.has(job.id)) return;
			if (status.code !== 0 || !status.data) {
				job.phase = 'failed';
				job.error = 'prepare';
				this.emit();
				return;
			}
			if (status.data.state === 'expired' || status.data.state === 'cancelled') {
				job.reservation = null;
				job.key = randomUuid();
			} else job.reservation = status.data;
		}

		if (!job.reservation) {
			job.phase = 'preparing';
			job.error = null;
			this.emit();
			const result = await this.host.prepare(job.title, job.key, this.controller.signal);
			if (this.disposed || !this.children.has(job.id)) {
				if (result.code === 0 && result.data)
					void this.host.cancelReservation(result.data.reservation_id);
				return;
			}
			if (
				result.code !== 0 ||
				!result.data ||
				result.data.source_object_id !== this.host.objectId
			) {
				job.phase = 'failed';
				job.error = 'prepare';
				this.emit();
				return;
			}
			job.reservation = result.data;
			job.title = result.data.title;
			job.phase = 'pending';
			this.emit();
		}
		if (!job.inserted) {
			const inserted = await this.host.insert(job.bookmark, {
				kind: 'childPage',
				attrs: { child_page_id: job.reservation.child_page_id }
			});
			if (this.disposed) return;
			if (!inserted) {
				job.phase = 'pending';
				job.error = 'position';
				this.emit();
				return;
			}
			job.inserted = inserted.blockId;
		}
		job.phase = 'inserting';
		job.error = null;
		this.emit();
		if (await this.host.saved(this.controller.signal)) {
			if (this.disposed) return;
			this.host.childReady(job.reservation.child_page_id);
			this.children.delete(job.id);
			this.emit();
		} else if (!this.disposed) {
			job.phase = 'failed';
			job.error = 'save';
			this.emit();
		}
	}
	repositionChild(job: ChildBodyJob, bookmark: BodyInsertionBookmark): void {
		if (job.inserted) return;
		job.bookmark = { ...bookmark };
		void this.retryChild(job);
	}
	async cancelChild(job: ChildBodyJob): Promise<void> {
		if (job.inserted) return;
		if (job.reservation) {
			const result = await this.host.cancelReservation(job.reservation.reservation_id);
			if (result.code !== 0) {
				job.error = 'cancel';
				this.emit();
				return;
			}
		}
		this.children.delete(job.id);
		this.emit();
	}
	async linkPage(childPageId: string, bookmark: BodyInsertionBookmark): Promise<boolean> {
		if (!this.host.allowed() || !(await this.host.ensure())) return false;
		const result = await this.host.insert(bookmark, {
			kind: 'childPage',
			attrs: { child_page_id: childPageId }
		});
		return !!result;
	}
	dispose(): void {
		if (this.disposed) return;
		this.disposed = true;
		this.controller.abort();
		for (const [job, slot] of this.uploads) {
			slot.unsubscribe();
			if (!slot.inserted && job.state.receipt) void job.discard();
			job.dispose();
		}
		for (const job of this.children.values())
			if (job.reservation && !job.inserted)
				void this.host.cancelReservation(job.reservation.reservation_id);
		this.uploads.clear();
		this.children.clear();
		this.listeners.clear();
	}
}
const registries = new WeakMap<object, PageBodyJobs>();
export function pageBodyJobs(doc: object, create: () => PageBodyJobs): PageBodyJobs {
	let jobs = registries.get(doc);
	if (!jobs) {
		jobs = create();
		registries.set(doc, jobs);
	}
	return jobs;
}
export function existingPageBodyJobs(doc: object): PageBodyJobs | undefined {
	return registries.get(doc);
}
export function disposePageBodyJobs(doc: object): void {
	registries.get(doc)?.dispose();
	registries.delete(doc);
}
