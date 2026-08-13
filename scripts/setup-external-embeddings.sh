#!/bin/bash

# ByteBurrow External Face Embedding Service Setup

# Configuration
EMBED_SERVICE="${EMBED_SERVICE:-/usr/local/bin/face-embed-service}"
EMBED_PORT="${EMBED_PORT:-8090}"
EMBED_ENDPOINT="${EMBED_ENDPOINT:-http://localhost:${EMBED_PORT}/}"

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

echo -e "${GREEN}=== ByteBurrow External Face Embedding Setup ===${NC}"

# Check if the embedding service exists
if [ ! -f "$EMBED_SERVICE" ]; then
    echo -e "${RED}Error: Embedding service not found at $EMBED_SERVICE${NC}"
    echo "Please set EMBED_SERVICE environment variable to the correct path"
    exit 1
fi

echo -e "${GREEN}✓ Embedding service found at $EMBED_SERVICE${NC}"

# Check if the service is already running
if curl -s "$EMBED_ENDPOINT" > /dev/null 2>&1; then
    echo -e "${YELLOW}⚠ Embedding service already running at $EMBED_ENDPOINT${NC}"
else
    echo -e "${GREEN}Starting embedding service...${NC}"
    
    # Start the embedding service in the background
    nohup "$EMBED_SERVICE" > /tmp/face-embed-service.log 2>&1 &
    EMBED_PID=$!
    
    # Wait for the service to start
    echo "Waiting for service to start..."
    for i in {1..30}; do
        if curl -s "$EMBED_ENDPOINT" > /dev/null 2>&1; then
            echo -e "${GREEN}✓ Embedding service started successfully (PID: $EMBED_PID)${NC}"
            echo "Logs: /tmp/face-embed-service.log"
            break
        fi
        if [ $i -eq 30 ]; then
            echo -e "${RED}✗ Failed to start embedding service${NC}"
            echo "Check logs: /tmp/face-embed-service.log"
            exit 1
        fi
        sleep 1
    done
fi

# Configure ByteBurrow environment
echo ""
echo -e "${GREEN}=== ByteBurrow Configuration ===${NC}"
echo "Add these environment variables to your .env file or export them:"
echo ""
echo -e "${YELLOW}export BYTEBURROW__FACE_EMBED_ENDPOINT=$EMBED_ENDPOINT${NC}"
echo ""

# Create .env file if it doesn't exist
if [ ! -f .env ]; then
    echo "Creating .env file..."
    cat > .env << EOF
# ByteBurrow Configuration
BYTEBURROW__FACE_EMBED_ENDPOINT=$EMBED_ENDPOINT
EOF
    echo -e "${GREEN}✓ .env file created with external embedding configuration${NC}"
else
    echo -e "${YELLOW}⚠ .env file already exists${NC}"
    echo "Please add this line to your .env file:"
    echo -e "${YELLOW}BYTEBURROW__FACE_EMBED_ENDPOINT=$EMBED_ENDPOINT${NC}"
fi

echo ""
echo -e "${GREEN}=== Build and Run Instructions ===${NC}"
echo ""
echo "Build plugins with external support:"
echo -e "${YELLOW}make build-plugins${NC}"
echo ""
echo "Run ByteBurrow:"
echo -e "${YELLOW}make run${NC}"
echo ""
echo "Or for development:"
echo -e "${YELLOW}make dev${NC}"
echo ""
echo -e "${GREEN}=== Testing the Setup ===${NC}"
echo ""
echo "Test the embedding service directly:"
echo -e "${YELLOW}curl -X POST $EMBED_ENDPOINT --data-binary @test-image.jpg${NC}"
echo ""
echo "Check ByteBurrow logs to confirm it's using the external service:"
echo -e "${YELLOW}grep -i 'embed' logs/byteburrow.log${NC}"
echo ""
echo -e "${GREEN}Setup complete!${NC}"