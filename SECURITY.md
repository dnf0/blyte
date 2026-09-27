# Security policy

## Reporting a vulnerability

Please report vulnerabilities privately, through GitHub's
[private vulnerability reporting](https://github.com/climate-x-org/superstate/security/advisories/new)
for this repository, not in a public issue. Include what you found, how to reproduce it, and the
blyte version (`blyte --version` prints the build commit).

We aim to acknowledge a report within a week and to agree a fix and disclosure date with you.

## Scope

In scope, most importantly: ways for a command run by blyte to get past its sandbox, for example
to write outside the allowed paths, read a denied credential store on macOS, see a withheld
environment variable, or connect out under `--network=off` where the platform claims to block it.

Known limits, documented and not vulnerabilities in themselves (see
[Security and the sandbox](docs/security.md)):

- On Linux the credential read-deny is not enforced (Landlock cannot express it).
- On Linux `--network=off` blocks TCP only; UDP and unix sockets (Docker) are not covered.
- On macOS `--network=off` still allows localhost and DNS lookups.
- `--sandbox=off`, `--allow-read` and `--pass-env` relax the sandbox by design.
- In `--container` mode the host sandbox does not apply; the container is the boundary.
- The sandbox does not review the code the model writes.

## Supported versions

Security fixes go to the latest release and `main`.
