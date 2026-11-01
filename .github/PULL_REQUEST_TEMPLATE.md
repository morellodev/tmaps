<!--
Use Conventional Commits for the PR title and for each commit, for example "feat: add a dark theme".
release-plz reads the commit messages on main to set the version and to write the changelog.
-->

## Summary

<!-- What does this PR change, and why? Link the issue, for example "Closes #12". -->

## Test plan

<!-- How did you test the change? Write one item for each check or manual test. -->

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
- [ ] `cargo test --workspace --locked`

## Checklist

<!-- Mark the items that do not apply as done, and write "not applicable" after them. -->

- [ ] A change to a CLI command, a flag, or a JSON document also updates `skills/tmaps/SKILL.md` and `skills/tmaps/references/cli-contract.md`.
- [ ] A change to a JSON document adds a schema in `skills/tmaps/assets/schemas/`, with a higher `@` number if the change is not compatible.
- [ ] A change to `tmaps-core` or `tmaps-braille` keeps the WebAssembly build working. See the README `Development` section.
- [ ] A change that users can see also updates the README or the crate README.
