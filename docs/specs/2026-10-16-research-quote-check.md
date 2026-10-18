# Spec: research reports must quote the code they cite

## Problem

Research runs on the provision machine (2026-10-15/16, blyte 0.30.0, gemini-4-argon) came back
"verified" with reports whose quotes were mostly invented: R-V5b7 had 44 of 442 numbered quotes
right, R-V5b6 2 of 178. Others cited files that don't exist.

## Facts (checked 2026-10-16)

- The research gate is `test -s report.md` (`crates/blyte-cli/src/dispatch.rs`), so any
  non-empty report is "verified".
- The runs used the default `folded` context (budget 48k tokens). They folded 4-17 times, the
  first fold by turns 5-15. A fold replaces tool results with one line (`bash nl -ba x.py -> exit
  0`), so the file text the model read is gone when it writes the report. R-V5b7 wrote 847 lines
  with 442 quotes from a final prompt of 30k tokens.
- `blyte dispatch --allow-read` reaches only the command sandbox. The `read` and `grep` tools
  accept only `--read-root` paths, which dispatch sets to the repository alone. With the
  worktree under a scratchpad, every `read` failed ("path escapes the workspace"), and the model
  fell back to `nl -ba` through bash.

## Decisions (user, 2026-10-16)

1. **Quote check.** A research run passes only if `report.md` is non-empty and its quotes match
   the code. The check runs inside blyte after the gate command passes, reading files with
   blyte's own access, and a failure is a failed gate whose output lists each bad quote, so the
   model re-reads and fixes them.
   - A citation is `path:N`, `path:N-M`, or `path:N-M,K...` where `path` has a file extension.
     URLs are ignored.
   - A path resolves, in order: as an absolute path; relative to the workspace; relative to each
     read root; then as a suffix of a file tracked in the workspace (`git ls-files`).
   - A prose citation fails if a path containing `/` resolves to nothing, or if a line number is
     past the end of the file. A bare file name that resolves to nothing is not checked (it may
     not be a file); one that matches several files is checked against all of them.
   - A fenced code block is a quote when the nearest non-blank line before it holds a citation;
     it is checked against the last citation on that line. Blocks tagged `diff`, `patch` or
     `proposed`, and blocks with no citation before them, are not checked.
   - Each non-blank quoted line is compared with whitespace collapsed. A line that starts with a
     line number (`245\t`, `245:`, `245|`, `245  `) must equal that line of the file. Other lines
     must occur within the cited range ±5 lines, or anywhere in the file if the citation has no
     range. Elision lines (`...`, `…`, `# ...`, `// ...`) are skipped.
   - Added after running the check over R-V5b7 and ~60 older local reports:
     - a block whose first line is a comment citing `path:N-M` quotes that citation;
     - a block of mostly numbered lines with no citation before it quotes the report's most
       recent citation (R-V5b7 had runs of `nl -ba` blocks with no prose between them);
     - `...` inside a line stands for omitted code (`run(ti, ...)`);
     - paths also resolve against each read root's parent (reports spanning repositories cite
       `repo/src/x.py`).
   - The output lists up to 40 problems with the report line, the citation, the quoted text and
     where the text really is, if anywhere.
2. **Folding in research runs.** Research runs fold at 96k tokens instead of 48k unless
   `--fold-budget` is given, keeping prompts under 200k. Every digest says the tool output is no
   longer in context and that a file must be read again before quoting it.
3. **`--allow-read` paths become read roots** for `read` and `grep` in every dispatch, not just
   the sandbox.
4. The research rules tell the model about the check and how to mark proposed code.

## Follow-up: closing the gaps (user, 2026-10-16)

Measured on 79 saved reports (691 code blocks) before choosing the rules:
- 254 blocks had no recognised citation. Many cited in forms the check didn't read (a path with
  no line number, `` `x.toml` (line 8) ``, `Makefile:6-21`, `In line 415:`); those forms are now
  read. Of the rest, a sample of the language-tagged ones was mostly code presented as real
  ("executes:", "contains:") with no file named, so an uncited block tagged with a language, or
  numbered, now fails; commands, output and untagged plain blocks don't. 116 such blocks remain
  in the saved reports, so a report written without the rule needs about 1.5 fixes on average;
  the research rules now state it up front.
