# Use a slim Debian image for the runner
FROM debian:bookworm-slim

# Install necessary runtime dependencies (e.g., SSL certificates)
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Set working directory
WORKDIR /app

# Copy binaries from the host build directory
# Expects binaries to be built with: cargo build --release
COPY target/release/web /usr/local/bin/web
COPY target/release/agent /usr/local/bin/agent

# Default command (can be overridden in docker-compose)
CMD ["web"]
