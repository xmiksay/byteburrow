# Running ByteBurrow with External Face Embedding Service

This guide shows how to configure ByteBurrow to delegate face embedding to an external service instead of running local ONNX inference.

## Why Use External Service?

- **Reduced resource usage**: Main server doesn't need GPU/memory for ML models
- **Scalability**: Embedding service can be scaled independently
- **Flexibility**: Switch between different embedding models/services easily
- **Separation of concerns**: ML inference isolated from file management

## Quick Start

### 1. Start Your Embedding Service

If you have the embedding service at `/usr/local/bin/face-embed-service`:

```bash
# Start the service (default port 8090)
/usr/local/bin/face-embed-service

# Or specify custom port and model path
MODEL_PATH=/path/to/model.onnx LISTEN_ADDR=0.0.0.0:8090 /usr/local/bin/face-embed-service
```

### 2. Build ByteBurrow with Remote Support

```bash
# Build plugins with remote embedding support
make build-plugins

# Build and run everything
make run
```

### 3. Configure Environment Variables

Add to your `.env` file or set environment variables:

```bash
# Use external embedding service
BYTEBURROW__FACE_EMBED_ENDPOINT=http://localhost:8090/

# Optional: specify model identity (must match what your service uses)
BYTEBURROW__FACE_EMBED_MODEL_ID=faceonnx-recognition-resnet27
BYTEBURROW__FACE_EMBED_MODEL_VERSION=1
```

## Configuration Options

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `BYTEBURROW__FACE_EMBED_ENDPOINT` | HTTP endpoint for embedding service | `http://localhost:8090/` |
| `BYTEBURROW__FACE_EMBED_MODEL` | Path to local model (only for local mode) | `/etc/byteburrow/models/recognition_resnet27.onnx` |

### Plugin Configuration

You can also configure the endpoint via plugin config in your config file:

```toml
[plugins.face_embedder]
face_embed_endpoint = "http://localhost:8090/"
```

## Building with Different Backends

### Remote Backend (Recommended)

```bash
# Build with remote support
cargo build --release --features remote --package byteburrow-plugin-face-embed

# Or using the Makefile
make build-plugins  # This now includes --features remote
```

### Local Backend (Default)

```bash
# Build with local ONNX support
cargo build --release --features local --package byteburrow-plugin-face-embed

# Or without specifying features (default is local)
cargo build --release --package byteburrow-plugin-face-embed
```

## Embedding Service API

The external service must implement the following API:

### POST `/` 

**Request:**
- Body: Raw image bytes (JPEG, PNG, etc.)

**Response (JSON):**
```json
{
  "embedding": [0.1234, -0.5678, ...]  // 512-dimensional vector
}
```

**Example using curl:**
```bash
curl -X POST http://localhost:8090/ \
  --data-binary @face_image.jpg \
  -H "Content-Type: application/octet-stream"
```

## Existing Standalone Service

The project includes a ready-to-use embedding service in `plugins/face-embedder/service/`:

```bash
cd plugins/face-embedder/service
cargo build --release

# Run with default settings
./target/release/face-embed-service

# Run with custom settings
MODEL_PATH=/path/to/model.onnx LISTEN_ADDR=0.0.0.0:8090 \
  ./target/release/face-embed-service
```

## Troubleshooting

### Plugin Fails to Initialize

**Error:** "Remote embedding backend not enabled"

**Solution:** Ensure you built with the `remote` feature:
```bash
cargo build --release --features remote --package byteburrow-plugin-face-embed
```

### Connection Refused

**Error:** "HTTP request failed: Connection refused"

**Solution:** 
1. Check that your embedding service is running: `curl http://localhost:8090/`
2. Verify the `BYTEBURROW__FACE_EMBED_ENDPOINT` is correct
3. Check firewall rules

### Empty Embeddings

If face embeddings are not being generated:

1. Verify the embedding service is working with a test image
2. Check that the face-detector plugin is working (faces must be detected first)
3. Check the ByteBurrow logs for embedding errors

## Performance Considerations

### Remote Service Benefits
- Main server uses less memory and CPU
- Embedding service can be scaled independently
- Can run on GPU-equipped machines

### Remote Service Trade-offs
- Network latency for each embedding request
- Requires service availability and monitoring
- May need connection pooling for high throughput

### Optimization Tips

1. **Batch Processing**: The embedding service handles many concurrent requests efficiently
2. **Local Network**: Run embedding service on same network or machine as ByteBurrow
3. **Connection Pooling**: The plugin reuses HTTP connections
4. **Service Scaling**: Run multiple instances behind a load balancer for high throughput

## Switching Between Local and Remote

You can switch backends by rebuilding the plugin:

```bash
# Switch to remote
make build-plugins  # Uses --features remote
BYTEBURROW__FACE_EMBED_ENDPOINT=http://localhost:8090/

# Switch to local  
cargo build --release --features local --package byteburrow-plugin-face-embed
# Remove BYTEBURROW__FACE_EMBED_ENDPOINT from env
```

## Development

For development purposes, you can also run the embedding service in debug mode:

```bash
cd plugins/face-embedder/service
cargo run

# In another terminal, run ByteBurrow
cargo run --bin byteburrow
```

This makes it easier to test changes and debug issues.