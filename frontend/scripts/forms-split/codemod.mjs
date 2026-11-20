#!/usr/bin/env node
// Codemod for the forms page split (FORMS-PAGE-DECOMPOSITION.md, section 4).
//
//   node scripts/forms-split/codemod.mjs lift      W0-C: state, types and constants leave the page
//
// The tool parses the page script with the TypeScript compiler and resolves every identifier with
// the type checker (`getSymbolAtLocation`), so a local variable that happens to share a name with a
// top-level declaration is never rewritten. Markup expressions are parsed with `svelte/compiler`.
// It aborts (non-zero exit, nothing written) on anything it does not expect instead of guessing.
// The output is deterministic: running it twice on the same input gives the same files.
//
// The table in `mapping.json` is the only configuration (Appendix A as data).
import { createRequire } from 'node:module';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const FE = resolve(HERE, '../..');
const require = createRequire(join(FE, 'package.json'));
const ts = require('typescript');
const { parse } = require('svelte/compiler');
const prettier = require('prettier');

const ROUTE = join(
	FE,
	'src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/forms/+page.svelte'
);
const LIB_FORMS = join(FE, 'src/lib/forms');
const MAPPING = JSON.parse(readFileSync(join(HERE, 'mapping.json'), 'utf8'));

class Abort extends Error {}
const fail = (message) => {
	throw new Abort(message);
};

// --- parsing ---------------------------------------------------------------------------------

function parsePage(src) {
	const ast = parse(src, { modern: true });
	if (!ast.instance || ast.module) fail('the page must have exactly one instance script');
	const cs = ast.instance.content.start;
	const ce = ast.instance.content.end;
	const code = src.slice(cs, ce);
	const sf = ts.createSourceFile('page.ts', code, ts.ScriptTarget.ESNext, true, ts.ScriptKind.TS);
	const host = {
		getSourceFile: (name) => (name === 'page.ts' ? sf : undefined),
		getDefaultLibFileName: () => 'lib.d.ts',
		writeFile() {},
		getCurrentDirectory: () => '/',
		getDirectories: () => [],
		fileExists: (name) => name === 'page.ts',
		readFile: () => undefined,
		getCanonicalFileName: (name) => name,
		useCaseSensitiveFileNames: () => true,
		getNewLine: () => '\n'
	};
	const program = ts.createProgram(
		['page.ts'],
		{ noLib: true, noResolve: true, target: ts.ScriptTarget.ESNext, types: [] },
		host
	);
	const checker = program.getTypeChecker();
	const page = {
		src,
		ast,
		cs,
		ce,
		code,
		sf,
		checker,
		infos: [],
		byNode: new Map(),
		statements: []
	};

	const add = (info) => {
		page.infos.push(info);
		page.byNode.set(info.decl, info);
	};
	let importIndex = 0;
	for (const st of sf.statements) {
		if (ts.isImportDeclaration(st)) {
			const clause = st.importClause;
			const from = st.moduleSpecifier.text;
			const stmtTypeOnly = Boolean(clause?.isTypeOnly);
			const index = importIndex++;
			if (!clause) fail(`side-effect import of ${from} is not supported`);
			if (clause.name) {
				add({
					kind: 'import',
					name: clause.name.text,
					decl: clause,
					stmt: st,
					from,
					form: 'default',
					typeOnly: stmtTypeOnly,
					index
				});
			}
			const nb = clause.namedBindings;
			if (nb && ts.isNamespaceImport(nb)) {
				add({
					kind: 'import',
					name: nb.name.text,
					decl: nb,
					stmt: st,
					from,
					form: 'namespace',
					typeOnly: stmtTypeOnly,
					index
				});
			} else if (nb) {
				for (const el of nb.elements) {
					add({
						kind: 'import',
						name: el.name.text,
						imported: el.propertyName?.text ?? el.name.text,
						decl: el,
						stmt: st,
						from,
						form: 'named',
						typeOnly: stmtTypeOnly || el.isTypeOnly,
						index
					});
				}
			}
		} else if (ts.isTypeAliasDeclaration(st) || ts.isInterfaceDeclaration(st)) {
			add({ kind: 'type', name: st.name.text, decl: st, stmt: st });
		} else if (ts.isFunctionDeclaration(st)) {
			if (!st.name || !st.body) fail('anonymous or bodiless top-level function');
			add({ kind: 'function', name: st.name.text, decl: st, stmt: st });
		} else if (ts.isVariableStatement(st)) {
			const list = st.declarationList;
			const isConst = (list.flags & ts.NodeFlags.Const) !== 0;
			if (list.declarations.length !== 1) fail('a variable statement with several declarators');
			const d = list.declarations[0];
			if (!ts.isIdentifier(d.name)) fail('a destructuring declaration at top level');
			const call =
				d.initializer && ts.isCallExpression(d.initializer)
					? d.initializer.expression.getText(sf)
					: '';
			let kind;
			if (!isConst) kind = call === '$state' ? 'state' : 'plain';
			else if (call === '$derived' || call === '$derived.by') kind = 'derived';
			else kind = 'const';
			if (/^\$state\./.test(call) || (/^\$derived\./.test(call) && kind !== 'derived')) {
				fail(`unsupported rune ${call} for ${d.name.text}`);
			}
			add({ kind, name: d.name.text, decl: d, stmt: st });
		} else if (ts.isExpressionStatement(st)) {
			page.statements.push(st);
		} else {
			fail(`unexpected top-level statement kind ${ts.SyntaxKind[st.kind]}`);
		}
	}
	page.byName = new Map(page.infos.map((info) => [info.name, info]));
	if (page.byName.size !== page.infos.length) fail('duplicate top-level names');
	return page;
}

