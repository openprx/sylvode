// Memory-only Forms tab state. Server data is reauthorized before presentation/drafts restore.
// Draft versions are opaque original server values; receiving a newer baseline never rebases them.
export interface FormsDraftVersion {
	recordId: string;
	updatedAt: string;
	schemaVersion: number;
}
export interface FormsSessionSnapshot {
	formId: string | null;
	state: Record<string, unknown>;
	clean: Record<string, string>;
	formSchemaVersion: number | null;
	recordVersion: FormsDraftVersion | null;
	viewUpdatedAt: string | null;
}
export function formsFingerprint(value: unknown): string {
	return JSON.stringify(value) ?? 'null';
}
export function formsDraftDirty(
	groups: Record<string, unknown>,
	clean: Record<string, string>
): boolean {
	return Object.entries(groups).some(([key, value]) => {
		const base = clean[key];
		return base !== undefined && formsFingerprint(value) !== base;
	});
}
export function cloneFormsSnapshot(snapshot: FormsSessionSnapshot): FormsSessionSnapshot {
	return structuredClone(snapshot);
}
export function restoredFormsSnapshot(
	value: unknown,
	formId: string | null,
	authorized: boolean
): FormsSessionSnapshot | null {
	if (!authorized || !value || typeof value !== 'object') return null;
	const snapshot = value as Partial<FormsSessionSnapshot>;
	if (snapshot.formId !== formId || !snapshot.state || !snapshot.clean) return null;
	return cloneFormsSnapshot(snapshot as FormsSessionSnapshot);
}
export function recordWriteVersion(
	recordId: string,
	draft: FormsDraftVersion | null,
	server: { id: string; updated_at: string } | null,
	schemaVersion: number | null
): { expected_updated_at: string; expected_schema_version: number } | null {
	if (draft?.recordId === recordId)
		return {
			expected_updated_at: draft.updatedAt,
			expected_schema_version: draft.schemaVersion
		};
	if (!server || server.id !== recordId || schemaVersion === null) return null;
	return { expected_updated_at: server.updated_at, expected_schema_version: schemaVersion };
}
export function mayRefreshForms(state: {
	active: boolean;
	visible: boolean;
	online: boolean;
	saving: boolean;
	loading: boolean;
}): boolean {
	return state.active && state.visible && state.online && !state.saving && !state.loading;
}
export class FormsRefreshLatch {
	private active = false;
	private disposed = false;
	async run(read: () => Promise<void>): Promise<boolean> {
		if (this.active || this.disposed) return false;
		this.active = true;
		try {
			await read();
			return !this.disposed;
		} finally {
			this.active = false;
		}
	}
	dispose(): void {
		this.disposed = true;
	}
}
