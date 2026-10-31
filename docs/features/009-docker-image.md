# 9. Docker image and CI

## Outcome

`docker build .` produces a small image that runs Sous from `scratch`: one statically linked binary and nothing else. A GitHub Actions workflow publishes multi-arch images (linux/amd64 and linux/arm64) to the GitHub Container Registry when a version tag is pushed. A second workflow runs the test suites on every pull request and on every push to `main`.

Depends on: nothing. The image runs the service as it is today.

## Why scratch works

The binary needs nothing from the system at run time:

- SQLite is compiled into the binary: the `sqlx` `sqlite` feature includes `sqlite-bundled`.
- Timezone data is compiled in by `chrono-tz`, so the image needs no `/usr/share/zoneinfo`.
- The service makes no outbound TLS connections, so it needs no CA certificates.
- Static linking with musl removes the need for a C library.

## Scope

### Dockerfile

- Use a multi-stage build. The builder stage uses an official `rust` Alpine image, pinned to a Rust minor version that supports edition 2024 (1.85 or later). It builds with `cargo build --release --locked` for the platform the image is built for. Builds are native, with no cross-compilation and no QEMU emulation.
- Strip the binary through the environment (`CARGO_PROFILE_RELEASE_STRIP=true`), not by editing `Cargo.toml`.
- Use BuildKit cache mounts for the cargo registry and `target/`, so a rebuild after a source change does not recompile every dependency.
- The final stage is `FROM scratch` and contains only:
  - the binary at `/sous`
  - an empty `/data`, owned by UID/GID `65532`, which holds the database
  - an empty `/tmp` with mode `1777`, which SQLite uses for temporary files
- Run as `USER 65532:65532`. Named volumes mounted at `/data` take its ownership.
- Set these defaults in the image. Each can be overridden with `-e`:

  | Variable | Image default |
  | --- | --- |
  | `SOUS_ADDR` | `0.0.0.0:3000` |
  | `SOUS_DATABASE` | `/data/sous.db` |
  | `RUST_LOG` | `info` |

  `APP_TIMEZONE` and the MCP variables keep their binary defaults.
- `EXPOSE 3000`, `VOLUME /data`, `ENTRYPOINT ["/sous"]`. Run `docker run <image> migrate` to apply migrations only.
- Leave out `HEALTHCHECK`: the image has no shell or HTTP client. Orchestrators should probe `GET /` instead.

### `.dockerignore`

Use an allowlist. Ignore everything, then re-include only the files the build needs: `Cargo.toml`, `Cargo.lock`, `build.rs`, `src/`, `web/` and `migrations/`. Local databases, `target/`, `.git` and `.mcp.json` must never enter the build context.

### Shutdown on SIGTERM

`docker stop` sends SIGTERM. The binary currently shuts down gracefully only on Ctrl+C (SIGINT). As PID 1 in a container it ignores SIGTERM and is killed when the stop timeout runs out. The server must shut down gracefully on either SIGINT or SIGTERM and exit with status 0. Gate the SIGTERM handling on `cfg(unix)`. On other platforms, keep Ctrl+C.

### Publishing workflow

Add `.github/workflows/docker.yml`:

- **Triggers.** A pushed tag matching `v*` builds and pushes. A pull request builds both platforms without pushing. `workflow_dispatch` lets a maintainer start a run by hand.
- **Version check.** On a tag, the job fails before building if the tag without its `v` prefix differs from the `version` in `Cargo.toml`.
- **Native builds.** Each platform builds on its own runner, `ubuntu-24.04` for amd64 and `ubuntu-24.04-arm` for arm64. Each build pushes by digest. A final job merges the digests into one multi-arch manifest.
- **Image name.** `ghcr.io/<owner>/<repo>`, lowercased and taken from the repository the workflow runs in. Nothing is hard-coded.
- **Tags.** `vX.Y.Z` publishes `X.Y.Z`, `X.Y` and `latest`. A pre-release tag such as `v1.0.0-rc.1` publishes only its own version and does not move `latest`. Generate tags and OCI labels with `docker/metadata-action`. Its labels include `org.opencontainers.image.source`, which links the package to the repository.
- **Cache.** Use the GitHub Actions cache (`type=gha`), scoped per platform.
- **Permissions.** `contents: read` everywhere. `packages: write` only on the jobs that push. Log in with `GITHUB_TOKEN`; no other secrets.
- **Actions.** Pin every third-party action to a full commit SHA, with the version in a trailing comment.