function refInfo(page, id) {
	const parent = id.parent;
	const sym =
		ts.isShorthandPropertyAssignment(parent) && parent.name === id
			? page.checker.getShorthandAssignmentValueSymbol(parent)
			: page.checker.getSymbolAtLocation(id);
	if (!sym) return null;
	for (const d of sym.declarations ?? []) {
		const info = page.byNode.get(d);
		if (info) return info;
	}
	return null;
}

function inTypePosition(id) {
	for (let p = id.parent; p; p = p.parent) {
		if (ts.isTypeNode(p)) return true;
		if (ts.isStatement(p) || ts.isBlock(p)) return false;
	}
	return false;
}

function inTypeQuery(id) {
	for (let p = id.parent; p; p = p.parent) {
		if (ts.isTypeQueryNode(p)) return true;
		if (ts.isStatement(p)) return false;
	}
	return false;
}

/** Every reference to a top-level declaration inside `node` (declaration names excluded). */
function collectRefs(page, node) {
	const refs = [];
	(function walk(n) {
		if (ts.isIdentifier(n)) {
			if (ts.isImportSpecifier(n.parent) || ts.isExportSpecifier(n.parent)) return;
			const info = refInfo(page, n);
			if (info && !(info.decl.name === n)) {
				refs.push({ id: n, info, typeOnly: inTypePosition(n), query: inTypeQuery(n) });
			}
			return;
		}
		ts.forEachChild(n, walk);
	})(node);
	return refs;
}

const STORE_NAMES = new Set(['t', 'locale', 'authStore']);

// --- text helpers ----------------------------------------------------------------------------

function applyEdits(text, edits, base = 0) {
	const sorted = [...edits].sort((a, b) => a.start - b.start || a.end - b.end);
	for (let i = 1; i < sorted.length; i += 1) {
		if (sorted[i].start < sorted[i - 1].end) fail(`overlapping edits at ${sorted[i].start}`);
	}
	let out = '';
	let cursor = base;
	for (const edit of sorted) {
		out += text.slice(cursor, edit.start) + edit.text;
		cursor = edit.end;
	}
	return out + text.slice(cursor);
}

/** Source range of a statement including the comments directly above it and its line breaks. */
function statementRange(page, st) {
	const { code, sf } = page;
	const ranges = ts.getLeadingCommentRanges(code, st.getFullStart()) ?? [];
	let begin = st.getStart(sf);
	for (let i = ranges.length - 1; i >= 0; i -= 1) {
		const gap = code.slice(ranges[i].end, begin);
		if (gap.trim() === '' && (gap.match(/\n/g) ?? []).length <= 1) begin = ranges[i].pos;
		else break;
	}
	while (begin > 0 && (code[begin - 1] === '\t' || code[begin - 1] === ' ')) begin -= 1;
	let end = st.end;
	while (end < code.length && (code[end] === ' ' || code[end] === '\t')) end += 1;
	if (code[end] === '\n') end += 1;
	return { start: begin, end };
}

function dedent(text) {
	const lines = text.split('\n');
	const indent = Math.min(
		...lines.filter((line) => line.trim() !== '').map((line) => /^\t*/.exec(line)[0].length)
	);
	return lines.map((line) => line.slice(Math.min(indent, /^\t*/.exec(line)[0].length))).join('\n');
}

