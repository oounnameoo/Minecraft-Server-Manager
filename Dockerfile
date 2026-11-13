# syntax=docker/dockerfile:1

###############################################################################
# Web build stage
###############################################################################
FROM docker.io/library/node:22-alpine AS web

WORKDIR /app

COPY web/ ./web/
WORKDIR /app/web

# pnpm is not present in the base image; install it to match the lockfile.
RUN npm install -g pnpm@11

# Use the lockfile when available, otherwise fall back to a fresh install.
RUN if [ -f pnpm-lock.yaml ]; then \
        pnpm install --frozen-lockfile; \
    else \
        pnpm install; \
    fi

ARG MCM_BUILD_COMMIT=unknown
RUN MCM_BUILD_COMMIT="$MCM_BUILD_COMMIT" pnpm build

###############################################################################
# Rust build stage
###############################################################################
FROM docker.io/library/rust:1-alpine AS rust

RUN apk add --no-cache musl-dev gcc g++ make cmake perl pkgconfig

WORKDIR /src

COPY migrations/ ./migrations/
COPY rust/ ./rust/
COPY --from=web /app/web/dist ./web/dist

ARG MCM_BUILD_COMMIT=unknown
ARG MCM_BUILD_DIRTY=false
ARG MCM_BUILD_TIME
WORKDIR /src/rust
RUN MCM_BUILD_COMMIT="${MCM_BUILD_COMMIT}" \
    MCM_BUILD_DIRTY="${MCM_BUILD_DIRTY}" \
    MCM_BUILD_TIME="${MCM_BUILD_TIME:-$(date -u +%Y-%m-%dT%H:%M:%SZ)}" \
    cargo build --release --workspace && \
    mkdir -p /out && \
    cp target/release/mcm /out/mcm && \
    cp target/release/mcm-cli /out/mcm-cli

###############################################################################
# Runtime stage
###############################################################################
FROM docker.io/library/alpine:3.20 AS runtime

RUN apk add --no-cache ca-certificates tzdata su-exec libgcc

COPY --from=rust /out/mcm /usr/local/bin/mcm
COPY --from=rust /out/mcm-cli /usr/local/bin/mcm-cli
COPY docker/entrypoint.sh /entrypoint.sh

RUN chmod +x /entrypoint.sh && mkdir -p /data

ENV MCM_ADDR=:8080 \
    MCM_DATA_DIR=/data

EXPOSE 8080

ENTRYPOINT ["/entrypoint.sh"]
