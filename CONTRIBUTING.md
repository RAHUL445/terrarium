# Contributing to Terrarium

Thanks for your interest. Terrarium is an experimental project maintained in
spare time, so small, focused changes are the easiest to review.

## Before you start

- For anything larger than a bug fix, open an issue first to agree on the
  approach.
- Security problems go through private reporting; see [SECURITY.md](SECURITY.md).
- Plugin changes belong in
  [terrarium-plugins](https://github.com/RAHUL445/terrarium-plugins). Changes to
  `crates/guest` or `wit/` affect every plugin and need a compatibility review.

## Development setup

Follow the prerequisites and build steps in the [README](README.md). The
[architecture overview](docs/develop/architecture.md) explains how the crates fit
together.

## Checks

Run these before opening a pull request; CI runs the same:

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --exclude terrarium-app --all-targets -- -D warnings
cargo test --locked --workspace --exclude terrarium-app
node --test ui/tests/*.test.cjs
python3 -m unittest discover -s scripts/tests -v
```

If you touch the runtime, sync or plugin APIs, also run the cross-repository
checks from the README.

## Guidelines

- Keep the host kernel small: features belong in plugins unless the plugin API
  cannot express them.
- Storage migrations are additive only; never rename or drop synced columns.
- Plugin templates stay declarative; never run plugin JavaScript in the shell.
- Add tests with synthetic data. Never commit databases, keys, recovery
  phrases or signing files.
- Use [Conventional Commits](https://www.conventionalcommits.org/) for commit
  messages, for example `fix(sync): reject replayed batches`.

## License

By contributing you agree that your contributions are licensed under the
[Apache License 2.0](LICENSE).
