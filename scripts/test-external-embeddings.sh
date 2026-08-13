#!/bin/bash

# Test script for external face embedding service

EMBED_ENDPOINT="${EMBED_ENDPOINT:-http://localhost:8090/}"

echo "Testing external face embedding service..."
echo "Endpoint: $EMBED_ENDPOINT"
echo ""

# Check if service is running
echo "1. Checking if embedding service is running..."
if curl -s -o /dev/null -w "%{http_code}" "$EMBED_ENDPOINT" > /dev/null 2>&1; then
    echo "✓ Embedding service is responding"
else
    echo "✗ Embedding service is not responding"
    echo "  Start it with: /usr/local/bin/face-embed-service"
    exit 1
fi

echo ""

# Create a test image if none exists
TEST_IMAGE="test-face.jpg"
if [ ! -f "$TEST_IMAGE" ]; then
    echo "2. Creating test image..."
    # Create a simple 128x128 RGB image
    convert -size 128x128 xc:blue "$TEST_IMAGE" 2>/dev/null || {
        echo "Creating simple test image with ImageMagick failed, creating basic one..."
        dd if=/dev/urandom of="$TEST_IMAGE" bs=1024 count=1 2>/dev/null
    }
    echo "✓ Test image created: $TEST_IMAGE"
else
    echo "2. Using existing test image: $TEST_IMAGE"
fi

echo ""

# Test embedding generation
echo "3. Testing embedding generation..."
echo "Sending test image to embedding service..."
echo ""

RESPONSE=$(curl -s -X POST "$EMBED_ENDPOINT" --data-binary @"$TEST_IMAGE" -H "Content-Type: application/octet-stream")

if echo "$RESPONSE" | jq -e '.embedding' > /dev/null 2>&1; then
    EMBEDDING_DIM=$(echo "$RESPONSE" | jq -r '.embedding | length')
    echo "✓ Embedding generated successfully!"
    echo "  Dimension: $EMBEDDING_DIM"
    echo "  First 5 values: $(echo "$RESPONSE" | jq -r '.embedding[:5] | join(", ")')"
else
    echo "✗ Failed to generate embedding"
    echo "  Response: $RESPONSE"
    exit 1
fi

echo ""

# Test ByteBurrow plugin
echo "4. Checking ByteBurrow plugin..."
if [ -f "target/plugins/libbyteburrow_plugin_face_embed.so" ]; then
    echo "✓ Face embedder plugin compiled and ready"
else
    echo "⚠ Plugin not found. Build it with: make build-plugins"
fi

echo ""

# Check configuration
echo "5. Checking configuration..."
if [ -f ".env" ] && grep -q "BYTEBURROW__FACE_EMBED_ENDPOINT" .env; then
    CONFIG_ENDPOINT=$(grep "BYTEBURROW__FACE_EMBED_ENDPOINT" .env | cut -d'=' -f2)
    echo "✓ Configuration found in .env"
    echo "  BYTEBURROW__FACE_EMBED_ENDPOINT=$CONFIG_ENDPOINT"
else
    echo "⚠ External embedding endpoint not configured in .env"
    echo "  Add: BYTEBURROW__FACE_EMBED_ENDPOINT=$EMBED_ENDPOINT"
fi

echo ""
echo "=== Test Summary ==="
echo "✓ External embedding service is running and working"
echo "✓ Your setup is ready to use external face embeddings"
echo ""
echo "Next steps:"
echo "  1. Add BYTEBURROW__FACE_EMBED_ENDPOINT to your .env file"
echo "  2. Run: make build-plugins"
echo "  3. Run: make run"
echo ""
echo "Clean up test image: rm $TEST_IMAGE"