# Research runs: ask for the report instead of stopping on a read loop

## Problem (runs on one machine, 2026-11-03 to 2026-11-05)

Ten research runs ended `budget_exhausted` with `read_loop: true` and no report, after 16 to 38
turns and 5 to 15 minutes. All their reading was lost.

The read-loop check (`READ_LOOP_LIMIT = 30` in `crates/blyte-agent/src/run.rs`) counts every
read, grep or glob whose exact arguments were already used since the workspace last changed. The
count resets only on an `edit`/`write` or a workspace change. In a research run:

1. Nothing changes until the model writes `report.md`, and these runs had not started it.
2. Re-reading is normal: folding drops old tool results, so the model reads files again. Research
   runs that finished peaked at 21 to 27 repeats.
3. The model is not warned. The count rises from a call's second use, but the warning is added
   only from its third. SMOKE1 re-read 31 different files once each and never saw a warning.
4. The research reminder to update `report.md` fires first at turn 30, after most of these runs
   had already been stopped.

Six other research runs reached the 30-minute limit. They all left a full report, so they are not
changed here.

## Decisions (research runs only; other runs keep today's behaviour)

1. **Early report reminder.** At turn 8, if `report.md` is missing or empty, tell the model to
   start it now with what it has found so far. The every-30-turns update reminder stays.
2. **Warning before the limit.** When the repeat count reaches 20, tell the model once (per reset
   of the count) that it has re-read 20 things, that it should write what it needs into
   `report.md` now, and that reading stops at 30. The per-call warning (third use of the same
   call) says "write what you need into report.md" instead of "make your change".
3. **Report, not stop, at the limit.** When the count reaches 30, the run does not end at once.
   It sets `read_loop` in the report, as today, and tells the model to stop reading and write
   `report.md` from what it has found, marking what it could not confirm as open questions, then
   stop calling tools. From then on, `read`, `grep` and `glob` calls return an error saying
   reading is closed and to write `report.md`. The model gets 3 more turns. If it finishes in
   them, the run ends the normal research way (report checks included). If not, the run ends
   `budget_exhausted` as today, and dispatch keeps whatever `report.md` holds, as it already does
   for runs that reach the time limit.

No fallbacks: none of this guesses or substitutes content. If `report.md` cannot be read, the
reminder treats it as missing, which is the case the reminder exists for.
