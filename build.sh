#!/bin/bash
set -e

echo "Building frontend..."
cd frontend
npm install
npm run build
cd ..

echo "Building backend..."
source "$HOME/.cargo/env"
cargo build --release

# cargo re-signs the binary ad-hoc on every build, which invalidates its Full
# Disk Access grant — and a folder the scanner may not read does not fail, it
# blocks. Restore the stable signature before anyone runs this.
./scripts/sign.sh

echo ""
echo "Build complete: ./target/release/webtop"
echo "Run: ./target/release/webtop --port 7890"
