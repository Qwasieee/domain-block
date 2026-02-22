#!/bin/bash

# Domain Blocker - Flutter Migration Script (Fully Automated)
# One-command integration: builds Rust if needed, copies artifacts,
# updates pubspec.yaml, and runs flutter pub get automatically.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUST_PROJECT_DIR="$SCRIPT_DIR"

# ── Colors ────────────────────────────────────────────────────────────────────
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
BLUE='\033[0;34m'; CYAN='\033[0;36m'; MAGENTA='\033[0;35m'; NC='\033[0m'

# ── Helpers ───────────────────────────────────────────────────────────────────
section()  { echo ""; echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"; echo -e "${BLUE}$1${NC}"; echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"; }
ok()       { echo -e "  ${GREEN}✓${NC} $1"; }
warn()     { echo -e "  ${YELLOW}⚠${NC}  $1"; }
err()      { echo -e "  ${RED}✗${NC} $1"; }
info()     { echo -e "  ${CYAN}ℹ${NC}  $1"; }
step()     { echo -e "${MAGENTA}▶${NC} $1"; }

echo -e "${CYAN}╔═══════════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}║   Domain Blocker Flutter Integration (Auto Mode)     ║${NC}"
echo -e "${CYAN}╚═══════════════════════════════════════════════════════╝${NC}"
echo ""

# ── Argument parsing ──────────────────────────────────────────────────────────
if [ -z "$1" ]; then
    err "Flutter project directory not specified"
    echo ""
    echo "Usage: $0 <flutter_project_path> [OPTIONS]"
    echo ""
    echo "Options:"
    echo "  --dry-run        Show what would happen without making changes"
    echo "  --no-build       Skip auto-building Rust (fail if artifacts missing)"
    echo "  --verbose        Show detailed output"
    echo "  --target TARGET  Build target if auto-building (linux|android|ios|macos|windows|all)"
    echo "                   Defaults to host platform"
    echo ""
    echo "Examples:"
    echo "  $0 ~/my_flutter_app"
    echo "  $0 ~/my_flutter_app --target android"
    echo "  $0 ~/my_flutter_app --dry-run"
    exit 1
fi

FLUTTER_PROJECT="$1"; shift
DRY_RUN=false
VERBOSE=false
NO_BUILD=false
BUILD_TARGET=""

while [[ $# -gt 0 ]]; do
    case $1 in
        --dry-run)   DRY_RUN=true;  shift ;;
        --verbose)   VERBOSE=true;  shift ;;
        --no-build)  NO_BUILD=true; shift ;;
        --target)    BUILD_TARGET="$2"; shift 2 ;;
        *) err "Unknown option: $1"; exit 1 ;;
    esac
done

[ "$DRY_RUN" = true ] && warn "DRY RUN MODE — no files will be changed"

FLUTTER_PROJECT="$(cd "$FLUTTER_PROJECT" 2>/dev/null && pwd || echo "$FLUTTER_PROJECT")"

# ── Step 1: Validate environment ──────────────────────────────────────────────
section "1/6  Validating Environment"

[ -d "$FLUTTER_PROJECT" ]            || { err "Directory not found: $FLUTTER_PROJECT"; exit 1; }
ok "Flutter project directory exists"

[ -f "$FLUTTER_PROJECT/pubspec.yaml" ] || { err "Not a Flutter project — pubspec.yaml missing"; exit 1; }
ok "pubspec.yaml found"

[ -d "$FLUTTER_PROJECT/lib" ]        || { err "lib/ directory missing — is this a Flutter project?"; exit 1; }
ok "lib/ directory found"

PROJECT_NAME=$(grep "^name:" "$FLUTTER_PROJECT/pubspec.yaml" | awk '{print $2}')
info "Flutter project: ${PROJECT_NAME}"

if ! command -v flutter &>/dev/null; then
    warn "flutter command not found — dependency installation will be skipped"
    FLUTTER_AVAILABLE=false
else
    FLUTTER_AVAILABLE=true
    ok "flutter found: $(flutter --version 2>/dev/null | head -n1)"
fi

# ── Step 2: Build Rust for the requested target ───────────────────────────────
section "2/6  Rust Library"

RUST_LIB_DIR="$RUST_PROJECT_DIR/target/release"

if [ ! -f "$RUST_PROJECT_DIR/build.sh" ]; then
    err "build.sh not found in $RUST_PROJECT_DIR"
    exit 1
fi

