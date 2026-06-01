// cosmetic.rs
//
// CosmeticEngine: parses and stores cosmetic filter rules (##, #@#, ##+js),
// then compiles per-domain JS injection bundles on demand.
//
// Rule types handled:
//   ##.selector          → global CSS hide rule (all pages)
//   #@#.selector         → global CSS exception (rarely used, but valid)
//   example.com##.sel    → domain-specific CSS hide rule
//   example.com#@#.sel   → domain-specific exception (overrides ## rules)
//   ~example.com##.sel   → global rule EXCLUDING this domain
//   example.com##+js(fn, arg) → scriptlet injection on this domain
//
// Lookup semantics:
//   Given host "ads.news.example.com", rules are collected from:
//     global rules (always)
//     + example.com bucket
//     + news.example.com bucket
//     + ads.news.example.com bucket
//   Then exceptions at every level are subtracted.
//   Then negated-domain rules that exclude this host are subtracted.
//
// Bundle output:
//   A single self-contained JS string ready to inject at AT_DOCUMENT_START.
//   Contains: one <style> injection for CSS, then scriptlet calls inline.

use std::collections::{HashMap, HashSet};

// ── Rule storage ─────────────────────────────────────────────────────────────

/// All rules attached to a single domain key.
#[derive(Debug, Default, Clone)]
pub struct DomainBucket {
    /// CSS selectors to hide on this domain (from ##).
    pub css: Vec<String>,
    /// CSS selectors to un-hide on this domain (from #@#).
    /// These override ## rules from any level (global or domain).
    pub exceptions: Vec<String>,
    /// Raw scriptlet calls for this domain (from ##+js).
    /// Each entry is the full argument string, e.g. "set-constant, adblock, false".
    pub scriptlets: Vec<String>,
}

impl DomainBucket {
    fn is_empty(&self) -> bool {
        self.css.is_empty() && self.exceptions.is_empty() && self.scriptlets.is_empty()
    }
}

/// A negated rule: ##.selector that applies globally EXCEPT on specific domains.
/// Example source: `~news.com##.ad` means hide .ad everywhere except news.com.
#[derive(Debug, Clone)]
struct NegatedRule {
    selector: String,
    /// Domains where this rule must NOT apply (plain strings, e.g. "news.com").
    excluded_domains: Vec<String>,
}

// ── Engine ───────────────────────────────────────────────────────────────────

pub struct CosmeticEngine {
    /// Global CSS rules (## with no domain prefix). Applied on every page.
    global_css: Vec<String>,
    /// Global exceptions (#@# with no domain prefix). Rarely used but valid.
    global_exceptions: HashSet<String>,
    /// Per-domain buckets. Key is the plain domain string ("example.com").
    domains: HashMap<String, DomainBucket>,
    /// Global rules that have domain exclusions (~domain##.selector).
    negated: Vec<NegatedRule>,
}

impl CosmeticEngine {
    pub fn new() -> Self {
        CosmeticEngine {
            global_css: Vec::new(),
            global_exceptions: HashSet::new(),
            domains: HashMap::new(),
            negated: Vec::new(),
        }
    }

    // ── Insertion ─────────────────────────────────────────────────────────────

    /// Parse and insert a single cosmetic filter line.
    /// Returns true if the line was recognized and inserted.
    pub fn insert_line(&mut self, line: &str) -> bool {
        if let Some(rule) = parse_cosmetic_line(line) {
            self.insert_parsed(rule);
            true
        } else {
            false
        }
    }