- A cited source file given without a directory (`scorer.py:40`) must exist.
- Rejected: requiring a citation in every findings paragraph. 2,922 of 6,130 prose lines of 60+
  characters cite no file, and a sample was mostly legitimate (sub-points of a cited parent,
  arithmetic, web research), so the rule would mostly flag honest text.
- Next, not built: checking that a cited quote supports the conclusion drawn from it needs a model
  call per claim; measure it on saved reports first.

## Validation (2026-10-16)

- R-V5b7 (fetched from provision-deployment, checked against the provision checkout): 59
  problems; every quote block whose file still exists failed (e.g. `fire.yaml:1-34`, 32 of 32
  lines wrong), and citations of files that were never in the repository's history
  (`hazard_registry.py`, `cog_writing.py`) were reported.
- Older local reports: flags sampled in blyte's own reports were checked against the code at the
  time; the quoted lines (`generateContent?key=`, `fn scrub(c: &mut Command)`,
  `let task_config = TaskConfig {`) never existed in the history. Reports with accurate quotes
  (SS1R, R-serving, R2/R3 in cloud-native-geospatial-engineering) passed.

## Out of scope

- Checking claims that aren't quotes or line citations.
- Implementation runs (their gate is the repository's checks).

## Measured 2026-10-18: a model claim check, and the R-V5b7 rerun

**Claim check** (`crates/blyte-agent/examples/claim_check.rs`, plan
`docs/plans/2026-10-18-claim-check-experiment.md`). Gemini-4-argon was given each prose claim that cites code, with
the cited lines and 5 lines either side, and asked `supported` / `contradicted` / `not_enough_evidence`. Five claims
per report were rewritten by the model to be false (seeded) and mixed in. 13 saved reports from 9 repositories, each
checked against its repository's HEAD at the run's start (from the reflog).

- Cost: $1.24 for 408 claims and 65 seeds (102 calls, about $0.10 a report, 20 minutes).
- Six reports read only that checkout; the other seven also read `origin/main` with `git show` or other
  repositories, so their code differs from what the checker saw and their numbers are not used. On the six:
  - seeded: 23 of 30 `contradicted` (77%); 4 judged `supported` (some seeds were label-only claims such as
    "*File*: `openai.rs`"), 2 parse errors;
  - the reports' own claims: 245; 95 `supported`, 80 `not_enough_evidence`, 57 `contradicted` (23%), 13 parse errors.
  - Flags checked by hand were right: RC2 quotes `parse_group_key` and `build_tile_group_key`, which never existed in
    any provision branch; reliability-orchestrator R1 cites `Dockerfile:19` for a line at 29. These reports predate
    the quote check (0.32.3).
- 27 of 408 claims (two batches) failed to parse; batching smaller or retrying once would fix that.

**R-V5b7 rerun on 0.32.5** (provision-deployment, same question, same code `ed047efe`, same 1,800 s limit): ran out
of time at $4.17 (original: verified at $3.60), 87 turns, model time 1,801 s, cached input 7.7M tokens (original
3.6M). The gate never ran, so the quote check never saw the report. Run by hand, the check finds 15 of 28 code blocks
(about 350 of 539 numbered lines) that match nothing; `_read_chunk_bands`, `build_sot_fire_v0_chunk_s3_keys` and
`_build_fire_v0_sot_source_paths` never existed in any provision branch, though the run made 49 `read` calls on the
right checkout.

Conclusions:
- The quote check catches invented code only if the gate runs; a research run that runs out of time or turns should
  still run it on whatever `report.md` exists and report the result.
- Argon invents numbered code blocks even after reading the files; the quote check, not instructions, is what stops it.
- A claim check adds about $0.10 a report and catches what the quote check cannot (wrong line numbers in prose,
  wrong figures, conclusions the cited code does not support); 77% recall on seeded errors.