run_build() {
    local args="$1"
    if [ "$DRY_RUN" = true ]; then
        info "[DRY RUN] Would run: ./build.sh $args"
        return
    fi
    step "Running: ./build.sh $args"
    if [ "$VERBOSE" = true ]; then
        (cd "$RUST_PROJECT_DIR" && bash build.sh $args)
    else
        (cd "$RUST_PROJECT_DIR" && bash build.sh $args 2>&1 | \
            grep -E "(✓|✗|⚠|Built|error\[|^error|linker|NDK|toolchain|could not)" || true)
    fi

    # Post-build: enumerate every artifact actually produced so user can see if build silently failed
    echo ""
    info "Artifacts after build:"
    local found=0
    while IFS= read -r f; do
        local size; size=$(du -h "$f" | cut -f1)
        echo "      ${size}  ${f#$RUST_PROJECT_DIR/}"
        found=$((found+1))
    done < <(find "$RUST_PROJECT_DIR/target" \
        \( -name "libdomain_blocker.so" -o \
           -name "libdomain_blocker.a"  -o \
           -name "libdomain_blocker.dylib" -o \
           -name "domain_blocker.dll" \) 2>/dev/null | sort)
    if [ $found -eq 0 ]; then
        warn "No artifacts produced — NDK/toolchain may not be configured"
        info "Run with --verbose to see full build output"
    fi
}

if [ "$NO_BUILD" = true ]; then
    # Just verify something exists
    if ! find "$RUST_PROJECT_DIR/target" \( -name "libdomain_blocker.*" -o -name "domain_blocker.dll" \) 2>/dev/null | grep -q .; then
        err "No Rust artifacts found and --no-build is set."
        info "Run:  ./build.sh -r   (or -t android -r, etc.)"
        exit 1
    fi
    ok "Rust artifacts present (skipping build)"
elif [ -n "$BUILD_TARGET" ]; then
    # A specific target was requested — always build it so artifacts are fresh
    info "Building for target: $BUILD_TARGET"
    run_build "-t $BUILD_TARGET -r"

    # For Android specifically: verify NDK is usable if no artifacts appeared
    if [ "$BUILD_TARGET" = "android" ] || [ "$BUILD_TARGET" = "all" ]; then
        android_built=0
        for triple in aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android; do
            [ -f "$RUST_PROJECT_DIR/target/$triple/release/libdomain_blocker.so" ] && android_built=$((android_built+1))
        done
        if [ $android_built -eq 0 ] && [ "$DRY_RUN" = false ]; then
            echo ""
            err "Android build produced no .so files. Possible causes:"
            echo ""
            echo "  1. Android NDK not installed or not on PATH"
            echo "     Install via Android Studio → SDK Manager → SDK Tools → NDK"
            echo "     Then add to ~/.cargo/config.toml:"
            echo ""
            echo "     [target.aarch64-linux-android]"
            echo "     linker = "\$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android21-clang""
            echo ""
            echo "  2. Rust Android targets not installed"
            echo "     Fix:  rustup target add aarch64-linux-android armv7-linux-androideabi"
            echo "                               i686-linux-android x86_64-linux-android"
            echo ""
            echo "  3. Run with --verbose to see full cargo error output"
            echo ""
        fi
    fi
    ok "Rust build step complete"
else
    # No target specified — build host platform if nothing exists yet
    if find "$RUST_PROJECT_DIR/target" \( -name "libdomain_blocker.*" -o -name "domain_blocker.dll" \) 2>/dev/null | grep -q .; then
        ok "Rust artifacts already present (pass --target to rebuild)"
    else
        warn "No artifacts found — building for host platform..."
        run_build "-r"
        ok "Rust build complete"
    fi
fi

# ── Step 3: Copy native artifacts ─────────────────────────────────────────────
section "3/6  Copying Native Libraries"

COPIED=0; SKIPPED=0

ensure_dir() {
    [ "$DRY_RUN" = true ] && { [ ! -d "$1" ] && info "[DRY RUN] mkdir -p $1"; return; }
    mkdir -p "$1"
}

copy_file() {
    local src="$1" dest="$2" label="$3"
    if [ -f "$src" ]; then
        local size; size=$(du -h "$src" | cut -f1)
        if [ "$DRY_RUN" = true ]; then
            info "[DRY RUN] copy $label ($size)"
        else
            cp "$src" "$dest"
            ok "$label  ($size)"
        fi
        COPIED=$((COPIED + 1)); return 0
    else
        [ "$VERBOSE" = true ] && warn "Not found: $label"
        SKIPPED=$((SKIPPED + 1)); return 1
    fi
}

# Linux
LINUX_LIB="$RUST_LIB_DIR/libdomain_blocker.so"
if [ -f "$LINUX_LIB" ]; then
    ensure_dir "$FLUTTER_PROJECT/linux"
    copy_file "$LINUX_LIB" "$FLUTTER_PROJECT/linux/libdomain_blocker.so" "libdomain_blocker.so → linux/"
    copy_file "$LINUX_LIB" "$FLUTTER_PROJECT/libdomain_blocker.so"       "libdomain_blocker.so → project root"
