#!/bin/bash

# Domain Blocker - Comprehensive Build Script
# Builds for all supported platforms

set -e  # Exit on error

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

# Configuration
PROJECT_NAME="domain_blocker"
BUILD_DIR="target"
LOG_FILE="build.log"

# Truncate log at start of each run
> "$LOG_FILE"
log() { echo "$@" >> "$LOG_FILE"; }

echo -e "${CYAN}╔═══════════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}║          Domain Blocker Build Script                 ║${NC}"
echo -e "${CYAN}╚═══════════════════════════════════════════════════════╝${NC}"
echo ""

print_section() {
    echo ""
    echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
}

print_success() { echo -e "${GREEN}✓${NC} $1"; }
print_warning() { echo -e "${YELLOW}⚠${NC}  $1"; }
print_error()   { echo -e "${RED}✗${NC} $1"; }
print_info()    { echo -e "${CYAN}ℹ${NC}  $1"; }

# ── Parse arguments ────────────────────────────────────────────────────────────
BUILD_TARGET=""
RELEASE_MODE=false
RUN_TESTS=false
VERBOSE=false

print_help() {
    echo "Usage: ./build.sh [OPTIONS]"
    echo ""
    echo "Options:"
    echo "  -t, --target TARGET    Build for specific target (linux, android, ios, macos, windows, all)"
    echo "  -r, --release         Build in release mode (default: debug)"
    echo "      --test            Run tests after building"
    echo "  -v, --verbose         Verbose output"
    echo "  -h, --help            Show this help message"
    echo ""
    echo "Examples:"
    echo "  ./build.sh                    # Build for host platform (debug)"
    echo "  ./build.sh -r                 # Build for host platform (release)"
    echo "  ./build.sh -t android -r      # Build for Android (release)"
    echo "  ./build.sh -t all -r          # Build for all platforms (release)"
}

while [[ $# -gt 0 ]]; do
    case $1 in
        -t|--target)  BUILD_TARGET="$2"; shift 2 ;;
        -r|--release) RELEASE_MODE=true; shift ;;
        --test)       RUN_TESTS=true; shift ;;
        -v|--verbose) VERBOSE=true; shift ;;
        -h|--help)    print_help; exit 0 ;;
        *) print_error "Unknown option: $1"; print_help; exit 1 ;;
    esac
done

if [ "$RELEASE_MODE" = true ]; then
    BUILD_MODE="release"
    BUILD_FLAGS="--release"
else
    BUILD_MODE="debug"
    BUILD_FLAGS=""
fi

print_info "Build mode: $BUILD_MODE"
[ -n "$BUILD_TARGET" ] && print_info "Target: $BUILD_TARGET" || print_info "Target: host platform"
echo ""

