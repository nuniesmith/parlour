# Parlour server image.
# Stage 1 builds the SvelteKit web UI; stage 2 copies that build
# over crates/server/assets (which the binary embeds) and compiles
# the server; the runtime image carries ffmpeg/ffprobe, which the
# transcode and catalog crates drive as subprocesses.

FROM node:22-bookworm-slim AS web
WORKDIR /src/web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock rustfmt.toml ./
COPY crates ./crates
COPY migrations ./migrations
COPY --from=web /src/web/build ./crates/server/assets
RUN cargo build --release -p parlour-server

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ffmpeg ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/parlour /usr/local/bin/parlour
ENV PARLOUR_PORT=8585
EXPOSE 8585
ENTRYPOINT ["parlour"]
