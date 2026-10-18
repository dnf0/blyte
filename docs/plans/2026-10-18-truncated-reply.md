# Plan: don't act on a reply cut off at the output limit (2026-10-18)

## Why

Research run R-sota (2026-10-18) found that `Runner::run_loop` (`crates/blyte-agent/src/run.rs`)
never reads `resp.finish_reason`. A reply cut off at the output token limit is handled like a
complete one: its partial text becomes the final message, its tool calls run (possibly with
truncated arguments), and a reply with no tool calls is taken as "done", so the gate runs or the
run ends. gemini-4-argon adds a `CONTINUATION` finish reason for the same case. The agent loop sends
no `maxOutputTokens`, so this is rare, but it fails silently.

Decision (user, 2026-10-18): add a guard in the agent loop. Resending with Argon's continuation
token is out of scope; add it only if the logs show cut-offs.

### Task T1: a truncated reply gets another turn instead of being acted on

Files: `crates/blyte-agent/src/run.rs`, a new test `crates/blyte-agent/tests/test_truncated_reply.rs`,
and the doc that describes the agent loop's turn handling (`docs/how-it-works.md` or
`docs/architecture.md`; grep `docs/` for `empty_turns` or "final message" to find it).

1. **Which finish reasons mean "cut off".** Add a small function in `run.rs`, for example
   `fn truncated(reason: Option<&str>) -> bool`, true for the output-limit reasons each provider
   reports. Read each provider's parse code in `crates/blyte-provider/src/` (`gemini.rs`,
   `anthropic.rs` `stop_reason`, `openai.rs` and `ollama.rs` `finish_reason`, `chatgpt.rs` response
   `status`) and match the exact strings they pass through: at least Gemini `MAX_TOKENS` and
   `CONTINUATION`, Anthropic `max_tokens`, OpenAI/Ollama `length`, and the ChatGPT Responses
   `incomplete` status. Do not match normal stops (`STOP`, `end_turn`, `stop`, `tool_use`,
   `tool_calls`, `completed`).
2. **The guard.** In `run_loop`, right after the provider returns `Ok(resp)` and the turn's usage
   is recorded and the `Event::Turn` emitted (around the `let trimmed = resp.text.trim();` block),
   if `truncated(resp.finish_reason.as_deref())`:
   - do not set `report.final_message`, do not push the partial text or any `ToolCall` to
     `history`, do not execute any tool call, and do not run the gate or end the run;
   - push one `Message::User` harness note: "Your last reply hit the output token limit and was
     cut off, so none of it was used. Reply again more briefly: make one tool call at a time and
     keep edits small.";
   - count it in a new `report.truncated_turns: u32` field on the run report (serialized like
     `empty_turns`, default 0, so old report JSON still deserializes), and `continue` to the next
     turn. The turn still counts toward the turn limit, so repeated truncation ends at that limit
     as any other run would.
3. **Tests** in `test_truncated_reply.rs`, with a scripted provider like the ones in
   `crates/blyte-agent/tests/test_empty_gate.rs` (copy its pattern):
   - first reply: `finish_reason: Some("MAX_TOKENS")` with a `write` tool call to a file; second
     reply: a normal stop with no tool calls. Assert the file was not written, the second request's
     messages contain the harness note, and `truncated_turns == 1`;
   - first reply: `finish_reason: Some("CONTINUATION")` with text and no tool calls, in a gated
     run. Assert the gate did not run on that turn (it runs only after the second, normal reply);
   - a normal reply with `finish_reason: Some("STOP")` behaves exactly as before.
4. **Docs**: one sentence where the turn handling is described.

Gate: the repository checks (`blyte.toml`). Commit message:
`fix(agent): don't act on a reply cut off at the output token limit`
