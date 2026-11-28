# Known secret values are removed from what the model is shown

Date: 2026-11-28. Research behind it: run SOR1 (2026-11-28).

## Problem

Nothing checks the text a tool or a gate step returns before it goes to the model and the log.
On macOS the sandbox keeps the shell away from credential files. On Linux a workspace `.env` stays
readable by the shell (`docs/security.md`), so `cat .env` or a test that prints its environment
puts a secret in front of the model and into `.wt/<id>.log`.

## Decision

Replace, in every tool result and every gate step's output, each exact occurrence of a secret value
blyte already knows. No pattern or entropy matching: a guess would redact ordinary text and break
edits and quote checks.

The known values, read again for each result (a few environment reads and one small file):

1. the value of each name in `shell::HARNESS_ENV` that is set (the model API keys);
2. the value of each variable `shell::withheld_env()` names (already kept out of the shell);
3. each `NAME=value` in the workspace's `.env` for which `shell::credential_like(name, value)` is
   true, the same test that decides what the shell is not given.

A value shorter than 8 bytes is not used: it would match ordinary text. Each occurrence becomes
`[redacted NAME]`, longest value first so that one value inside another is not left half shown.

## Where

- `tools::execute_with_timeout`, after the output is bounded: every tool result of a run and of
  the surface's model passes through it.
- `run::run_gate_step`, on the output it returns.

Direct edits, setup steps and the second-model review are not covered by this change; they are
listed in `docs/security.md` as still open.

## What this changes for a run

- A file that contains a known secret reads back with `[redacted NAME]`. An `edit` whose
  `old_string` spans that text fails with its usual "not found" error. That is intended.
- A research report cannot quote such a line: the quote check compares with the file and fails.
- A workspace `.env` that exists and cannot be read is an error on the tool result, naming the
  file. It is not treated as empty.

## Not done

- Secrets blyte does not know (a token printed by a command that fetched it).
- Encoded forms of a known value (base64, hex, split across lines).
