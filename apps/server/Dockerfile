# syntax=docker/dockerfile:1
# Kooperatif API server — local pilot packaging (PILOT-FIX-001 F2).
#
# Build context MUST be the repository root: migrations/ is embedded
# into the binary at compile time (sqlx::migrate! "../../migrations"),
# and Cargo.lock lives under apps/server/.
#
#   docker build -f apps/server/Dockerfile -t kooperatif-server:pilot .

FROM rust:1.92-bookworm AS build
WORKDIR /src

# Dependency layer: build a dummy target first so source changes do not
# invalidate the (expensive) dependency compile cache.
COPY apps/server/Cargo.toml apps/server/Cargo.lock apps/server/
RUN mkdir -p apps/server/src \
    && echo 'fn main() {}' > apps/server/src/main.rs \
    && touch apps/server/src/lib.rs \
    && cargo build --release --manifest-path apps/server/Cargo.toml \
    && rm -rf apps/server/src

# Real source + embedded migrations. `cargo clean -p` before the build
# is mandatory: COPY preserves source mtimes, so without invalidating
# the crate's fingerprint cargo would happily reuse the DUMMY binary
# built above.
COPY apps/server/src apps/server/src
COPY migrations migrations
RUN cargo clean --release --manifest-path apps/server/Cargo.toml -p kooperatif-server \
    && cargo build --release --manifest-path apps/server/Cargo.toml

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system kooperatif \
    && useradd --system --gid kooperatif kooperatif
COPY --from=build /src/apps/server/target/release/kooperatif-server /usr/local/bin/kooperatif-server
USER kooperatif
EXPOSE 8080
# Subcommands: serve (default) | migrate | create-user | grant-role.
ENTRYPOINT ["kooperatif-server"]
CMD ["serve"]
