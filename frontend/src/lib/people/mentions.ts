// Mentions in comment text. Rendering highlights only the `@Name` tokens of the people the saved
// comment says it mentions (the answer's `mentions`); a plain `@word` is not a mention. The
// composer helpers keep the list of people picked from the picker and send only those whose
// `@Name` is still in the text; names typed by hand are resolved by the server.

import { personLabel, usableName, type Person, type Translate } from './display.js';

export interface MentionSegment {
	text: string;
	/** Set for a highlighted `@Name`: the mentioned person. */
	mention?: Person;
}

const WORD_CHAR = /[\p{L}\p{N}_]/u;
const ASCII_WORD_CHAR = /[A-Za-z0-9_]/;

function escapeHtml(text: string): string {
	return text
		.replace(/&/g, '&amp;')
		.replace(/</g, '&lt;')
		.replace(/>/g, '&gt;')
		.replace(/"/g, '&quot;')
		.replace(/'/g, '&#39;');
}

interface Candidate {
	token: string;
	person: Person;
}

/** `@name` for every mentioned person who has a name, longest first. */
function candidates(mentions: readonly Person[], escape: boolean): Candidate[] {
	const list: Candidate[] = [];
	for (const person of mentions) {
		const name = usableName(person.name);
		if (!name) continue;
		list.push({ token: `@${escape ? escapeHtml(name) : name}`, person });
	}
	return list.sort((a, b) => b.token.length - a.token.length);
}

function segmentsOf(text: string, list: readonly Candidate[]): MentionSegment[] {
	if (list.length === 0 || !text.includes('@')) return text ? [{ text }] : [];
	const lower = text.toLowerCase();
	const segments: MentionSegment[] = [];
	let plainFrom = 0;
	let at = 0;
	while (at < text.length) {
		const found = text.charAt(at) === '@' && !(at > 0 && WORD_CHAR.test(text.charAt(at - 1)));
		const hit = found
			? list.find((item) => {
					if (!lower.startsWith(item.token.toLowerCase(), at)) return false;
					const next = text.charAt(at + item.token.length);
					// A Latin name ends at a word boundary; a Chinese name needs no space after it.
					return !(
						next &&
						ASCII_WORD_CHAR.test(item.token.charAt(item.token.length - 1)) &&
						WORD_CHAR.test(next)
					);
				})
			: undefined;
		if (!hit) {
			at += 1;
			continue;
		}
		if (at > plainFrom) segments.push({ text: text.slice(plainFrom, at) });
		segments.push({ text: text.slice(at, at + hit.token.length), mention: hit.person });
		at += hit.token.length;
		plainFrom = at;
	}
	if (plainFrom < text.length) segments.push({ text: text.slice(plainFrom) });
	return segments;
}

/** Plain text split into plain and mention parts (for text shown without markup). */
export function mentionSegments(text: string, mentions: readonly Person[]): MentionSegment[] {
	return segmentsOf(text, candidates(mentions, false));
}

/**
 * Highlights the mentions in rendered, already sanitised comment HTML. Only text outside tags,
 * links and code is touched; the tooltip is the person's current name.
 */
export function highlightMentions(html: string, mentions: readonly Person[], t: Translate): string {
	const list = candidates(mentions, true);
	if (list.length === 0 || !html.includes('@')) return html;
	let skip = 0;
	return html
		.split(/(<[^>]*>)/)
		.map((part) => {
			if (part.startsWith('<')) {
				const tag = /^<(\/?)(a|code|pre)\b/i.exec(part);
				if (tag) skip = Math.max(0, skip + (tag[1] ? -1 : 1));
				return part;
			}
			if (skip > 0) return part;
			return segmentsOf(part, list)
				.map((segment) =>
					segment.mention
						? `<span class="comment-mention" title="${escapeHtml(personLabel(segment.mention, t))}">${segment.text}</span>`
						: segment.text
				)
				.join('');
		})
		.join('');
}

/** A person picked from the picker, and the text that was put into the comment for them. */
export interface PickedMention {
	id: string;
	/** What was inserted after the `@`. */
	text: string;
}

/** Where an `@` picker query starts before the cursor: `start` is null when there is none. */
export function mentionTrigger(
	text: string,
	cursor: number
): { start: number | null; filter: string } {
	const before = text.slice(0, cursor);
	const match = before.match(/(^|\s)@([^\s@]*)$/);
	if (!match) return { start: null, filter: '' };
	return { start: before.lastIndexOf('@'), filter: match[2] ?? '' };
}

/** Puts `@Name ` in place of the `@query` the picker was opened on. */
export function insertMentionText(
	text: string,
	start: number | null,
	cursor: number,
	name: string
): { text: string; cursor: number } {
	const at = start ?? text.lastIndexOf('@', Math.max(0, cursor - 1));
	if (at < 0) return { text, cursor };
	const before = text.slice(0, at);
	const inserted = `@${name} `;
	return {
		text: `${before}${inserted}${text.slice(cursor)}`,
		cursor: before.length + inserted.length
	};
}

/** The picked list with this person added (one entry per person). */
export function withPicked(
	picked: readonly PickedMention[],
	entry: PickedMention
): PickedMention[] {
	return [...picked.filter((item) => item.id !== entry.id), entry];
}

/** The ids to send: picked people whose `@Name` is still in the text. */
export function pickedIds(body: string, picked: readonly PickedMention[]): string[] {
	const lower = body.toLowerCase();
	return picked
		.filter((item) => lower.includes(`@${item.text.toLowerCase()}`))
		.map((item) => item.id);
}