# ── NDK detection & env-var export (portable, no hardcoding) ──────────────────
#
# We need to export CC_* and AR_* so that cc-rs (used by ring and other C deps)
# can find the right compiler on any machine.  The linker= entries in
# ~/.cargo/config.toml only fix the Rust linker step; cc-rs is completely
# separate and looks for these env vars.
#
# Search order for the NDK bin dir:
#   1. ANDROID_NDK_HOME (user-set)
#   2. ANDROID_NDK_ROOT (user-set)
#   3. $ANDROID_HOME/ndk/<newest version>
#   4. ~/Android/Sdk/ndk/<newest version>   (Linux default)
#   5. ~/Library/Android/sdk/ndk/<newest>   (macOS default)
#
# We pick the newest NDK found (sort -V, take last).
#
find_ndk_bin() {
    local ndk_root=""

    # 1. Explicit env vars
    if [ -n "$ANDROID_NDK_HOME" ] && [ -d "$ANDROID_NDK_HOME" ]; then
        ndk_root="$ANDROID_NDK_HOME"
    elif [ -n "$ANDROID_NDK_ROOT" ] && [ -d "$ANDROID_NDK_ROOT" ]; then
        ndk_root="$ANDROID_NDK_ROOT"
    else
        # 2. Walk common SDK locations
        local sdk_dirs=()
        [ -n "$ANDROID_HOME" ]             && sdk_dirs+=("$ANDROID_HOME")
        [ -n "$ANDROID_SDK_ROOT" ]         && sdk_dirs+=("$ANDROID_SDK_ROOT")
        sdk_dirs+=("$HOME/Android/Sdk")           # Linux / Android Studio default
        sdk_dirs+=("$HOME/Library/Android/sdk")   # macOS / Android Studio default

        for sdk in "${sdk_dirs[@]}"; do
            if [ -d "$sdk/ndk" ]; then
                ndk_root=$(find "$sdk/ndk" -maxdepth 1 -mindepth 1 -type d 2>/dev/null \
                           | sort -V | tail -1)
                [ -n "$ndk_root" ] && break
            fi
        done
    fi

    if [ -z "$ndk_root" ]; then
        return 1
    fi

    local bin_dir="$ndk_root/toolchains/llvm/prebuilt"

    # Detect host tag (linux-x86_64, darwin-x86_64, darwin-arm64, windows-x86_64)
    local host_os host_arch host_tag
    host_os=$(uname -s | tr '[:upper:]' '[:lower:]')
    host_arch=$(uname -m)
    case "$host_os" in
        linux)  host_tag="linux-${host_arch}" ;;
        darwin) host_tag="darwin-${host_arch}" ;;
        *)      host_tag="linux-x86_64" ;;  # fallback
    esac

    bin_dir="$bin_dir/$host_tag/bin"

    if [ ! -d "$bin_dir" ]; then
        # Try without arch suffix (some NDK layouts differ)
        bin_dir="$ndk_root/toolchains/llvm/prebuilt/$(ls "$ndk_root/toolchains/llvm/prebuilt/" 2>/dev/null | head -1)/bin"
    fi

    [ -d "$bin_dir" ] && echo "$bin_dir" || return 1
}

# Detect the API-level suffix from whatever clang wrappers actually exist.
# e.g. "aarch64-linux-android21-clang" → suffix is "21"
find_api_level() {
    local bin_dir="$1"
    # Look for any aarch64 clang wrapper and extract the numeric API level
    local wrapper
    wrapper=$(ls "$bin_dir"/aarch64-linux-android*-clang 2>/dev/null | sort -V | tail -1)
    if [ -n "$wrapper" ]; then
        basename "$wrapper" | grep -oP '(?<=android)\d+'
    else
        echo "21"  # safe default
    fi
}

export_ndk_env() {
    local ndk_bin
    ndk_bin=$(find_ndk_bin) || {
        print_error "Android NDK not found."
        echo "  Set ANDROID_NDK_HOME or install via Android Studio → SDK Manager → NDK"
        return 1
    }

    local api
    api=$(find_api_level "$ndk_bin")

    print_info "NDK bin: $ndk_bin  (API $api)"

    # Linker wrappers
    local cc_arm64="$ndk_bin/aarch64-linux-android${api}-clang"
    local cc_arm32="$ndk_bin/armv7a-linux-androideabi${api}-clang"
    local cc_x86="$ndk_bin/i686-linux-android${api}-clang"
    local cc_x86_64="$ndk_bin/x86_64-linux-android${api}-clang"
    local ar="$ndk_bin/llvm-ar"

    # Validate at least one compiler exists
    if [ ! -f "$cc_arm64" ]; then
        print_error "Expected compiler not found: $cc_arm64"
        return 1
    fi

    # Export CC_* so cc-rs finds the right C compiler for each target.
    # Variable names use underscores (cc-rs converts hyphens to underscores).
    export CC_aarch64_linux_android="$cc_arm64"
    export CC_armv7_linux_androideabi="$cc_arm32"
    export CC_i686_linux_android="$cc_x86"
    export CC_x86_64_linux_android="$cc_x86_64"

    # Export AR_* so cc-rs finds the right archiver.
    export AR_aarch64_linux_android="$ar"
    export AR_armv7_linux_androideabi="$ar"
    export AR_i686_linux_android="$ar"
    export AR_x86_64_linux_android="$ar"

    # Also write ~/.cargo/config.toml linker entries if missing,
    # so the Rust linker step is covered too.
    ensure_cargo_linker_config "$ndk_bin" "$api"

    print_success "NDK env exported (CC_*, AR_* set for all Android targets)"
}

