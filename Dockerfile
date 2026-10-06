# Static Linux build of rusty, plus a small runtime image.
#   docker build -t rusty .                       # runtime image
#   docker build --target bin -o out .            # just the binary, in ./out
FROM rust:1-alpine AS build
RUN apk add --no-cache gcc musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src src
COPY skills skills
RUN cargo build --release --locked && strip target/release/rusty target/release/rusty-memoryd

FROM scratch AS bin
COPY --from=build /src/target/release/rusty /rusty
COPY --from=build /src/target/release/rusty-memoryd /rusty-memoryd

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends git ripgrep ca-certificates python3 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/rusty /usr/local/bin/rusty
COPY --from=build /src/target/release/rusty-memoryd /usr/local/bin/rusty-memoryd
WORKDIR /work
ENTRYPOINT ["rusty"]
