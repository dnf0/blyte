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
| **Credential reads** refused: `~/.ssh`, `~/.aws`, `~/.azure`, `~/.config/gcloud`, `~/.config/gh`, `~/.kube`, `~/.gnupg`, `~/.docker/config.json`, `~/.git-credentials`, `~/.netrc`, `~/.pypirc`, cargo credentials, `~/.codex`, keychains, cookies and browser profiles, and any file named `.env` | Seatbelt | not enforced | `--allow-read PATH` grants one back |
| **Environment**: variables that look like credentials, and the AWS profile variables, are withheld from commands | yes | yes | `--pass-env NAME` keeps one |
| **Network**: commands cannot connect out | Seatbelt | Landlock (kernel 6.7+) | opt in with `--network=off` |

blyte's own provider keys (`GEMINI_API_KEY`, `ANTHROPIC_API_KEY`, …) and `BLYTE_*` variables are
always removed from commands, sandbox or not.

## Writes

Commands may write under:

- the workspace (the directory blyte runs in) and its git common directory, so commits work in a
  worktree;
- system temp directories (`/tmp`, `/var/tmp`, `$TMPDIR`, and macOS's `/private/var/folders`);
- tool caches under `$HOME`: `.cargo`, `.rustup`, `.cache`, `.npm`, `.local`, `Library/Caches`,
  `.gradle`, `.m2`, `go`, `.docker/buildx`;
- `/dev` (for `/dev/null` and terminals).

Anything else fails with "Operation not permitted". This is what stops a model that "fixes" a
failing test by clearing a system cache, editing your shell profile or writing into another
checkout.

## Credential reads (macOS)

The paths in the table above are denied as `file-read-data`: their contents and directory listings
are refused, but `stat` still works, so tools that check whether `.env` exists (pytest does while
collecting) keep working. `--allow-read PATH` (repeatable) grants a path back, for a task that
legitimately needs, say, a kubeconfig.

Linux's Landlock can only *allow* paths, not carve holes in an allowed tree, so the read-deny is
macOS-only; blyte says so when it starts on Linux. The environment filter and the write sandbox
apply on both.

## The environment filter

A variable is withheld when a `_`-separated part of its name is one of `TOKEN`, `SECRET`,
`SECRETS`, `PASSWORD`, `PASSWD`, `PASS`, `KEY`, `CREDENTIAL`, `CREDENTIALS`, `AUTH`, `COOKIE` or
`SESSION` (so `GITHUB_TOKEN`, `AWS_SECRET_ACCESS_KEY` and `SSH_AUTH_SOCK` are withheld, while
`TOKENIZERS_PARALLELISM` and `PATH` are not), or when its value contains a URL with an inline
password (`postgres://user:pass@host`). blyte prints the withheld names (never values) once at
start, and `--pass-env NAME` keeps one.

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

## Containers

With `--container`, commands run inside that container and the host sandbox does not apply.
Container commands never inherit the host environment. Isolate the container itself: its network
(`--network none`, or an internal network), its mounts and its user.

## Auditing a run

Every `--report` records the settings the run actually had: `sandbox` (`seatbelt`, `landlock`,
`landlock-offline`, `container` or `off`), `network`, `allow_read`, `pass_env` and `withheld_env`.
See [Run reports](reports.md).

## Reporting a vulnerability

See [`SECURITY.md`](https://github.com/climate-x-org/blyte/blob/main/SECURITY.md).
