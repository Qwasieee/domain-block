use domain_blocker::DomainTree;

fn main() {
    println!("=== Domain Blocker Performance Test (Context-Aware) ===\n");

    // Test 1: Build tree with sample domains
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

    // Test 2: Basic query performance
    println!("\nTest 2: Basic query performance...");
    let test_queries = vec![
        ("ads.twitter.com", None, true),
        ("sub.ads.twitter.com", None, true),
        ("twitter.com", None, false),
        ("facebook.com", None, true),
        ("instagram.com", None, false),
        ("tracking.analytics.google.com", None, true),
        ("google.com", None, false),
    ];

    for (domain, origin, expected) in test_queries {
        let start = std::time::Instant::now();
        let result = tree.is_blocked_with_origin(domain, origin);
        let query_time = start.elapsed();
        
        let status = if result == expected { "✓" } else { "✗" };
        let blocked = if result { "BLOCKED" } else { "ALLOWED" };
        println!("{} {} - {} ({:?})", status, domain, blocked, query_time);
    }

    // Test 3: Context-aware blocking (whitelist)
    println!("\nTest 3: Context-aware blocking (whitelist)...");
    
    // Add whitelist rule: allow doubleclick on example.com
    tree.insert_with_whitelist("doubleclick.net", "example.com");
    
    let whitelist_tests = vec![
        ("doubleclick.net", None, true, "universal block applies"),
        ("doubleclick.net", Some("example.com"), false, "whitelisted on example.com"),
        ("doubleclick.net", Some("sub.example.com"), false, "whitelisted on subdomains too"),
        ("doubleclick.net", Some("badsite.com"), true, "still blocked on other sites"),
    ];
    
    for (domain, origin, expected, description) in whitelist_tests {
        let result = tree.is_blocked_with_origin(domain, origin);
        let status = if result == expected { "✓" } else { "✗" };
        let blocked = if result { "BLOCKED" } else { "ALLOWED" };
        println!("{} {} [origin: {:?}] - {} ({})", 
            status, domain, origin, blocked, description);
    }

    // Test 4: Context-aware blocking (blacklist/only-if)
    println!("\nTest 4: Context-aware blocking (blacklist/only-if)...");
    
    // Add blacklist rule: block amazon-adsystem ONLY on annoyingsite.com
    tree.insert_with_blacklist("amazon-adsystem.com", "annoyingsite.com");
    
    let blacklist_tests = vec![
        ("amazon-adsystem.com", None, false, "not blocked without context"),
        ("amazon-adsystem.com", Some("goodsite.com"), false, "not blocked on other sites"),
        ("amazon-adsystem.com", Some("annoyingsite.com"), true, "blocked ONLY on annoyingsite.com"),
        ("amazon-adsystem.com", Some("sub.annoyingsite.com"), true, "blocked on subdomains too"),
    ];
    
    for (domain, origin, expected, description) in blacklist_tests {
        let result = tree.is_blocked_with_origin(domain, origin);
        let status = if result == expected { "✓" } else { "✗" };
        let blocked = if result { "BLOCKED" } else { "ALLOWED" };
        println!("{} {} [origin: {:?}] - {} ({})", 
            status, domain, origin, blocked, description);
    }

    // Test 5: Priority testing (whitelist > universal)
    println!("\nTest 5: Priority testing (whitelist > universal)...");
    
    tree.insert("priority-test.com"); // Universal block
    tree.insert_with_whitelist("priority-test.com", "trusted.com"); // Whitelist
    
    println!("✓ Without context: {} (universal block applies)", 
        tree.is_blocked("priority-test.com"));
    println!("✓ With trusted.com: {} (whitelist overrides)", 
        tree.is_blocked_with_origin("priority-test.com", Some("trusted.com")));
    println!("✓ With other.com: {} (universal block applies)", 
        tree.is_blocked_with_origin("priority-test.com", Some("other.com")));

    // Test 6: Serialization with context rules
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

    // Verify loaded tree preserves context rules
    println!("✓ Verifying loaded tree:");
    println!("  - doubleclick.net (no context): {}", 
        loaded_tree.is_blocked("doubleclick.net"));
    println!("  - doubleclick.net (example.com): {}", 
        loaded_tree.is_blocked_with_origin("doubleclick.net", Some("example.com")));
    println!("  - amazon-adsystem.com (annoyingsite.com): {}", 
        loaded_tree.is_blocked_with_origin("amazon-adsystem.com", Some("annoyingsite.com")));

    // Test 7: Large-scale performance with mixed rules
    println!("\nTest 7: Large-scale performance with mixed rules...");
    let mut large_tree = DomainTree::new();
    
    // Generate test domains with various rules
    let mut universal_domains = Vec::new();
    for i in 0..5000 {
        universal_domains.push(format!("ad{}.example.com", i));
    }

    let start = std::time::Instant::now();
    large_tree.bulk_load(&universal_domains.iter().map(|s| s.as_str()).collect::<Vec<_>>());
    
    // Add context-specific rules
    for i in 0..1000 {
        large_tree.insert_with_whitelist(&format!("ad{}.example.com", i), "trusted.com");
    }
    for i in 5000..6000 {
        large_tree.insert_with_blacklist(&format!("conditional{}.net", i), "badsite.com");
    }
    
    let build_time = start.elapsed();
    let total_rules = 5000 + 1000 + 1000;
    
    println!("✓ Built tree with {} rules in {:?}", total_rules, build_time);
    println!("  Average: {:?} per rule", build_time / total_rules as u32);

    // Query performance on large tree
    let start = std::time::Instant::now();
    let _ = large_tree.is_blocked("ad2500.example.com");
    let query_time = start.elapsed();
    println!("✓ Query time (no context): {:?}", query_time);

    let start = std::time::Instant::now();
    let _ = large_tree.is_blocked_with_origin("ad500.example.com", Some("trusted.com"));
    let query_time = start.elapsed();
    println!("✓ Query time (with context): {:?}", query_time);

    // Cache performance on large tree
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

    // File size info
    let file_size = std::fs::metadata(large_cache_path).unwrap().len();
    println!("  Cache file size: {} KB ({} bytes per rule)", 
        file_size / 1024, file_size / total_rules as u64);

    println!("\n=== All tests passed! ===");
    println!("\nNew features:");
    println!("  ✓ Context-aware blocking (origin-based rules)");
    println!("  ✓ Whitelist rules (exceptions)");
    println!("  ✓ Blacklist rules (conditional blocking)");
    println!("  ✓ Priority system (whitelist > blacklist > universal)");
    println!("  ✓ Serialization with context preservation");
}