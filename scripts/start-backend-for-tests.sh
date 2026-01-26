#!/bin/bash

# Script to start the backend server for integration tests
# Requires 'yarn build' to have been run first

set -e

echo "Checking if build artifacts are present..."

# Check if server lib directory exists
if [ ! -d "src/server/lib" ]; then
    echo "Error: Server build artifacts missing. Please run 'yarn build' first."
    exit 1
fi

echo "Starting backend server..."
cd src/server && yarn start:backend