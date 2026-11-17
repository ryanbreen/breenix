FROM rust:1.91-slim

# Binary filesystem tools only; the BreenFS formatter uses this checkout's code.
RUN apt-get update && apt-get install -y --no-install-recommends e2fsprogs python3 \
    && rm -rf /var/lib/apt/lists/*
