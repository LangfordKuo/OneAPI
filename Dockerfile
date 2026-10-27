FROM node:22-alpine AS web-builder
WORKDIR /app/web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

FROM rust:1-slim-bookworm AS rust-builder
WORKDIR /app
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential pkg-config \
    && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
COPY migrations ./migrations
COPY --from=web-builder /app/web/dist ./web/dist
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -r -u 10001 -m oneapi
WORKDIR /app
COPY --from=rust-builder /app/target/release/oneapi /usr/local/bin/oneapi
RUN mkdir -p /data && chown -R oneapi:oneapi /data
USER oneapi
ENV ONEAPI_ADDR=0.0.0.0:3000 \
    ONEAPI_DATA_DIR=/data
EXPOSE 3000
VOLUME ["/data"]
ENTRYPOINT ["/usr/local/bin/oneapi", "serve"]
