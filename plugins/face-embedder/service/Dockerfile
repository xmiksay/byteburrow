FROM rust:1.86 AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock* ./
COPY src/ src/
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/face-embed-service /usr/local/bin/
COPY recognition_resnet27.onnx /models/recognition_resnet27.onnx
EXPOSE 8090
ENV MODEL_PATH=/models/recognition_resnet27.onnx
ENV LISTEN_ADDR=0.0.0.0:8090
CMD ["face-embed-service"]
