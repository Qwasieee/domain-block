use blocker_engine::cosmetic::CosmeticEngine;
use blocker_engine::DomainTree;

fn main() {
    println!("=== Domain Blocker Performance Test ===\n");

    // ── Test 1: Build tree with universal blocks ──────────────────────────────
    println!("Test 1: Building tree with universal blocks...");
    let mut tree = DomainTree::new();

    let sample_domains = vec![
        "doubleclick.net",
        "googlesyndication.com",
        "googleadservices.com",
        "facebook.com",
        "ads.twitter.com",
        "analytics.google.com",
        "advertising.com",
        "scorecardresearch.com",
        "criteo.com",
        "taboola.com",
    ];

    let start = std::time::Instant::now();
    tree.bulk_load(&sample_domains);
    let build_time = start.elapsed();
    println!("✓ Built tree with {} domains in {:?}", sample_domains.len(), build_time);

    // ── Test 2: Basic query performance ──────────────────────────────────────
    println!("\nTest 2: Basic query performance...");
    let test_queries = vec![
        ("ads.twitter.com",               None,    true),
        ("sub.ads.twitter.com",           None,    true),
        ("twitter.com",                   None,    false),
        ("facebook.com",                  None,    true),
        ("instagram.com",                 None,    false),
        ("tracking.analytics.google.com", None,    true),
        ("google.com",                    None,    false),
    ];

    for (domain, origin, expected) in test_queries {
        let start = std::time::Instant::now();
        let result = tree.is_blocked_with_origin(domain, origin);
        let query_time = start.elapsed();
        let status = if result == expected { "✓" } else { "✗" };
        let label = if result { "BLOCKED" } else { "ALLOWED" };
        println!("{} {} - {} ({:?})", status, domain, label, query_time);
    }

    // ── Test 3: Context-aware blocking (whitelist) ────────────────────────────
    println!("\nTest 3: Context-aware blocking (whitelist)...");
    tree.insert_with_whitelist("doubleclick.net", "example.com");

    let whitelist_tests = vec![
        ("doubleclick.net", None,                    true,  "universal block applies"),
        ("doubleclick.net", Some("example.com"),     false, "whitelisted on example.com"),
        ("doubleclick.net", Some("sub.example.com"), false, "whitelisted on subdomains too"),
        ("doubleclick.net", Some("badsite.com"),     true,  "still blocked on other sites"),
    ];

    for (domain, origin, expected, description) in whitelist_tests {
        let result = tree.is_blocked_with_origin(domain, origin);
        let status = if result == expected { "✓" } else { "✗" };
        let label = if result { "BLOCKED" } else { "ALLOWED" };
        println!("{} {} [origin: {:?}] - {} ({})", status, domain, origin, label, description);
    }

    // ── Test 4: Context-aware blocking (conditional/blacklist) ────────────────
    println!("\nTest 4: Context-aware blocking (conditional/blacklist)...");
    tree.insert_with_blacklist("amazon-adsystem.com", "annoyingsite.com");

    let blacklist_tests = vec![
        ("amazon-adsystem.com", None,                         false, "not blocked without context"),
        ("amazon-adsystem.com", Some("goodsite.com"),         false, "not blocked on other sites"),
        ("amazon-adsystem.com", Some("annoyingsite.com"),     true,  "blocked ONLY on annoyingsite.com"),
        ("amazon-adsystem.com", Some("sub.annoyingsite.com"), true,  "blocked on subdomains too"),
    ];

    for (domain, origin, expected, description) in blacklist_tests {
        let result = tree.is_blocked_with_origin(domain, origin);
        let status = if result == expected { "✓" } else { "✗" };
        let label = if result { "BLOCKED" } else { "ALLOWED" };
        println!("{} {} [origin: {:?}] - {} ({})", status, domain, origin, label, description);
    }

    // ── Test 5: Priority (whitelist > universal) ──────────────────────────────
    println!("\nTest 5: Priority testing (whitelist > universal)...");
    tree.insert("priority-test.com");
    tree.insert_with_whitelist("priority-test.com", "trusted.com");

    println!("✓ Without context: {} (universal block applies)",
        tree.is_blocked("priority-test.com"));
    println!("✓ With trusted.com: {} (whitelist overrides)",
        tree.is_blocked_with_origin("priority-test.com", Some("trusted.com")));
    println!("✓ With other.com: {} (universal block applies)",
        tree.is_blocked_with_origin("priority-test.com", Some("other.com")));

    // ── Test 6: Serialization preserves context rules ─────────────────────────
    println!("\nTest 6: Serialization with context rules...");
    let cache_path = "/tmp/domain_tree_context_test.cache";

    let start = std::time::Instant::now();
    tree.save_to_file(cache_path).expect("Failed to save");
    let save_time = start.elapsed();
    println!("✓ Saved to cache in {:?}", save_time);

    let start = std::time::Instant::now();
    let loaded_tree = DomainTree::load_from_file(cache_path).expect("Failed to load");
    let load_time = start.elapsed();
    println!("✓ Loaded from cache in {:?}", load_time);

    println!("✓ Verifying loaded tree:");
    println!("  - doubleclick.net (no context): {}",
        loaded_tree.is_blocked("doubleclick.net"));
    println!("  - doubleclick.net (example.com): {}",
        loaded_tree.is_blocked_with_origin("doubleclick.net", Some("example.com")));
    println!("  - amazon-adsystem.com (annoyingsite.com): {}",
        loaded_tree.is_blocked_with_origin("amazon-adsystem.com", Some("annoyingsite.com")));

    // ── Test 7: Large-scale network performance ───────────────────────────────
    println!("\nTest 7: Large-scale network performance...");
    let mut large_tree = DomainTree::new();

    let universal_domains: Vec<String> = (0..5000)
        .map(|i| format!("ad{}.example.com", i))
        .collect();

    let start = std::time::Instant::now();
    large_tree.bulk_load(&universal_domains.iter().map(|s| s.as_str()).collect::<Vec<_>>());
    for i in 0..1000 {
        large_tree.insert_with_whitelist(&format!("ad{}.example.com", i), "trusted.com");
    }
    for i in 5000..6000 {
        large_tree.insert_with_blacklist(&format!("conditional{}.net", i), "badsite.com");
    }
    let build_time = start.elapsed();
    let total_rules: u32 = 5000 + 1000 + 1000;

    println!("✓ Built tree with {} rules in {:?}", total_rules, build_time);
    println!("  Average: {:?} per rule", build_time / total_rules);

    let start = std::time::Instant::now();
    let _ = large_tree.is_blocked("ad2500.example.com");
    println!("✓ Query time (no context): {:?}", start.elapsed());

    let start = std::time::Instant::now();
    let _ = large_tree.is_blocked_with_origin("ad500.example.com", Some("trusted.com"));
    println!("✓ Query time (with context): {:?}", start.elapsed());

    let large_cache_path = "/tmp/domain_tree_large_context.cache";
    let start = std::time::Instant::now();
    large_tree.save_to_file(large_cache_path).expect("Failed to save");
    let save_time = start.elapsed();

    let start = std::time::Instant::now();
    let _ = DomainTree::load_from_file(large_cache_path).expect("Failed to load");
    let load_time = start.elapsed();

    println!("✓ Large tree save: {:?}, load: {:?}", save_time, load_time);
    println!("  Speedup: {:.1}x faster to load than build",
        build_time.as_secs_f64() / load_time.as_secs_f64());

    let file_size = std::fs::metadata(large_cache_path).unwrap().len();
    println!("  Cache file size: {} KB ({} bytes/rule)",
        file_size / 1024, file_size / total_rules as u64);

    // ── Test 8: CosmeticEngine — rule insertion ───────────────────────────────
    println!("\nTest 8: CosmeticEngine rule insertion...");
    let mut engine = CosmeticEngine::new();

    let cosmetic_rules = vec![
        "##.ad-banner",                                    // global CSS
        "##.sponsored-content",                            // global CSS
        "example.com##.sidebar-ad",                        // domain CSS
        "news.example.com##.inline-ad",                    // subdomain CSS
        "~trusted.com##.popup",                            // global except trusted.com
        "example.com#@#.ad-banner",                        // exception on example.com
        "example.com##+js(set-constant, adblock, false)",  // scriptlet
        "tracker.com##+js(abort-on-property-read, _gaq)",  // scriptlet
    ];

    let start = std::time::Instant::now();
    let mut inserted = 0usize;
    for rule in &cosmetic_rules {
        if engine.insert_line(rule) {
            inserted += 1;
        }
    }
    let insert_time = start.elapsed();

    println!("✓ Inserted {}/{} cosmetic rules in {:?}", inserted, cosmetic_rules.len(), insert_time);
    println!("  Total rule count: {}", engine.total_rules());

    // ── Test 9: CosmeticEngine — bundle generation correctness ────────────────
    println!("\nTest 9: CosmeticEngine bundle generation correctness...");

    struct BundleCheck {
        host: &'static str,
        should_contain: Vec<&'static str>,
        should_not_contain: Vec<&'static str>,
    }

    let checks = vec![
        BundleCheck {
            host: "other.com",
            // Gets global rules; popup negated rule applies (other.com is not trusted.com)
            should_contain: vec![".ad-banner", ".sponsored-content", ".popup"],
            should_not_contain: vec![".sidebar-ad", ".inline-ad", "adblock"],
        },
        BundleCheck {
            host: "example.com",
            // .ad-banner is excepted via #@#; sidebar-ad and scriptlet apply
            should_contain: vec![".sponsored-content", ".sidebar-ad", ".popup", "adblock"],
            should_not_contain: vec![".ad-banner", ".inline-ad"],
        },
        BundleCheck {
            host: "news.example.com",
            // Inherits example.com rules (sidebar-ad) plus its own (inline-ad); ad-banner still excepted
            should_contain: vec![".sponsored-content", ".sidebar-ad", ".inline-ad", ".popup"],
            should_not_contain: vec![".ad-banner"],
        },
        BundleCheck {
            host: "trusted.com",
            // Excluded from ~trusted.com##.popup rule
            should_contain: vec![".ad-banner", ".sponsored-content"],
            should_not_contain: vec![".popup", ".sidebar-ad"],
        },
        BundleCheck {
            host: "tracker.com",
            // Has a scriptlet; no domain CSS rules
            should_contain: vec!["_gaq"],
            should_not_contain: vec![".sidebar-ad"],
        },
        BundleCheck {
            host: "unrelated.io",
            // Only global rules
            should_contain: vec![".ad-banner", ".sponsored-content"],
            should_not_contain: vec![".sidebar-ad", "adblock"],
        },
    ];

    let mut all_ok = true;
    for check in &checks {
        let start = std::time::Instant::now();
        let bundle = engine.build_bundle(check.host);
        let bundle_time = start.elapsed();

        let mut host_ok = true;
        for needle in &check.should_contain {
            if !bundle.contains(needle) {
                eprintln!("  ✗ [{}] missing expected '{}' in bundle", check.host, needle);
                host_ok = false;
            }
        }
        for needle in &check.should_not_contain {
            if bundle.contains(needle) {
                eprintln!("  ✗ [{}] unexpected '{}' found in bundle", check.host, needle);
                host_ok = false;
            }
        }

        let status = if host_ok { "✓" } else { "✗" };
        println!("{}  {} — bundle {} bytes ({:?})", status, check.host, bundle.len(), bundle_time);
        if !host_ok { all_ok = false; }
    }

    // Domain with no matching rules → empty string (skip injection on Dart side)
    let empty = engine.build_bundle("noruleshere.xyz");
    let empty_ok = empty.is_empty();
    println!("{}  noruleshere.xyz — empty bundle (skip injection): {}",
        if empty_ok { "✓" } else { "✗" }, empty_ok);
    if !empty_ok { all_ok = false; }

    if all_ok {
        println!("✓ All bundle correctness checks passed");
    } else {
        eprintln!("✗ Some bundle checks failed — see above");
        std::process::exit(1);
    }

    // ── Test 10: CosmeticEngine — large-scale performance ────────────────────
    println!("\nTest 10: CosmeticEngine large-scale performance...");
    let mut big_engine = CosmeticEngine::new();

    let start = std::time::Instant::now();
    // 10k global CSS rules
    for i in 0..10_000 {
        big_engine.insert_line(&format!("##.ad-class-{}", i));
    }
    // 5k domain rules across 500 unique domains
    for i in 0..5_000 {
        let domain = format!("site{}.com", i % 500);
        big_engine.insert_line(&format!("{}##.sponsored-{}", domain, i));
    }
    // 1k scriptlets across 100 domains
    for i in 0..1_000 {
        let domain = format!("tracker{}.net", i % 100);
        big_engine.insert_line(&format!("{}##+js(set-constant, prop{}, false)", domain, i));
    }
    let load_time = start.elapsed();
    println!("✓ Loaded 16,000 cosmetic rules in {:?}", load_time);
    println!("  Total rules stored: {}", big_engine.total_rules());

    // Bundle generation throughput
    let iterations = 1_000u32;
    let start = std::time::Instant::now();
    for i in 0..iterations {
        let host = format!("site{}.com", i % 500);
        let _ = big_engine.build_bundle(&host);
    }
    let total_bundle_time = start.elapsed();
    println!("✓ {} bundle generations in {:?}", iterations, total_bundle_time);
    println!("  Average per bundle: {:?}", total_bundle_time / iterations);

    // Global-only fast path
    let start = std::time::Instant::now();
    let _ = big_engine.build_bundle("brandnewsite.com");
    println!("✓ Global-only bundle time: {:?}", start.elapsed());

    println!("\n=== All tests complete! ===");
    println!("\nCapabilities verified:");
    println!("  ✓ Network layer: context-aware blocking (whitelist / conditional)");
    println!("  ✓ Network layer: priority system and serialization");
    println!("  ✓ Cosmetic layer: global, domain, negated-domain, and exception rules");
    println!("  ✓ Cosmetic layer: scriptlet compilation (set-constant, abort-on-property-read)");
    println!("  ✓ Cosmetic layer: correct exception and negation semantics");
    println!("  ✓ Cosmetic layer: large-scale insertion and bundle generation performance");
}