    fn insert_parsed(&mut self, rule: ParsedCosmeticRule) {
        match rule {
            ParsedCosmeticRule::GlobalCss { selector } => {
                self.global_css.push(selector);
            }
            ParsedCosmeticRule::GlobalException { selector } => {
                self.global_exceptions.insert(selector);
            }
            ParsedCosmeticRule::DomainCss { domains, selector } => {
                for domain in domains {
                    self.domains
                        .entry(domain)
                        .or_default()
                        .css
                        .push(selector.clone());
                }
            }
            ParsedCosmeticRule::DomainException { domains, selector } => {
                for domain in domains {
                    self.domains
                        .entry(domain)
                        .or_default()
                        .exceptions
                        .push(selector.clone());
                }
            }
            ParsedCosmeticRule::NegatedDomainCss { excluded_domains, selector } => {
                // Check if an existing NegatedRule for this selector already exists
                // to avoid duplicates from multiple list sources.
                if let Some(existing) = self.negated.iter_mut().find(|r| r.selector == selector) {
                    for d in excluded_domains {
                        if !existing.excluded_domains.contains(&d) {
                            existing.excluded_domains.push(d);
                        }
                    }
                } else {
                    self.negated.push(NegatedRule { selector, excluded_domains });
                }
            }
            ParsedCosmeticRule::Scriptlet { domains, payload } => {
                for domain in domains {
                    self.domains
                        .entry(domain)
                        .or_default()
                        .scriptlets
                        .push(payload.clone());
                }
            }
        }
    }

    // ── Bundle generation ─────────────────────────────────────────────────────

    /// Build a JS injection bundle for the given host.
    /// Returns an empty string if there are no rules for this host.
    /// The returned string is ready to inject at AT_DOCUMENT_START.
    pub fn build_bundle(&self, host: &str) -> String {
        let host = host.to_lowercase();
        let host = host.trim_matches('.');

        // Collect active exceptions first (they win over everything).
        // Walk suffix chain to find all domain-level exceptions.
        let mut active_exceptions: HashSet<&str> = HashSet::new();
        for exc in &self.global_exceptions {
            active_exceptions.insert(exc.as_str());
        }
        for_each_suffix(host, |suffix| {
            if let Some(bucket) = self.domains.get(suffix) {
                for exc in &bucket.exceptions {
                    active_exceptions.insert(exc.as_str());
                }
            }
        });

        // Collect CSS selectors.
        let mut css_selectors: Vec<&str> = Vec::new();

        // 1. Global CSS rules, filtered by exceptions.
        for sel in &self.global_css {
            if !active_exceptions.contains(sel.as_str()) {
                css_selectors.push(sel.as_str());
            }
        }

        // 2. Negated rules: apply if this host is not in the exclusion list.
        for neg in &self.negated {
            if !active_exceptions.contains(neg.selector.as_str())
                && !host_matches_any(host, &neg.excluded_domains)
            {
                css_selectors.push(neg.selector.as_str());
            }
        }

        // 3. Domain-specific CSS rules (suffix chain, most general → most specific).
        let mut scriptlets: Vec<&str> = Vec::new();
        for_each_suffix(host, |suffix| {
            if let Some(bucket) = self.domains.get(suffix) {
                for sel in &bucket.css {
                    if !active_exceptions.contains(sel.as_str()) {
                        css_selectors.push(sel.as_str());
                    }
                }
                for s in &bucket.scriptlets {
                    scriptlets.push(s.as_str());
                }
            }
        });

        if css_selectors.is_empty() && scriptlets.is_empty() {
            return String::new();
        }

        compile_bundle(&css_selectors, &scriptlets)
    }

    // ── Stats ─────────────────────────────────────────────────────────────────

    pub fn total_rules(&self) -> usize {
        let domain_rules: usize = self
            .domains
            .values()
            .map(|b| b.css.len() + b.exceptions.len() + b.scriptlets.len())
            .sum();
        self.global_css.len()
            + self.global_exceptions.len()
            + self.negated.len()
            + domain_rules
    }

    pub fn is_empty(&self) -> bool {
        self.global_css.is_empty()
            && self.global_exceptions.is_empty()
            && self.domains.is_empty()
            && self.negated.is_empty()
    }
}

// ── Suffix chain traversal ────────────────────────────────────────────────────

/// Call `f` with each suffix of `host`, from most general to most specific.
/// Example: "ads.news.example.com" → ["example.com", "news.example.com", "ads.news.example.com"]
fn for_each_suffix<F: FnMut(&str)>(host: &str, mut f: F) {
    let parts: Vec<&str> = host.split('.').collect();
    let len = parts.len();
    // Need at least 2 parts to form a valid domain (e.g. "example.com").
    if len < 2 {
        return;
    }
    // Start from the shortest valid suffix (TLD + 1 label).
    for start in (0..len - 1).rev() {
        let suffix = parts[start..].join(".");
        f(&suffix);
    }
}

