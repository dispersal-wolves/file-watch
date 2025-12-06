<p align="center">
  <img src="docs/banner.svg" alt="File Watch — Dispersal Wolves" width="100%">
</p>

# File Watch

**Portable file-integrity baselines with no service to join.**

Builds SHA-256 baselines, compares directory trees, and watches them by polling. It never uploads file contents.

## Start

```console
cargo run -- baseline ./watched --output baseline.dw
```

Run the command with `--help` for every option. The tool works locally, collects no telemetry, and supports machine-readable output where applicable.

## Principles

- **Local first.** Host data stays on the host unless you explicitly configure a webhook.
- **Safe by default.** Inspection is read-only and mutation requires a deliberate command.
- **Small contract.** The tool solves one defensive job and reports its limits plainly.
- **Scriptable.** Stable exit codes and structured output make automation practical.

## Platform

The initial release targets Linux. Portable behavior is also tested on Windows where the underlying operating-system facilities allow it. See [the threat model](docs/threat-model.md) for trust boundaries and non-goals.

## Development

This repository uses **Rust** without third-party crates.

```console
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo build --release
```

## License

[MIT](LICENSE) © Dispersal Wolves.
