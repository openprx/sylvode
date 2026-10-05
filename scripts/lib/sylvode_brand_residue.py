#!/usr/bin/env python3
"""Brand residue gate (ADR-0020 判据 4): every `OpenPR` / `openpr` in the tracked files of the
Sylvode checkouts is either covered by a narrow, reasoned allow-list entry or a failure.

A hit is one occurrence of `OpenPR` not followed by `X` (case-sensitive) or of `openpr` not
followed by `x`. Files come from `git ls-files`, so build output, dependencies and anything
ignored are excluded by construction.

An allow-list entry covers a hit when its repository matches, one of its path globs matches the
file, and one match of its pattern on that line spans the whole hit. Entries are refused when
they are malformed, carry a reason outside the closed set, or waive the bare name across a whole
repository. An entry that covers nothing in a scanned repository is stale.

Exit codes: 0 clean; 1 uncovered hits, an unreachable or empty repository, or (with --strict)
a stale entry; 2 usage error or a refused allow-list.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path

HIT = re.compile(r"OpenPR(?!X)|openpr(?!x)")

REASONS = (
    "stable_identifier",
    "legacy_alias_documented",
    "formerly_note",
    "migration_page",
    "redirect_rule",
    "historical_document",
    "frozen_evidence",
    "legacy_behaviour_test",
    "kept_repository_url",
)

EXPECTED_REPOS = ("sylvode", "openpr-webhook", "docs", "site", ".github")

# Strings a blanket waiver would cover: the bare name in ordinary prose.
BLANKET_PROBES = (
    "openpr",
    "OpenPR",
    "the openpr product",
    "The OpenPR product",
    "OpenPR.",
    "(openpr)",
    "use OpenPR to",
)


class AllowlistError(Exception):
    """The allow-list is malformed or too broad; the gate refuses to run with it."""


def glob_to_regex(glob: str) -> re.Pattern[str]:
    out = []
    i = 0
    while i < len(glob):
        char = glob[i]
        if glob.startswith("**/", i):
            out.append("(?:.*/)?")
            i += 3
        elif glob.startswith("**", i):
            out.append(".*")
            i += 2
        elif char == "*":
            out.append("[^/]*")
            i += 1
        elif char == "?":
            out.append("[^/]")
            i += 1
        else:
            out.append(re.escape(char))
            i += 1
    return re.compile("".join(out) + r"\Z")


def is_repository_wide(glob: str) -> bool:
    """A glob without a literal leading directory or file name reaches the whole repository."""
    first = glob.split("/", 1)[0]
    return "*" in first or "?" in first


@dataclass
class Entry:
    id: str
    reason: str
    repo: str
    paths: list[str]
    pattern: re.Pattern[str]
    explanation: str
    path_res: list[re.Pattern[str]] = field(default_factory=list)
    covered: int = 0

    def covers(self, repo: str, path: str, line: str, start: int, end: int) -> bool:
        if repo != self.repo or not any(rx.match(path) for rx in self.path_res):
            return False
        return any(m.start() <= start and end <= m.end() for m in self.pattern.finditer(line))


def covers_bare_name(pattern: re.Pattern[str]) -> bool:
    for probe in BLANKET_PROBES:
        for hit in HIT.finditer(probe):
            if any(m.start() <= hit.start() and hit.end() <= m.end() for m in pattern.finditer(probe)):
                return True
    return False


def load_allowlist(path: Path) -> list[Entry]:
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise AllowlistError(f"cannot read allow-list {path}: {error}") from error
    if not isinstance(document, dict) or document.get("schema") != "sylvode.brand-residue-allowlist.v1":
        raise AllowlistError('allow-list must be an object with "schema": "sylvode.brand-residue-allowlist.v1"')
    raw_entries = document.get("entries")
    if not isinstance(raw_entries, list) or not raw_entries:
        raise AllowlistError('allow-list "entries" must be a non-empty array')
    entries: list[Entry] = []
    seen: set[str] = set()
    problems: list[str] = []
    for index, raw in enumerate(raw_entries):
        where = f"entries[{index}]"
        if not isinstance(raw, dict):
            problems.append(f"{where}: not an object")
            continue
        unknown = set(raw) - {"id", "reason", "repo", "paths", "pattern", "explanation"}
        if unknown:
            problems.append(f"{where}: unknown keys {sorted(unknown)}")
        entry_id = raw.get("id")
        if not isinstance(entry_id, str) or not re.fullmatch(r"[a-z0-9][a-z0-9._-]{2,80}", entry_id):
            problems.append(f"{where}: id must be a lowercase slug")
            continue
        where = f"entry {entry_id}"
        if entry_id in seen:
            problems.append(f"{where}: duplicate id")
        seen.add(entry_id)
        reason = raw.get("reason")
        if reason not in REASONS:
            problems.append(f"{where}: reason {reason!r} is not one of {', '.join(REASONS)}")
        repo = raw.get("repo")
        if not isinstance(repo, str) or not repo:
            problems.append(f"{where}: repo must be a repository name")
        paths = raw.get("paths")
        if (
            not isinstance(paths, list)
            or not paths
            or not all(isinstance(p, str) and p and not p.startswith("/") for p in paths)
        ):
            problems.append(f"{where}: paths must be a non-empty array of repository-relative globs")
            paths = []
        explanation = raw.get("explanation")
        if not isinstance(explanation, str) or len(explanation.strip()) < 20:
            problems.append(f"{where}: explanation must say why (at least 20 characters)")
        pattern_text = raw.get("pattern")
        try:
            pattern = re.compile(pattern_text) if isinstance(pattern_text, str) and pattern_text else None
        except re.error as error:
            problems.append(f"{where}: pattern does not compile: {error}")
            pattern = None
        if pattern is None:
            problems.append(f"{where}: pattern must be a non-empty regular expression")
            continue
        if covers_bare_name(pattern) and any(is_repository_wide(p) for p in paths):
            problems.append(
                f"{where}: blanket waiver refused: a repository-wide path ({', '.join(paths)}) with a "
                "pattern that matches the bare name; narrow the paths or the pattern"
            )
        entries.append(
            Entry(
                id=entry_id,
                reason=str(reason),
                repo=str(repo),
                paths=list(paths),
                pattern=pattern,
                explanation=str(explanation),
                path_res=[glob_to_regex(p) for p in paths],
            )
        )
    if problems:
        raise AllowlistError("allow-list refused:\n  " + "\n  ".join(problems))
    return entries


def git(path: Path, *args: str) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(["git", "-C", str(path), *args], capture_output=True, check=False)


def scan_repo(name: str, path: Path, entries: list[Entry]) -> dict:
    result: dict = {
        "name": name,
        "path": str(path),
        "reachable": False,
        "head": None,
        "dirty": None,
        "files_scanned": 0,
        "hits": 0,
        "covered": 0,
        "hits_by_reason": {reason: 0 for reason in REASONS},
        "uncovered": [],
        "errors": [],
    }
    if not path.is_dir():
        result["errors"].append("not a directory")
        return result
    top = git(path, "rev-parse", "--show-toplevel")
    if top.returncode != 0:
        result["errors"].append("not a git checkout")
        return result
    if Path(top.stdout.decode().strip()).resolve() != path.resolve():
        result["errors"].append(f"not the root of a git checkout (root is {top.stdout.decode().strip()})")
        return result
    head = git(path, "rev-parse", "HEAD")
    if head.returncode != 0:
        result["errors"].append("checkout has no commit")
        return result
    result["head"] = head.stdout.decode().strip()
    status = git(path, "status", "--porcelain=v1")
    result["dirty"] = bool(status.stdout.strip())
    listed = git(path, "ls-files", "-z")
    if listed.returncode != 0:
        result["errors"].append("git ls-files failed")
        return result
    files = [f for f in listed.stdout.decode("utf-8", "surrogateescape").split("\0") if f]
    result["reachable"] = True
    for rel in files:
        full = path / rel
        if full.is_symlink() or not full.is_file():
            continue
        try:
            data = full.read_bytes()
        except OSError as error:
            result["errors"].append(f"{rel}: {error}")
            continue
        result["files_scanned"] += 1
        if b"\0" in data[:8192]:
            continue
        text = data.decode("utf-8", "replace")
        if not HIT.search(text):
            continue
        for number, line in enumerate(text.splitlines(), start=1):
            for hit in HIT.finditer(line):
                result["hits"] += 1
                owner = next(
                    (e for e in entries if e.covers(name, rel, line, hit.start(), hit.end())),
                    None,
                )
                if owner is None:
                    result["uncovered"].append(
                        {"file": rel, "line": number, "column": hit.start() + 1, "text": line.strip()[:240]}
                    )
                else:
                    owner.covered += 1
                    result["covered"] += 1
                    result["hits_by_reason"][owner.reason] += 1
    if result["files_scanned"] == 0:
        result["errors"].append("zero files scanned")
    return result


def run_scan(repos: list[tuple[str, Path]], allowlist: Path, strict: bool, release: bool) -> tuple[dict, int]:
    entries = load_allowlist(allowlist)
    report: dict = {
        "schema": "sylvode.brand-residue.v1",
        "allowlist": str(allowlist),
        "allowlist_entries": len(entries),
        "strict": strict,
        "release": release,
        "repos": [],
        "stale_entries": [],
        "missing_expected_repos": [],
    }
    names = [name for name, _ in repos]
    if release:
        report["missing_expected_repos"] = [n for n in EXPECTED_REPOS if n not in names]
    for name, path in repos:
        report["repos"].append(scan_repo(name, path, entries))
    scanned = {r["name"] for r in report["repos"] if r["reachable"]}
    report["stale_entries"] = [
        {"id": e.id, "repo": e.repo, "reason": e.reason} for e in entries if e.repo in scanned and e.covered == 0
    ]
    report["entries_by_reason"] = {reason: sum(1 for e in entries if e.reason == reason) for reason in REASONS}
    uncovered = sum(len(r["uncovered"]) for r in report["repos"])
    broken = [r["name"] for r in report["repos"] if r["errors"] and (not r["reachable"] or r["files_scanned"] == 0)]
    report["summary"] = {
        "repos": len(report["repos"]),
        "files_scanned": sum(r["files_scanned"] for r in report["repos"]),
        "hits": sum(r["hits"] for r in report["repos"]),
        "covered": sum(r["covered"] for r in report["repos"]),
        "uncovered": uncovered,
        "unreachable_or_empty": broken,
        "stale_entries": len(report["stale_entries"]),
        "warnings": len(report["stale_entries"]) if not strict else 0,
    }
    failed = bool(
        uncovered
        or broken
        or report["missing_expected_repos"]
        or (strict and report["stale_entries"])
        or not repos
    )
    report["passed"] = not failed
    return report, 1 if failed else 0


# ---------------------------------------------------------------------------------------------
# Built-in mutation controls. Everything happens in a temporary copy; the real trees are only read.
# ---------------------------------------------------------------------------------------------


def copy_checkout(source: Path, destination: Path) -> None:
    listed = git(source, "ls-files", "-z")
    if listed.returncode != 0:
        raise RuntimeError(f"cannot list {source}")
    for rel in [f for f in listed.stdout.decode("utf-8", "surrogateescape").split("\0") if f]:
        src = source / rel
        if src.is_symlink() or not src.is_file():
            continue
        dst = destination / rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(src, dst)
    env = {**os.environ, "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t", "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t"}
    for args in (["init", "-q"], ["add", "-A"], ["commit", "-q", "--no-gpg-sign", "-m", "copy"]):
        done = subprocess.run(["git", "-C", str(destination), *args], capture_output=True, env=env, check=False)
        if done.returncode != 0:
            raise RuntimeError(f"git {' '.join(args)} failed in the copy: {done.stderr.decode()}")


def self_test(name: str, source: Path, allowlist: Path) -> tuple[dict, int]:
    controls: list[dict] = []
    with tempfile.TemporaryDirectory(prefix="sylvode-brand-residue-") as tmp:
        root = Path(tmp)
        copy = root / "copy"
        copy.mkdir()
        copy_checkout(source, copy)
        base, base_exit = run_scan([(name, copy)], allowlist, strict=True, release=False)
        controls.append(
            {"control": "baseline_copy_is_clean", "expected_exit": 0, "exit": base_exit,
             "uncovered": base["summary"]["uncovered"], "stale": base["summary"]["stale_entries"]}
        )

        # 1. An uncovered product name in a scanned file goes red.
        target = copy / "README.md"
        original = target.read_text(encoding="utf-8")
        target.write_text(original + "\nRun OpenPR for this.\n", encoding="utf-8")
        injected, injected_exit = run_scan([(name, copy)], allowlist, strict=True, release=False)
        target.write_text(original, encoding="utf-8")
        found = any(u["file"] == "README.md" and "Run OpenPR for this." in u["text"] for r in injected["repos"] for u in r["uncovered"])
        controls.append({"control": "injected_uncovered_name_is_red", "expected_exit": 1, "exit": injected_exit, "reported": found})

        # 2. Deleting an entry that is in use goes red.
        document = json.loads(allowlist.read_text(encoding="utf-8"))
        counted = load_allowlist(allowlist)
        scan_repo(name, copy, counted)
        in_use = max((e for e in counted if e.repo == name), key=lambda e: e.covered, default=None)
        if in_use is None or in_use.covered == 0:
            controls.append({"control": "deleted_entry_in_use_is_red", "expected_exit": 1, "exit": None, "error": "no entry in use"})
        else:
            pruned = dict(document)
            pruned["entries"] = [e for e in document["entries"] if e["id"] != in_use.id]
            pruned_path = root / "pruned.json"
            pruned_path.write_text(json.dumps(pruned), encoding="utf-8")
            pruned_report, pruned_exit = run_scan([(name, copy)], pruned_path, strict=True, release=False)
            controls.append({"control": "deleted_entry_in_use_is_red", "expected_exit": 1, "exit": pruned_exit,
                             "deleted": in_use.id, "uncovered": pruned_report["summary"]["uncovered"]})

        # 3. A blanket entry is refused.
        blanket = dict(document)
        blanket["entries"] = document["entries"] + [{
            "id": "blanket-waiver", "reason": "historical_document", "repo": name, "paths": ["**"],
            "pattern": "[Oo]pen[Pp][Rr]", "explanation": "a waiver for every occurrence in the repository",
        }]
        blanket_path = root / "blanket.json"
        blanket_path.write_text(json.dumps(blanket), encoding="utf-8")
        try:
            run_scan([(name, copy)], blanket_path, strict=True, release=False)
            controls.append({"control": "blanket_entry_is_refused", "expected_exit": 2, "exit": 0})
        except AllowlistError:
            controls.append({"control": "blanket_entry_is_refused", "expected_exit": 2, "exit": 2})

        # 4. A requested repository that is an empty directory is red, not zero hits.
        empty = root / "empty"
        empty.mkdir()
        empty_report, empty_exit = run_scan([(name, empty)], allowlist, strict=False, release=False)
        controls.append({"control": "empty_directory_is_red", "expected_exit": 1, "exit": empty_exit,
                         "errors": empty_report["repos"][0]["errors"]})

        # 4b. A requested repository that does not exist is red.
        missing_report, missing_exit = run_scan([(name, root / "does-not-exist")], allowlist, strict=False, release=False)
        controls.append({"control": "missing_directory_is_red", "expected_exit": 1, "exit": missing_exit,
                         "errors": missing_report["repos"][0]["errors"]})

        # 4c. A checkout whose scan examined zero files is red, not zero hits.
        bare = root / "zero-files"
        bare.mkdir()
        env = {**os.environ, "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t", "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t"}
        for args in (["init", "-q"], ["commit", "-q", "--allow-empty", "--no-gpg-sign", "-m", "empty"]):
            subprocess.run(["git", "-C", str(bare), *args], capture_output=True, env=env, check=False)
        zero_report, zero_exit = run_scan([(name, bare)], allowlist, strict=False, release=False)
        controls.append({"control": "zero_files_scanned_is_red", "expected_exit": 1, "exit": zero_exit,
                         "files_scanned": zero_report["repos"][0]["files_scanned"],
                         "errors": zero_report["repos"][0]["errors"]})

        # 4d. --release without the five expected repositories is red.
        release_report, release_exit = run_scan([(name, copy)], allowlist, strict=False, release=True)
        controls.append({"control": "release_requires_all_expected_repos", "expected_exit": 1, "exit": release_exit,
                         "missing": release_report["missing_expected_repos"]})

        # 5. A stale entry is a warning, and a failure under --strict.
        stale = dict(document)
        stale["entries"] = document["entries"] + [{
            "id": "stale-entry-control", "reason": "stable_identifier", "repo": name, "paths": ["no/such/file.txt"],
            "pattern": "openpr_never_present_anywhere", "explanation": "an entry that matches nothing in the copy",
        }]
        stale_path = root / "stale.json"
        stale_path.write_text(json.dumps(stale), encoding="utf-8")
        lax, lax_exit = run_scan([(name, copy)], stale_path, strict=False, release=False)
        strict_report, strict_exit = run_scan([(name, copy)], stale_path, strict=True, release=False)
        controls.append({"control": "stale_entry_warns", "expected_exit": 0, "exit": lax_exit,
                         "warnings": lax["summary"]["warnings"]})
        controls.append({"control": "stale_entry_fails_strict", "expected_exit": 1, "exit": strict_exit,
                         "stale": strict_report["summary"]["stale_entries"]})

    ok = all(c.get("exit") == c["expected_exit"] for c in controls)
    ok = ok and next(c for c in controls if c["control"] == "injected_uncovered_name_is_red").get("reported") is True
    ok = ok and next(c for c in controls if c["control"] == "stale_entry_warns").get("warnings", 0) >= 1
    return {"schema": "sylvode.brand-residue-self-test.v1", "repo": name, "controls": controls, "passed": ok}, 0 if ok else 1


def parse_repo(value: str) -> tuple[str, Path]:
    name, sep, path = value.partition("=")
    if not sep or not name or not path:
        raise argparse.ArgumentTypeError(f"--repo expects name=path, got {value!r}")
    return name, Path(path)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="verify-sylvode-brand-residue", add_help=True)
    parser.add_argument("--repo", action="append", type=parse_repo, default=[])
    parser.add_argument("--allowlist", type=Path, required=True)
    parser.add_argument("--strict", action="store_true")
    parser.add_argument("--release", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)
    names = [name for name, _ in args.repo]
    if len(names) != len(set(names)):
        print("FAIL: a repository name is given twice", file=sys.stderr)
        return 2
    try:
        if args.self_test:
            name, path = args.repo[0]
            report, code = self_test(name, path, args.allowlist)
        else:
            report, code = run_scan(args.repo, args.allowlist, args.strict, args.release)
    except AllowlistError as error:
        print(json.dumps({"schema": "sylvode.brand-residue.v1", "passed": False, "refused": str(error)}, indent=2))
        print(f"FAIL: {error}", file=sys.stderr)
        return 2
    print(json.dumps(report, indent=2, sort_keys=False))
    return code


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