fi

# Android
declare -A ANDROID_TARGETS=(
    ["aarch64-linux-android"]="arm64-v8a"
    ["armv7-linux-androideabi"]="armeabi-v7a"
    ["i686-linux-android"]="x86"
    ["x86_64-linux-android"]="x86_64"
)
ANDROID_JNI="$FLUTTER_PROJECT/android/app/src/main/jniLibs"
android_count=0

# Always purge stale domain blocker libs before copying fresh ones.
# This prevents a host x86_64 .so from a prior run from lingering in
# arm64-v8a/ and causing an EM_X86_64 vs EM_AARCH64 crash at runtime.
if [ "$DRY_RUN" = false ] && [ -d "$ANDROID_JNI" ]; then
    find "$ANDROID_JNI" -name "libdomain_blocker.so" -delete 2>/dev/null || true
    [ "$VERBOSE" = true ] && info "Cleared stale JNI artifacts"
fi

for triple in "${!ANDROID_TARGETS[@]}"; do
    abi="${ANDROID_TARGETS[$triple]}"
    src="$RUST_PROJECT_DIR/target/$triple/release/libdomain_blocker.so"
    if [ -f "$src" ]; then
        ensure_dir "$ANDROID_JNI/$abi"
        copy_file "$src" "$ANDROID_JNI/$abi/libdomain_blocker.so" "libdomain_blocker.so → android/$abi/" && android_count=$((android_count+1))
    fi
done
if [ $android_count -gt 0 ]; then
    ok "Android: $android_count ABI(s) copied"
else
    warn "No Android libraries found — cleaning any stale JNI artifacts to prevent arch mismatch crashes"
    # Remove any leftover .so from previous runs (e.g. host x86_64 copied into arm64-v8a)
    if [ "$DRY_RUN" = false ] && [ -d "$ANDROID_JNI" ]; then
        find "$ANDROID_JNI" -name "libdomain_blocker.so" -delete 2>/dev/null && \
            info "Removed stale JNI artifacts" || true
    fi
    info "To build for Android and re-run:"
    echo "       cd $RUST_PROJECT_DIR && ./build.sh -t android -r"
    echo "       ./migrate_to_flutter.sh $FLUTTER_PROJECT"
fi

# iOS
IOS_LIBS="$FLUTTER_PROJECT/ios/Libs"
ios_count=0
for triple in "aarch64-apple-ios" "x86_64-apple-ios" "aarch64-apple-ios-sim"; do
    src="$RUST_PROJECT_DIR/target/$triple/release/libdomain_blocker.a"
    if [ -f "$src" ]; then
        ensure_dir "$IOS_LIBS"
        copy_file "$src" "$IOS_LIBS/libdomain_blocker_${triple}.a" "libdomain_blocker.a → ios/$triple" && ios_count=$((ios_count+1))
    fi
done
[ $ios_count -gt 0 ] && ok "iOS: $ios_count slice(s) copied" || warn "No iOS libraries found (run with --target ios to build)"

# macOS
MACOS_LIB="$RUST_LIB_DIR/libdomain_blocker.dylib"
if [ -f "$MACOS_LIB" ]; then
    ensure_dir "$FLUTTER_PROJECT/macos"
    copy_file "$MACOS_LIB" "$FLUTTER_PROJECT/macos/libdomain_blocker.dylib" "libdomain_blocker.dylib → macos/"
fi

# Windows
WINDOWS_LIB="$RUST_LIB_DIR/domain_blocker.dll"
if [ -f "$WINDOWS_LIB" ]; then
    ensure_dir "$FLUTTER_PROJECT/windows"
    copy_file "$WINDOWS_LIB" "$FLUTTER_PROJECT/windows/domain_blocker.dll" "domain_blocker.dll → windows/"
fi

# Dart wrapper files
DART_SRC="$RUST_PROJECT_DIR/dart"
if [ -d "$DART_SRC" ]; then
    ensure_dir "$FLUTTER_PROJECT/lib/services"
    copy_file "$DART_SRC/domain_blocker.dart"         "$FLUTTER_PROJECT/lib/services/domain_blocker.dart"         "domain_blocker.dart → lib/services/"
    copy_file "$DART_SRC/domain_blocker_service.dart" "$FLUTTER_PROJECT/lib/services/domain_blocker_service.dart" "domain_blocker_service.dart → lib/services/"
fi

# ── Step 4: Update pubspec.yaml ────────────────────────────────────────────────
section "4/6  Updating pubspec.yaml"

PUBSPEC="$FLUTTER_PROJECT/pubspec.yaml"
PUBSPEC_MODIFIED=false

