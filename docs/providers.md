# Providers

`--model` (or `BLYTE_MODEL`) names the model, and the name picks the provider. blyte reads the
credentials itself; they are removed from the environment of every command the model runs.

| model name | provider | credentials |
|---|---|---|
| `gemini-3.8-flash-high` (default) | Google Gemini; `-low` / `-medium` / `-high` sets the thinking level | `GEMINI_API_KEY` |
| `vertex/<model>` (e.g. `vertex/gemini-3.8-flash-high`) | Google Gemini on Vertex AI; `-low` / `-medium` / `-high` sets the thinking level | Application Default Credentials (`gcloud auth application-default login`) or `GOOGLE_OAUTH_ACCESS_TOKEN`, `GOOGLE_CLOUD_PROJECT` |
| `claude-…` or `anthropic/…` | Anthropic | `ANTHROPIC_API_KEY` |
| `openai/<model>` | any OpenAI-compatible API: OpenAI, or a local vLLM or LM Studio server | `OPENAI_API_KEY`, `OPENAI_BASE_URL` (default `https://api.openai.com/v1`) |
| `openrouter/<vendor>/<model>` | OpenRouter | `OPENROUTER_API_KEY` |
| `<name>:<tag>` or `ollama/…` | Ollama, local or cloud; thinking off unless the name ends in `+think` | `OLLAMA_HOST` (e.g. `http://localhost:11434`) or `OLLAMA_API_KEY` |
| `chatgpt/<model>` (e.g. `chatgpt/gpt-5.5`) | a ChatGPT subscription, through the Codex CLI's login | `~/.codex/auth.json` from `codex login` |

## Choosing an implementer model

In the plan → delegate → review pattern, blyte's model does the implementation, so it should be
**cheap, fast and good at following precise instructions**. It does not need to be good at
planning: the frontier model above it does that.

| model | status |
|---|---|
| **Gemini 3.8 Flash** (default) | measured: every paired run and the FrontierHarness run used it. Start here. |
| Claude Haiku, GPT mini-class models | supported (via `claude-…`, `openai/…`, `openrouter/…`), not yet measured as implementers |
| local models via Ollama | supported; `qwen3:14b` handled small tasks but failed a real repository task in our tests. Worth trying on simple, well-tested tasks; slow on a laptop. |

Only Gemini 3.8 Flash has a list price in blyte today, so it is the only model for which the
report's `cost_usd` and `--max-cost` work. For another model, compare token counts in the report.

## Notes per provider

**Gemini.** The default. The thinking suffix is part of the name: `gemini-3.8-flash-low` thinks
least and costs least.

**Gemini on Vertex AI.** Select by prefixing the model with `vertex/`, e.g. `--model vertex/gemini-3.8-flash-high`.
Runs Gemini through Vertex AI instead of the Gemini API (AI Studio key), authenticated with your own
Application Default Credentials (`gcloud auth application-default login`) or `GOOGLE_OAUTH_ACCESS_TOKEN`.
blyte ships no account and assumes no project: it uses the user's own credentials and project.

Configuration:
- `GOOGLE_CLOUD_PROJECT`: Google Cloud project id (falls back to the ADC file's `quota_project_id`).
- `GOOGLE_CLOUD_LOCATION`: Vertex AI region (defaults to `global`).
- Service-account key files are not supported yet: set `GOOGLE_OAUTH_ACCESS_TOKEN`, for example from `gcloud auth print-access-token`.

**Anthropic.** Prompt caching is on: blyte marks cache breakpoints on the tool definitions and the
latest message, so each turn pays full price only for what is new.

**OpenAI-compatible.** Anything that speaks `/v1/chat/completions`. Point `OPENAI_BASE_URL` at a
local server to run open models without a key.

**Ollama.** A local model needs its context window set above blyte's fold budget. Start the server
with a larger context and lower the budget:

```bash
OLLAMA_CONTEXT_LENGTH=32768 ollama serve
blyte --model qwen3:14b --fold-budget 16000 "..." --gate "..."
```

Thinking is off by default (it costs turns of time on small models); add `+think` to the name to
turn it on.

**ChatGPT subscription.** Uses the login the Codex CLI stores in `~/.codex/auth.json`. blyte only
reads it: it never refreshes or rewrites the token. If the token is about to expire, blyte stops
with an authentication error and asks you to run any `codex` command to refresh it. Usage counts
against your ChatGPT plan's limits; when a plan's quota is exhausted, blyte stops at once and says
when it resets.

## Retries and quotas

Burst rate limits (HTTP 429 with a short retry delay), server errors and dropped connections are
retried with backoff (2, 4, 8, 16, 32, then 60 seconds, six times), never past `--max-seconds`.
An exhausted **quota** (a daily limit, `insufficient_quota`, or a reset more than five minutes
away) is not retried: the run ends with a message saying when it resets.

## Cost

When blyte knows a model's list price, the report carries `cost_usd` and `--max-cost` can cap it.
Costs are computed from the provider's reported usage, with cache hits at their discounted price.
Today the price table covers **Gemini 3.8 Flash** only; for other models `cost_usd` is `null` and
`--max-cost` is refused rather than silently ignored. Token counts are reported for every
provider.
