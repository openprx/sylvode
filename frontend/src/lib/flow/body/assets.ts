import type { BodyErrorReason } from './labels';
/** Page-owned staged uploads. Transport and insertion stay explicit, with no clipboard or public URL fallback. */
import type { ApiResult, UploadProgress } from '#lib/api/client.js';
export type BodyAssetKind = 'image' | 'attachment';
export interface BodyAssetLimits {
	max_bytes: number;
	image_content_types: string[];
	upload_transport: 'multipart';
	editor_model: 5;
}
export interface BodyAssetReceipt {
	asset_id: string;
	object_id: string;
	kind: BodyAssetKind;
	filename: string;
	content_type: string;
	byte_size: number;
	sha256: string;
	state: 'staged';
	read_path: string;
}
export interface BodyAssetTransport {
	limits(objectId: string, signal?: AbortSignal): Promise<ApiResult<BodyAssetLimits>>;
	upload(
		objectId: string,
		kind: BodyAssetKind,
		file: File,
		key: string,
		signal: AbortSignal,
		progress: (value: UploadProgress) => void
	): Promise<ApiResult<BodyAssetReceipt>>;
	discard(objectId: string, assetId: string): Promise<ApiResult<unknown>>;
}
export interface BodyInsertionBookmark {
	/** Null targets the first block of an empty Page. */
	blockId: string | null;
	side: 'before' | 'after';
}
export interface BodyUploadState {
	phase: 'idle' | 'uploading' | 'staged' | 'inserting' | 'complete' | 'failed' | 'canceled';
	loaded: number;
	total: number | null;
	maxBytes: number | null;
	error: BodyErrorReason | null;
	receipt: BodyAssetReceipt | null;
}
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
export function validBodyReceipt(
	value: BodyAssetReceipt,
	objectId: string,
	kind: BodyAssetKind,
	file: File
): boolean {
	return (
		!!value &&
		uuid.test(value.asset_id) &&
		value.object_id === objectId &&
		value.kind === kind &&
		typeof value.filename === 'string' &&
		value.filename.length > 0 &&
		value.filename.length <= 512 &&
		value.byte_size === file.size &&
		/^[a-f0-9]{64}$/i.test(value.sha256) &&
		value.state === 'staged' &&
		typeof value.content_type === 'string' &&
		value.content_type.length <= 128 &&
		(kind !== 'image' ||
			['image/png', 'image/jpeg', 'image/webp', 'image/gif'].includes(value.content_type)) &&
		value.read_path === `/api/v2/flow/objects/${objectId}/body-assets/${value.asset_id}`
	);
}
export class BodyAssetUpload {
	readonly key = crypto.randomUUID();
	private controller: AbortController | null = null;
	private epoch = 0;
	private disposed = false;
	private task: Promise<BodyAssetReceipt | null> | null = null;
	private value: BodyUploadState = {
		phase: 'idle',
		loaded: 0,
		total: null,
		maxBytes: null,
		error: null,
		receipt: null
	};
	private listeners = new Set<(value: BodyUploadState) => void>();
	constructor(
		readonly objectId: string,
		readonly kind: BodyAssetKind,
		readonly file: File,
		public bookmark: BodyInsertionBookmark,
		private readonly transport: BodyAssetTransport
	) {}
	get state(): BodyUploadState {
		return this.value;
	}
	get pending(): boolean {
		return ['uploading', 'staged', 'inserting', 'failed', 'canceled'].includes(this.value.phase);
	}
	subscribe(listener: (value: BodyUploadState) => void): () => void {
		this.listeners.add(listener);
		listener(this.value);
		return () => this.listeners.delete(listener);
	}
	private set(patch: Partial<BodyUploadState>): void {
		this.value = { ...this.value, ...patch };
		for (const listener of this.listeners) listener(this.value);
	}
	async start(): Promise<BodyAssetReceipt | null> {
		if (this.disposed) return null;
		if (this.task) return this.task;
		if (this.value.receipt) return this.value.receipt;
		const epoch = ++this.epoch;
		const controller = new AbortController();
		this.controller = controller;
		this.set({ phase: 'uploading', loaded: 0, total: this.file.size, error: null });
		const current = () => !this.disposed && !controller.signal.aborted && epoch === this.epoch;
		this.task = (async () => {
			try {
				const limits = await this.transport.limits(this.objectId, controller.signal);
				if (!current()) return null;
				if (
					limits.code !== 0 ||
					!limits.data ||
					limits.data.editor_model !== 5 ||
					limits.data.upload_transport !== 'multipart' ||
					!Number.isSafeInteger(limits.data.max_bytes) ||
					limits.data.max_bytes <= 0
				) {
					this.set({ phase: 'failed', error: 'unavailable' });
					return null;
				}
				this.set({ maxBytes: limits.data.max_bytes });
				if (this.file.size <= 0 || this.file.size > limits.data.max_bytes) {
					this.set({ phase: 'failed', error: 'size' });
					return null;
				}
				const result = await this.transport.upload(
					this.objectId,
					this.kind,
					this.file,
					this.key,
					controller.signal,
					(progress) => {
						if (current())
							this.set({ loaded: Math.max(0, progress.loaded), total: progress.total });
					}
				);
				if (!current()) {
					if (
						result.code === 0 &&
						result.data &&
						validBodyReceipt(result.data, this.objectId, this.kind, this.file)
					) {
						if (!this.disposed) this.set({ receipt: result.data });
						else if (this.value.phase !== 'complete' && this.value.phase !== 'inserting')
							void this.transport.discard(this.objectId, result.data.asset_id);
					}
					return null;
				}
				if (
					result.code !== 0 ||
					!result.data ||
					!validBodyReceipt(result.data, this.objectId, this.kind, this.file)
				) {
					this.set({ phase: 'failed', error: 'upload' });
					return null;
				}
				this.set({
					phase: 'staged',
					receipt: result.data,
					loaded: this.file.size,
					total: this.file.size
				});
				return result.data;
			} catch {
				if (current()) this.set({ phase: 'failed', error: 'network' });
				return null;
			} finally {
				if (epoch === this.epoch) {
					this.task = null;
					this.controller = null;
				}
			}
		})();
		return this.task;
	}
	cancel(): void {
		if (this.value.phase === 'inserting' || this.value.phase === 'complete') return;
		this.epoch++;
		this.controller?.abort();
		this.controller = null;
		this.task = null;
		this.set({ phase: 'canceled', error: null });
	}
	beginInsertion(): void {
		if (this.value.receipt) this.set({ phase: 'inserting', error: null });
	}
	insertionFailed(reason: 'position' | 'insert'): void {
		this.set({ phase: 'staged', error: reason });
	}
	complete(): void {
		this.set({ phase: 'complete', error: null });
	}
	async discard(): Promise<boolean> {
		if (this.value.phase === 'inserting' || this.value.phase === 'complete') return false;
		this.cancel();
		const receipt = this.value.receipt;
		if (!receipt) {
			this.dispose();
			return true;
		}
		const result = await this.transport.discard(this.objectId, receipt.asset_id);
		if (result.code !== 0) {
			this.set({ phase: 'staged', error: 'discard' });
			return false;
		}
		this.dispose();
		return true;
	}
	dispose(): void {
		this.disposed = true;
		this.epoch++;
		this.controller?.abort();
		this.task = null;
		this.listeners.clear();
	}
}
