# Parlour server image (placeholder build: compiles the skeleton binary).
# Runtime includes ffmpeg/ffprobe, which the transcode and catalog
# crates drive as subprocesses.

FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml rustfmt.toml ./
COPY crates ./crates
COPY migrations ./migrations
RUN cargo build --release -p parlour-server

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ffmpeg ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/parlour /usr/local/bin/parlour
ENV PARLOUR_PORT=8585
EXPOSE 8585
ENTRYPOINT ["parlour"]
