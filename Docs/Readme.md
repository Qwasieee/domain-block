# 🛡️ Domain Blocker

High-performance context-aware domain blocking library written in Rust with Flutter/Dart bindings.

## ✨ Features

- **⚡ Blazing Fast** - Microsecond query times (~3-5μs per lookup)
- **🎯 Context-Aware** - Block domains based on origin/referrer
- **🌳 Radix Tree** - Memory-efficient subdomain matching
- **💾 Binary Cache** - Fast serialization (5-10ms load for 10K rules)
- **🔗 Zero-Copy FFI** - Native performance in Flutter
- **📦 Cross-Platform** - Linux, Android, iOS, macOS, Windows

## 🚀 Quick Start

### Prerequisites

- Rust 1.70+ (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`)
- Flutter 3.0+ (optional, for Flutter integration)

### Setup (One Command)

```bash
./setup.sh
```

This interactive script will:
1. Check prerequisites
2. Build the library
3. Run tests
4. Guide you through next steps

### Manual Build

```bash
# Build for host platform
./build.sh -r

# Build for Android
./build.sh -t android -r

# Build for all platforms
./build.sh -t all -r

# Show all options
./build.sh --help
```

## 📱 Flutter Integration

### 1. Migrate Libraries

```bash
./migrate.sh /path/to/your/flutter/project
```

This will:
- Copy Dart wrapper files to `lib/services/`
- Copy native libraries to platform-specific directories
- Create migration report
- Check dependencies

### 2. Add Dependencies

Add to your `pubspec.yaml`:

```yaml
dependencies:
  ffi: ^2.1.0
  path_provider: ^2.1.0
```

### 3. Use in Your App

```dart
import 'services/domain_blocker_service.dart';

// Universal block (blocks everywhere)
await DomainBlockerService.insert('doubleclick.net');

// Whitelist (don't block on specific origin)
await DomainBlockerService.insert(
  'google-analytics.com',
  whitelist: 'mysite.com',
);

// Conditional block (block ONLY on specific origin)
await DomainBlockerService.insert(
  'amazon-adsystem.com',
  onlyIf: 'annoyingsite.com',
);

// Check if blocked
bool blocked = await DomainBlockerService.isBlocked('doubleclick.net');

// Check with context
bool blocked = await DomainBlockerService.isBlocked(
  'google-analytics.com',
  origin: 'mysite.com',
);
```

## 🎯 Context-Aware Blocking

### Three Rule Types

#### 1. Universal Block
Blocks domain everywhere (traditional ad blocking):

```dart
await DomainBlockerService.insert('ads.com');
```

#### 2. Whitelist (Exception)
Allows domain on specific origins:

```dart
await DomainBlockerService.insert('analytics.com', whitelist: 'mysite.com');
// Blocked everywhere EXCEPT on mysite.com
```

#### 3. Blacklist (Conditional)
Blocks domain ONLY on specific origins:

```dart
await DomainBlockerService.insert('popups.com', onlyIf: 'spamsite.com');
// Blocked ONLY on spamsite.com, allowed elsewhere
```

### Priority System

Rules are evaluated in this order:
1. **Whitelist** (highest) → Always allows
2. **Blacklist** → Blocks if origin matches
3. **Universal** (lowest) → Blocks everywhere

### Subdomain Matching

All rules automatically match subdomains:

```dart
await DomainBlockerService.insert('ads.com', whitelist: 'mysite.com');

// These all work:
isBlocked('ads.com', origin: 'mysite.com')          // ✅ allowed
isBlocked('sub.ads.com', origin: 'mysite.com')      // ✅ allowed
isBlocked('ads.com', origin: 'sub.mysite.com')      // ✅ allowed
```

## 📊 Performance

Benchmarks on Apple M1:

| Operation | Time | Details |
|-----------|------|---------|
| Query (no context) | ~3μs | Single domain lookup |
| Query (with context) | ~5μs | With origin checking |
| Bulk insert | ~6μs/domain | 1000 domains |
| Serialize | ~3ms | 7000 rules |
| Deserialize | ~7ms | 7000 rules (5.7x faster) |
| Cache size | ~19 bytes/rule | Binary format |

## 🛠️ Development

### Project Structure

```
domain_blocker/
├── src/
│   ├── lib.rs              # Core library + FFI
│   └── main.rs             # CLI demo
├── dart/
│   ├── domain_blocker.dart         # FFI wrapper
│   └── domain_blocker_service.dart # Service layer
├── examples/
│   └── performance_test.rs # Comprehensive benchmarks
├── build.sh                # Enhanced build script
├── migrate.sh              # Flutter migration tool
├── setup.sh                # Quick setup script
└── README.md               # This file
```

### Scripts

#### `setup.sh`
Interactive setup for new users:
```bash
./setup.sh
```

#### `build.sh`
Flexible build system:
```bash
./build.sh                    # Host platform (debug)
./build.sh -r                 # Host platform (release)
./build.sh -t android -r      # Android (release)
./build.sh -t all -r          # All platforms
./build.sh --help             # Show all options
```

Options:
- `-t, --target` - Platform (linux, android, ios, macos, windows, all)
- `-r, --release` - Release build
- `--test` - Run tests after build
- `-v, --verbose` - Verbose output

#### `migrate.sh`
Copy libraries to Flutter project:
```bash
./migrate.sh /path/to/flutter/project
./migrate.sh ~/my_app --dry-run    # Preview changes
./migrate.sh ~/my_app --verbose    # Detailed output
```

### Running Tests

```bash
# Unit tests
cargo test

# Performance benchmarks
cargo run --example performance_test --release

# With features
cargo test --all-features
```

## 📖 API Reference

### Dart API

#### Insert Rules

```dart
// Universal block
Future<bool> insert(String domain)

// With context
Future<bool> insert(String domain, {String? whitelist, String? onlyIf})
```

#### Query

```dart
// Check without context
Future<bool> isBlocked(String domain)

// Check with context
Future<bool> isBlocked(String domain, {String? origin})
```

#### Bulk Operations

```dart
// Bulk load (universal blocks)
Future<bool> bulkLoad(List<String> domains)

// Get statistics
Future<int> countBlocked()
Future<List<String>> getAllBlocked()
```

#### Cache Management

```dart
Future<bool> saveCache()
Future<bool> reloadCache()
Future<void> reset()
Future<Map<String, dynamic>> getCacheInfo()
```

### Rust API

```rust
use domain_blocker::DomainTree;

let mut tree = DomainTree::new();

// Insert rules
tree.insert("ads.com");
tree.insert_with_whitelist("analytics.com", "mysite.com");
tree.insert_with_blacklist("tracker.com", "badsite.com");

// Query
let blocked = tree.is_blocked("ads.com");
let blocked = tree.is_blocked_with_origin("ads.com", Some("mysite.com"));

// Serialization
tree.save_to_file("cache.bin")?;
let tree = DomainTree::load_from_file("cache.bin")?;
```

## 🎨 Example Use Cases

### Browser Extension
```dart
// Block ads but support favorite creators
await DomainBlockerService.insert('doubleclick.net');
await DomainBlockerService.insert('doubleclick.net', whitelist: 'creator-blog.com');

String currentSite = getCurrentPage();
bool shouldBlock = await DomainBlockerService.isBlocked(
  adDomain,
  origin: currentSite,
);
```

### Parental Controls
```dart
// Block social media during study hours
if (isStudyTime()) {
  await DomainBlockerService.insert('facebook.com', onlyIf: 'study-mode');
  await DomainBlockerService.insert('instagram.com', onlyIf: 'study-mode');
}

bool blocked = await DomainBlockerService.isBlocked(
  'facebook.com',
  origin: isStudyTime() ? 'study-mode' : null,
);
```

### Privacy Protection
```dart
// Block trackers universally
await DomainBlockerService.bulkLoad([
  'google-analytics.com',
  'facebook-pixel.com',
  'doubleclick.net',
]);

// Allow own analytics
await DomainBlockerService.insert(
  'google-analytics.com',
  whitelist: 'mywebsite.com',
);
```

## 🔧 Troubleshooting

### Build Issues

**Error: Target not installed**
```bash
rustup target add aarch64-linux-android
```

**Error: NDK not found (Android)**
```bash
export ANDROID_NDK_HOME=/path/to/ndk
```

### Flutter Issues

**FFI errors**
```bash
flutter clean
flutter pub get
flutter run
```

**Library not found**
```bash
# Make sure libraries are in the right place
./migrate.sh /path/to/project --verbose
```

## 📝 Migration Notes

### From v1 to v2

v2 adds context-aware blocking while maintaining backward compatibility:

```dart
// Old API still works
await DomainBlockerService.insert('ads.com');
bool blocked = await DomainBlockerService.isBlocked('ads.com');

// New context-aware features
await DomainBlockerService.insert('ads.com', whitelist: 'mysite.com');
bool blocked = await DomainBlockerService.isBlocked('ads.com', origin: 'mysite.com');
```

Old cache files (v1) are automatically upgraded on load.

## 🤝 Contributing

Contributions welcome! Please ensure:

```bash
# Tests pass
cargo test

# Code is formatted
cargo fmt

# No clippy warnings
cargo clippy
```

## 📄 License

MIT License - see LICENSE file for details

## 🙏 Acknowledgments

- Built with Rust 🦀
- Flutter integration via FFI
- Radix tree implementation
- Context-aware filtering inspired by AdBlock Plus syntax

---

**Built with ❤️ for privacy and performance**