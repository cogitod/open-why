# Beta releases and upgrades

`0.1.0-beta.1` is a local release candidate, not a published release. There is no
beta tag to install yet. Public beta is a usability milestone; it does not imply
the stable guarantees in STABILITY.md.

## Version policy

Use `0.1.0-beta.N` for the initial beta series. Increment N whenever distributed
bytes or behavior change; never replace a tag or release asset. Document all
user-visible changes, schema expectations, toolchain requirements, feature
policy and known limitations. Before 1.0, breaking Rust/CLI changes require a
minor-version bump after this initial beta series; patch releases are compatible
fixes. Named MCP `/vN` contracts remain versioned independently and must not
silently change meaning. No MSRV increase in a patch release. Downgrades never
migrate newer databases backwards.

## Prepare and validate locally

Use a clean, reviewed topic-branch commit. Python 3.9+, Git, Rust 1.88+ and native
build tools are required by the candidate preparation script.

```bash
cargo fmt --check
cargo build --release --locked
cargo clippy --release --all-targets --locked -- -D warnings
cargo test --locked
cargo test --locked --no-default-features
bash scripts/evaluate-local.sh
python3 scripts/prepare-release.py /absolute/new/candidate-directory
```

The script refuses a dirty tree, packages the locked source, extracts it into a
fresh directory, installs from that archive, and exercises that installed binary:
version, isolated demo, index, capture, retrieval, MCP initialize, backup,
verification, restore and exact evidence read. It produces a source `.crate`, a
host-specific lexical binary archive with resolved Cargo dependency license texts,
platform-named SHA256SUMS, a dependency SBOM and an
unsigned build record containing the source commit, features and toolchain.
No tag, upload, attestation or release is created locally. Cargo caches may be
reused; this is a fresh-source install, not proof of an offline or hermetic build.
Binaries are not claimed to be bit-for-bit reproducible across toolchains/hosts.

Only lexical binaries are initially packaged: this avoids runtime/model download
complexity for the first use. Source builds retain the default local-embeddings
feature. The SBOM describes the lexical Cargo resolution, including build and
development dependencies; it excludes host OS libraries and optional models.

Linux GNU archives inherit the builder's native runtime requirements. The local
Ubuntu 24.04 ARM64 candidate requires glibc 2.39; it is not a portable binary for
older Linux distributions or musl/Alpine. Before publishing each target, record
`ldd why` and `readelf --version-info why` with its tested OS/architecture.
Source installation can support environments that a particular archive cannot.

## Authorized publication process

These steps require maintainer authorization; they are not part of local work.

1. Merge a reviewed PR through the required `leak-check` and `build-and-test`
   checks. The latter explicitly fails if macOS checks fail or are cancelled.
2. Confirm CHANGELOG.md, version and Cargo.lock match. Create a signed annotated
   `v0.1.0-beta.1` tag at the reviewed main commit and push it only after approval.
3. Manually dispatch **Validate release candidate** on that tag. It reruns CI,
   verifies tag/version equality and main ancestry, builds and exercises packaged
   artifacts on Linux/macOS, generates GitHub provenance attestations over final
   bytes, and retains candidate artifacts. It does not create a GitHub release.
4. Download each candidate bundle into separate platform directories. Check its
   platform-named SHA256SUMS and verify each asset's attestation, including expected repository,
   workflow, source revision and runner identity. Do not combine same-named
   manifests from different hosts. Final archives must be tested on their target
   platform before attaching those exact bytes to a release.
5. Publish only with authorization, using the changelog entry as release notes.
   Binary archives, build records, SBOMs and checksum manifests already carry
   platform names. The source archive is shared: require identical source archive
   hashes across builders and publish one copy. Do not modify any final bytes
   after validation or combine conflicting same-named assets.
6. Test the public download URLs and clean installation after publishing; record
   those results. Until then the public-distribution acceptance gate remains open.

Example verification after downloading an asset from an authorized release:

```bash
shasum -a 256 -c SHA256SUMS-aarch64-apple-darwin
gh attestation verify open-why-0.1.0-beta.1-aarch64-apple-darwin-lexical.tar.gz \
  --repo cogitod/open-why \
  --signer-workflow cogitod/open-why/.github/workflows/release-candidate.yml
```

A checksum detects byte changes; a checksum served alongside modified content
cannot authenticate a publisher. Verify provenance and the reviewed commit too.
Local unsigned candidates have no GitHub attestation and must be labeled as such.

## Immutable installation

Before publication, use a supplied reviewed checkout or the locally generated
source archive. The archive contains its Cargo.lock and `.cargo_vcs_info.json`.

```bash
tar -xzf open-why-0.1.0-beta.1.crate
cargo install --locked --path open-why-0.1.0-beta.1 --bin why --no-default-features
why --version
```

After the authorized tag exists publicly:

```bash
cargo install --locked --git https://github.com/cogitod/open-why \
  --tag v0.1.0-beta.1 --bin why --no-default-features
```

For immutable source identity, use `--rev <full-reviewed-40-character-commit>`
in place of `--tag`; record the commit and downloaded archive checksum. Tags
must never be moved, but a full commit also protects against accidental retagging.
Neither command tracks moving main. Omit `--no-default-features` for pinned local
model support. Published lexical archives contain `why`; verify before extraction
and place it in a directory on PATH. Check `why --version` before reconnecting.

Before adding `--force` to upgrade an existing installation, follow
[backup and recovery](docs/maintenance.md). Keep the previous executable and a
verified pre-upgrade snapshot until the restored-copy trial succeeds.
