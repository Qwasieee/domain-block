# Domain Blocker

High-performance, context-aware domain blocking library written in Rust, with a Flutter/Dart FFI layer.

## Features

- Microsecond query times (~3–5μs per lookup)
- Context-aware rules: block globally, whitelist on specific origins, or block only on specific origins
- Radix tree with path compression for memory-efficient subdomain matching
- Binary cache format (DOMTREE3) — ~5–10ms load for 10K rules vs. ~120ms cold parse
- EasyList/ABP filter list support with HTTP download and conditional GET updates
- Cosmetic filtering: CSS selector hiding and scriptlet injection
- Resource-type and third-party modifiers (`$script`, `$image`, `$third-party`, etc.)
- Zero-copy FFI — native performance in Flutter via `dart:ffi`
- Cross-platform: Linux, Android, iOS, macOS, Windows

## Prerequisites

- Rust 1.70+ (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`)
- Flutter 3.0+ (optional, for Flutter integration)

## Quick Start

```bash
./setup.sh
```

Pick **option 5** to build Rust and integrate into a Flutter project in one step, or follow the manual steps below.

## Manual Build

```bash
# Host platform (debug)
./build.sh

# Host platform (release)
./build.sh -r

# Android (release)
./build.sh -t android -r

# All platforms (release)
./build.sh -t all -r

# All options
./build.sh --help
```

## Flutter Integration

### 1. Copy libraries

```bash
./migrate_to_flutter.sh /path/to/your/flutter/project
```

Options:
- `--dry-run` — preview what would be copied without writing anything
- `--verbose` — detailed output

### 2. Add dependencies

```yaml
dependencies:
  ffi: ^2.1.0
  path_provider: ^2.1.0
```

### 3. Use in Dart

```dart
import 'services/domain_blocker_service.dart';

// Universal block
await DomainBlockerService.insert('doubleclick.net');

// Whitelist on a specific origin (blocked everywhere except mysite.com)
await DomainBlockerService.insert(
  'google-analytics.com',
  whitelist: 'mysite.com',
);

// Conditional block (blocked only on annoyingsite.com)
await DomainBlockerService.insert(
  'amazon-adsystem.com',
  onlyIf: 'annoyingsite.com',
);

// Check without context
bool blocked = await DomainBlockerService.isBlocked('doubleclick.net');

// Check with origin
bool blocked = await DomainBlockerService.isBlocked(
  'google-analytics.com',
  origin: 'mysite.com',
);
```

## Context-Aware Blocking

### Rule types

**Universal block** — blocks the domain everywhere:

```dart
await DomainBlockerService.insert('ads.com');
```

**Whitelist (exception)** — blocked everywhere *except* on a specific origin:

```dart
await DomainBlockerService.insert('analytics.com', whitelist: 'mysite.com');
```

**Conditional block** — blocked *only* on a specific origin, allowed elsewhere:

```dart
await DomainBlockerService.insert('popups.com', onlyIf: 'spamsite.com');
```

### Evaluation order

1. Whitelist — always wins
2. Conditional block — fires if origin matches
3. Universal block — fallback

### Subdomain matching

Rules automatically match subdomains on both the domain and origin side:

```dart
await DomainBlockerService.insert('ads.com', whitelist: 'mysite.com');

isBlocked('ads.com', origin: 'mysite.com')        // allowed
isBlocked('sub.ads.com', origin: 'mysite.com')    // allowed
isBlocked('ads.com', origin: 'sub.mysite.com')    // allowed
isBlocked('ads.com', origin: 'other.com')         // blocked
```

## Filter Lists

The library supports EasyList/ABP-format filter lists managed via `BlockerEngine` (the high-level FFI layer that owns both the domain tree and the cosmetic engine).

Supported rule syntax:

```
||domain.com^                          universal block
@@||domain.com^                        exception (whitelist)
||domain.com^$domain=x.com|y.com       block only on x.com or y.com
@@||domain.com^$domain=x.com           exception only on x.com
||domain.com^$script,third-party       block scripts from third-party domain.com
example.com##.ad-banner                CSS hide rule (cosmetic)
example.com#@#.ad-banner               CSS exception
example.com##+js(set-constant, ...)    scriptlet injection
```

Cosmetic rules (CSS hiding, scriptlets) are handled by `CosmeticEngine` and served as a self-contained JS bundle via `blocker_engine_get_cosmetic_bundle`.

## Performance

Benchmarks on Apple M1:

| Operation | Time | Notes |
|-----------|------|-------|
| Query (no context) | ~3μs | Single domain lookup |
| Query (with context) | ~5μs | Origin matching |
| Bulk insert | ~6μs/domain | 1000 domains |
| Serialize | ~3ms | 7000 rules |
| Deserialize | ~7ms | 7000 rules |
| Cache size | ~19 bytes/rule | Binary format |

Benchmarks on Snapdragon 730:

| Operation | 100K domains | 500K domains |
|-----------|-------------|-------------|
| Build (cold) | 120ms | 650ms |
| Load from cache | 12ms | 65ms |
| Query | 0.8μs | 1.2μs |

## Project Structure

```
src/
├── lib.rs              # DomainTree, BlockerEngine, full FFI surface
├── filter_list.rs      # FilterListManager: download, parse, store EasyList files
├── cosmetic.rs         # CosmeticEngine: CSS/scriptlet rule storage and bundle output
└── main.rs             # Minimal CLI demo
examples/
└── performance_test.rs # Benchmark suite
build.sh                # Build script (Linux, Android, iOS, macOS, Windows)
migrate_to_flutter.sh   # Copies native libs into a Flutter project
setup.sh                # Interactive first-time setup
```

## API Reference

### Rust — `DomainTree`

Direct tree API, used when you manage rules programmatically without filter list files.

```rust
use blocker_engine::DomainTree;

let mut tree = DomainTree::new();

tree.insert("ads.com");
tree.insert_with_whitelist("analytics.com", "mysite.com");
tree.insert_with_blacklist("tracker.com", "badsite.com");

// Full insert with resource type and third-party modifiers
use blocker_engine::{Action, OriginConstraint, resource_type};
tree.insert_with_modifiers(
    "cdn.ads.com",
    Action::Block,
    OriginConstraint::Any,
    resource_type::SCRIPT | resource_type::IMAGE,
    true, // third_party_only
);

// Queries
tree.is_blocked("ads.com");
tree.is_blocked_with_origin("ads.com", Some("mysite.com"));
tree.is_blocked_ex("ads.com", Some("mysite.com"), resource_type::SCRIPT, true);

// Cache
tree.save_to_file("cache.bin")?;
let tree = DomainTree::load_from_file("cache.bin")?;
```

Cache format: `DOMTREE3` (little-endian binary). Files from earlier versions (`DOMTREE1`, `DOMTREE2`) are not forward-compatible and must be regenerated.

### Rust — `BlockerEngine` (high-level)

Owns a `FilterListManager` and a `CosmeticEngine`. This is the handle used by the FFI layer.

```rust
// Managed via FFI; see C API below for Dart usage.
// Each BlockerEngine stores filter lists on disk under a given storage_dir
// and rebuilds the tree from them on demand.
```

### C FFI (for Dart)

All functions are `#[no_mangle]` exports prefixed `blocker_engine_`.

**Lifecycle**

```c
void* blocker_engine_new(const char* storage_dir);
void  blocker_engine_free(void* engine);
```

**Filter lists**

```c
usize blocker_engine_rebuild(void* engine, progress_cb);
usize blocker_engine_install(void* engine, const char* name, const char* url, progress_cb);
usize blocker_engine_update(void* engine, const char* name, progress_cb);
usize blocker_engine_update_all(void* engine, progress_cb);
bool  blocker_engine_remove(void* engine, const char* name, progress_cb);
bool  blocker_engine_set_enabled(void* engine, const char* name, bool enabled, progress_cb);
char* blocker_engine_get_list_info(void* engine, const char* name);   // JSON; free with blocker_engine_free_string
char* blocker_engine_get_all_lists(void* engine);                     // JSON array; free with blocker_engine_free_string
```

**Manual rule insertion**

```c
bool  blocker_engine_insert(void* engine, const char* domain);
bool  blocker_engine_insert_with_whitelist(void* engine, const char* domain, const char* origin);
bool  blocker_engine_insert_with_blacklist(void* engine, const char* domain, const char* origin);
bool  blocker_engine_bulk_insert(void* engine, const char** domains, usize count);
```

**Queries**

```c
bool  blocker_engine_is_blocked(void* engine, const char* domain);
bool  blocker_engine_is_blocked_with_origin(void* engine, const char* domain, const char* origin);
bool  blocker_engine_is_blocked_ex(void* engine, const char* domain, const char* origin,
                                    uint16_t resource_mask, bool is_third_party);
```

**Stats**

```c
usize blocker_engine_total_rules(void* engine);
usize blocker_engine_count_blocked(void* engine);
char** blocker_engine_get_all_blocked(void* engine, usize* out_count);
void   blocker_engine_free_string_array(char** arr, usize count);
```

**Cosmetics**

```c
// Returns a self-contained JS bundle (CSS injection + scriptlet calls) for host.
// host is a bare hostname, e.g. "news.example.com" — no scheme, path, or port.
// Free the result with blocker_engine_free_string.
char* blocker_engine_get_cosmetic_bundle(void* engine, const char* host);
usize blocker_engine_cosmetic_rule_count(void* engine);
```

**Memory**

```c
void blocker_engine_free_string(char* s);
void blocker_engine_free_string_array(char** arr, usize count);
```

### Dart API (service layer)

```dart
// Rule insertion
Future<bool> insert(String domain)
Future<bool> insert(String domain, {String? whitelist, String? onlyIf})
Future<bool> bulkLoad(List<String> domains)

// Queries
Future<bool> isBlocked(String domain)
Future<bool> isBlocked(String domain, {String? origin})

// Stats
Future<int>          countBlocked()
Future<List<String>> getAllBlocked()

// Cache
Future<bool> saveCache()
Future<bool> reloadCache()
Future<void> reset()
Future<Map<String, dynamic>> getCacheInfo()
```

## Example Use Cases

### WebView ad blocking

```dart
String currentSite = getCurrentPage();
bool shouldBlock = await DomainBlockerService.isBlocked(
  adDomain,
  origin: currentSite,
);
// Support a specific site while still blocking ads elsewhere
await DomainBlockerService.insert('doubleclick.net', whitelist: 'creator-blog.com');
```

### Parental controls

```dart
if (isStudyTime()) {
  await DomainBlockerService.insert('facebook.com', onlyIf: 'study-mode');
  await DomainBlockerService.insert('instagram.com', onlyIf: 'study-mode');
}
bool blocked = await DomainBlockerService.isBlocked(
  'facebook.com',
  origin: isStudyTime() ? 'study-mode' : null,
);
```

### Privacy protection with own analytics exempted

```dart
await DomainBlockerService.bulkLoad([
  'google-analytics.com',
  'facebook-pixel.com',
  'doubleclick.net',
]);
await DomainBlockerService.insert(
  'google-analytics.com',
  whitelist: 'mywebsite.com',
);
```

## Troubleshooting

**`EM_X86_64 instead of EM_AARCH64` on Android** — stale host-platform `.so` in JNI libs. Run `./build.sh -t android -r && ./migrate_to_flutter.sh /path/to/app`. The migrate script removes stale artifacts.

**`Target not installed`** — `rustup target add aarch64-linux-android` (or the relevant target).

**`NDK not found`** — `export ANDROID_NDK_HOME=/path/to/ndk`, or install via Android Studio → SDK Manager → SDK Tools → NDK.

**Cache rejected at load** — the file is from an older format version. Delete it and let the engine rebuild.

**FFI errors after `flutter run`** — `flutter clean && flutter pub get && flutter run`.

**Library not found** — re-run `./migrate_to_flutter.sh /path/to/project --verbose` to check where the `.so`/`.dylib` landed.

## Running Tests

```bash
cargo test                                          # unit tests
cargo test --all-features                           # with all features
cargo run --example performance_test --release      # benchmark suite
```

## Contributing

Before submitting a PR:

```bash
cargo test
cargo fmt
cargo clippy
```

## License

MIT — see LICENSE.
