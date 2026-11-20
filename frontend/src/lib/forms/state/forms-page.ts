import type { taskEditorAction } from '#lib/workbench/edit-surface.js';
import type { TaskWorkbench } from '#lib/workbench/tabs.js';
import { FormsPageState } from '#lib/forms/state/forms-page-state.svelte.js';

export interface FormsPageDeps {
	workbench: TaskWorkbench | undefined;
	recordEditorAction: ReturnType<typeof taskEditorAction>;
}

/** Root of the forms page: its dependencies, its state and (from W0-D) its controllers. */
export class FormsPage {
	readonly deps: FormsPageDeps;
	readonly state: FormsPageState;

	constructor(deps: FormsPageDeps) {
		this.deps = deps;
		this.state = new FormsPageState(this);
	}
}
