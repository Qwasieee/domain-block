# Domain Blocker — Project Structure

## File layout

```
src/
├── lib.rs              # DomainTree, BlockerEngine, and the full C FFI surface (~1200 lines)
├── filter_list.rs      # FilterListManager: HTTP download, EasyList/ABP parsing, on-disk storage
├── cosmetic.rs         # CosmeticEngine: CSS selector rules and scriptlet injection
└── main.rs             # Minimal CLI demo
examples/
└── performance_test.rs # Benchmark suite (10K+ domains, serialize/deserialize)
Docs/
├── Readme.md           # Full documentation
├── Quickstart.md       # This, but shorter
└── Project_structure.md
build.sh                # Build script for all platforms
migrate_to_flutter.sh   # Copies native libs into a Flutter project
setup.sh                # Interactive first-time setup
```

---

## What each file does

### `src/lib.rs`

The core of the library. Three main layers:

**`DomainTree`** — radix tree over reversed domain labels. A rule for `ads.example.com` is stored at path `com → example → ads`. Lookups walk the tree in the same direction (TLD first), so parent domains are checked naturally during traversal.

Public Rust API:
- `insert(domain)` — universal block
- `insert_with_whitelist(domain, origin)` — blocked everywhere except `origin`
- `insert_with_blacklist(domain, origin)` — blocked only on `origin`
- `insert_with_modifiers(domain, action, origin_constraint, resource_mask, third_party_only)` — full rule
- `is_blocked(domain)` / `is_blocked_with_origin(domain, origin)` / `is_blocked_ex(...)` — queries
- `save_to_file(path)` / `load_from_file(path)` — binary cache (`DOMTREE3` format)
- `bulk_load(domains)`, `count_blocked()`, `get_all_blocked()`

**`BlockerEngine`** — the handle exposed over FFI. Owns a `FilterListManager` and a `CosmeticEngine`. All `blocker_engine_*` C functions operate on a `*mut BlockerEngine`.

**FFI surface** — `#[no_mangle]` C functions for Dart:
- Lifecycle: `blocker_engine_new`, `blocker_engine_free`
- Filter list management: `install`, `update`, `update_all`, `remove`, `set_enabled`, `rebuild`, `get_list_info`, `get_all_lists`
- Manual rules: `insert`, `insert_with_whitelist`, `insert_with_blacklist`, `bulk_insert`
- Queries: `is_blocked`, `is_blocked_with_origin`, `is_blocked_ex`
- Stats: `total_rules`, `count_blocked`, `get_all_blocked`
- Cosmetics: `get_cosmetic_bundle`, `cosmetic_rule_count`
- Memory: `free_string`, `free_string_array`

### `src/filter_list.rs`

**`FilterListManager`** downloads, stores, and re-parses EasyList/ABP-format filter lists. Design notes:

- Raw list files are kept on disk per list. Disabling a list means rebuilding the tree by re-parsing the remaining enabled files — no second in-memory copy needed.
- Parsing is streaming, line by line. The full list text is never held in memory.
- HTTP uses `reqwest` blocking (no async runtime). Callers invoke this from a Flutter `Isolate.run()` so blocking is fine.
- Progress is reported via a C function pointer (`ProgressCallback`). The callback is called from the same thread that made the FFI call — no cross-thread concerns.
- Metadata (`lists.json`) lives in the same storage directory as the cached trees, recording each list's name, URL, enabled state, and last-update timestamp.

Supported ABP syntax:
```
||domain.com^                           universal block
@@||domain.com^                         exception / whitelist
||domain.com^$domain=x.com|y.com        block only on listed origins
@@||domain.com^$domain=x.com            exception only on listed origins
||domain.com^$script,third-party        resource type + third-party modifiers
```

Intentionally ignored: cosmetic/element-hiding rules (handled by `CosmeticEngine`), path-level rules, regex rules, IP address rules.

### `src/cosmetic.rs`

**`CosmeticEngine`** stores CSS selector rules and scriptlet calls, keyed by domain.