### Test workflow

Add `.github/workflows/ci.yml`:

- **Triggers.** Pull requests and pushes to `main`. Releases are tagged from `main`, which has already passed, so this workflow does not gate the publishing workflow.
- **Jobs.** Four separate jobs on `ubuntu-24.04`, so each failure is reported on its own:

  | Job | Command |
  | --- | --- |
  | Format | `cargo fmt --all --check` |
  | Lint | `cargo clippy --locked --all-targets -- -D warnings` |
  | Tests | `cargo test --locked` |
  | Browser tests | `cargo test --locked --test browser -- --ignored` |

  Fix any existing formatting differences and clippy warnings as part of this feature, so `main` passes from the first run. Fix the code; don't add `allow` attributes unless a lint is wrong for the case, and then explain why in a comment.
- **Toolchain.** Use the same Rust minor version as the Dockerfile's builder image, with the `rustfmt` and `clippy` components. Bump the two together.
- **Chrome.** The browser job installs a pinned Chrome with a setup action and sets `CHROME` to its path. It does not depend on whatever browser the runner image happens to ship.
- **Cache.** Cache the cargo registry and `target/`, keyed on `Cargo.lock`.
- **Permissions.** `contents: read`; no secrets.
- **Actions.** Pin every third-party action to a full commit SHA, as in the publishing workflow.

### Documentation

- README: add a "Docker" section covering:
  - building the image locally
  - running it with a named volume and `-p 3000:3000`
  - the image defaults above
  - pulling the published image
  - bind mounts, which need a host directory writable by UID 65532 or a matching `--user`
  - running behind a reverse proxy, which requires setting `SOUS_MCP_ALLOWED_HOSTS` to the public host name
  - seeding: the image has no shell or `sqlite3`, so seed the database file from the host
- Technical design: in Architecture, add one sentence saying the service ships as a static binary in a `scratch` image.
- Add a row for this feature to the [verification record](../verification.md).

## Out of scope

- A `HEALTHCHECK` or a self-probe subcommand.
- Docker Compose files and deployment manifests.
- Images for platforms other than linux/amd64 and linux/arm64.

## Acceptance criteria

1. `docker build .` succeeds from a clean checkout on amd64 and arm64 hosts.
2. The final stage is `FROM scratch` and copies only `/sous`, `/data` and `/tmp`. The binary is statically linked: `file` on the extracted binary reports "statically linked" or "static-pie linked".
3. `docker run -p 3000:3000 -v sous-data:/data <image>` serves `/` and `/api/<year>/<week>` on `http://localhost:3000`. The process runs as UID 65532. Sous creates the database at `/data/sous.db`, and its data survives a container restart.
4. MCP requests to `http://localhost:3000/mcp` through the published port succeed with the default allowed hosts.
5. `docker run --rm -v sous-data:/data <image> migrate` applies migrations and exits 0.
6. `docker stop` exits the container with status 0 well before the default 10-second timeout, and the logs show a graceful shutdown.
7. The build context excludes local databases, `target/` and `.git`. A `sous.db` in the working tree does not reach the image.
8. On a pull request, the workflow builds both platforms and pushes nothing.
9. Pushing `vX.Y.Z` with a matching `Cargo.toml` version publishes a manifest list for linux/amd64 and linux/arm64 under `X.Y.Z`, `X.Y` and `latest`. A tag that does not match the `Cargo.toml` version fails before building.
10. On a pull request, the test workflow runs all four jobs and passes. A deliberately failing test in either suite, a formatting difference or a clippy warning fails its job.

## Verification

Add an integration test that runs the built binary with a temporary database. It waits for the "listening" log line, sends SIGTERM and asserts exit status 0 within 5 seconds. Checks 1–7 are manual Docker checks; record the image size alongside them in the [verification record](../verification.md). Checks 8 and 10 are pull-request runs; for check 10, push one commit that breaks a test, misformats a line and introduces a clippy warning, then revert it. Check 9 is a test tag pushed to a fork or a pre-release tag.
