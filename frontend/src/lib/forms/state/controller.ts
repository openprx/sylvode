import type { FormsPage } from '#lib/forms/state/forms-page.js';
import type { FormsPageState } from '#lib/forms/state/forms-page-state.svelte.js';

/** Base of every controller: the page root and its state, no logic of its own. */
export abstract class FormsController {
	protected readonly fp: FormsPage;
	protected readonly s: FormsPageState;

	constructor(fp: FormsPage) {
		this.fp = fp;
		this.s = fp.state;
	}
}
