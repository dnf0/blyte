# Choose the default model at install

Date: 2026-10-14. Status: agreed with the user.

## Why

The built-in default is `gemini-3.8-flash-high`. In the Argon pair pilot (2026-10-14), Argon implementer dispatches did much better:

| Implementer | Passed gate | Turns | Time | Cost |
|---|---|---|---|---|
| Argon | 5 of 5 | 26 | 5 min | $0.93 |
| Flash 3.8 | 2 of 4 | 122 | 17 min | $1.68 |

Turns, time and cost are averages per dispatch. Argon works with a plain `GEMINI_API_KEY` (`gemini/gemini-4-argon`, checked 2026-10-14), so it needs no Vertex setup. Which model to use is still the user's call, because the two have different prices per token. So the setup step asks.

## Behaviour

- `blyte skill install` asks for the default model:
  - when stdin and stdout are terminals;
  - when `~/.config/blyte/env` has no `BLYTE_MODEL` line;
  - when `--model` is not given.
- The question offers:
  1. `gemini-4-argon`, the recommended choice, with a one-line reason;
  2. `gemini-3.8-flash-high`, cheaper per token but with more turns;
  3. any other model id, typed in.

  An empty answer takes the recommended choice.
- The answer is written to the env file (`$BLYTE_ENV_FILE`, else `$XDG_CONFIG_HOME/blyte/env`, else `~/.config/blyte/env`, the same lookup blyte loads from) as a `BLYTE_MODEL=<id>` line.
  - When the model is an Argon one, it also writes `BLYTE_MODEL_TIMEOUT_SECS=600`, since its calls can think for about 3 minutes.
  - A new file is created with mode 0600; blyte does not load an env file that other users can read.
  - Other lines in the file are kept.
- `blyte skill install --model <id>` writes the same lines without asking, for scripts.
- With no terminal and no `--model`, nothing is asked or written, and the built-in default applies.
- If the file already has `BLYTE_MODEL`, the install prints it and leaves it alone. `--model` replaces it.
- Model precedence is unchanged: `--model` flag, then `blyte.toml` `model`, then `BLYTE_MODEL`, then the built-in default.

## Not doing

- Changing the built-in default.
- Per-model timeout defaults in code. Streaming with an idle timeout is the real fix for long Argon calls, and is a separate change.
