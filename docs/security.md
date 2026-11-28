# Security and the sandbox

A coding agent runs commands a model chose. blyte assumes those commands can be wrong, or steered
by something the model read, and limits what they can reach. The limits are on by default and
apply to every command the model runs on the host: `bash`, and the gate.

!!! note "What the sandbox protects"
    It protects **your machine and your credentials** from the commands. It does not review the
    code the model writes: a gated, sandboxed run can still produce a bad change. Review the diff.

## The layers

| layer | macOS | Linux | turn off / adjust |
|---|---|---|---|
| **Writes** only inside the workspace, its git directory, temp directories and tool caches | Seatbelt | Landlock (kernel 5.13+) | `--sandbox=off` |
| **Credential reads** refused: `~/.ssh`, `~/.aws`, `~/.azure`, `~/.config/gcloud`, `~/.config/gh`, `~/.config/blyte/env`, the surface token directory (`$XDG_STATE_HOME/blyte/surface` or `~/.local/state/blyte/surface`), `~/.kube`, `~/.gnupg`, `~/.docker/config.json`, `~/.git-credentials`, `~/.netrc`, `~/.pypirc`, cargo credentials, `~/.codex`, keychains, cookies and browser profiles, and any file named `.env` | Seatbelt | Landlock: reads are confined to an allow-list that leaves these out (see below) | `--allow-read PATH` grants one back (except blyte's own surface token directory and `~/.config/blyte/env`) |
| **Environment**: variables that look like credentials, and the AWS profile variables, are withheld from commands | yes | yes | `--pass-env NAME` keeps one |
| **Network**: commands cannot connect out | Seatbelt | Landlock (kernel 6.7+) | opt in with `--network=off` |

blyte's own provider keys (`GEMINI_API_KEY`, `ANTHROPIC_API_KEY`, …) and `BLYTE_*` variables are
always removed from commands, sandbox or not, and `--pass-env` cannot keep them. The one exception is
`BLYTE_SESSION`, the session id, which is kept so that dispatches started by a command are counted
with their session.

## Writes

Commands may write under:

- the workspace (the directory blyte runs in) and its git common directory, so commits work in a
  worktree;
- system temp directories (`/tmp`, `/var/tmp`, `$TMPDIR`, and macOS's `/private/var/folders`);
- tool caches under `$HOME`: `.cargo`, `.cache`, `.npm`, `Library/Caches`, `.gradle`, `.m2`,
  `go/pkg`, `.docker/buildx`, and `.local` (with program and configuration folders carved out,
  below);
- `/dev` (for `/dev/null` and terminals).

`~/.rustup` and `~/go` as a whole are not granted for writes (only `~/go/pkg` is). On macOS,
Seatbelt also denies writes to program directories, tool configuration, and blyte's state inside
the granted trees:

- `~/.cargo/bin`, `~/.cargo/config`, `~/.cargo/config.toml`, `~/.cargo/credentials`,
  `~/.cargo/credentials.toml`, and `~/.cargo/env`, `~/.cargo/env.fish`, `~/.cargo/env.nu` (the
  files shells load at start);
- `~/.local/bin`, `~/.local/lib`, `~/.local/state/blyte`, `~/.local/share/uv/tools`,
  `~/.local/share/mise`, `~/.local/share/pipx`, `~/.local/share/keyrings`;
- `~/.gradle/init.d`, `~/.m2/settings.xml`;
- `<git common dir>/hooks` and `<git common dir>/config` in a dispatch (the rest of the git
  directory stays writable so commits work).

Because these paths are read-only, installing programs (`cargo install`, `go install`,
`uv tool install`, `pip install --user`, rustup toolchain or component installs) fails inside a
run; install tools on the host beforehand, or run outside the sandbox when needed. Builds and
existing toolchains continue to work because the caches (`~/.cargo/registry`, `~/.cargo/git`,
`~/.cargo/.package-cache`, `~/go/pkg`, `~/.cache`, `~/.local/share/uv/python`, `~/.gradle/caches`,
`~/.m2/repository`) stay writable and the protected directories remain readable.

On Linux, Landlock can only *allow* paths and cannot carve deny rules out of an allowed directory.
`~/.rustup`, `~/go/bin` (only `~/go/pkg` is granted), `~/.local/bin`, `~/.local/lib`,
`~/.local/state/blyte`, `~/.cargo/bin`, `~/.cargo/config*`, `~/.cargo/credentials*`, and
`~/.cargo/env*` are not writable there because they are omitted from the allow-list (only the
non-protected `~/.cargo` cache children are granted). However, subpaths inside granted trees —
`~/.local/share/uv/tools`, `~/.local/share/mise`, `~/.local/share/pipx`,
`~/.local/share/keyrings`, `~/.gradle/init.d`, `~/.m2/settings.xml`, and
`<git common dir>/hooks` and `<git common dir>/config` — remain writable on Linux.

Anything else fails with "Operation not permitted". This is what stops a model that "fixes" a
failing test by clearing a system cache, editing your shell profile, installing trojaned binaries
on `PATH`, or writing into another checkout.

## Credential reads

The paths in the table above are denied as `file-read-data`: their contents and directory listings
are refused, but `stat` still works, so tools that check whether `.env` exists (pytest does while
collecting) keep working. `--allow-read PATH` (repeatable) grants a path back, for a task that
legitimately needs, say, a kubeconfig.

Linux's Landlock can only *allow* paths, not carve holes in an allowed tree, so on Linux reads are
confined to an allow-list instead: the system directories (`/usr`, `/lib`, `/bin`, `/etc`, `/opt`,
`/proc`, `/sys`, `/dev`, `/run`, `/var`, `/tmp`, `/snap`, `/nix`), the workspace and the other
writable paths, and tool directories under `$HOME` (`~/.cargo` granted by its children, without
`credentials` and `credentials.toml`, plus `.rustup`, `.cache`, `.npm`, `.nvm`, `.pyenv`,
`.local/bin`, `.local/lib`, `go`, the git and shell configuration files, and similar). The
credential stores in the table are outside that list, so they cannot be read; `--allow-read PATH`
adds a path. A temp folder (`/tmp`, `/var/tmp`, `$TMPDIR`) that contains a protected path is not
granted and the run gets a private `TMPDIR`. When `/var` or `/opt` contains a protected path, the
run is granted everything in them except the home folder and the protected paths. A run whose
workspace, `--allow-read` path or another system folder contains a protected path does not start,
with the error `on Linux the sandbox cannot protect <path>: granted path <path> contains it` (for
example a workspace at `$HOME`).

One limit remains on Linux: a `.env` file in the workspace is readable by shell commands, because
Landlock cannot deny a file inside the granted workspace; blyte's own `read`, `glob` and `grep`
tools refuse it on both platforms. The environment filter and the write sandbox apply on both.

## The environment filter

A variable is withheld when a `_`-separated part of its name is one of `TOKEN`, `SECRET`,
`SECRETS`, `PASSWORD`, `PASSWD`, `PASS`, `KEY`, `CREDENTIAL`, `CREDENTIALS`, `AUTH`, `COOKIE` or
`SESSION` (so `GITHUB_TOKEN`, `AWS_SECRET_ACCESS_KEY` and `SSH_AUTH_SOCK` are withheld, while
`TOKENIZERS_PARALLELISM` and `PATH` are not), or when its value contains a URL with an inline
password (`postgres://user:pass@host`). blyte prints the withheld names (never values) once at
start, and `--pass-env NAME` keeps one. Passing `--pass-env SSH_AUTH_SOCK` also lifts the macOS
sandbox's deny on that socket, which lets a run push over ssh (on Linux Landlock cannot block unix
sockets by path, so only environment scrubbing applies).

## Network (`--network=off`)

For code that must not leave the machine. blyte's own model calls are unaffected; only the
commands it runs are cut off.

- **macOS:** outbound connections to any non-local host are refused, over TCP and UDP. Localhost
  servers and DNS lookups still work. The Docker daemon's sockets are refused too, since a
  container could otherwise reach the network for the command.
- **Linux (6.7+):** every TCP `connect()` is refused, localhost included. UDP and unix sockets
  (so Docker) are not covered; blyte says so when it starts.

`--network=off` is refused, never silently ignored, when the sandbox is off or unavailable, with
`--container` (set the container's network instead), or on a Linux kernel without Landlock
network support. Package installs need the network, so install dependencies before an offline run.

## The conversation model

Under `--converse`, the conversation model's `bash` shell runs inside the same host sandbox as any
other run, with `--network=off` by default (`--network=on` turns the network back on;
`--sandbox=off` turns the sandbox off). When started from a linked git worktree, writes are also
allowed in the main checkout's `.wt/` and the git common directory.

blyte's own commands (`dispatch`, `wait`, `status`, `review`, `plan`, `init`, `clean`) are run
through the `blyte` tool on the host in the main checkout, outside the shell's sandbox, so
dispatched implementers can start their own sandboxes.

A shell command that must leave the sandbox (for example `git push` or `gh`) can be requested with
`unsandboxed: true` and a one-line `reason`. In a conversation at a terminal, blyte prompts on
`/dev/tty` and runs the command outside the sandbox only when you answer `y` or `yes`. In a
one-shot run, or when no terminal is attached, it is refused.

To allow specific command prefixes without prompting, list them one per line in
`$XDG_CONFIG_HOME/blyte/allow-unsandboxed` (or `~/.config/blyte/allow-unsandboxed`). Blank lines are
ignored. By default no file exists and nothing is allowed in advance.

## Containers

With `--container`, commands run inside that container and the host sandbox does not apply.
Container commands never inherit the host environment. Isolate the container itself: its network
(`--network none`, or an internal network), its mounts and its user.

## Auditing a run

Every `--report` records the settings the run actually had: `sandbox` (`seatbelt`, `landlock`,
`landlock-offline`, `container` or `off`), `network`, `allow_read`, `pass_env` and `withheld_env`.
See [Run reports](reports.md).

## Reporting a vulnerability

See [`SECURITY.md`](https://github.com/dnf0/blyte/blob/main/SECURITY.md).