/// Returns true if `host` matches any domain in the list (exact or subdomain).
fn host_matches_any(host: &str, domains: &[String]) -> bool {
    for d in domains {
        if host == d.as_str() {
            return true;
        }
        // host is a subdomain of d
        if host.ends_with(d.as_str()) {
            let prefix_len = host.len() - d.len();
            if host.as_bytes().get(prefix_len.wrapping_sub(1)) == Some(&b'.') {
                return true;
            }
        }
    }
    false
}

// ── JS bundle compiler ────────────────────────────────────────────────────────

/// Compile CSS selectors and scriptlet payloads into a single JS string.
/// The JS is wrapped in an IIFE to avoid polluting the global scope.
fn compile_bundle(css_selectors: &[&str], scriptlets: &[&str]) -> String {
    let mut out = String::with_capacity(256 + css_selectors.len() * 32 + scriptlets.len() * 64);

    out.push_str("(function(){");

    // CSS injection block
    if !css_selectors.is_empty() {
        // Deduplicate selectors while preserving order.
        let mut seen = HashSet::new();
        let unique: Vec<&str> = css_selectors
            .iter()
            .copied()
            .filter(|s| seen.insert(*s))
            .collect();

        let joined = unique.join(",");
        out.push_str("var s=document.createElement('style');");
        out.push_str("s.textContent='");
        // Escape the CSS content for embedding in a JS string literal.
        push_js_escaped(&mut out, &joined);
        out.push_str("{display:none!important}';");
        out.push_str("document.documentElement.appendChild(s);");
    }

    // Scriptlet block
    for payload in scriptlets {
        if let Some(js) = compile_scriptlet(payload) {
            out.push_str(&js);
        }
    }

    out.push_str("})();");
    out
}

/// Escape a string for embedding inside a JS single-quoted string literal.
fn push_js_escaped(out: &mut String, s: &str) {
    for ch in s.chars() {
        match ch {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(ch),
        }
    }
}

/// Convert a scriptlet payload string into executable JS.
///
/// Supported scriptlets (the most common uBO/ABP ones):
///   set-constant, <prop>, <value>
///   abort-on-property-read, <prop>
///   abort-on-property-write, <prop>
///   prevent-setTimeout / prevent-setInterval (no args = block all)
///   no-op (identity: does nothing, used to defuse analytics)
///
/// Unknown scriptlets are silently ignored — safe default.
fn compile_scriptlet(payload: &str) -> Option<String> {
    let mut parts = payload.splitn(3, ',').map(str::trim);
    let name = parts.next()?;
    let arg1 = parts.next().unwrap_or("").trim();
    let arg2 = parts.next().unwrap_or("").trim();

    let js = match name {
        "set-constant" => {
            if arg1.is_empty() {
                return None;
            }
            let val = js_primitive_literal(arg2);
            format!(
                "try{{Object.defineProperty(window,{prop},{{value:{val},writable:false}})}}catch(e){{}};",
                prop = js_string_literal(arg1),
                val = val
            )
        }
        "abort-on-property-read" => {
            if arg1.is_empty() {
                return None;
            }
            format!(
                "try{{Object.defineProperty(window,{prop},{{get:function(){{throw new ReferenceError()}}}})}}catch(e){{}};",
                prop = js_string_literal(arg1)
            )
        }
        "abort-on-property-write" => {
            if arg1.is_empty() {
                return None;
            }
            format!(
                "try{{Object.defineProperty(window,{prop},{{set:function(){{throw new ReferenceError()}}}})}}catch(e){{}};",
                prop = js_string_literal(arg1)
            )
        }
        "prevent-setTimeout" => {
            // Patch setTimeout to no-op (blunt but effective for simple cases)
            "window.setTimeout=function(){};".to_string()
        }
        "prevent-setInterval" => {
            "window.setInterval=function(){};".to_string()
        }
        "no-op" | "noop" => {
            // Used to replace tracking scripts with a harmless stub
            "void 0;".to_string()
        }
        _ => return None, // Unknown scriptlet — skip silently
    };

    Some(js)
}

