FROM node:22-slim AS web
WORKDIR /build/web
COPY web/ .
RUN npm ci && npm run build

FROM rust:1-slim AS builder
RUN apt-get update && apt-get install -y pkg-config g++ && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
COPY lib/ lib/
COPY --from=web /build/web/dist/ web/dist/
RUN cargo build --release --features bundle-web

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/hearth-mud /usr/local/bin/hearth-mud
WORKDIR /data
EXPOSE 4000 8000
ENTRYPOINT ["hearth-mud"]
