// The artifact self-check: no output file may contain a token prefix or a test password.
//
// The findings name the file and the needle's label, never the needle, so the report itself is
// safe to keep.

import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';

export interface Needle {
	label: string;
	value: string;
}

export interface Finding {
	file: string;
	needle: string;
}

export interface ScanReport {
	files: number;
	findings: Finding[];
}

export function scanDirectory(
	dir: string,
	needles: Needle[],
	skip: (path: string) => boolean = () => false
): ScanReport {
	const findings: Finding[] = [];
	let files = 0;
	const usable = needles.filter((needle) => needle.value.length > 0);
	const walk = (current: string): void => {
		for (const entry of readdirSync(current)) {
			const path = join(current, entry);
			if (skip(path)) continue;
			const info = statSync(path);
			if (info.isDirectory()) {
				walk(path);
				continue;
			}
			files += 1;
			const content = readFileSync(path);
			for (const needle of usable) {
				if (content.includes(needle.value)) findings.push({ file: path, needle: needle.label });
			}
		}
	};
	walk(dir);
	return { files, findings };
}