ensure_cargo_linker_config() {
    local ndk_bin="$1"
    local api="$2"

    if grep -q "aarch64-linux-android" ~/.cargo/config.toml 2>/dev/null; then
        return 0  # already configured
    fi

    print_info "Writing linker config to ~/.cargo/config.toml"
    mkdir -p ~/.cargo
    cat >> ~/.cargo/config.toml << CARGO_EOF

[target.aarch64-linux-android]
linker = "${ndk_bin}/aarch64-linux-android${api}-clang"

[target.armv7-linux-androideabi]
linker = "${ndk_bin}/armv7a-linux-androideabi${api}-clang"

[target.i686-linux-android]
linker = "${ndk_bin}/i686-linux-android${api}-clang"

[target.x86_64-linux-android]
linker = "${ndk_bin}/x86_64-linux-android${api}-clang"
CARGO_EOF
    print_success "Linker config written"
}

# ── Prerequisite checks ───────────────────────────────────────────────────────
#
# iOS  → requires Xcode (macOS only). xcrun must be on PATH.
# macOS → cross-compile from Linux requires osxcross. We check for
#          o64-clang or aarch64-apple-darwin21-clang on PATH.
# Windows → requires MinGW: x86_64-w64-mingw32-gcc on PATH.
#           Install on Ubuntu: sudo apt install gcc-mingw-w64-x86-64
#
HOST_OS=$(uname -s)

check_ios_prereqs() {
    if [ "$HOST_OS" != "Darwin" ]; then
        print_error "iOS builds require macOS with Xcode installed."
        echo "  xcrun (from Xcode) is not available on Linux."
        echo "  Run this script on a Mac, or use a CI service like GitHub Actions (macos runner)."
        return 1
    fi
    if ! command -v xcrun &>/dev/null; then
        print_error "xcrun not found. Install Xcode from the App Store, then run:"
        echo "  xcode-select --install"
        return 1
    fi
    return 0
}

check_macos_prereqs() {
    if [ "$HOST_OS" = "Darwin" ]; then
        # Native build — just need clang
        if ! command -v clang &>/dev/null; then
            print_error "clang not found. Install Xcode command line tools: xcode-select --install"
            return 1
        fi
        return 0
    fi

    # Cross-compiling from Linux → need osxcross
    # osxcross provides tools like aarch64-apple-darwin*-clang or o64-clang
    local osxcross_cc=""
    for candidate in \
        aarch64-apple-darwin23-clang \
        aarch64-apple-darwin22-clang \
        aarch64-apple-darwin21-clang \
        o64-clang; do
        if command -v "$candidate" &>/dev/null; then
            osxcross_cc="$candidate"
            break
        fi
    done

    if [ -z "$osxcross_cc" ]; then
        print_error "macOS cross-compilation from Linux requires osxcross."
        echo ""
        echo "  osxcross setup (one-time):"
        echo "    git clone https://github.com/tpoechtrager/osxcross"
        echo "    # Place MacOSX*.sdk.tar.xz in osxcross/tarballs/"
        echo "    # (obtain the SDK from Xcode on a Mac)"
        echo "    cd osxcross && ./build.sh"
        echo "    export PATH=\"\$PWD/target/bin:\$PATH\""
        echo "    export CC_aarch64_apple_darwin=aarch64-apple-darwin21-clang"
        echo "    export AR_aarch64_apple_darwin=aarch64-apple-darwin21-ar"
        echo ""
        echo "  Alternatively, build macOS targets natively on a Mac."
        return 1
    fi

    # osxcross found — export CC/AR for the macOS target
    local osxcross_ar="${osxcross_cc/clang/ar}"
    export CC_aarch64_apple_darwin="$osxcross_cc"
    export AR_aarch64_apple_darwin="$osxcross_ar"
    print_info "osxcross found: $osxcross_cc"
    return 0
}

