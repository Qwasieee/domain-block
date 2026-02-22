use blocker_engine::DomainTree;

fn main() {
    println!("Domain Blocker Library");
    println!("======================\n");

    let mut tree = DomainTree::new();

    // Example usage
    tree.insert("ads.example.com");
    tree.insert("tracker.net");

    println!("Testing domain blocking:");
    println!("  ads.example.com: {}", tree.is_blocked("ads.example.com"));
    println!("  sub.ads.example.com: {}", tree.is_blocked("sub.ads.example.com"));
    println!("  example.com: {}", tree.is_blocked("example.com"));
    println!("  tracker.net: {}", tree.is_blocked("tracker.net"));
    println!("  google.com: {}", tree.is_blocked("google.com"));

    println!("\nLibrary is working correctly!");
    println!("Use build.sh to build for Flutter integration.");
}