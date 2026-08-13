# Keyword Extraction Model Selection

ByteBurrow uses Ollama vision models for automatic image keyword extraction. By default, it uses `qwen3.5:9b`, but you can configure it to use any Ollama vision model.

## Quick Configuration

Add these environment variables to your `.env` file:

```bash
# Use a different model
BYTEBURROW__OLLAMA_MODEL=llava:13b

# Change Ollama server URL if needed
BYTEBURROW__OLLAMA_URL=http://127.0.0.1:11434

# Adjust timeout for slower models
BYTEBURROW__OLLAMA_TIMEOUT=300
```

## Available Vision Models

Here are some popular Ollama vision models that work well with ByteBurrow:

### Recommended Models

| Model | Size | Speed | Quality | Best For |
|-------|------|-------|---------|----------|
| `qwen3.5:9b` | 9B | Fast | Good | General purpose (default) |
| `llava:7b` | 7B | Very Fast | Decent | Quick processing |
| `llava:13b` | 13B | Medium | Good | Better accuracy |
| `llava:34b` | 34B | Slow | Excellent | Highest quality |

### Alternative Models

| Model | Size | Notes |
|-------|------|-------|
| `bakllava:latest` | 7.9B | Good balance of speed/quality |
| `minicpm-v:latest` | 8B | Compact and efficient |
| `moondream:latest` | 1.8B | Very fast, basic accuracy |
| `lava:latest` | 7B | Lightweight alternative |

## Model Selection Guide

### For Speed (Batch Processing)
```bash
BYTEBURROW__OLLAMA_MODEL=llava:7b
BYTEBURROW__OLLAMA_TIMEOUT=60
```

### For Quality (Single Image Analysis)
```bash
BYTEBURROW__OLLAMA_MODEL=llava:34b
BYTEBURROW__OLLAMA_TIMEOUT=300
```

### For Balance (Recommended)
```bash
BYTEBURROW__OLLAMA_MODEL=llava:13b
BYTEBURROW__OLLAMA_TIMEOUT=180
```

### For Low-Resource Systems
```bash
BYTEBURROW__OLLAMA_MODEL=moondream:latest
BYTEBURROW__OLLAMA_TIMEOUT=60
```

## Setting Up Ollama

### Install Ollama

```bash
# macOS
brew install ollama

# Linux
curl -fsSL https://ollama.com/install.sh | sh

# Start Ollama service
ollama serve
```

### Pull a Model

```bash
# Pull the default model
ollama pull qwen3.5:9b

# Pull alternative models
ollama pull llava:13b
ollama pull bakllava:latest
```

### Test Vision Models

```bash
# Test if a model supports vision
ollama run llava:13b "Describe this image" @path/to/image.jpg
```

## Configuration Options

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `BYTEBURROW__OLLAMA_URL` | Ollama server URL | `http://127.0.0.1:11434` |
| `BYTEBURROW__OLLAMA_MODEL` | Vision model name | `qwen3.5:9b` |
| `BYTEBURROW__OLLAMA_TIMEOUT` | Request timeout (seconds) | `120` |

### Advanced Configuration

You can also configure via plugin config in your main config:

```toml
[plugins.keyword_extractor]
ollama_url = "http://127.0.0.1:11434"
ollama_model = "llava:13b"
ollama_timeout = "180"
```

## Testing Model Performance

### Test Different Models

```bash
# Set different models in your .env
BYTEBURROW__OLLAMA_MODEL=llava:7b
# Restart ByteBurrow and upload an image

BYTEBURROW__OLLAMA_MODEL=llava:13b
# Restart and compare results
```

### Monitor Performance

```bash
# Check processing time in logs
grep -i "keyword" logs/byteburrow.log | grep -i "time"

# Monitor Ollama requests
curl http://127.0.0.1:11434/api/tags
```

## Troubleshooting

### Model Not Found

**Error:** "Ollama request failed: model not found"

**Solution:** Pull the model:
```bash
ollama pull your-model-name
```

### Timeout Errors

**Error:** "Ollama request failed: timeout"

**Solution:** Increase timeout:
```bash
BYTEBURROW__OLLAMA_TIMEOUT=300
```

### Poor Quality Keywords

**Solution:** Try a larger model:
```bash
# For better quality
BYTEBURROW__OLLAMA_MODEL=llava:34b

# Or adjust the model settings via Ollama
ollama pull llava:34b
```

### Connection Refused

**Error:** "Ollama request failed: connection refused"

**Solution:** Check if Ollama is running:
```bash
# Check if Ollama service is running
curl http://127.0.0.1:11434/api/tags

# Start Ollama if needed
ollama serve
```

## Performance Tips

1. **Model Selection**: Larger models produce better keywords but are slower
2. **Hardware**: Use GPU for faster inference with larger models
3. **Timeout**: Adjust based on your model and hardware
4. **Batch Processing**: Smaller models work better for large batches
5. **Caching**: Keywords are cached in the database, avoiding reprocessing

## Disabling Keyword Extraction

If you don't want to use keyword extraction:

```bash
# Remove the keyword-extractor plugin from the plugins directory
rm target/plugins/libbyteburrow_plugin_keyword_extractor.so

# Or set an invalid model to disable it
BYTEBURROW__OLLAMA_MODEL=invalid-model-name
```

## Remote Ollama Server

You can run Ollama on a different machine:

```bash
# On the remote server
ollama serve --host 0.0.0.0 --port 11434

# On ByteBurrow machine
BYTEBURROW__OLLAMA_URL=http://remote-server:11434
BYTEBURROW__OLLAMA_MODEL=llava:13b
```

## Example Configurations

### Development Setup
```bash
BYTEBURROW__OLLAMA_URL=http://127.0.0.1:11434
BYTEBURROW__OLLAMA_MODEL=llava:7b
BYTEBURROW__OLLAMA_TIMEOUT=60
```

### Production Setup
```bash
BYTEBURROW__OLLAMA_URL=http://ollama-server.internal:11434
BYTEBURROW__OLLAMA_MODEL=llava:34b
BYTEBURROW__OLLAMA_TIMEOUT=300
```

### Low-Resource Setup
```bash
BYTEBURROW__OLLAMA_URL=http://127.0.0.1:11434
BYTEBURROW__OLLAMA_MODEL=moondream:latest
BYTEBURROW__OLLAMA_TIMEOUT=120
```

## Comparing Models

Test different models with the same image to find what works best for your use case:

1. Set different models in your `.env` file
2. Restart ByteBurrow
3. Upload the same test image
4. Compare the generated keywords in the database or UI
5. Choose the model that gives the best results for your needs

## Next Steps

1. **Test Current Setup**: Upload an image and check keyword quality
2. **Try Different Models**: Experiment with the models listed above
3. **Monitor Performance**: Check processing times and resource usage
4. **Optimize**: Choose the best model for your hardware and use case