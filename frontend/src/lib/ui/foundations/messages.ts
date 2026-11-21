export type Messages = { [key: string]: string | Messages };

function isMessages(value: unknown): value is Messages {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/**
 * Deep-merges message trees. A key defined twice with different content throws, so two
 * components that both claim the same `ui.*` key fail loudly instead of one silently winning.
 */
export function mergeMessages(base: Messages, ...parts: Messages[]): Messages {
	const out: Messages = structuredClone(base);
	const visit = (target: Messages, source: Messages, path: string) => {
		for (const [key, value] of Object.entries(source)) {
			const here = path ? `${path}.${key}` : key;
			const existing = target[key];
			if (isMessages(value)) {
				if (existing === undefined) target[key] = {};
				else if (!isMessages(existing)) throw new Error(`message key conflict at ${here}`);
				visit(target[key] as Messages, value, here);
			} else {
				if (existing !== undefined && existing !== value)
					throw new Error(`message key conflict at ${here}`);
				target[key] = value;
			}
		}
	};
	for (const part of parts) visit(out, part, '');
	return out;
}
