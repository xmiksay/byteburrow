FROM rust:1.86 AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock* ./
COPY src/ src/
RUN cargo build --release

FROM debian:bookworm-slim
# curl is required by the docker-compose healthcheck (GET /health).
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/face-embed-service /usr/local/bin/
# The ONNX model is NOT baked into the image (not tracked in git — see README).
# Mount it at /models/recognition_resnet27.onnx (docker-compose does this via
# FACE_EMBED_MODEL_PATH) or point MODEL_PATH at wherever it lives.
RUN mkdir -p /models
EXPOSE 8090
ENV MODEL_PATH=/models/recognition_resnet27.onnx
ENV LISTEN_ADDR=0.0.0.0:8090
ENV RUST_LOG=face_embed_service=info,tower_http=info
CMD ["face-embed-service"]