add_dependency() {
    local pkg="$1" version="$2"
    if grep -q "^  ${pkg}:" "$PUBSPEC" 2>/dev/null || grep -q "^  ${pkg} :" "$PUBSPEC" 2>/dev/null; then
        ok "${pkg} already in pubspec.yaml"
        return
    fi

    if [ "$DRY_RUN" = true ]; then
        info "[DRY RUN] Would add to pubspec.yaml:  ${pkg}: ${version}"
        return
    fi

    # Insert after the 'dependencies:' line
    # Works on both Linux (GNU sed) and macOS (BSD sed)
    if sed --version 2>&1 | grep -q GNU; then
        sed -i "/^dependencies:/a\\  ${pkg}: ${version}" "$PUBSPEC"
    else
        sed -i '' "/^dependencies:/a\\
  ${pkg}: ${version}" "$PUBSPEC"
    fi

    ok "Added ${pkg}: ${version} to pubspec.yaml"
    PUBSPEC_MODIFIED=true
}

add_dependency "ffi"           "^2.1.0"
add_dependency "path_provider" "^2.1.0"

# ── Step 5: flutter pub get ────────────────────────────────────────────────────
section "5/6  Installing Flutter Dependencies"

if [ "$FLUTTER_AVAILABLE" = false ]; then
    warn "flutter not on PATH — skipping pub get"
    info "Run manually:  cd $FLUTTER_PROJECT && flutter pub get"
elif [ "$DRY_RUN" = true ]; then
    info "[DRY RUN] Would run: flutter pub get"
else
    step "Running flutter pub get..."
    (cd "$FLUTTER_PROJECT" && flutter pub get)
    ok "flutter pub get succeeded"
fi

# ── Step 6: Summary & next steps ──────────────────────────────────────────────
section "6/6  Summary"

echo ""
if [ "$DRY_RUN" = true ]; then
    warn "DRY RUN — no changes were made. Re-run without --dry-run to apply."
else
    ok "Integration complete!"
fi

echo ""
info "Flutter project : $FLUTTER_PROJECT"
info "Files copied    : $COPIED"
info "Files skipped   : $SKIPPED"

# Write log
if [ "$DRY_RUN" = false ]; then
    LOG="$FLUTTER_PROJECT/.domain_blocker_migration.log"
    cat > "$LOG" <<EOF
Domain Blocker Integration Log
================================
Date        : $(date)
Rust project: $RUST_PROJECT_DIR
Flutter app : $FLUTTER_PROJECT ($PROJECT_NAME)
Files copied: $COPIED  |  skipped: $SKIPPED

Platforms:
$([ -f "$FLUTTER_PROJECT/linux/libdomain_blocker.so" ]   && echo "  ✓ Linux"   || echo "  - Linux")
$([ -d "$ANDROID_JNI" ] && [ "$(ls -A "$ANDROID_JNI" 2>/dev/null)" ] && echo "  ✓ Android" || echo "  - Android")
$([ -d "$IOS_LIBS" ]    && [ "$(ls -A "$IOS_LIBS" 2>/dev/null)" ]    && echo "  ✓ iOS"     || echo "  - iOS")
$([ -f "$FLUTTER_PROJECT/macos/libdomain_blocker.dylib" ] && echo "  ✓ macOS"  || echo "  - macOS")
$([ -f "$FLUTTER_PROJECT/windows/domain_blocker.dll" ]    && echo "  ✓ Windows" || echo "  - Windows")
EOF
    ok "Log saved: .domain_blocker_migration.log"
fi

echo ""
echo -e "${CYAN}┌─────────────────────────────────────────────────────┐${NC}"
echo -e "${CYAN}│  Next Steps                                         │${NC}"
echo -e "${CYAN}└─────────────────────────────────────────────────────┘${NC}"
echo ""
echo -e "  1. ${YELLOW}Import in your Dart code:${NC}"
echo "     import 'package:${PROJECT_NAME}/services/domain_blocker_service.dart';"
echo ""
echo -e "  2. ${YELLOW}Use the API:${NC}"
echo "     await DomainBlockerService.insert('ads.example.com');"
echo "     bool blocked = await DomainBlockerService.isBlocked('ads.example.com');"
echo ""
echo -e "  3. ${YELLOW}Run your app:${NC}"
echo "     cd $FLUTTER_PROJECT && flutter run"
echo ""
if [ $ios_count -gt 0 ]; then
    echo -e "  ${YELLOW}iOS note:${NC} Add ios/Libs/*.a files to your Xcode project"
    echo "  under 'Build Phases → Link Binary With Libraries'."
    echo ""
fi
echo -e "${GREEN}Done!${NC}"