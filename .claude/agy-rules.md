Repo rules (blyte):
- Do NOT run cargo publish, release-plz, or anything that creates a tag or release.
- Commit with --no-gpg-sign.
- Before committing, run: cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets. Report the results.