check_windows_prereqs() {
    if ! command -v x86_64-w64-mingw32-gcc &>/dev/null; then
        print_error "MinGW cross-compiler not found."
        echo ""
        echo "  Install on Ubuntu/Debian:"
        echo "    sudo apt install gcc-mingw-w64-x86-64"
        echo ""
        echo "  Then export the CC variable:"
        echo "    export CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc"
        echo "    export AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar"
        echo ""
        echo "  And add to ~/.cargo/config.toml:"
        echo "    [target.x86_64-pc-windows-gnu]"
        echo "    linker = \"x86_64-w64-mingw32-gcc\""
        return 1
    fi

    export CC_x86_64_pc_windows_gnu="x86_64-w64-mingw32-gcc"
    export AR_x86_64_pc_windows_gnu="x86_64-w64-mingw32-ar"

    # Ensure linker is in cargo config
    if ! grep -q "x86_64-pc-windows-gnu" ~/.cargo/config.toml 2>/dev/null; then
        mkdir -p ~/.cargo
        cat >> ~/.cargo/config.toml << CARGO_EOF

[target.x86_64-pc-windows-gnu]
linker = "x86_64-w64-mingw32-gcc"
CARGO_EOF
        print_info "Windows linker config written to ~/.cargo/config.toml"
    fi

    print_info "MinGW found: $(command -v x86_64-w64-mingw32-gcc)"
    return 0
}

# ── Test runner ────────────────────────────────────────────────────────────────
if [ "$RUN_TESTS" = true ]; then
    print_section "Running Tests"
    cargo test $BUILD_FLAGS
    print_success "All tests passed"
fi

# ── Target builder ─────────────────────────────────────────────────────────────
build_target() {
    local target=$1
    local target_triple=$2

    echo -e "\n${CYAN}═══ Building $target ($target_triple) ═══${NC}"

    if ! rustup target list | grep -q "$target_triple (installed)"; then
        print_warning "Target $target_triple not installed. Installing..."
        rustup target add "$target_triple"
    fi

    log ""
    log "═══ Building $target ($target_triple) ═══"

    if [ "$VERBOSE" = true ]; then
        cargo build $BUILD_FLAGS --target "$target_triple" 2>&1 | tee -a "$LOG_FILE"
    else
        if ! cargo build $BUILD_FLAGS --target "$target_triple" >> "$LOG_FILE" 2>&1; then
            local key_errors
            key_errors=$(grep -E "(^error[^s]|cannot find|No such file)" "$LOG_FILE" | tail -5)
            echo ""
            print_error "Build failed for $target_triple"
            echo "$key_errors" | sed 's/^/    /'
            echo "  └─ Full output: $LOG_FILE"
            echo ""
            return 1
        fi
    fi

    local lib_name lib_path
    case $target in
        linux)         lib_name="lib${PROJECT_NAME}.so" ;;
        android-*)     lib_name="lib${PROJECT_NAME}.so" ;;
        ios*)          lib_name="lib${PROJECT_NAME}.a" ;;
        macos)         lib_name="lib${PROJECT_NAME}.dylib" ;;
        windows)       lib_name="${PROJECT_NAME}.dll" ;;
        *)             lib_name="lib${PROJECT_NAME}.so" ;;
    esac
    lib_path="$BUILD_DIR/$target_triple/$BUILD_MODE/$lib_name"

    if [ -f "$lib_path" ]; then
        local size
        size=$(du -h "$lib_path" | cut -f1)
        print_success "$target built successfully ($size)"
    else
        print_error "$target build produced no output (expected $lib_path)"
        return 1
    fi
}