/// Wrap a Rust string as a JS string literal (single-quoted, escaped).
fn js_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    push_js_escaped(&mut out, s);
    out.push('\'');
    out
}

/// Convert a string token to a JS primitive literal.
/// "true", "false", "null", "undefined", and numeric strings are emitted bare.
/// Everything else is emitted as a quoted string.
fn js_primitive_literal(s: &str) -> String {
    match s {
        "true" | "false" | "null" | "undefined" => s.to_string(),
        _ if s.parse::<f64>().is_ok() => s.to_string(),
        _ => js_string_literal(s),
    }
}

// ── ABP cosmetic line parser ──────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
enum ParsedCosmeticRule {
    /// `##selector` — no domain prefix
    GlobalCss { selector: String },
    /// `#@#selector` — global exception
    GlobalException { selector: String },
    /// `domain1,domain2##selector`
    DomainCss { domains: Vec<String>, selector: String },
    /// `domain1,domain2#@#selector`
    DomainException { domains: Vec<String>, selector: String },
    /// `~domain##selector` — applies globally except on listed domains
    NegatedDomainCss { excluded_domains: Vec<String>, selector: String },
    /// `domain##+js(payload)`
    Scriptlet { domains: Vec<String>, payload: String },
}

/// Parse one filter list line into a cosmetic rule.
/// Returns None for non-cosmetic lines or malformed rules.
pub fn parse_cosmetic_line(line: &str) -> Option<ParsedCosmeticRule> {
    // Determine separator and whether it's an exception.
    // Order matters: check ##+js before ##, check #@# before ##.
    let (before_sep, sep, after_sep) = find_cosmetic_separator(line)?;

    let selector = after_sep.trim();
    if selector.is_empty() {
        return None;
    }

    // Scriptlet rule: domain##+js(...)
    if sep == "##" && selector.starts_with("+js(") && selector.ends_with(')') {
        let payload = selector[4..selector.len() - 1].trim().to_string();
        if payload.is_empty() {
            return None;
        }
        if before_sep.is_empty() {
            // Global scriptlet — unusual, skip for safety.
            return None;
        }
        let domains = parse_domain_list(before_sep)?;
        // Negated-only scriptlets don't make sense; require at least one positive domain.
        let positive: Vec<String> = domains.into_iter().filter(|d| !d.starts_with('~')).collect();
        if positive.is_empty() {
            return None;
        }
        return Some(ParsedCosmeticRule::Scriptlet { domains: positive, payload });
    }

    // No domain prefix → global rule.
    if before_sep.is_empty() {
        return Some(if sep == "#@#" {
            ParsedCosmeticRule::GlobalException { selector: selector.to_string() }
        } else {
            ParsedCosmeticRule::GlobalCss { selector: selector.to_string() }
        });
    }

    // Parse domain list (may contain plain and ~negated entries).
    let raw_domains = parse_domain_list(before_sep)?;

    let mut positive: Vec<String> = Vec::new();
    let mut negated: Vec<String> = Vec::new();

    for d in raw_domains {
        if let Some(nd) = d.strip_prefix('~') {
            negated.push(nd.to_lowercase());
        } else {
            positive.push(d.to_lowercase());
        }
    }

    match sep {
        "#@#" => {
            // Exceptions only make sense on positive domains.
            if positive.is_empty() {
                return None;
            }
            Some(ParsedCosmeticRule::DomainException {
                domains: positive,
                selector: selector.to_string(),
            })
        }
        "##" => {
            if positive.is_empty() && !negated.is_empty() {
                // All domains are negated → global rule with exclusions.
                Some(ParsedCosmeticRule::NegatedDomainCss {
                    excluded_domains: negated,
                    selector: selector.to_string(),
                })
            } else if !positive.is_empty() && negated.is_empty() {
                Some(ParsedCosmeticRule::DomainCss {
                    domains: positive,
                    selector: selector.to_string(),
                })
            } else if !positive.is_empty() && !negated.is_empty() {
                // Mixed (positive + negated on same rule) — treat as domain-specific,
                // negated entries ignored. This is an uncommon and ambiguous form.
                Some(ParsedCosmeticRule::DomainCss {
                    domains: positive,
                    selector: selector.to_string(),
                })
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Find the cosmetic separator (##, #@#) in a line.
/// Returns (before_separator, separator, after_separator) or None.
fn find_cosmetic_separator(line: &str) -> Option<(&str, &str, &str)> {
    // Search for #@# first (it's longer and would be misidentified as ## + @...).
    if let Some(pos) = line.find("#@#") {
        return Some((&line[..pos], "#@#", &line[pos + 3..]));
    }
    if let Some(pos) = line.find("##") {
        return Some((&line[..pos], "##", &line[pos + 2..]));
    }
    None
}

/// Parse a comma-separated domain list. Returns None if any entry is clearly invalid.
/// Preserves ~ prefix on negated entries. Does not lowercase here.
fn parse_domain_list(s: &str) -> Option<Vec<String>> {
    let domains: Vec<String> = s
        .split(',')
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .collect();

    if domains.is_empty() {
        return None;
    }

    // Basic validity: each entry (after stripping ~) must contain a dot.
    for d in &domains {
        let bare = d.strip_prefix('~').unwrap_or(d.as_str());
        if !bare.contains('.') {
            return None;
        }
    }

    Some(domains)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Parser tests ──────────────────────────────────────────────────────────

    #[test]
    fn test_parse_global_css() {
        let r = parse_cosmetic_line("##.ad-banner").unwrap();
        assert_eq!(r, ParsedCosmeticRule::GlobalCss { selector: ".ad-banner".to_string() });
    }

    #[test]
    fn test_parse_global_exception() {
        let r = parse_cosmetic_line("#@#.ad-banner").unwrap();
        assert_eq!(r, ParsedCosmeticRule::GlobalException { selector: ".ad-banner".to_string() });
    }

    #[test]
    fn test_parse_domain_css() {
        let r = parse_cosmetic_line("example.com##.ad").unwrap();
        assert_eq!(r, ParsedCosmeticRule::DomainCss {
            domains: vec!["example.com".to_string()],
            selector: ".ad".to_string(),
        });
    }

    #[test]
    fn test_parse_multi_domain() {
        let r = parse_cosmetic_line("a.com,b.com##.ad").unwrap();
        assert_eq!(r, ParsedCosmeticRule::DomainCss {
            domains: vec!["a.com".to_string(), "b.com".to_string()],
            selector: ".ad".to_string(),
        });
    }

    #[test]
    fn test_parse_domain_exception() {
        let r = parse_cosmetic_line("example.com#@#.ad").unwrap();
        assert_eq!(r, ParsedCosmeticRule::DomainException {
            domains: vec!["example.com".to_string()],
            selector: ".ad".to_string(),
        });
    }

    #[test]
    fn test_parse_negated_domain() {
        let r = parse_cosmetic_line("~example.com##.ad").unwrap();
        assert_eq!(r, ParsedCosmeticRule::NegatedDomainCss {
            excluded_domains: vec!["example.com".to_string()],
            selector: ".ad".to_string(),
        });
    }

    #[test]
    fn test_parse_scriptlet() {
        let r = parse_cosmetic_line("example.com##+js(set-constant, adblock, false)").unwrap();
        assert_eq!(r, ParsedCosmeticRule::Scriptlet {
            domains: vec!["example.com".to_string()],
            payload: "set-constant, adblock, false".to_string(),
        });
    }

    #[test]
    fn test_parse_invalid_no_dot() {
        assert!(parse_cosmetic_line("localhost##.ad").is_none());
    }

    #[test]
    fn test_parse_non_cosmetic_line() {
        assert!(parse_cosmetic_line("||ads.com^").is_none());
        assert!(parse_cosmetic_line("! comment").is_none());
        assert!(parse_cosmetic_line("").is_none());
    }

    // ── Suffix traversal tests ────────────────────────────────────────────────

    #[test]
    fn test_suffix_chain_order() {
        let mut suffixes = Vec::new();
        for_each_suffix("ads.news.example.com", |s| suffixes.push(s.to_string()));
        assert_eq!(suffixes, vec![
            "example.com",
            "news.example.com",
            "ads.news.example.com",
        ]);
    }

    #[test]
    fn test_suffix_chain_bare_domain() {
        let mut suffixes = Vec::new();
        for_each_suffix("example.com", |s| suffixes.push(s.to_string()));
        assert_eq!(suffixes, vec!["example.com"]);
    }

    #[test]
    fn test_suffix_chain_tld_only_skipped() {
        let mut suffixes = Vec::new();
        for_each_suffix("com", |s| suffixes.push(s.to_string()));
        assert!(suffixes.is_empty());
    }

    // ── Engine integration tests ──────────────────────────────────────────────

    #[test]
    fn test_global_rule_applies_everywhere() {
        let mut engine = CosmeticEngine::new();
        engine.insert_line("##.ad-banner");
        let bundle = engine.build_bundle("any.site.com");
        assert!(bundle.contains(".ad-banner"));
    }

    #[test]
    fn test_domain_rule_applies_to_subdomain() {
        let mut engine = CosmeticEngine::new();
        engine.insert_line("example.com##.sponsored");
        assert!(engine.build_bundle("example.com").contains(".sponsored"));
        assert!(engine.build_bundle("sub.example.com").contains(".sponsored"));
        assert!(!engine.build_bundle("other.com").contains(".sponsored"));
    }

    #[test]
    fn test_exception_overrides_global() {
        let mut engine = CosmeticEngine::new();
        engine.insert_line("##.ad-banner");
        engine.insert_line("example.com#@#.ad-banner");
        // Exception on example.com — selector should not appear in bundle.
        assert!(!engine.build_bundle("example.com").contains(".ad-banner"));
        // But still applies everywhere else.
        assert!(engine.build_bundle("other.com").contains(".ad-banner"));
    }

    #[test]
    fn test_negated_domain_rule() {
        let mut engine = CosmeticEngine::new();
        engine.insert_line("~example.com##.ad");
        // Should apply to other sites.
        assert!(engine.build_bundle("other.com").contains(".ad"));
        // Should NOT apply to example.com.
        assert!(!engine.build_bundle("example.com").contains(".ad"));
        // Should NOT apply to subdomain of example.com.
        assert!(!engine.build_bundle("sub.example.com").contains(".ad"));
    }

    #[test]
    fn test_scriptlet_set_constant() {
        let mut engine = CosmeticEngine::new();
        engine.insert_line("example.com##+js(set-constant, adblock, false)");
        let bundle = engine.build_bundle("example.com");
        assert!(bundle.contains("Object.defineProperty"));
        assert!(bundle.contains("adblock"));
        assert!(!engine.build_bundle("other.com").contains("adblock"));
    }

    #[test]
    fn test_empty_bundle_for_unmatched_host() {
        let mut engine = CosmeticEngine::new();
        engine.insert_line("example.com##.ad");
        assert_eq!(engine.build_bundle("other.com"), "");
    }

    #[test]
    fn test_no_duplicate_selectors_in_bundle() {
        let mut engine = CosmeticEngine::new();
        // Same selector from two sources.
        engine.insert_line("##.ad");
        engine.insert_line("example.com##.ad");
        let bundle = engine.build_bundle("example.com");
        // .ad should appear exactly once in the CSS blob.
        let count = bundle.matches(".ad").count();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_host_matches_any() {
        let domains = vec!["example.com".to_string()];
        assert!(host_matches_any("example.com", &domains));
        assert!(host_matches_any("sub.example.com", &domains));
        assert!(!host_matches_any("notexample.com", &domains));
        assert!(!host_matches_any("fakeexample.com", &domains));
    }

    #[test]
    fn test_compile_scriptlet_set_constant_bool() {
        let js = compile_scriptlet("set-constant, adblock, false").unwrap();
        assert!(js.contains("adblock"));
        assert!(js.contains("false"));
        assert!(js.contains("Object.defineProperty"));
    }

    #[test]
    fn test_compile_scriptlet_unknown_ignored() {
        assert!(compile_scriptlet("unknown-scriptlet, arg").is_none());
    }

    #[test]
    fn test_global_exception_suppresses_global_rule() {
        let mut engine = CosmeticEngine::new();
        engine.insert_line("##.ad");
        engine.insert_line("#@#.ad");
        // Global exception — should be gone everywhere.
        assert!(!engine.build_bundle("any.com").contains(".ad"));
    }
}