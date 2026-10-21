# Releasing

Three packages ship together at one version:

| Package | Registry | Built from |
| -- | -- | -- |
| `docrs` (library) | crates.io | `crates/docrs` |
| `docrs-cli` (binary `docrs`) | crates.io and GitHub release binaries | `crates/docrs-cli` |
| `docrs` (WebAssembly) | npm | `crates/docrs-wasm` |

`docrs-compat` (the oracle harness) and the `docrs-wasm` crate are not published.

## Before the first release

1. Fill in the `TODO(release)` field in `Cargo.toml`: `repository`.
2. Add `repository` to `crates/docrs-wasm/js/package.json`, and delete `"private": true`.
3. Create the GitHub repository and push to it. `.github/workflows/release.yml` builds the CLI binaries.
4. Get publish rights: a crates.io API token (`cargo login`) and an npm account (`npm login`). The names `docrs` and `docrs-cli` on crates.io and `docrs` on npm were free on 2026-10-21.

## Each release

1. Set the version in `Cargo.toml`, in `[workspace.package]` and in the `docrs` workspace dependency next to it. `tools/release/version.sh` checks that the two agree. The npm package takes the same version when `build.sh` builds it.
2. Date the version's section in `CHANGELOG.md`.
3. Run `tools/release/check.sh` and expect:
   - "nothing; ready to publish";
   - all tests passing;
   - the minimum Rust version checking;
   - both crates packaging;
   - the npm tests passing.

   The oracle parity gates need `.reference/` and `.venv/`: `cargo run --release -p docrs-compat -- check --upstream-rev 3c0f9a5`.
4. Commit, then tag and push: `git tag v0.1.0 && git push origin main v0.1.0`. The release workflow checks the tag against the version (`tools/release/version.sh v0.1.0`). It builds the CLI for Linux (x86_64, arm64), macOS (arm64, x86_64) and Windows (x86_64), builds the npm tarball, and drafts a GitHub release with the archives, `SHA256SUMS` and the changelog section.
5. Publish:
   ```bash
   cargo publish -p docrs -p docrs-cli   # library first, then the CLI
   (cd crates/docrs-wasm/pkg && npm publish)     # the tarball check.sh built
   ```
6. Review the draft GitHub release and publish it.

A version on crates.io or npm can't be replaced, only yanked or deprecated. Fix forward with a new version.