// --- imports for a generated file ------------------------------------------------------------

/**
 * `needs` is a list of { info, typeOnly } (every use of the same name merged: a name is
 * type-only when none of its uses is a value use).
 */
function mergeNeeds(refs) {
	const merged = new Map();
	for (const ref of refs) {
		const seen = merged.get(ref.info);
		if (seen) seen.typeOnly = seen.typeOnly && ref.typeOnly;
		else merged.set(ref.info, { info: ref.info, typeOnly: ref.typeOnly });
	}
	return [...merged.values()];
}

const LOCAL_MODULES = {
	type: '#lib/forms/page-types.js',
	constant: '#lib/forms/page-constants.js'
};

function localModuleOf(info) {
	if (info.kind === 'type') return LOCAL_MODULES.type;
	if (info.kind === 'const' && MAPPING.constants.includes(info.name)) return LOCAL_MODULES.constant;
	if (info.kind === 'function' && MAPPING.earlyPure[info.name]) {
		return `#lib/forms/${MAPPING.earlyPure[info.name]}.js`;
	}
	return null;
}

/** Import statements for the needs of one generated file; `self` is its own module path. */
function buildImports(needs, self) {
	const lines = [];
	const byFrom = new Map();
	const local = new Map();
	const sorted = [...needs].sort((a, b) => {
		const ia = a.info.kind === 'import' ? a.info.index : Infinity;
		const ib = b.info.kind === 'import' ? b.info.index : Infinity;
		return ia - ib || a.info.name.localeCompare(b.info.name);
	});
	for (const need of sorted) {
		const { info } = need;
		if (info.kind === 'import') {
			if (!byFrom.has(info.from)) byFrom.set(info.from, []);
			byFrom.get(info.from).push(need);
		} else {
			const from = localModuleOf(info);
			if (!from) fail(`${info.name} (${info.kind}) cannot be imported by a generated file`);
			if (from === self) continue;
			if (!local.has(from)) local.set(from, []);
			local.get(from).push(need);
		}
	}
	for (const [from, list] of byFrom) {
		const named = [];
		for (const { info, typeOnly } of list) {
			const isType = typeOnly || info.typeOnly;
			if (info.form === 'default')
				lines.push(`import ${isType ? 'type ' : ''}${info.name} from '${from}';`);
			else if (info.form === 'namespace') {
				lines.push(`import ${isType ? 'type ' : ''}* as ${info.name} from '${from}';`);
			} else {
				const spec = info.imported === info.name ? info.name : `${info.imported} as ${info.name}`;
				named.push(`${isType ? 'type ' : ''}${spec}`);
			}
		}
		if (named.length) {
			const allType = named.every((spec) => spec.startsWith('type '));
			const specs = allType ? named.map((spec) => spec.slice(5)) : named;
			lines.push(`import ${allType ? 'type ' : ''}{ ${specs.join(', ')} } from '${from}';`);
		}
	}
	for (const [from, list] of [...local].sort((a, b) => a[0].localeCompare(b[0]))) {
		const allType = list.every((need) => need.info.kind === 'type');
		const names = list.map((need) => need.info.name);
		lines.push(`import ${allType ? 'type ' : ''}{ ${names.join(', ')} } from '${from}';`);
	}
	return lines.join('\n');
}

// --- markup ----------------------------------------------------------------------------------

function patternNames(node, out = new Set()) {
	if (!node) return out;
	switch (node.type) {
		case 'Identifier':
			out.add(node.name);
			break;
		case 'ObjectPattern':
			for (const p of node.properties)
				patternNames(p.type === 'RestElement' ? p.argument : p.value, out);
			break;
		case 'ArrayPattern':
			for (const e of node.elements) patternNames(e, out);
			break;
		case 'AssignmentPattern':
			patternNames(node.left, out);
			break;
		case 'RestElement':
			patternNames(node.argument, out);
			break;
		default:
			break;
	}
	return out;
}

/**
 * Walks the markup. `visit(name, node, role)` is called for each expression identifier;
 * `declared` collects the names the template declares itself. Identifier nodes inside TS type
 * annotations are reported with role 'type'.
 */
