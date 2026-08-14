<!--
Thanks for contributing! Please keep PRs focused and small — the project is
small on purpose. See CONTRIBUTING.md for the two contribution paths
(Rust changes vs. template changes) and the required local checks.
-->

## What does this PR do?

<!-- One or two sentences: the problem and the fix. Link the issue if one exists. -->

## Checklist

- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy --all-targets -- -D warnings` passes
- [ ] `cargo test --all-targets` passes
- [ ] If templates were touched: snapshots regenerated (`INSTA_UPDATE=always cargo test --test snapshots`) and the diff reviewed
- [ ] CHANGELOG.md updated (if user-visible behavior changed)
