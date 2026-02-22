#!/bin/bash

# Domain Blocker - Quick Setup Script
# One-command guided setup for new users

set -e

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
BLUE='\033[0;34m'; CYAN='\033[0;36m'; NC='\033[0m'

echo -e "${CYAN}"
cat << "EOF"
╔═══════════════════════════════════════════════════════╗
║                                                       ║
║        Domain Blocker - Quick Setup                   ║
║                                                       ║
║  Context-aware domain blocking for Flutter           ║
║                                                       ║
╚═══════════════════════════════════════════════════════╝
EOF
echo -e "${NC}"

ok()   { echo -e "${GREEN}✓${NC} $1"; }
warn() { echo -e "${YELLOW}⚠${NC}  $1"; }
err()  { echo -e "${RED}✗${NC} $1"; }
step() { echo -e "${BLUE}▶${NC} $1"; }

# ── Prerequisites ─────────────────────────────────────────────────────────────
step "Checking prerequisites..."

if ! command -v cargo &>/dev/null; then
    err "Rust/Cargo not found. Install from: https://rustup.rs/"
    exit 1
fi
ok "Rust: $(cargo --version)"

if ! command -v rustup &>/dev/null; then
    err "rustup not found. Install from: https://rustup.rs/"; exit 1
fi
ok "rustup found"

FLUTTER_AVAILABLE=false
if command -v flutter &>/dev/null; then
    ok "Flutter: $(flutter --version 2>/dev/null | head -n1)"
    FLUTTER_AVAILABLE=true
else
    warn "Flutter not found — Rust-only build still works"
fi

chmod +x build.sh migrate_to_flutter.sh 2>/dev/null || true
echo ""

# ── Menu ──────────────────────────────────────────────────────────────────────
echo -e "${CYAN}What would you like to do?${NC}"
echo ""
echo "  1) Build for host platform only"
echo "  2) Build for Android"
echo "  3) Build for iOS  (macOS only)"
echo "  4) Build for all platforms"
echo "  5) Build + integrate into a Flutter project  ← recommended"
echo "  6) Run tests only"
echo "  7) Exit"
echo ""
read -rp "Enter choice [1-7]: " choice

case $choice in
    1)
        step "Building for host platform..."
        ./build.sh -r
        ;;
    2)
        step "Building for Android..."
        ./build.sh -t android -r
        ;;
    3)
        step "Building for iOS..."
        ./build.sh -t ios -r
        ;;
    4)
        step "Building for all platforms..."
        ./build.sh -t all -r
        ;;
    5)
        echo ""
        if [ "$FLUTTER_AVAILABLE" = false ]; then
            warn "Flutter not detected. The migrate script will copy files but cannot run flutter pub get."
        fi
        read -rp "  Flutter project path: " FLUTTER_PATH
        if [ -z "$FLUTTER_PATH" ]; then
            err "No path provided."; exit 1
        fi

        echo ""
        echo "  Build target (leave blank for host platform only):"
        echo "    android | ios | macos | linux | windows | all"
        echo "    Note: 'all' builds every platform before copying — takes a few minutes"
        read -rp "  Target: " BUILD_TARGET

        echo ""
        step "Launching fully automated integration..."
        echo ""
        MIGRATE_ARGS="$FLUTTER_PATH"
        [ -n "$BUILD_TARGET" ] && MIGRATE_ARGS="$MIGRATE_ARGS --target $BUILD_TARGET"
        ./migrate_to_flutter.sh $MIGRATE_ARGS
        ;;
    6)
        step "Running tests..."
        cargo test
        cargo run --example performance_test --release
        ;;
    7)
        echo "Goodbye!"; exit 0
        ;;
    *)
        err "Invalid choice"; exit 1
        ;;
esac

echo ""
echo -e "${GREEN}╔═══════════════════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║                   All done!                           ║${NC}"
echo -e "${GREEN}╚═══════════════════════════════════════════════════════╝${NC}"
echo ""
echo -e "Useful commands:"
echo -e "  Rebuild:    ${CYAN}./build.sh -r${NC}"
echo -e "  Re-migrate: ${CYAN}./migrate_to_flutter.sh /path/to/flutter/app${NC}"
echo -e "  Help:       ${CYAN}./build.sh --help${NC}"
echo ""