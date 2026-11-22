FROM rust:bookworm

RUN apt-get update -qq \
    && apt-get install -y -qq --no-install-recommends \
       openjdk-17-jre-headless \
       ca-certificates \
       curl \
       gnupg \
       openssl \
    && mkdir -p /etc/apt/keyrings \
    && curl -fsSL https://packages.adoptium.net/artifactory/api/gpg/key/public | gpg --dearmor -o /etc/apt/keyrings/adoptium.gpg \
    && echo "deb [signed-by=/etc/apt/keyrings/adoptium.gpg] https://packages.adoptium.net/artifactory/deb bookworm main" > /etc/apt/sources.list.d/adoptium.list \
    && apt-get update -qq \
    && apt-get install -y -qq --no-install-recommends \
       temurin-21-jre \
       temurin-25-jre \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace/rust