# ── Main dispatch ──────────────────────────────────────────────────────────────
case $BUILD_TARGET in
    "")
        print_section "Building for Host Platform"
        if [ "$VERBOSE" = true ]; then
            cargo build $BUILD_FLAGS 2>&1 | tee -a "$LOG_FILE"
        else
            cargo build $BUILD_FLAGS >> "$LOG_FILE" 2>&1
        fi
        print_success "Host build complete"
        ;;

    linux)
        build_target "linux" "x86_64-unknown-linux-gnu"
        ;;

    android)
        print_section "Building for Android (All Architectures)"
        export_ndk_env || exit 1
        android_ok=0
        build_target "android-arm64"  "aarch64-linux-android"   && android_ok=$((android_ok+1)) || true
        build_target "android-armv7"  "armv7-linux-androideabi"  && android_ok=$((android_ok+1)) || true
        build_target "android-x86"    "i686-linux-android"       && android_ok=$((android_ok+1)) || true
        build_target "android-x86_64" "x86_64-linux-android"     && android_ok=$((android_ok+1)) || true
        if [ $android_ok -eq 0 ]; then
            print_error "All Android targets failed. See $LOG_FILE for details."
            exit 1
        fi
        ;;

    ios)
        print_section "Building for iOS (All Architectures)"
        check_ios_prereqs || exit 1
        build_target "ios"            "aarch64-apple-ios"
        build_target "ios-sim-arm64"  "aarch64-apple-ios-sim"
        build_target "ios-sim-x86_64" "x86_64-apple-ios"
        ;;

    macos)
        print_section "Building for macOS"
        check_macos_prereqs || exit 1
        build_target "macos" "aarch64-apple-darwin"
        ;;

    windows)
        print_section "Building for Windows"
        check_windows_prereqs || exit 1
        build_target "windows" "x86_64-pc-windows-gnu"
        ;;

    all)
        print_section "Building for All Platforms"

        print_info "Building Linux..."
        build_target "linux" "x86_64-unknown-linux-gnu" || true

        print_info "Building Android..."
        export_ndk_env || print_warning "NDK setup failed — Android targets may fail"
        build_target "android-arm64"  "aarch64-linux-android"   || print_warning "arm64 skipped"
        build_target "android-armv7"  "armv7-linux-androideabi"  || print_warning "armv7 skipped"
        build_target "android-x86"    "i686-linux-android"       || print_warning "x86 skipped"
        build_target "android-x86_64" "x86_64-linux-android"     || print_warning "x86_64 skipped"

        print_info "Building iOS..."
        if check_ios_prereqs 2>/dev/null; then
            build_target "ios"            "aarch64-apple-ios"        || print_warning "ios skipped"
            build_target "ios-sim-arm64"  "aarch64-apple-ios-sim"    || print_warning "ios-sim-arm64 skipped"
            build_target "ios-sim-x86_64" "x86_64-apple-ios"         || print_warning "ios-sim-x86_64 skipped"
        else
            print_warning "iOS skipped (requires macOS + Xcode)"
        fi

        print_info "Building macOS..."
        if check_macos_prereqs 2>/dev/null; then
            build_target "macos" "aarch64-apple-darwin" || print_warning "macos skipped"
        else
            print_warning "macOS skipped (requires Xcode on macOS, or osxcross on Linux)"
        fi

        print_info "Building Windows..."
        if check_windows_prereqs 2>/dev/null; then
            build_target "windows" "x86_64-pc-windows-gnu" || print_warning "windows skipped"
        else
            print_warning "Windows skipped (install: sudo apt install gcc-mingw-w64-x86-64)"
        fi
        ;;

    *)
        print_error "Unknown target: $BUILD_TARGET"
        print_help
        exit 1
        ;;
esac

# ── Summary ────────────────────────────────────────────────────────────────────
echo ""
print_section "Build Summary"

BUILT_COUNT=0
TOTAL_SIZE=0

if [ -d "$BUILD_DIR" ]; then
    while IFS= read -r -d '' file; do
        BUILT_COUNT=$((BUILT_COUNT + 1))
        size=$(stat -f%z "$file" 2>/dev/null || stat -c%s "$file" 2>/dev/null || echo 0)
        TOTAL_SIZE=$((TOTAL_SIZE + size))
    done < <(find "$BUILD_DIR" \( -name "lib${PROJECT_NAME}.*" -o -name "${PROJECT_NAME}.dll" \) -print0 2>/dev/null)
fi

if [ $BUILT_COUNT -gt 0 ]; then
    TOTAL_SIZE_MB=$(awk "BEGIN {printf \"%.2f\", $TOTAL_SIZE/1024/1024}")
    print_success "Built $BUILT_COUNT libraries (Total: ${TOTAL_SIZE_MB}MB)"
else
    print_warning "No libraries found"
fi

echo ""
print_info "Build directory: $BUILD_DIR"
print_info "Build mode: $BUILD_MODE"
echo ""

print_section "Next Steps"
echo ""
echo "To migrate to Flutter project:"
echo "  ${CYAN}./migrate.sh /path/to/flutter/project${NC}"
echo ""
echo "To run performance tests:"
echo "  ${CYAN}cargo run --example performance_test --release${NC}"
echo ""
echo "To run unit tests:"
echo "  ${CYAN}cargo test${NC}"
echo ""

print_success "Build complete!"
echo ""
print_info "Full build log: $LOG_FILE"