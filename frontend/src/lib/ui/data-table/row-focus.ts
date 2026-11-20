import { nextRow } from './data-table';

/** Controls of a row that take part in keyboard navigation. */
export const ROW_CONTROLS = 'input[type="checkbox"], a[href], button:not([disabled])';

export interface RowFocusOptions {
	/** Selector of the row elements inside the container. */
	rows: string;
}

/**
 * Svelte action: roving focus over the rows of a table or list. Exactly one row's controls are
 * in the Tab order (the row last focused, else the first); ArrowUp/Down move to the same kind of
 * control (same position in the row) in the previous or next row, Ctrl/Cmd+Home/End jump.
 */
export function rowFocus(node: HTMLElement, options: RowFocusOptions) {
	let current = 0;
	const rows = () => Array.from(node.querySelectorAll<HTMLElement>(options.rows));
	const controls = (row: HTMLElement) =>
		Array.from(row.querySelectorAll<HTMLElement>(ROW_CONTROLS));

	function sync() {
		const list = rows();
		if (current >= list.length) current = Math.max(0, list.length - 1);
		list.forEach((row, index) => {
			for (const control of controls(row)) {
				const wanted = index === current ? '0' : '-1';
				if (control.getAttribute('tabindex') !== wanted) control.setAttribute('tabindex', wanted);
			}
		});
	}

	function onFocusin(event: FocusEvent) {
		const row = (event.target as HTMLElement).closest<HTMLElement>(options.rows);
		if (!row) return;
		const index = rows().indexOf(row);
		if (index >= 0 && index !== current) {
			current = index;
			sync();
		}
	}

	function onKeydown(event: KeyboardEvent) {
		if (event.defaultPrevented || event.altKey || event.shiftKey) return;
		const target = event.target as HTMLElement;
		const row = target.closest<HTMLElement>(options.rows);
		if (!row) return;
		const list = rows();
		const index = list.indexOf(row);
		const next = nextRow(event.key, event.ctrlKey || event.metaKey, index, list.length);
		if (next === null) return;
		// Ctrl/Cmd+Home/End inside a text control belong to the control.
		if (target.matches('input:not([type="checkbox"]), textarea, select')) return;
		event.preventDefault();
		const kind = Math.max(0, controls(row).indexOf(target));
		const to = controls(list[next]);
		const control = to[Math.min(kind, to.length - 1)];
		current = next;
		sync();
		(control ?? list[next]).focus();
	}

	node.addEventListener('focusin', onFocusin);
	node.addEventListener('keydown', onKeydown);
	sync();
	const observer = new MutationObserver(() => sync());
	observer.observe(node, { childList: true, subtree: true });
	return {
		update(next: RowFocusOptions) {
			options = next;
			sync();
		},
		destroy() {
			observer.disconnect();
			node.removeEventListener('focusin', onFocusin);
			node.removeEventListener('keydown', onKeydown);
		}
	};
}
