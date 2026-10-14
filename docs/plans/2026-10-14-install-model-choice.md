# Plan: choose the default model at install

Spec: `docs/specs/2026-10-14-install-model-choice.md`.

### Task IM1

**Goal:** `blyte skill install` sets the user's default model, as the spec describes.

**Where:**
- `crates/blyte-cli/src/cli.rs`: `SkillAction::Install` (around line 282). Add `--model <id>`.
- `crates/blyte-cli/src/main.rs`: the `SkillAction::Install` handler (around line 69). Run the model step once, after the skill is installed for every target.
- `crates/blyte-cli/src/env_file.rs`: it already knows the env file path and line format. Add a function that sets one or more `NAME=value` lines in that file:
  - it replaces an existing line for the same name, keeps all other lines and comments, and appends new names;
  - it creates the file, and its parent directory, with mode 0600.
  - Read the file path the same way loading does, so writing and loading cannot disagree.
- `README.md` (the install section, around line 85, and the models table around line 155) and `docs/getting-started.md`: say that install asks for the default model, and mention `--model` and `BLYTE_MODEL`.

**Behaviour** (see the spec for full detail):
- Ask only when stdin and stdout are terminals (`std::io::IsTerminal`), the env file has no `BLYTE_MODEL`, and `--model` is not given.
- Choices: `gemini-4-argon` (recommended, the default on an empty answer), `gemini-3.8-flash-high`, or a typed model id. Re-ask once on an answer that is not one of these.
- Write `BLYTE_MODEL=<id>`. For a model whose id contains `argon`, also write `BLYTE_MODEL_TIMEOUT_SECS=600`.
- Print what was written and where. If `BLYTE_MODEL` was already set and `--model` was not given, print the current value and change nothing.
- With no terminal and no `--model`, do nothing.
- Keep the prompt's input and output testable: put the decision logic in a function that takes a reader and a writer, not stdin and stdout directly.

**Tests:**
- The env-file setter:
  - creates a file with mode 0600;
  - replaces an existing `BLYTE_MODEL` line and keeps the others;
  - appends to a file with no trailing newline.
- The prompt logic, with an in-memory reader and writer:
  - an empty answer gives `gemini-4-argon`;
  - "2" gives Flash;
  - a typed id is used as given;
  - an Argon choice also sets the timeout.
- `--model` writes without asking. A set `BLYTE_MODEL` without `--model` is left unchanged.
- Point `BLYTE_ENV_FILE` at a temporary path in every test; never touch the real `~/.config/blyte/env`.
