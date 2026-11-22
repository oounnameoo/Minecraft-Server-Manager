# Multi-stage production Dockerfile for MCM (Minecraft Server Manager)
FROM node:24-bookworm-slim AS web-builder
WORKDIR /src/web
RUN corepack enable && corepack prepare pnpm@11 --activate
COPY web/package.json web/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY web ./
RUN pnpm build

FROM rust:1-bookworm AS rust-builder
WORKDIR /src
COPY rust ./rust
COPY migrations ./migrations
COPY --from=web-builder /src/web/dist ./web/dist
RUN cd rust && cargo build --release -p mcm-server -p mcm-cli

FROM debian:bookworm-slim AS runtime
RUN apt-get update -qq \
    && apt-get install -y -qq --no-install-recommends \
       openjdk-17-jre-headless \
       ca-certificates \
       curl \
       gnupg \
       openssl \
       python3 \
       tar \
       gzip \
    && mkdir -p /etc/apt/keyrings \
    && curl -fsSL https://packages.adoptium.net/artifactory/api/gpg/key/public | gpg --dearmor -o /etc/apt/keyrings/adoptium.gpg \
    && echo "deb [signed-by=/etc/apt/keyrings/adoptium.gpg] https://packages.adoptium.net/artifactory/deb bookworm main" > /etc/apt/sources.list.d/adoptium.list \
    && apt-get update -qq \
    && apt-get install -y -qq --no-install-recommends \
       temurin-21-jre \
       temurin-25-jre \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /var/lib/mcm/backups /var/lib/mcm/servers

COPY --from=rust-builder /src/rust/target/release/mcm /usr/local/bin/mcm
COPY --from=rust-builder /src/rust/target/release/mcm-cli /usr/local/bin/mcm-cli

WORKDIR /var/lib/mcm
EXPOSE 8080 25565
CMD ["mcm"]
