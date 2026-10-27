FROM node:24-alpine AS web-builder
WORKDIR /app/web
COPY web/package.json web/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm npm ci
COPY web/ ./
RUN npm run build

FROM rust:1-slim-bookworm AS rust-builder
WORKDIR /app
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential \
    && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
COPY migrations ./migrations
COPY --from=web-builder /app/web/dist ./web/dist
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --release --locked

FROM debian:bookworm-slim
LABEL org.opencontainers.image.title="OneAPI" \
      org.opencontainers.image.description="Self-hosted AI API gateway" \
      org.opencontainers.image.licenses="GPL-3.0-only"
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
