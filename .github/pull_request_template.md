## Summary

<!-- What does this change and why? Link the issue if there is one. -->

## Checks

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --locked --workspace --exclude terrarium-app --all-targets -- -D warnings`
- [ ] `cargo test --locked --workspace --exclude terrarium-app`
- [ ] `node --test ui/tests/*.test.cjs`
- [ ] Cross-repository checks (if runtime, sync, SDK or WIT changed)
- [ ] Docs and CHANGELOG updated where relevant
