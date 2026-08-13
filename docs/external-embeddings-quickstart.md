# Running ByteBurrow with External Face Embeddings

This guide shows how to configure ByteBurrow to delegate face embedding to your external service at `/usr/local/bin/face-embed-service`.

## Quick Start

### 1. Start Your Embedding Service

Ensure your embedding service is running and accessible:

```bash
# Start the service (default port 8090)
/usr/local/bin/face-embed-service

# Or specify custom port and model path
MODEL_PATH=/path/to/model.onnx LISTEN_ADDR=0.0.0.0:8090 /usr/local/bin/face-embed-service
```

### 2. Configure ByteBurrow

Add this environment variable to your `.env` file:

```bash
BYTEBURROW__FACE_EMBED_ENDPOINT=http://localhost:8090/
```

### 3. Build and Run

```bash
# Build the plugins
make build-plugins

# Run ByteBurrow
make run
```

## Automatic Setup Script

Use the provided setup script to configure everything automatically:

```bash
./scripts/setup-external-embeddings.sh
```

This script will:
- Verify your embedding service exists
- Start the service if it's not running
- Configure the `.env` file with the correct endpoint
- Provide instructions for building and running

## Manual Setup

### Step 1: Verify Embedding Service

Check that your embedding service is working:

```bash
# Test if service is running
curl -X POST http://localhost:8090/ --data-binary @test-image.jpg
```

You should get a JSON response with an embedding vector.

### Step 2: Update Configuration

Create or update your `.env` file:

```bash
# ByteBurrow Configuration
BYTEBURROW__FACE_EMBED_ENDPOINT=http://localhost:8090/
```

### Step 3: Build Plugins

Build the updated face embedding plugin with remote support:

```bash
make build-plugins
```

This will compile the plugin and place it in `target/plugins/`.

### Step 4: Run ByteBurrow

```bash
# Release mode
make run

# Or development mode
make dev
```

## Configuration Options

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `BYTEBURROW__FACE_EMBED_ENDPOINT` | HTTP endpoint for embedding service | `http://localhost:8090/` |
| `EMBED_SERVICE` | Path to embedding service (setup script) | `/usr/local/bin/face-embed-service` |
| `EMBED_PORT` | Port for embedding service (setup script) | `8090` |

### Plugin Configuration

You can also configure the endpoint via plugin config in your config file:

```toml
[plugins.face_embedder]
face_embed_endpoint = "http://localhost:8090/"
```

## Embedding Service API

Your external service must implement the following API:

### POST `/` 

**Request:**
- Body: Raw image bytes (JPEG, PNG, etc.)
- Content-Type: `application/octet-stream`

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

## Troubleshooting

### Plugin Fails to Initialize

**Error:** "HTTP client not initialized"

**Solution:** Ensure the plugin was built successfully:
```bash
make build-plugins
# Check if the plugin exists
ls -la target/plugins/libbyteburrow_plugin_face_embed.so
```

### Connection Refused

**Error:** "HTTP request failed: Connection refused"

**Solution:**
1. Check that your embedding service is running:
   ```bash
   curl http://localhost:8090/
   ```
2. Verify the `BYTEBURROW__FACE_EMBED_ENDPOINT` is correct
3. Check the embedding service logs:
   ```bash
   cat /tmp/face-embed-service.log
   ```

### Empty Embeddings

If face embeddings are not being generated:

1. Verify the embedding service is working with a test image
2. Check that the face-detector plugin is working (faces must be detected first)
3. Check ByteBurrow logs for embedding errors:
   ```bash
   grep -i 'embed' logs/byteburrow.log
   ```

### Build Issues

If you encounter build errors:

```bash
# Clean build artifacts
cargo clean

# Rebuild plugins
make build-plugins

# Or build the specific plugin
cargo build --release --package byteburrow-plugin-face-embed
```

## Performance Considerations

### External Service Benefits
- Main server uses less memory and CPU
- Embedding service can be scaled independently
- Can run on GPU-equipped machines
- Better resource isolation

### External Service Trade-offs
- Network latency for each embedding request
- Requires service availability and monitoring
- May need connection pooling for high throughput

### Optimization Tips

1. **Local Network**: Run embedding service on same machine or local network
2. **Connection Pooling**: The plugin reuses HTTP connections automatically
3. **Service Scaling**: Run multiple instances behind a load balancer for high throughput
4. **Monitoring**: Monitor the embedding service health and response times

## Monitoring and Debugging

### Check Plugin Status

```bash
# Verify plugin loaded
ls -la target/plugins/libbyteburrow_plugin_face_embed.so

# Check plugin logs
grep -i 'face.*embed' logs/byteburrow.log
```

### Monitor Embedding Service

```bash
# Check if service is running
ps aux | grep face-embed-service

# Check service logs
tail -f /tmp/face-embed-service.log

# Test service response time
time curl -X POST http://localhost:8090/ --data-binary @test-image.jpg
```

## Architecture

```
┌─────────────────┐
│  ByteBurrow     │
│  Main Server    │
│                 │
│  ┌───────────┐  │
│  │  Face     │  │
│  │  Embedder│───┼─── HTTP POST ─────┐
│  │  Plugin   │  │                   │
│  └───────────┘  │                   ▼
└─────────────────┘           ┌──────────────────┐
                              │  External        │
                              │  Embed Service   │
                              │  /usr/local/bin/ │
                              │  face-embed-     │
                              │  service         │
                              └──────────────────┘
```

## Switching from Local to Remote

If you were previously using local embeddings:

1. The plugin now defaults to remote mode
2. Your existing configuration should work with the external service
3. No database migration needed - the embedding format is identical

## Development

For development purposes:

```bash
# Start embedding service in one terminal
/usr/local/bin/face-embed-service

# In another terminal, run ByteBurrow
make dev

# Or with more debugging
RUST_LOG=byteburrow=debug,byteburrow_plugin_face_embed=debug make dev
```

## Next Steps

1. **Verify Setup**: Upload a photo with faces and check if embeddings are generated
2. **Monitor Performance**: Check response times and resource usage
3. **Configure Scaling**: Set up multiple embedding service instances if needed
4. **Set Up Monitoring**: Configure alerts for embedding service failures

## Support

For issues or questions:
- Check logs in `logs/byteburrow.log` and `/tmp/face-embed-service.log`
- Review the main documentation at `docs/external-embeddings.md`
- Verify the embedding service is compatible with the expected API format