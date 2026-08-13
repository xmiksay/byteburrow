#!/bin/bash

# Test script for keyword extraction with different models

OLLAMA_URL="${OLLAMA_URL:-http://127.0.0.1:11434}"
TEST_IMAGE="${1:-}"

echo "=== Keyword Extraction Model Test ==="
echo ""

# Check if Ollama is running
echo "1. Checking Ollama service..."
if curl -s "$OLLAMA_URL/api/tags" > /dev/null 2>&1; then
    echo "✓ Ollama service is running at $OLLAMA_URL"
else
    echo "✗ Ollama service is not responding"
    echo "  Start it with: ollama serve"
    exit 1
fi

echo ""

# List available models
echo "2. Checking available vision models..."
echo ""
curl -s "$OLLAMA_URL/api/tags" | jq -r '.models[] | select(.details.pipeline == "llava" or .name | contains("llava") or .name | contains("qwen") or .name | contains("bakllava") or .name | contains("minicpm") or .name | contains("moondream")) | "- \(.name) (\(.size | tonumber / 1024 / 1024 | floor)MB)"' 2>/dev/null || echo "  Could not fetch model list"

echo ""
echo "3. Creating test prompt for keyword extraction..."
echo ""

PROMPT='Analyze this image and extract descriptive keywords. Return ONLY a JSON array of lowercase English keyword strings, nothing else. Include keywords for: objects, scene type, colors, mood, activities, setting (indoor/outdoor), weather/lighting if visible, and any notable details. Example: ["sunset","beach","ocean","waves","orange sky","silhouette","person","outdoor"] Keep keywords concise (1-3 words each). Return 5-20 keywords.'

if [ -z "$TEST_IMAGE" ] || [ ! -f "$TEST_IMAGE" ]; then
    echo "⚠ No test image provided or image not found"
    echo "  Usage: $0 <path-to-image.jpg>"
    echo ""
    echo "  Testing with placeholder (this will fail without a real image)..."
    TEST_IMAGE=""
fi

echo ""
echo "4. Available models for testing:"
echo ""
cat << 'EOF'
- qwen3.5:9b       (9B, 4.7GB)  - Default model, good balance
- llava:7b         (7B, 3.5GB)  - Fast, decent quality
- llava:13b        (13B, 7.3GB) - Better quality, medium speed
- llava:34b        (34B, 19GB)  - Best quality, slowest
- bakllava:latest  (7.9B, 4.2GB) - Good balance
- minicpm-v:latest (8B, 4.8GB)  - Compact and efficient
- moondream:latest (1.8B, 1.7GB) - Very fast, basic accuracy
EOF

echo ""
echo "5. Configuration for ByteBurrow:"
echo ""
cat << 'EOF'
Add these to your .env file:

# Use a specific model
BYTEBURROW__OLLAMA_URL=http://127.0.0.1:11434
BYTEBURROW__OLLAMA_MODEL=llava:13b
BYTEBURROW__OLLAMA_TIMEOUT=120

# Pull a model first
ollama pull llava:13b
EOF

if [ -n "$TEST_IMAGE" ] && [ -f "$TEST_IMAGE" ]; then
    echo ""
    echo "6. Quick test with default model..."
    echo ""
    
    # Create a simple test request
    TEMP_RESPONSE=$(mktemp)
    
    if curl -s "$OLLAMA_URL/api/generate" \
        -H "Content-Type: application/json" \
        -d "{
            \"model\": \"qwen3.5:9b\",
            \"prompt\": \"$PROMPT\",
            \"images\": [\"$(base64 -w 0 "$TEST_IMAGE")\"],
            \"stream\": false,
            \"options\": {\"temperature\": 0.3}
        }" -o "$TEMP_RESPONSE" 2>&1; then
        
        RESPONSE=$(cat "$TEMP_RESPONSE")
        if echo "$RESPONSE" | jq -e '.response' > /dev/null 2>&1; then
            KEYWORDS=$(echo "$RESPONSE" | jq -r '.response')
            echo "✓ Test successful! Generated keywords:"
            echo "$KEYWORDS" | head -3
        else
            echo "⚠ Test request sent but response format unexpected"
            echo "  Response: $RESPONSE" | head -5
        fi
    else
        echo "⚠ Test request failed (model might not be available)"
    fi
    
    rm -f "$TEMP_RESPONSE"
fi

echo ""
echo "=== Test Summary ==="
echo "✓ Your Ollama service is configured and accessible"
echo ""
echo "Next steps:"
echo "  1. Pull a model: ollama pull llava:13b"
echo "  2. Configure ByteBurrow: Add BYTEBURROW__OLLAMA_MODEL to .env"
echo "  3. Restart ByteBurrow: make run"
echo ""
echo "For more model options, see: docs/keyword-model-selection.md"