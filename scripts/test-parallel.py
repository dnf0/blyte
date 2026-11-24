#!/usr/bin/env python3

from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
import json
import os
from pathlib import Path
import subprocess
import sys
import time

try:
    import tomllib
except ImportError:
    tomllib = None


class ScriptError(Exception):
    pass


def default_jobs() -> int:
    return min(4, os.cpu_count() or 1)


def load_cargo_env(root: Path) -> dict[str, str]:
    config_path = root / ".cargo" / "config.toml"
    if not config_path.exists():
        return {}
    if tomllib is None:
        raise ScriptError("Python 3.11 or later is required to import tomllib")
    try:
        with config_path.open("rb") as handle:
            data = tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise ScriptError(f"cannot read {config_path}: {exc}") from exc
    if not isinstance(data, dict):
        raise ScriptError(f"{config_path} must parse to a TOML table")
    env_table = data.get("env")
    if env_table is None:
        return {}
    if not isinstance(env_table, dict):
        raise ScriptError(f"[env] in {config_path} must be a table")
    extra_env: dict[str, str] = {}
    for key, value in env_table.items():
        if not isinstance(value, str):
            raise ScriptError(
                f"[env] entry '{key}' in {config_path} must be a plain string, got {type(value).__name__}"
            )
        extra_env[key] = value
    return extra_env


def load_executables_from(path_str: str) -> list[tuple[str, str]]:
    try:
        text = Path(path_str).read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as exc:
        raise ScriptError(f"cannot read {path_str}: {exc}") from exc
    entries: list[tuple[str, str]] = []
    for lineno, raw_line in enumerate(text.splitlines(), start=1):
        if not raw_line.strip():
            continue
        parts = raw_line.split("\t")
        if len(parts) != 2 or not parts[0] or not parts[1]:
            raise ScriptError(
                f"{path_str}:{lineno}: expected '<executable><TAB><directory>'"
            )
        entries.append((parts[0], str(Path(parts[1]).resolve())))
    return entries


def build_and_discover(
    root: Path, cargo_args: list[str]
) -> tuple[int, list[tuple[str, str]]]:
    build_cmd = ["cargo", "test", *cargo_args, "--no-run"]
    try:
        build_proc = subprocess.run(build_cmd, cwd=root, check=False)
    except OSError as exc:
        raise ScriptError(f"failed to run {' '.join(build_cmd)}: {exc}") from exc
    if build_proc.returncode != 0:
        return build_proc.returncode, []

    json_cmd = ["cargo", "test", *cargo_args, "--no-run", "--message-format=json"]
    try:
        json_proc = subprocess.run(
            json_cmd,
            cwd=root,
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        raise ScriptError(f"failed to run {' '.join(json_cmd)}: {exc}") from exc
    if json_proc.returncode != 0:
        if json_proc.stderr:
            sys.stderr.write(json_proc.stderr)
            sys.stderr.flush()
        return json_proc.returncode, []

    entries: list[tuple[str, str]] = []
    seen: set[str] = set()
    for line in json_proc.stdout.splitlines():
        stripped = line.strip()
        if not stripped:
            continue
        try:
            msg = json.loads(stripped)
        except json.JSONDecodeError as exc:
            raise ScriptError(f"failed to parse cargo JSON output: {exc}") from exc
        if not isinstance(msg, dict):
            continue
        if msg.get("reason") != "compiler-artifact":
            continue
        executable = msg.get("executable")
        if not isinstance(executable, str) or not executable:
            continue
        profile = msg.get("profile")
        if not isinstance(profile, dict) or profile.get("test") is not True:
            continue
        manifest_path = msg.get("manifest_path")
        if not isinstance(manifest_path, str) or not manifest_path:
            raise ScriptError(
                f"compiler-artifact for '{executable}' missing 'manifest_path'"
            )
        if executable in seen:
            continue
        seen.add(executable)
        manifest_dir = str(Path(manifest_path).resolve().parent)
        entries.append((executable, manifest_dir))
    return 0, entries


def run_one(
    executable: str, manifest_dir: str, cargo_env: dict[str, str]
) -> tuple[str, bool, float, str]:
    env = dict(os.environ)
    env["CARGO_MANIFEST_DIR"] = manifest_dir
    env.update(cargo_env)
    start = time.monotonic()
    try:
        proc = subprocess.run(
            [executable],
            cwd=manifest_dir,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            errors="replace",
            check=False,
        )
        elapsed = time.monotonic() - start
        return executable, proc.returncode == 0, elapsed, proc.stdout
    except OSError as exc:
        elapsed = time.monotonic() - start
        return executable, False, elapsed, f"failed to start {executable}: {exc}\n"


def test_result_lines(output: str) -> list[str]:
    return [
        line.strip()
        for line in output.splitlines()
        if line.strip().startswith("test result:")
    ]


def main(argv: list[str] | None = None) -> int:
    if tomllib is None:
        print(
            "error: Python 3.11 or later is required (tomllib module not available)",
            file=sys.stderr,
        )
        return 2

    parser = argparse.ArgumentParser(
        description="Build and run cargo test binaries in parallel."
    )
    parser.add_argument(
        "--jobs",
        type=int,
        default=default_jobs(),
        help="Number of test binaries to run at a time (default: min(4, CPU count)).",
    )
    parser.add_argument(
        "--root",
        help="Workspace root directory (default: parent of this script's directory).",
    )
    parser.add_argument(
        "--executables-from",
        help="Read '<executable><TAB><directory>' lines from this file instead of building with cargo.",
    )
    parser.add_argument(
        "cargo_args",
        nargs="*",
        help="Arguments forwarded to 'cargo test' (pass after '--').",
    )
    args = parser.parse_args(argv)

    if args.jobs < 1:
        print("error: --jobs must be at least 1", file=sys.stderr)
        return 2

    if args.root is not None:
        root = Path(args.root).resolve()
    else:
        root = Path(__file__).resolve().parent.parent

    if not root.is_dir():
        print(f"error: root directory does not exist: {root}", file=sys.stderr)
        return 2

    try:
        cargo_env = load_cargo_env(root)
        if args.executables_from is not None:
            entries = load_executables_from(args.executables_from)
        else:
            code, entries = build_and_discover(root, args.cargo_args)
            if code != 0:
                return code
    except ScriptError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    if not entries:
        print("error: no test binaries found", file=sys.stderr)
        return 2

    total_start = time.monotonic()
    failed: list[str] = []

    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        futures = [
            pool.submit(run_one, executable, manifest_dir, cargo_env)
            for executable, manifest_dir in entries
        ]
        for future in as_completed(futures):
            executable, passed, elapsed, output = future.result()
            name = Path(executable).name
            if passed:
                results = test_result_lines(output)
                if results:
                    print(
                        f"ok {name} ({elapsed:.2f}s) {' | '.join(results)}",
                        flush=True,
                    )
                else:
                    print(f"ok {name} ({elapsed:.2f}s)", flush=True)
            else:
                failed.append(name)
                print(f"FAILED {name} ({elapsed:.2f}s)", flush=True)
                if output:
                    sys.stdout.write(output)
                    if not output.endswith("\n"):
                        sys.stdout.write("\n")
                    sys.stdout.flush()

    total_elapsed = time.monotonic() - total_start
    if failed:
        print(
            f"summary: {len(entries)} binaries, {len(failed)} failed in {total_elapsed:.2f}s; failed: {', '.join(failed)}",
            flush=True,
        )
        return 1

    print(
        f"summary: {len(entries)} binaries, 0 failed in {total_elapsed:.2f}s",
        flush=True,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