function walkMarkup(fragment, src, callbacks) {
	const declared = new Set();
	const walk = (node, parent, key) => {
		if (Array.isArray(node)) {
			for (const item of node) walk(item, parent, key);
			return;
		}
		if (!node || typeof node !== 'object' || typeof node.type !== 'string') return;
		if (
			['Component', 'UseDirective', 'TransitionDirective', 'AnimateDirective'].includes(
				node.type
			) &&
			typeof node.name === 'string'
		) {
			// <Component />, use:action and friends name a top-level binding without an Identifier node.
			callbacks.typeIdentifier({ name: node.name.split('.')[0] });
		}
		switch (node.type) {
			case 'EachBlock':
				walk(node.expression, node, 'expression');
				patternNames(node.context, declared);
				if (node.index) declared.add(node.index);
				if (node.context) walkTypes(node.context);
				walk(node.key, node, 'key');
				walk(node.body, node, 'body');
				walk(node.fallback, node, 'fallback');
				return;
			case 'SnippetBlock':
				for (const param of node.parameters ?? []) {
					patternNames(param, declared);
					walkTypes(param);
				}
				walk(node.body, node, 'body');
				return;
			case 'ConstTag':
				for (const declarator of node.declaration.declarations) {
					patternNames(declarator.id, declared);
					walkTypes(declarator.id);
					walk(declarator.init, declarator, 'init');
				}
				return;
			case 'ArrowFunctionExpression':
			case 'FunctionExpression':
				for (const param of node.params) {
					patternNames(param, declared);
					walkTypes(param);
				}
				if (node.returnType) walkTypes(node.returnType);
				walk(node.body, node, 'body');
				return;
			case 'VariableDeclarator':
				patternNames(node.id, declared);
				walkTypes(node.id);
				walk(node.init, node, 'init');
				return;
			case 'Identifier':
				callbacks.identifier(node, parent, key);
				if (node.typeAnnotation) walkTypes(node.typeAnnotation);
				return;
			case 'MemberExpression':
				walk(node.object, node, 'object');
				if (node.computed) walk(node.property, node, 'property');
				return;
			case 'Property':
				if (node.shorthand) {
					callbacks.shorthand(node);
					return;
				}
				if (node.computed) walk(node.key, node, 'key');
				walk(node.value, node, 'value');
				return;
			case 'TSAsExpression':
			case 'TSSatisfiesExpression':
				walk(node.expression, node, 'expression');
				walkTypes(node.typeAnnotation);
				return;
			case 'TSNonNullExpression':
				walk(node.expression, node, 'expression');
				return;
			case 'Attribute': {
				const value = node.value;
				if (
					!Array.isArray(value) &&
					value !== true &&
					value.type === 'ExpressionTag' &&
					value.expression.type === 'Identifier' &&
					value.expression.name === node.name &&
					src[node.start] === '{'
				) {
					callbacks.shorthandAttribute(node, value.expression);
					return;
				}
				walk(node.value, node, 'value');
				return;
			}
			default:
				break;
		}
		for (const [k, v] of Object.entries(node)) {
			if (k === 'type' || k === 'start' || k === 'end' || k === 'parent' || k === 'metadata')
				continue;
			if (v && typeof v === 'object') walk(v, node, k);
		}
	};
	const walkTypes = (node) => {
		if (!node || typeof node !== 'object') return;
		if (Array.isArray(node)) {
			for (const item of node) walkTypes(item);
			return;
		}
		if (node.type === 'Identifier') callbacks.typeIdentifier(node);
		for (const [k, v] of Object.entries(node)) {
			if (k === 'start' || k === 'end' || k === 'parent' || k === 'metadata') continue;
			if (v && typeof v === 'object') walkTypes(v);
		}
	};
	walk(fragment, null, 'fragment');
	return declared;
}

// --- formatting and writing ------------------------------------------------------------------

async function format(text, filepath) {
	const config = (await prettier.resolveConfig(filepath)) ?? {};
	return prettier.format(text, { ...config, filepath });
}

const written = [];
const pending = [];
/** Queues a file; nothing touches the disk until every file has been generated and formatted. */
function writeOut(path, text) {
	pending.push({ path, text });
}

async function flush() {
	const out = [];
	for (const { path, text } of pending) out.push({ path, text: await format(text, path) });
	for (const { path, text } of out) {
		mkdirSync(dirname(path), { recursive: true });
		writeFileSync(path, text);
		written.push(path);
	}
}

// --- lift (W0-C) -----------------------------------------------------------------------------

