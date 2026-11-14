FROM rust:bookworm

RUN apt-get update -qq \
    && apt-get install -y -qq --no-install-recommends \
       openjdk-17-jre-headless \
       ca-certificates \
       curl \
       openssl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace/rust