Rule types:
```
##.selector              global CSS hide rule
#@#.selector             global CSS exception
example.com##.sel        domain-specific hide
example.com#@#.sel       domain-specific exception
~example.com##.sel       global rule excluding this domain
example.com##+js(fn, arg) scriptlet injection
```

`build_bundle(host)` collects rules for a hostname (and all its parent domains) into a single self-contained JS string ready to inject at `AT_DOCUMENT_START`. Contains one `<style>` block for CSS hiding, followed by scriptlet calls.

### `src/main.rs`

Minimal demo. Inserts a couple of domains and prints query results. Useful for a quick smoke test.

### `examples/performance_test.rs`

Benchmark tool. Tests insert throughput, query latency, and serialize/deserialize times across large domain sets (10K+). Run with:

```bash
cargo run --example performance_test --release
```

---

## Key design decisions

**Reverse domain storage** — `com.example.ads` instead of `ads.example.com`. Tree traversal goes TLD → domain → subdomain. This makes parent-domain blocking a natural side effect of the lookup path rather than a special case.

**Radix tree with path compression** — common prefixes (like `com`, `net`, `org`) are shared across all rules. Memory per domain: ~50–60 bytes after compression.

**Binary cache (DOMTREE3)** — avoids re-parsing list files on every launch. Format is little-endian binary: 8-byte magic header, then recursive node encoding (prefix length + bytes, rule array, children map). Each rule stores action, resource mask, third-party flag, and origin constraint. Files from earlier versions (`DOMTREE1`, `DOMTREE2`) are rejected at load with a clear error — delete and rebuild.

**No async runtime** — FFI calls from Dart run inside `Isolate.run()`, so blocking I/O is fine and keeps the dependency surface small.

**Per-list on-disk storage** — disabling/removing a filter list is a re-parse operation on the remaining files, not a tree subtraction. Slightly slower for toggle operations; much simpler and lower-memory overall.

---

## Time and space complexity

| Operation | Complexity | Notes |
|-----------|-----------|-------|
| Insert | O(k) | k = domain label length |
| Query | O(k) | k = domain label length |
| Serialize | O(n) | n = total tree nodes |
| Deserialize | O(n) | |
| Space | O(n·k) | Compressed; ~50–60 bytes/domain in practice |

---

## Thread safety

`DomainTree` and `BlockerEngine` are not `Send + Sync` by default. The expected pattern for Flutter:

- Each Flutter isolate that needs blocking creates its own `BlockerEngine` handle.
- The engine is effectively read-only after `rebuild()` — all in-flight queries are safe.
- Mutations (install, update, insert) should be serialized at the Dart layer.

---

## Serialization format (DOMTREE3)

```
[8 bytes]   Magic: "DOMTREE3"
[Node]      Root node, recursively encoded:
  [4]       Prefix byte length (LE u32)
  [n]       Prefix UTF-8 bytes
  [4]       Rule count (LE u32)
  [rule…]   Rules:
    [1]     Action (0 = Block, 1 = Allow)
    [2]     Resource mask (LE u16; 0 = any type)
    [1]     Third-party only (0/1)
    [1]     Origin constraint tag (0 = Any, 1 = OnlyOn, 2 = ExceptOn)
    [list?] If tag != 0: [4] count, then [4+n] per string
  [4]       Children count (LE u32)
  [child…]  Children:
    [4]     Key char (LE u32 codepoint)
    [Node]  Child node (recursive)
```

---

## Distribution strategies

**Pre-built cache** — build the cache at compile time, bundle it in assets. Fast first launch, larger binary.

**On-demand parse** — download the blocklist on first launch, parse and cache locally. Smaller binary, slower first launch (~100–650ms depending on list size).

**Hybrid** — ship a minimal essential list with the app; download the full list in the background and swap once cached.

---

## Extension ideas

- Rule categories / priorities (Block/Allow/Redirect instead of plain bool)
- Bloom filter as a fast pre-check before the radix tree query
- Wildcard patterns (`*.ads.*`)
- Hot reload: watch list files on disk and rebuild on change
- Checksums on the cache file to detect corruption