function ownerOf(name) {
	for (const [owner, names] of Object.entries(MAPPING.fieldOwners.explicit)) {
		if (names.includes(name)) return owner;
	}
	for (const [pattern, owner] of MAPPING.fieldOwners.patterns) {
		if (new RegExp(pattern, 'i').test(name)) return owner;
	}
	return fail(`no owner for state field ${name} (add it to mapping.json fieldOwners)`);
}

async function lift() {
	const src = readFileSync(ROUTE, 'utf8');
	const page = parsePage(src);
	const { code, cs, sf } = page;

	for (const reserved of ['s', 'fp']) {
		if (page.byName.has(reserved)) fail(`the page already declares a top-level ${reserved}`);
	}
	// A local named like the state alias would capture the rewritten references.
	(function scan(n) {
		if (ts.isIdentifier(n) && (n.text === 's' || n.text === 'fp')) {
			fail(`the script uses the name ${n.text} (offset ${n.getStart(sf)})`);
		}
		ts.forEachChild(n, scan);
	})(sf);

	const fields = page.infos.filter(
		(i) =>
			i.kind === 'state' ||
			i.kind === 'plain' ||
			(i.kind === 'const' && MAPPING.instanceConstants.includes(i.name))
	);
	const constants = MAPPING.constants.map((name) => {
		const info = page.byName.get(name);
		if (!info || info.kind !== 'const') fail(`constant ${name} not found`);
		return info;
	});
	const types = page.infos.filter((i) => i.kind === 'type');
	const creation = MAPPING.creationConstants.map((name) => {
		const info = page.byName.get(name);
		if (!info || info.kind !== 'const') fail(`creation constant ${name} not found`);
		return info;
	});
	const early = Object.keys(MAPPING.earlyPure).map((name) => {
		const info = page.byName.get(name);
		if (!info || info.kind !== 'function') fail(`early pure function ${name} not found`);
		return info;
	});
	const fieldNames = new Set(fields.map((f) => f.name));
	const unowned = fields.filter((f) => {
		try {
			ownerOf(f.name);
			return false;
		} catch {
			return true;
		}
	});
	if (unowned.length)
		fail(`no owner in mapping.json fieldOwners for: ${unowned.map((f) => f.name).join(', ')}`);

	// Expected counts: the contract says 185 $state; W0-B deleted duplicatingForm, leaving 184.
	const counts = Object.fromEntries(
		['state', 'plain'].map((k) => [k, page.infos.filter((i) => i.kind === k).length])
	);
	if (counts.state !== 184 || counts.plain !== 15)
		fail(`unexpected field counts ${JSON.stringify(counts)}`);
	if (types.length !== 60) fail(`expected 60 local types, found ${types.length}`);

	const removed = [...fields, ...constants, ...types, ...creation, ...early];
	const removedRanges = removed.map((info) => ({ info, ...statementRange(page, info.stmt) }));
	const inRemoved = (pos) =>
		removedRanges.some((r) => !creation.includes(r.info) && pos >= r.start && pos < r.end);

	// --- text of a moved declaration, with references rewritten for its new home ---
	function moved(info, { exportKeyword, ctx }) {
		const node = info.stmt;
		const range = statementRange(page, node);
		const refs = collectRefs(page, node);
		const edits = [];
		for (const ref of refs) {
			const { id, info: target } = ref;
			if (ref.query && (fieldNames.has(target.name) || target.kind === 'derived')) {
				fail(`${info.name}: typeof of reactive state ${target.name}`);
			}
			if (fieldNames.has(target.name)) {
				if (ctx !== 'class')
					fail(`${info.name} (outside the state class) reads state ${target.name}`);
				if (ts.isShorthandPropertyAssignment(id.parent)) {
					edits.push({
						start: id.getStart(sf),
						end: id.end,
						text: `${target.name}: this.${target.name}`
					});
				} else edits.push({ start: id.getStart(sf), end: id.end, text: `this.${target.name}` });
			} else if (
				target.kind === 'derived' ||
				(target.kind === 'function' && !MAPPING.earlyPure[target.name])
			) {
				fail(`${info.name} uses ${target.kind} ${target.name}, which stays in the page in W0-C`);
			} else if (target.kind === 'const' && !MAPPING.constants.includes(target.name)) {
				fail(`${info.name} uses the page constant ${target.name}`);
			}
		}
		const text = code.slice(range.start, range.end);
		const rel = edits.map((e) => ({
			...e,
			start: e.start - range.start,
			end: e.end - range.start
		}));
		let out = applyEdits(text, rel);
		if (exportKeyword) {
			// `export` goes in front of the declaration keyword, after any leading comments.
			const first = node.getStart(sf) - range.start;
			const shift = rel
				.filter((e) => e.start < first)
				.reduce((n, e) => n + e.text.length - (e.end - e.start), 0);
			out = out.slice(0, first + shift) + 'export ' + out.slice(first + shift);
		}
		return {
			text: dedent(out.replace(/\n$/, '')),
			refs: refs.filter((r) => !fieldNames.has(r.info.name) || ctx === 'class')
		};
	}

	// --- page-types.ts ---
	const typeParts = types.map((info) => moved(info, { exportKeyword: true, ctx: 'types' }));
	writeOut(
		join(LIB_FORMS, 'page-types.ts'),
		`${buildImports(
			mergeNeeds(typeParts.flatMap((p) => p.refs)).filter((n) => n.info.kind !== 'type'),
			LOCAL_MODULES.type
		)}\n\n${typeParts.map((p) => p.text).join('\n\n')}\n`
	);

	// --- page-constants.ts ---
	const constParts = constants.map((info) =>
		moved(info, { exportKeyword: true, ctx: 'constants' })
	);
	writeOut(
		join(LIB_FORMS, 'page-constants.ts'),
		`${buildImports(mergeNeeds(constParts.flatMap((p) => p.refs)), LOCAL_MODULES.constant)}\n\n${constParts.map((p) => p.text).join('\n\n')}\n`
	);

	// --- early pure functions (their final modules are completed in W0-D) ---
	const earlyByModule = new Map();
	for (const info of early) {
		const mod = MAPPING.earlyPure[info.name];
		if (!earlyByModule.has(mod)) earlyByModule.set(mod, []);
		earlyByModule.get(mod).push(moved(info, { exportKeyword: true, ctx: 'pure' }));
	}
	for (const [mod, parts] of earlyByModule) {
		const self = `#lib/forms/${mod}.js`;
		const needs = mergeNeeds(parts.flatMap((p) => p.refs)).filter(
			(n) => localModuleOf(n.info) !== self
		);
		writeOut(
			join(LIB_FORMS, `${mod}.ts`),
			`${buildImports(needs, self)}\n\n${parts.map((p) => p.text).join('\n\n')}\n`
		);
	}

	// --- state/forms-page-state.svelte.ts ---
	const stateParts = [];
	let lastOwner = null;
	for (const info of fields) {
		const owner = ownerOf(info.name);
		const d = info.decl;
		const range = statementRange(page, info.stmt);
		const refs = collectRefs(page, d);
		const edits = [];
		for (const ref of refs) {
			if (fieldNames.has(ref.info.name)) {
				edits.push({ start: ref.id.getStart(sf), end: ref.id.end, text: `this.${ref.info.name}` });
			} else if (ref.info.kind === 'function' && !MAPPING.earlyPure[ref.info.name]) {
				fail(`${info.name} initialiser calls ${ref.info.name}`);
			}
		}
		let initText = null;
		if (d.initializer) {
			const s0 = d.initializer.getStart(sf);
			const sub = edits
				.filter((e) => e.start >= s0 && e.end <= d.initializer.end)
				.map((e) => ({ ...e, start: e.start - s0, end: e.end - s0 }));
			initText = applyEdits(code.slice(s0, d.initializer.end), sub);
		}
		let line;
		if (info.kind === 'const') {
			line = `readonly ${d.name.text}${d.type ? `: ${d.type.getText(sf)}` : ''} = ${initText};`;
		} else if (info.kind === 'state') {
			line = `${d.name.text} = ${initText};`;
		} else if (initText === null) {
			if (!d.type || !/\bundefined\b/.test(d.type.getText(sf))) {
				fail(`plain field ${info.name} has no initialiser and its type does not allow undefined`);
			}
			line = `${d.name.text}: ${d.type.getText(sf)};`;
		} else {
			line = `${d.name.text}${d.type ? `: ${d.type.getText(sf)}` : ''} = ${initText};`;
		}
		const comments = code.slice(range.start, info.stmt.getStart(sf)).trim();
		const header = owner !== lastOwner ? `\n// owner: ${owner}\n` : '';
		lastOwner = owner;
		stateParts.push({ text: `${header}${comments ? `${comments}\n` : ''}${line}`, refs });
	}
	const fieldRefs = stateParts.flatMap((p) => p.refs).filter((r) => !fieldNames.has(r.info.name));
	const stateNeeds = mergeNeeds(fieldRefs);
	const stateImports = buildImports(stateNeeds, '#lib/forms/state/forms-page-state.svelte.js');
	writeOut(
		join(LIB_FORMS, 'state/forms-page-state.svelte.ts'),
		`import type { FormsPage } from '#lib/forms/state/forms-page.js';\n${stateImports}\n\n` +
			`/** Every piece of reactive state of the forms page (fields only; deriveds join in W0-D). */\n` +
			`export class FormsPageState {\nreadonly fp: FormsPage;\n\nconstructor(fp: FormsPage) {\nthis.fp = fp;\n}\n${stateParts.map((p) => p.text).join('\n')}\n}\n`
	);

	// --- state/controller.ts, forms-page.ts, context.ts ---
	writeOut(
		join(LIB_FORMS, 'state/controller.ts'),
		`import type { FormsPage } from '#lib/forms/state/forms-page.js';
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
`
	);
	writeOut(
		join(LIB_FORMS, 'state/forms-page.ts'),
		`import type { taskEditorAction } from '#lib/workbench/edit-surface.js';
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
`
	);
	writeOut(
		join(LIB_FORMS, 'state/context.ts'),
		`import { getContext, setContext } from 'svelte';
import type { FormsPage } from '#lib/forms/state/forms-page.js';

export const FORMS_PAGE = Symbol('forms-page');

export function setFormsPageContext(fp: FormsPage): FormsPage {
	return setContext(FORMS_PAGE, fp);
}

export function getFormsPageContext(): FormsPage {
	const fp = getContext<FormsPage | undefined>(FORMS_PAGE);
	if (!fp) throw new Error('getFormsPageContext() outside the forms page');
	return fp;
}
`
	);

	// --- the page: script ---
	const scriptEdits = [];
	const usedAfter = new Set();
	const usedBefore = new Set();
	const keptRefs = [];
	for (const ref of collectRefs(page, sf)) {
		const pos = ref.id.getStart(sf);
		if (ref.info.kind === 'import') usedBefore.add(ref.info);
		if (inRemoved(pos)) continue;
		keptRefs.push(ref);
		if (ref.info.kind === 'import') usedAfter.add(ref.info);
		if (fieldNames.has(ref.info.name)) {
			// `typeof viewDraft` becomes `typeof s.viewDraft`, which TypeScript accepts.
			const text = ts.isShorthandPropertyAssignment(ref.id.parent)
				? `${ref.info.name}: s.${ref.info.name}`
				: `s.${ref.info.name}`;
			scriptEdits.push({ start: cs + pos, end: cs + ref.id.end, text });
		}
	}
	for (const r of removedRanges)
		scriptEdits.push({ start: cs + r.start, end: cs + r.end, text: '' });

	// --- the page: markup ---
	const markupEdits = [];
	const markupNames = new Set();
	const rewriteSet = fieldNames;
	const declared = walkMarkup(page.ast.fragment, src, {
		identifier(node) {
			markupNames.add(node.name);
			if (rewriteSet.has(node.name)) {
				markupEdits.push({ start: node.start, end: node.end, text: `s.${node.name}` });
			}
		},
		typeIdentifier(node) {
			markupNames.add(node.name);
		},
		shorthand(prop) {
			markupNames.add(prop.key.name);
			if (rewriteSet.has(prop.key.name)) {
				markupEdits.push({
					start: prop.start,
					end: prop.end,
					text: `${prop.key.name}: s.${prop.key.name}`
				});
			}
		},
		shorthandAttribute(attr, expression) {
			markupNames.add(expression.name);
			if (rewriteSet.has(expression.name)) {
				markupEdits.push({
					start: attr.start,
					end: attr.end,
					text: `${attr.name}={s.${attr.name}}`
				});
			}
		}
	});
	for (const name of declared) {
		if (rewriteSet.has(name) || page.byName.get(name)?.kind === 'type') {
			fail(`the template declares ${name}, which shadows a moved top-level name`);
		}
		if (name === 's' || name === 'fp') fail(`the template declares ${name}`);
	}

	// --- the page: imports ---
	const pageImports = page.infos.filter((i) => i.kind === 'import');
	const usedNames = new Set([...markupNames]);
	for (const name of [...usedNames]) if (name.startsWith('$')) usedNames.add(name.slice(1));
	for (const ref of keptRefs) usedNames.add(ref.info.name);
	// `$t` style store references are plain identifiers the checker cannot resolve.
	(function scan(n) {
		if (ts.isIdentifier(n) && n.text.startsWith('$') && !inRemoved(n.getStart(sf))) {
			usedNames.add(n.text.slice(1));
		}
		ts.forEachChild(n, scan);
	})(sf);
	const removedImports = new Set();
	for (const info of pageImports) {
		const wasUsed =
			usedBefore.has(info) || STORE_NAMES.has(info.name) || markupNames.has(info.name);
		const isUsed = usedNames.has(info.name);
		if (wasUsed && !isUsed) removedImports.add(info);
	}
	const importEdits = [];
	const byStmt = new Map();
	for (const info of pageImports) {
		if (!byStmt.has(info.stmt)) byStmt.set(info.stmt, []);
		byStmt.get(info.stmt).push(info);
	}
	for (const [st, infos] of byStmt) {
		const dropped = infos.filter((i) => removedImports.has(i));
		if (!dropped.length) continue;
		const range = statementRange(page, st);
		if (dropped.length === infos.length) {
			importEdits.push({ start: cs + range.start, end: cs + range.end, text: '' });
			continue;
		}
		const kept = infos.filter((i) => !removedImports.has(i));
		const stmtType = st.importClause.isTypeOnly;
		const def = kept.find((i) => i.form === 'default');
		const ns = kept.find((i) => i.form === 'namespace');
		const named = kept
			.filter((i) => i.form === 'named')
			.map(
				(i) =>
					`${i.decl.isTypeOnly ? 'type ' : ''}${i.imported === i.name ? i.name : `${i.imported} as ${i.name}`}`
			);
		const parts = [];
		if (def) parts.push(def.name);
		if (ns) parts.push(`* as ${ns.name}`);
		if (named.length) parts.push(`{ ${named.join(', ')} }`);
		importEdits.push({
			start: cs + st.getStart(sf),
			end: cs + st.end,
			text: `import ${stmtType ? 'type ' : ''}${parts.join(', ')} from '${st.moduleSpecifier.text}';`
		});
	}

	// New imports the page needs from the generated modules.
	const neededLocal = new Map();
	const consider = (info) => {
		if (localModuleOf(info)) neededLocal.set(info, { info, typeOnly: info.kind === 'type' });
	};
	for (const ref of keptRefs) consider(ref.info);
	for (const name of markupNames) {
		const info = page.byName.get(name);
		if (info && (info.kind === 'type' || localModuleOf(info))) consider(info);
	}
	// Types used as annotations in kept code are references too (collectRefs saw them).
	const localImports = buildImports([...neededLocal.values()], '');
	const lastImport = [...page.infos].filter((i) => i.kind === 'import').at(-1).stmt;
	const insertAt = cs + lastImport.end;
	const creationText = creation
		.map((info) => dedent(code.slice(info.stmt.getStart(sf), info.stmt.end)))
		.join('\n');
	const block = `\n${localImports}\nimport { FormsPage } from '#lib/forms/state/forms-page.js';\nimport { setFormsPageContext } from '#lib/forms/state/context.js';\n\n${creationText}\nconst fp = new FormsPage({ workbench, recordEditorAction });\nsetFormsPageContext(fp);\nconst s = fp.state;\n`;
	scriptEdits.push({ start: insertAt, end: insertAt, text: block });

	const all = [...scriptEdits, ...importEdits, ...markupEdits];
	writeOut(ROUTE, applyEdits(src, all));
	await flush();
	return {
		fields: fields.length,
		types: types.length,
		constants: constants.length,
		removedImports: [...removedImports].map((i) => i.name)
	};
}

// --- main ------------------------------------------------------------------------------------

async function main() {
	const command = process.argv[2];
	if (command === 'lift') {
		const summary = await lift();
		console.log(`codemod lift: ${JSON.stringify(summary)}`);
		for (const path of written) console.log(`  wrote ${path.replace(`${FE}/`, '')}`);
	} else {
		console.error('usage: codemod.mjs lift');
		process.exitCode = 2;
	}
}

if (!existsSync(ROUTE)) {
	console.error(`codemod: ${ROUTE} not found`);
	process.exit(1);
}
main().catch((error) => {
	if (error instanceof Abort) {
		console.error(`codemod: ABORT ${error.message}`);
		process.exit(1);
	}
	throw error;
});
