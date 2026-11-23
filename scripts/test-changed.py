#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
from pathlib import Path, PurePosixPath
import subprocess
import sys


class ScriptError(Exception):
    pass


def run_capture(cmd: list[str], cwd: Path | None = None) -> str:
    try:
        proc = subprocess.run(
            cmd,
            cwd=cwd,
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        raise ScriptError(f"failed to run {' '.join(cmd)}: {exc}") from exc
    if proc.returncode != 0:
        detail = proc.stderr.strip()
        if detail:
            raise ScriptError(
                f"command failed (exit {proc.returncode}): {' '.join(cmd)}: {detail}"
            )
        raise ScriptError(f"command failed (exit {proc.returncode}): {' '.join(cmd)}")
    return proc.stdout


def require_field(obj: dict, key: str, expected_type: type, context: str):
    if key not in obj:
        raise ScriptError(f"{context} missing required field '{key}'")
    value = obj[key]
    if not isinstance(value, expected_type):
        raise ScriptError(
            f"{context} field '{key}' has unexpected type {type(value).__name__}"
        )
    return value


def read_files_from(path_str: str) -> list[str]:
    try:
        text = Path(path_str).read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as exc:
        raise ScriptError(f"cannot read {path_str}: {exc}") from exc
    return [line.strip() for line in text.splitlines() if line.strip()]


def git_changed_files(base: str) -> tuple[Path, list[str]]:
    top = run_capture(["git", "rev-parse", "--show-toplevel"]).strip()
    if not top:
        raise ScriptError("git rev-parse --show-toplevel returned empty output")
    repo_root = Path(top).resolve()
    merge_base = run_capture(
        ["git", "merge-base", "HEAD", base], cwd=repo_root
    ).strip()
    if not merge_base:
        raise ScriptError(f"git merge-base HEAD {base} returned empty output")
    diff_out = run_capture(["git", "diff", "--name-only", merge_base], cwd=repo_root)
    untracked_out = run_capture(
        ["git", "ls-files", "--others", "--exclude-standard"], cwd=repo_root
    )
    seen: set[str] = set()
    files: list[str] = []
    for output in (diff_out, untracked_out):
        for line in output.splitlines():
            stripped = line.strip()
            if stripped and stripped not in seen:
                seen.add(stripped)
                files.append(stripped)
    return repo_root, files


def load_packages(
    metadata_cwd: Path, repo_root: Path | None
) -> tuple[Path, dict[str, tuple[str, ...]], dict[str, set[str]]]:
    raw = run_capture(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"], cwd=metadata_cwd
    )
    try:
        data = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise ScriptError(f"failed to parse cargo metadata JSON: {exc}") from exc
    if not isinstance(data, dict):
        raise ScriptError("cargo metadata output must be a JSON object")

    workspace_root_str = require_field(data, "workspace_root", str, "cargo metadata")
    resolved_root = (repo_root or Path(workspace_root_str)).resolve()
    workspace_members_raw = require_field(
        data, "workspace_members", list, "cargo metadata"
    )
    workspace_members: set[str] = set()
    for entry in workspace_members_raw:
        if not isinstance(entry, str):
            raise ScriptError(
                "cargo metadata field 'workspace_members' contains non-string entry"
            )
        workspace_members.add(entry)

    packages_raw = require_field(data, "packages", list, "cargo metadata")
    pkg_dirs: dict[str, tuple[str, ...]] = {}
    raw_deps: dict[str, list[str]] = {}

    for pkg in packages_raw:
        if not isinstance(pkg, dict):
            raise ScriptError(
                "cargo metadata field 'packages' contains non-object entry"
            )
        pkg_id = require_field(pkg, "id", str, "cargo metadata package")
        if pkg_id not in workspace_members:
            continue
        name = require_field(pkg, "name", str, "cargo metadata package")
        manifest_path = require_field(pkg, "manifest_path", str, f"package '{name}'")
        deps_raw = require_field(pkg, "dependencies", list, f"package '{name}'")

        pkg_dir = Path(manifest_path).resolve().parent
        try:
            rel_dir = pkg_dir.relative_to(resolved_root)
        except ValueError as exc:
            raise ScriptError(
                f"package '{name}' directory '{pkg_dir}' is outside repository root '{resolved_root}'"
            ) from exc
        pkg_dirs[name] = rel_dir.parts

        path_dep_names: list[str] = []
        for dep in deps_raw:
            if not isinstance(dep, dict):
                raise ScriptError(
                    f"package '{name}' field 'dependencies' contains non-object entry"
                )
            dep_name = require_field(dep, "name", str, f"package '{name}' dependency")
            dep_path = dep.get("path")
            if dep_path is not None:
                if not isinstance(dep_path, str):
                    raise ScriptError(
                        f"package '{name}' dependency '{dep_name}' field 'path' has unexpected type {type(dep_path).__name__}"
                    )
                path_dep_names.append(dep_name)
        raw_deps[name] = path_dep_names

    dependents: dict[str, set[str]] = {name: set() for name in pkg_dirs}
    for name, dep_names in raw_deps.items():
        for dep_name in dep_names:
            if dep_name in pkg_dirs and dep_name != name:
                dependents[dep_name].add(name)

    return resolved_root, pkg_dirs, dependents


def match_package(
    file_parts: tuple[str, ...], sorted_pkg_dirs: list[tuple[str, tuple[str, ...]]]
) -> str | None:
    for pkg_name, dir_parts in sorted_pkg_dirs:
        if (
            dir_parts
            and len(file_parts) >= len(dir_parts)
            and file_parts[: len(dir_parts)] == dir_parts
        ):
            return pkg_name
    return None


def is_ignored_doc(file_parts: tuple[str, ...]) -> bool:
    if len(file_parts) >= 2 and file_parts[0] == "docs":
        return True
    if len(file_parts) == 1 and file_parts[0].endswith(".md"):
        return True
    return False


def affected_packages(
    changed_files: list[str],
    pkg_dirs: dict[str, tuple[str, ...]],
    dependents: dict[str, set[str]],
) -> set[str]:
    all_names = set(pkg_dirs.keys())
    sorted_pkg_dirs = sorted(
        pkg_dirs.items(), key=lambda item: len(item[1]), reverse=True
    )
    affected: set[str] = set()

    for raw_path in changed_files:
        parts = PurePosixPath(raw_path.replace("\\", "/")).parts
        matched = match_package(parts, sorted_pkg_dirs)
        if matched is not None:
            affected.add(matched)
        elif is_ignored_doc(parts):
            continue
        else:
            return set(all_names)

    queue = list(affected)
    while queue:
        pkg = queue.pop()
        for dep_pkg in dependents.get(pkg, ()):
            if dep_pkg not in affected:
                affected.add(dep_pkg)
                queue.append(dep_pkg)

    return affected


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Run cargo test only for workspace packages affected by changes."
    )
    parser.add_argument(
        "--base",
        default="origin/main",
        help="Git ref to measure changes from (default: origin/main).",
    )
    parser.add_argument(
        "--files-from",
        help="Read changed repository-relative file paths from this file instead of calling git.",
    )
    parser.add_argument(
        "--print",
        dest="print_only",
        action="store_true",
        help="Print the decision and exit 0 without running cargo.",
    )
    args = parser.parse_args(argv)

    try:
        if args.files_from is not None:
            changed_files = read_files_from(args.files_from)
            repo_root, pkg_dirs, dependents = load_packages(Path.cwd(), None)
        else:
            git_root, changed_files = git_changed_files(args.base)
            repo_root, pkg_dirs, dependents = load_packages(git_root, git_root)
    except ScriptError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    affected = affected_packages(changed_files, pkg_dirs, dependents)
    all_names = set(pkg_dirs.keys())

    if not affected:
        print("packages: (none)", flush=True)
        if args.print_only:
            return 0
        print("no package affected; no tests run", flush=True)
        return 0

    if affected == all_names:
        print("packages: (all)", flush=True)
        if args.print_only:
            return 0
        cargo_cmd = ["cargo", "test", "--workspace", "--all-targets"]
    else:
        sorted_names = sorted(affected)
        print(f"packages: {' '.join(sorted_names)}", flush=True)
        if args.print_only:
            return 0
        cargo_cmd = ["cargo", "test"]
        for name in sorted_names:
            cargo_cmd.extend(["-p", name])
        cargo_cmd.append("--all-targets")

    try:
        proc = subprocess.run(cargo_cmd, cwd=repo_root, check=False)
    except OSError as exc:
        print(f"error: failed to run {' '.join(cargo_cmd)}: {exc}", file=sys.stderr)
        return 2
    return proc.returncode


if __name__ == "__main__":
    sys.exit(main())
