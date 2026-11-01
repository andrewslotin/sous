# syntax=docker/dockerfile:1

# Bump the Rust minor version together with the toolchain in .github/workflows/ci.yml.
FROM rust:1.99-alpine AS builder
ENV CARGO_PROFILE_RELEASE_STRIP=true
WORKDIR /build
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
COPY web ./web
COPY migrations ./migrations
# target/ is a cache mount, so the binary is copied out in the same step.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release --locked \
    && mkdir -p /out/data /out/tmp \
    && chmod 1777 /out/tmp \
    && cp target/release/sous /out/sous

# The binary is static and embeds SQLite, time zone data and the frontend, so it needs nothing else.
FROM scratch
COPY --from=builder /out/sous /sous
COPY --from=builder --chown=65532:65532 /out/data /data
# SQLite writes temporary files here.
COPY --from=builder /out/tmp /tmp

ENV SOUS_ADDR=0.0.0.0:3000 SOUS_DATABASE=/data/sous.db RUST_LOG=info
USER 65532:65532
EXPOSE 3000
VOLUME /data
ENTRYPOINT ["/sous"]
