# Releasing

The Go module and the Rust crate are versioned and released **independently**,
each by pushing a tag that points at a commit on `main`:

| Client | Tag | Version source | Published to |
|---|---|---|---|
| Go | `go/vX.Y.Z` | `const Version` in `go/typesafe.go` | Go module proxy / [pkg.go.dev](https://pkg.go.dev/github.com/haileyok/typesafe-client/go) |
| Rust | `rust/vX.Y.Z` | `version` in `rust/Cargo.toml` | [crates.io](https://crates.io/crates/typesafe-system-one) as `typesafe-system-one` |

The Go tag must carry the `go/` prefix because the module lives in the `go/`
subdirectory. That's how Go versions subdirectory modules.

Pushing a tag runs [`.github/workflows/release.yml`](.github/workflows/release.yml), which:

1. rejects tags that aren't `go/vX.Y.Z` or `rust/vX.Y.Z` (semver; a `-suffix` marks a prerelease),
2. rejects tags whose commit isn't on `main`,
3. rejects tags that don't match the version in the code,
4. runs the full CI matrix on the tagged commit,
5. publishes: Go through proxy.golang.org, Rust through `cargo publish`,
6. creates a GitHub Release with generated notes since the previous tag for that client.

Re-running a partially failed release is safe. An already-published crate version
or an existing GitHub Release is skipped.

## Cutting a release

1. On a branch, bump the version:
   - Go: `const Version` in `go/typesafe.go`.
   - Rust: `version` in `rust/Cargo.toml`, then run `cargo check` in `rust/` to
     update `Cargo.lock`. CI builds with `--locked` and fails on a stale lockfile.
2. Merge to `main` with CI green.
3. Tag the merged commit and push the tag:

   ```sh
   git fetch origin
   git tag -a go/v0.2.0 -m "Go v0.2.0" origin/main      # or rust/v0.2.0
   git push origin go/v0.2.0
   ```

4. Watch the run: `gh run watch $(gh run list --workflow release.yml --limit 1 --json databaseId -q '.[0].databaseId')`.

Go versions are permanent once the proxy has fetched them: a tag can't be
reused for different code. Fix mistakes with a new patch version. A Go major
version 2 or higher also needs a `/v2` suffix on the module path
(`github.com/haileyok/typesafe-client/go/v2`).

## One-time setup: first crates.io publish

crates.io only allows trusted (tokenless) publishing for a crate that already
exists, so the **first** Rust release needs an API token. Either path works.

**Option A: publish from your machine, then tag**

```sh
cargo login                       # token from https://crates.io/settings/tokens
cd rust && cargo publish --locked
git tag -a rust/v0.1.0 -m "Rust v0.1.0" origin/main && git push origin rust/v0.1.0
```

The workflow sees that 0.1.0 is already on crates.io, skips the publish, and
creates the GitHub Release.

**Option B: let the workflow publish with a temporary token**

1. Create a crates.io token with the `publish-new` and `publish-update` scopes,
   restricted to the crate `typesafe-system-one`.
2. Store it as a secret of the `release` environment:
   `gh secret set CARGO_REGISTRY_TOKEN --env release --repo haileyok/typesafe-client`
3. Push `rust/v0.1.0`.

**Then, for either option, switch to trusted publishing:**

1. On crates.io, open **typesafe-system-one → Settings → Trusted Publishing** and add
   a GitHub publisher: owner `haileyok`, repository `typesafe-client`, workflow
   `release.yml`, environment `release`.
2. If you used Option B, delete the secret and revoke the token:
   `gh secret delete CARGO_REGISTRY_TOKEN --env release --repo haileyok/typesafe-client`.
   While that secret exists, the workflow uses it instead of trusted publishing.
3. Optionally enable **Trusted publishing only** in the crate settings.

From then on, pushing a `rust/v*` tag publishes with a short-lived OIDC token
and no stored credentials. The `release` environment only accepts deployments
from `rust/v*` tags.
