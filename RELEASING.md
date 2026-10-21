# Releasing

Three packages ship together at one version:

| Package | Registry | Built from |
| -- | -- | -- |
| `docrs` (library) | crates.io | `crates/docrs` |
| `docrs-cli` (binary `docrs`) | crates.io and GitHub release binaries | `crates/docrs-cli` |
| `docrs-wasm` (WebAssembly) | npm | `crates/docrs-wasm` |

`docrs-compat` (the oracle harness) and the `docrs-wasm` crate are not published.

A version tag publishes everything: `.github/workflows/release.yml` builds the CLI for Linux (x86_64, arm64), macOS (arm64, x86_64) and Windows (x86_64) and the npm tarball. It then publishes both crates to crates.io and the tarball to npm, and creates the GitHub release with the archives, the npm tarball, `SHA256SUMS` and the changelog section (when the release already exists, it only uploads the files). Pushing the tag, or creating a release with a new tag on GitHub, starts it.

A version on crates.io or npm can't be replaced, only yanked or deprecated. Fix forward with a new version.

## Registry access

The publish jobs run in the GitHub environment `release`. Give it required reviewers (Settings → Environments) if you want to approve each publish.

**Trusted publishing** (no stored tokens) is the default. It needs each package to exist on its registry first, so it can be set up only after the first release:

- crates.io: on each of `docrs` and `docrs-cli`, Settings → Trusted Publishing → GitHub. Repository `endigo/docrs`, workflow `release.yml`, environment `release`.
- npm: on `docrs-wasm`, Settings → Trusted publishing → GitHub Actions. Same repository, workflow and environment. Allow `npm publish`, because new configurations otherwise allow only `npm stage publish`.

**Tokens**, for the first release, or instead of trusted publishing. Add them as secrets of the `release` environment. A set `CARGO_REGISTRY_TOKEN` takes precedence over trusted publishing; npm tries trusted publishing first and falls back to `NPM_TOKEN`:

- `CARGO_REGISTRY_TOKEN`: a crates.io API token (crates.io → Account Settings → API Tokens) with the publish-new and publish-update scopes.
- `NPM_TOKEN`: an npm granular access token with read and write access to packages and "bypass two-factor authentication" enabled. npm is phasing out direct publishing with such tokens. When it asks for a one-time password instead (`EOTP`), publish the first version by hand from the run's `npm` artifact (`npm publish docrs-wasm-VERSION.tgz --access public`, with your 2FA code) and rerun the failed jobs; the workflow then skips npm.

After the first release, set up trusted publishing and delete both secrets.

## Each release

1. Set the version in `Cargo.toml`, in `[workspace.package]` and in the `docrs` workspace dependency next to it. `tools/release/version.sh` checks that the two agree. The npm package takes the same version when `build.sh` builds it.
2. Date the version's section in `CHANGELOG.md`.
3. Run `tools/release/check.sh` (it needs the minimum Rust toolchain installed, wasm-bindgen-cli at the Cargo.lock version and Node.js) and expect, in order:
   - the version, with a matching `CHANGELOG.md` section;
   - format, clippy and all tests passing;
   - "checks with" the minimum Rust version;
   - both crates packaging (`cargo publish --dry-run`);
   - `docrs --version` showing the version;
   - the npm package building and its tests passing;
   - finally "nothing; ready to publish (RELEASING.md)".

   The oracle parity gates need the oracle checkout at `.reference/paper-docx` and `.venv/` (and LibreOffice for the loader gate): `cargo run --release -p docrs-compat -- check --upstream-rev 3c0f9a5`.
4. Merge to `main`, then tag and push: `git tag v0.1.0 && git push origin v0.1.0`. You can also create the release on GitHub with a new tag `v0.1.0` on `main`.

The workflow refuses a tag that doesn't match the version, or a version with anything left open (`tools/release/ready.sh`). Each publish step skips what its registry already has, so a failed run can be rerun from the Actions tab.
