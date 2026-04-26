#!/usr/bin/env sh
# Install the latest bv release binary.
set -e

REPO="mlberkeley/bv"
BIN_DIR="${BV_BIN_DIR:-$HOME/.local/bin}"
BIN_NAME="bv"

# Detect OS and architecture
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
    Linux)
        case "$ARCH" in
            x86_64)  TARGET="x86_64-unknown-linux-gnu" ;;
            aarch64) TARGET="aarch64-unknown-linux-gnu" ;;
            *)
                echo "Unsupported architecture: $ARCH" >&2
                exit 1
                ;;
        esac
        ;;
    Darwin)
        case "$ARCH" in
            x86_64)  TARGET="x86_64-apple-darwin" ;;
            arm64)   TARGET="aarch64-apple-darwin" ;;
            *)
                echo "Unsupported architecture: $ARCH" >&2
                exit 1
                ;;
        esac
        ;;
    *)
        echo "Unsupported OS: $OS. Install from source: cargo install bio-bv" >&2
        exit 1
        ;;
esac

# Find latest release
LATEST=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | grep '"tag_name"' | sed 's/.*"tag_name": *"\(.*\)".*/\1/')

if [ -z "$LATEST" ]; then
    echo "Could not determine latest release. Install with: cargo install bio-bv" >&2
    exit 1
fi

URL="https://github.com/$REPO/releases/download/$LATEST/bv-$TARGET"

echo "Installing bv $LATEST ($TARGET) to $BIN_DIR/$BIN_NAME"

mkdir -p "$BIN_DIR"
curl -fsSL "$URL" -o "$BIN_DIR/$BIN_NAME"
chmod +x "$BIN_DIR/$BIN_NAME"

echo "Done. Make sure $BIN_DIR is in your PATH."
echo "  Run: bv --version"
