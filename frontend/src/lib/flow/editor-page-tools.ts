import type { Node } from 'prosemirror-model';

export interface PageTextMatch {
	from: number;
	to: number;
}

/** Blocks are flat in the schema. Indent identifies the descendants of a collapsed toggle. */
export function visiblePageBlocks(doc: Node, visit: (node: Node, pos: number) => void): void {
	let collapsedIndent: number | null = null;
	doc.forEach((node, pos) => {
		const indent = Number(node.attrs.indent) || 0;
		if (collapsedIndent !== null && indent <= collapsedIndent) collapsedIndent = null;
		if (collapsedIndent !== null) return;
		visit(node, pos);
		if (node.type.name === 'toggle' && node.attrs.collapsed) collapsedIndent = indent;
	});
}

export function pageTextMatches(doc: Node, query: string): PageTextMatch[] {
	if (!query.trim()) return [];
	// Match literal Unicode case-insensitively against original text, preserving UTF-16 offsets.
	const pattern = new RegExp(query.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'giu');
	const matches: PageTextMatch[] = [];
	visiblePageBlocks(doc, (node, pos) => {
		if (!node.isTextblock) return;
		// The current schema contains only text (possibly split by different marks) in text blocks.
		for (const match of node.textContent.matchAll(pattern)) {
			const from = pos + 1 + match.index;
			matches.push({ from, to: from + match[0].length });
		}
	});
	return matches;
}

export interface PageHeading {
	pos: number;
	level: number;
	text: string;
}

export interface PageOutlineEntry extends PageHeading {
	children: PageOutlineEntry[];
}

export function outlineTree(headings: PageHeading[]): PageOutlineEntry[] {
	const roots: PageOutlineEntry[] = [];
	const parents: PageOutlineEntry[] = [];
	for (const heading of headings) {
		while (parents.length && parents.at(-1)!.level >= heading.level) parents.pop();
		const item = { ...heading, children: [] };
		(parents.at(-1)?.children ?? roots).push(item);
		parents.push(item);
	}
	return roots;
}
