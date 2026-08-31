# syntax=docker/dockerfile:1
#
# Minimal CLI image for firecrawl/anydoc (issue #11).
#
# Builds the existing examples/convert.rs as the container entrypoint so a user
# can run:
#
#   docker run --rm -v "$PWD":/work anydoc convert <file>
#
# Multi-stage: the builder carries the Rust toolchain and is discarded; the
# runtime image holds only the compiled `convert` binary + a minimal glibc userland.
# No Node/Python/WASM bindings, no network service, nothing baked in.
#
# NOTE on base images: the runtime is debian:bookworm-slim (glibc) rather than
# alpine/musl. pdf-inspector (the only thing one might suspect of needing native
# deps) is pure Rust, so musl is feasible, but glibc matches the CI runner
# (ubuntu-2404) and a musl/static validation is deferred to a follow-up.

########################################
# Builder
########################################
FROM rust:1.88-bookworm AS build

WORKDIR /srv

# The root Cargo.toml is a workspace root (members = node/python/wasm), so
# cargo refuses to load it unless every member manifest is present and parses.
# Copy the manifests (root + the three tiny binding manifests) so the workspace
# resolves, then stub every crate's src/lib.rs. This lets us pre-build the root
# package's dependencies in a layer cached independently of the real sources.
# We build only the root package (-p anydoc): the binding crates depend ON
# anydoc, not the other way around, so their heavy deps (pyo3, napi,
# wasm-bindgen) are never compiled — the stubs just have to parse.
COPY Cargo.toml Cargo.lock ./
COPY node/Cargo.toml ./node/Cargo.toml
COPY python/Cargo.toml ./python/Cargo.toml
COPY wasm/Cargo.toml ./wasm/Cargo.toml

RUN mkdir -p src node/src python/src wasm/src \
    && echo 'fn main() {}' > src/lib.rs \
    && echo '' > node/src/lib.rs \
    && echo '' > python/src/lib.rs \
    && echo '' > wasm/src/lib.rs \
    && cargo build --release --locked -p anydoc --lib \
    && rm -rf src

# Now copy the real sources and build the CLI example. `--locked` keeps the
# build hermetic against Cargo.lock drift.
COPY src/ ./src/
COPY examples/ ./examples/
COPY README.md LICENSE ./

RUN cargo build --release --locked --example convert

# The example binary lands here.
# target/release/examples/convert

########################################
# Runtime
########################################
FROM debian:bookworm-slim

# ca-certificates keeps future https fetches working; nothing else is needed at
# runtime (documents are read from the mounted /work volume).
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=build /srv/target/release/examples/convert /usr/local/bin/convert

# Documents live on the host; the container converts from a mounted volume.
WORKDIR /work

ENTRYPOINT ["convert"]
