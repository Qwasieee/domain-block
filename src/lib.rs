pub mod filter_list;

use std::collections::HashMap;
use std::ffi::CStr;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::os::raw::c_char;

// ============================================================================
// Resource Type Bitmask
// ============================================================================

/// Bitmask constants for ABP resource type modifiers.
/// A mask value of 0 means "all resource types" (no restriction).
pub mod resource_type {
    pub const SCRIPT: u16      = 1 << 0;
    pub const IMAGE: u16       = 1 << 1;
    pub const STYLESHEET: u16  = 1 << 2;
    pub const XHR: u16         = 1 << 3; // xmlhttprequest
    pub const MEDIA: u16       = 1 << 4;
    pub const FONT: u16        = 1 << 5;
    pub const DOCUMENT: u16    = 1 << 6;
    pub const SUBDOCUMENT: u16 = 1 << 7;
    pub const OTHER: u16       = 1 << 8;
    /// Sentinel: means "apply to any resource type" (no mask restriction).
    pub const ALL: u16         = 0;
}

// ============================================================================
// Rule model
// ============================================================================

/// What a rule does when it matches.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Block,
    Allow, // Exception / whitelist
}

/// Origin constraint for a rule.
#[derive(Debug, Clone)]
pub enum OriginConstraint {
    /// Applies to all origins (no $domain= present).
    Any,
    /// Block/allow ONLY when origin matches one of these.
    OnlyOn(Vec<String>),
    /// Block/allow when origin does NOT match any of these (negated domain=).
    ExceptOn(Vec<String>),
}

/// A single filter rule attached to a trie node.
#[derive(Debug, Clone)]
pub struct Rule {
    pub action: Action,
    pub origin_constraint: OriginConstraint,
    /// Bitmask of resource types this rule applies to. 0 = any.
    pub resource_mask: u16,
    /// If true, this rule only fires for third-party requests.
    pub third_party_only: bool,
}

impl Rule {
    /// Returns true if this rule fires given the request context.
    fn matches_context(
        &self,
        origin: Option<&str>, // already reversed
        resource_mask: u16,
        is_third_party: bool,
    ) -> bool {
        // Third-party gate
        if self.third_party_only && !is_third_party {
            return false;
        }

        // Resource type gate: both sides must have a specific mask for the
        // check to be meaningful. If either is ALL (0), skip the check.
        if self.resource_mask != resource_type::ALL && resource_mask != resource_type::ALL {
            if self.resource_mask & resource_mask == 0 {
                return false;
            }
        }

        // Origin constraint gate
        match &self.origin_constraint {
            OriginConstraint::Any => true,
            OriginConstraint::OnlyOn(list) => {
                origin.map_or(false, |o| Self::origin_in_list(o, list))
            }
            OriginConstraint::ExceptOn(list) => {
                origin.map_or(true, |o| !Self::origin_in_list(o, list))
            }
        }
    }

    fn origin_in_list(reversed_origin: &str, reversed_patterns: &[String]) -> bool {
        for pattern in reversed_patterns {
            if reversed_origin == pattern.as_str() {
                return true;
            }
            if reversed_origin.starts_with(pattern.as_str()) {
                let rest = &reversed_origin[pattern.len()..];
                if rest.starts_with('.') {
                    return true;
                }
            }
        }
        false
    }
}

// ============================================================================
// Radix tree node
// ============================================================================

#[derive(Debug)]
struct RadixNode {
    prefix: String,
    /// All rules whose domain terminates at this node.
    rules: Vec<Rule>,
    children: HashMap<char, Box<RadixNode>>,
}

impl RadixNode {
    fn new(prefix: String) -> Self {
        RadixNode { prefix, rules: Vec::new(), children: HashMap::new() }
    }
}

// ============================================================================
// DomainTree
// ============================================================================

pub struct DomainTree {
    root: RadixNode,
}

impl DomainTree {
    pub fn new() -> Self {
        DomainTree { root: RadixNode::new(String::new()) }
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn reverse_domain(domain: &str) -> String {
        domain.split('.').rev().collect::<Vec<_>>().join(".")
    }

    fn reverse_origins(origins: &[String]) -> Vec<String> {
        origins.iter().map(|o| Self::reverse_domain(o)).collect()
    }

    fn common_prefix_len(s1: &str, s2: &str) -> usize {
        s1.chars().zip(s2.chars()).take_while(|(a, b)| a == b).count()
    }

    // ── Public insert API ────────────────────────────────────────────────────

    /// Universal block rule (backward-compatible).
    pub fn insert(&mut self, domain: &str) -> bool {
        self.insert_rule(domain, Rule {
            action: Action::Block,
            origin_constraint: OriginConstraint::Any,
            resource_mask: resource_type::ALL,
            third_party_only: false,
        })
    }

    /// Whitelist rule for a specific origin (backward-compatible).
    pub fn insert_with_whitelist(&mut self, domain: &str, origin: &str) -> bool {
        if origin.is_empty() { return false; }
        self.insert_rule(domain, Rule {
            action: Action::Allow,
            origin_constraint: OriginConstraint::OnlyOn(vec![Self::reverse_domain(origin)]),
            resource_mask: resource_type::ALL,
            third_party_only: false,
        })
    }

    /// "Block only on this origin" rule (backward-compatible).
    pub fn insert_with_blacklist(&mut self, domain: &str, origin: &str) -> bool {
        if origin.is_empty() { return false; }
        self.insert_rule(domain, Rule {
            action: Action::Block,
            origin_constraint: OriginConstraint::OnlyOn(vec![Self::reverse_domain(origin)]),
            resource_mask: resource_type::ALL,
            third_party_only: false,
        })
    }

    /// Full-featured insert with resource type and third-party support.
    /// Origins inside `origin_constraint` should be plain domain strings
    /// (not yet reversed); this function reverses them internally.
    pub fn insert_with_modifiers(
        &mut self,
        domain: &str,
        action: Action,
        origin_constraint: OriginConstraint,
        resource_mask: u16,
        third_party_only: bool,
    ) -> bool {
        let reversed_constraint = match origin_constraint {
            OriginConstraint::Any => OriginConstraint::Any,
            OriginConstraint::OnlyOn(o) => OriginConstraint::OnlyOn(Self::reverse_origins(&o)),
            OriginConstraint::ExceptOn(o) => OriginConstraint::ExceptOn(Self::reverse_origins(&o)),
        };
        self.insert_rule(domain, Rule {
            action,
            origin_constraint: reversed_constraint,
            resource_mask,
            third_party_only,
        })
    }

    fn insert_rule(&mut self, domain: &str, rule: Rule) -> bool {
        if domain.is_empty() { return false; }
        let reversed = Self::reverse_domain(domain);
        self.insert_reversed(&reversed, rule)
    }

    fn insert_reversed(&mut self, reversed: &str, rule: Rule) -> bool {
        let mut current = &mut self.root;
        let mut remaining = reversed;

        loop {
            if remaining.is_empty() {
                current.rules.push(rule);
                return true;
            }

            let first_char = remaining.chars().next().unwrap();

            let needs_split = if let Some(child) = current.children.get(&first_char) {
                let common_len = Self::common_prefix_len(&child.prefix, remaining);
                common_len > 0 && common_len < child.prefix.len()
            } else {
                false
            };

            if needs_split {
                let (prefix, common_len, new_suffix_str) = {
                    let child = current.children.get(&first_char).unwrap();
                    let prefix = child.prefix.clone();
                    let common_len = Self::common_prefix_len(&prefix, remaining);
                    (prefix.clone(), common_len, remaining[common_len..].to_string())
                };

                let common = prefix[..common_len].to_string();
                let old_suffix = prefix[common_len..].to_string();
                let new_suffix = if !new_suffix_str.is_empty() { Some(new_suffix_str) } else { None };

                let child = current.children.get_mut(&first_char).unwrap();
                let mut intermediate = RadixNode::new(common);

                let old_node = std::mem::replace(child.as_mut(), RadixNode::new(String::new()));
                let old_first_char = old_suffix.chars().next().unwrap();
                let mut old_child = Box::new(old_node);
                old_child.prefix = old_suffix;
                intermediate.children.insert(old_first_char, old_child);

                if let Some(new_suffix) = new_suffix {
                    let new_first_char = new_suffix.chars().next().unwrap();
                    let mut new_node = RadixNode::new(new_suffix);
                    new_node.rules.push(rule);
                    intermediate.children.insert(new_first_char, Box::new(new_node));
                } else {
                    intermediate.rules.push(rule);
                }

                *child.as_mut() = intermediate;
                return true;
            }

            if current.children.contains_key(&first_char) {
                let child = current.children.get_mut(&first_char).unwrap();
                let common_len = Self::common_prefix_len(&child.prefix, remaining);
                if common_len == 0 { return false; }
                if common_len == child.prefix.len() {
                    remaining = &remaining[common_len..];
                    current = child;
                } else {
                    unreachable!();
                }
            } else {
                let mut new_node = RadixNode::new(remaining.to_string());
                new_node.rules.push(rule);
                current.children.insert(first_char, Box::new(new_node));
                return true;
            }
        }
    }

    // ── Public query API ──────────────────────────────────────────────────────

    pub fn is_blocked(&self, domain: &str) -> bool {
        self.is_blocked_ex(domain, None, resource_type::ALL, false)
    }

    pub fn is_blocked_with_origin(&self, domain: &str, origin: Option<&str>) -> bool {
        let is_third_party = origin.map_or(false, |o| {
            // Simple third-party check: origin does not end with the request domain
            // (and isn't equal to it). Proper eTLD+1 matching requires a public
            // suffix list; this is a good-enough approximation.
            !o.ends_with(domain) && o != domain
        });
        self.is_blocked_ex(domain, origin, resource_type::ALL, is_third_party)
    }

    /// Full check with all modifiers.
    pub fn is_blocked_ex(
        &self,
        domain: &str,
        origin: Option<&str>,
        resource_mask: u16,
        is_third_party: bool,
    ) -> bool {
        if domain.is_empty() { return false; }
        let reversed = Self::reverse_domain(domain);
        let reversed_origin = origin.map(Self::reverse_domain);
        self.is_blocked_reversed(&reversed, reversed_origin.as_deref(), resource_mask, is_third_party)
    }

    fn is_blocked_reversed(
        &self,
        reversed: &str,
        origin: Option<&str>,
        resource_mask: u16,
        is_third_party: bool,
    ) -> bool {
        let mut current = &self.root;
        let mut remaining = reversed;
        let mut blocked = false;

        loop {
            // Evaluate rules at this node.
            // Allow (whitelist) wins immediately; Block is noted and continues
            // in case a deeper node has a more-specific Allow.
            for rule in &current.rules {
                if rule.matches_context(origin, resource_mask, is_third_party) {
                    match rule.action {
                        Action::Allow => return false,
                        Action::Block => blocked = true,
                    }
                }
            }

            if remaining.is_empty() {
                return blocked;
            }

            let first_char = remaining.chars().next().unwrap();

            if let Some(child) = current.children.get(&first_char) {
                if remaining.starts_with(child.prefix.as_str()) {
                    remaining = &remaining[child.prefix.len()..];
                    current = child;
                } else {
                    return blocked;
                }
            } else {
                return blocked;
            }
        }
    }

    // ── Serialization: DOMTREE3 ───────────────────────────────────────────────

    pub fn save_to_file(&self, path: &str) -> Result<(), std::io::Error> {
        let file = File::create(path)?;
        let mut w = BufWriter::new(file);
        w.write_all(b"DOMTREE3")?;
        self.serialize_node(&self.root, &mut w)?;
        w.flush()?;
        Ok(())
    }

    fn serialize_node<W: Write>(&self, node: &RadixNode, w: &mut W) -> Result<(), std::io::Error> {
        let pb = node.prefix.as_bytes();
        w.write_all(&(pb.len() as u32).to_le_bytes())?;
        w.write_all(pb)?;

        w.write_all(&(node.rules.len() as u32).to_le_bytes())?;
        for rule in &node.rules {
            self.serialize_rule(rule, w)?;
        }

        w.write_all(&(node.children.len() as u32).to_le_bytes())?;
        for (key, child) in &node.children {
            w.write_all(&(*key as u32).to_le_bytes())?;
            self.serialize_node(child, w)?;
        }
        Ok(())
    }

    fn serialize_rule<W: Write>(&self, rule: &Rule, w: &mut W) -> Result<(), std::io::Error> {
        w.write_all(&[match rule.action { Action::Block => 0, Action::Allow => 1 }])?;
        w.write_all(&rule.resource_mask.to_le_bytes())?;
        w.write_all(&[if rule.third_party_only { 1 } else { 0 }])?;
        match &rule.origin_constraint {
            OriginConstraint::Any => { w.write_all(&[0])?; }
            OriginConstraint::OnlyOn(list) => { w.write_all(&[1])?; self.serialize_string_vec(list, w)?; }
            OriginConstraint::ExceptOn(list) => { w.write_all(&[2])?; self.serialize_string_vec(list, w)?; }
        }
        Ok(())
    }

    fn serialize_string_vec<W: Write>(&self, list: &[String], w: &mut W) -> Result<(), std::io::Error> {
        w.write_all(&(list.len() as u32).to_le_bytes())?;
        for s in list {
            let b = s.as_bytes();
            w.write_all(&(b.len() as u32).to_le_bytes())?;
            w.write_all(b)?;
        }
        Ok(())
    }

    pub fn load_from_file(path: &str) -> Result<Self, std::io::Error> {
        let file = File::open(path)?;
        let mut r = BufReader::new(file);
        let mut header = [0u8; 8];
        r.read_exact(&mut header)?;
        if &header != b"DOMTREE3" {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Incompatible cache format (expected DOMTREE3); rebuild the tree",
            ));
        }
        let root = Self::deserialize_node(&mut r)?;
        Ok(DomainTree { root })
    }

    fn deserialize_node<R: Read>(r: &mut R) -> Result<RadixNode, std::io::Error> {
        let mut lb = [0u8; 4];

        r.read_exact(&mut lb)?;
        let mut prefix_bytes = vec![0u8; u32::from_le_bytes(lb) as usize];
        r.read_exact(&mut prefix_bytes)?;
        let prefix = String::from_utf8(prefix_bytes)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid UTF-8 in prefix"))?;

        r.read_exact(&mut lb)?;
        let rules_count = u32::from_le_bytes(lb) as usize;
        let mut rules = Vec::with_capacity(rules_count);
        for _ in 0..rules_count {
            rules.push(Self::deserialize_rule(r)?);
        }

        r.read_exact(&mut lb)?;
        let children_count = u32::from_le_bytes(lb) as usize;
        let mut children = HashMap::new();
        for _ in 0..children_count {
            let mut kb = [0u8; 4];
            r.read_exact(&mut kb)?;
            let key = char::from_u32(u32::from_le_bytes(kb))
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid char key"))?;
            children.insert(key, Box::new(Self::deserialize_node(r)?));
        }

        Ok(RadixNode { prefix, rules, children })
    }

    fn deserialize_rule<R: Read>(r: &mut R) -> Result<Rule, std::io::Error> {
        let mut b1 = [0u8; 1];
        let mut b2 = [0u8; 2];

        r.read_exact(&mut b1)?;
        let action = match b1[0] {
            0 => Action::Block,
            1 => Action::Allow,
            _ => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid action byte")),
        };

        r.read_exact(&mut b2)?;
        let resource_mask = u16::from_le_bytes(b2);

        r.read_exact(&mut b1)?;
        let third_party_only = b1[0] != 0;

        r.read_exact(&mut b1)?;
        let origin_constraint = match b1[0] {
            0 => OriginConstraint::Any,
            1 => OriginConstraint::OnlyOn(Self::deserialize_string_vec(r)?),
            2 => OriginConstraint::ExceptOn(Self::deserialize_string_vec(r)?),
            _ => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid origin constraint byte")),
        };

        Ok(Rule { action, origin_constraint, resource_mask, third_party_only })
    }

    fn deserialize_string_vec<R: Read>(r: &mut R) -> Result<Vec<String>, std::io::Error> {
        let mut lb = [0u8; 4];
        r.read_exact(&mut lb)?;
        let count = u32::from_le_bytes(lb) as usize;
        let mut list = Vec::with_capacity(count);
        for _ in 0..count {
            r.read_exact(&mut lb)?;
            let mut bytes = vec![0u8; u32::from_le_bytes(lb) as usize];
            r.read_exact(&mut bytes)?;
            list.push(String::from_utf8(bytes)
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid UTF-8"))?);
        }
        Ok(list)
    }

    // ── Utility ───────────────────────────────────────────────────────────────

    pub fn bulk_load(&mut self, domains: &[&str]) {
        for d in domains { self.insert(d); }
    }

    pub fn count_blocked(&self) -> usize {
        self.count_rules_node(&self.root)
    }

    fn count_rules_node(&self, node: &RadixNode) -> usize {
        node.rules.len() + node.children.values().map(|c| self.count_rules_node(c)).sum::<usize>()
    }

    pub fn get_all_blocked(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.collect_domains(&self.root, String::new(), &mut out);
        out
    }

    fn collect_domains(&self, node: &RadixNode, path: String, out: &mut Vec<String>) {
        let cur = format!("{}{}", path, node.prefix);
        if !node.rules.is_empty() {
            out.push(cur.split('.').rev().collect::<Vec<_>>().join("."));
        }
        for child in node.children.values() {
            self.collect_domains(child, cur.clone(), out);
        }
    }
}

// ============================================================================
// FFI Interface — blocker_engine_*
//
// All public functionality is routed through FilterListManager, which owns the
// DomainTree. The old domain_tree_* symbols are gone; callers must migrate to
// these blocker_engine_* equivalents.
//
// Lifetime contract:
//   blocker_engine_new        → returns opaque *mut BlockerEngine handle
//   blocker_engine_free       → must be called exactly once to release memory
//   All other functions       → require a non-null handle
// ============================================================================

use crate::filter_list::FilterListManager;

// Type alias so the rest of the FFI code reads naturally.
type BlockerEngine = FilterListManager;

// ── Lifecycle ─────────────────────────────────────────────────────────────────

/// Create a new BlockerEngine backed by `storage_dir`.
/// The directory is created if it does not exist.
/// Returns an opaque handle; free with `blocker_engine_free`.
#[no_mangle]
pub extern "C" fn blocker_engine_new(
    storage_dir: *const c_char,
) -> *mut BlockerEngine {
    if storage_dir.is_null() {
        return std::ptr::null_mut();
    }
    let dir_str = unsafe {
        match CStr::from_ptr(storage_dir).to_str() {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        }
    };
    Box::into_raw(Box::new(BlockerEngine::new(dir_str)))
}

/// Free a handle returned by `blocker_engine_new`.
#[no_mangle]
pub extern "C" fn blocker_engine_free(engine: *mut BlockerEngine) {
    if !engine.is_null() {
        unsafe { let _ = Box::from_raw(engine); }
    }
}

// ── Tree management ───────────────────────────────────────────────────────────

/// Rebuild the active tree from all enabled filter lists stored on disk.
/// Call once after creating the engine to apply previously saved lists.
/// Returns the total rule count (0 if no lists or on failure).
#[no_mangle]
pub extern "C" fn blocker_engine_rebuild(
    engine: *mut BlockerEngine,
    progress: crate::filter_list::ProgressCallback,
) -> usize {
    if engine.is_null() { return 0; }
    unsafe {
        match (*engine).rebuild_tree(progress) {
            Ok(count) => count,
            Err(_) => 0,
        }
    }
}

// ── Filter list management ────────────────────────────────────────────────────

/// Download and install a new filter list from `url` under the given `name`.
/// `name` must be unique; returns 0 if it already exists or on failure.
/// Returns the number of rules loaded.
#[no_mangle]
pub extern "C" fn blocker_engine_install(
    engine: *mut BlockerEngine,
    name: *const c_char,
    url: *const c_char,
    progress: crate::filter_list::ProgressCallback,
) -> usize {
    if engine.is_null() || name.is_null() || url.is_null() { return 0; }
    unsafe {
        let name_str = match CStr::from_ptr(name).to_str() { Ok(s) => s, Err(_) => return 0 };
        let url_str  = match CStr::from_ptr(url).to_str()  { Ok(s) => s, Err(_) => return 0 };
        match (*engine).install(name_str, url_str, progress) {
            Ok(count) => count,
            Err(_) => 0,
        }
    }
}

/// Re-download a single filter list (conditional GET, re-parse).
/// Returns rule count, or 0 on failure.
#[no_mangle]
pub extern "C" fn blocker_engine_update(
    engine: *mut BlockerEngine,
    name: *const c_char,
    progress: crate::filter_list::ProgressCallback,
) -> usize {
    if engine.is_null() || name.is_null() { return 0; }
    unsafe {
        let name_str = match CStr::from_ptr(name).to_str() { Ok(s) => s, Err(_) => return 0 };
        match (*engine).update(name_str, progress) {
            Ok(count) => count,
            Err(_) => 0,
        }
    }
}

/// Update all installed filter lists.
/// Returns total rule count across all lists, or 0 on failure.
#[no_mangle]
pub extern "C" fn blocker_engine_update_all(
    engine: *mut BlockerEngine,
    progress: crate::filter_list::ProgressCallback,
) -> usize {
    if engine.is_null() { return 0; }
    unsafe {
        match (*engine).update_all(progress) {
            Ok(count) => count,
            Err(_) => 0,
        }
    }
}

/// Remove a filter list entirely (deletes raw file, rebuilds tree).
/// Returns true on success.
#[no_mangle]
pub extern "C" fn blocker_engine_remove(
    engine: *mut BlockerEngine,
    name: *const c_char,
    progress: crate::filter_list::ProgressCallback,
) -> bool {
    if engine.is_null() || name.is_null() { return false; }
    unsafe {
        let name_str = match CStr::from_ptr(name).to_str() { Ok(s) => s, Err(_) => return false };
        (*engine).remove(name_str, progress).is_ok()
    }
}

/// Enable or disable a filter list, rebuilding the tree if changed.
/// Returns true on success.
#[no_mangle]
pub extern "C" fn blocker_engine_set_enabled(
    engine: *mut BlockerEngine,
    name: *const c_char,
    enabled: bool,
    progress: crate::filter_list::ProgressCallback,
) -> bool {
    if engine.is_null() || name.is_null() { return false; }
    unsafe {
        let name_str = match CStr::from_ptr(name).to_str() { Ok(s) => s, Err(_) => return false };
        (*engine).set_enabled(name_str, enabled, progress).is_ok()
    }
}

// ── Direct domain management ──────────────────────────────────────────────────
//
// These replace the old domain_tree_insert / domain_tree_bulk_load family.
// Rules inserted this way live in the engine's tree alongside parsed filter
// list rules but are NOT persisted to any list file — they survive only until
// the next rebuild_tree() call. For permanent rules, install a filter list.

/// Insert a universal block rule for `domain` and all its subdomains.
/// Returns true if the rule was added.
#[no_mangle]
pub extern "C" fn blocker_engine_insert(
    engine: *mut BlockerEngine,
    domain: *const c_char,
) -> bool {
    if engine.is_null() || domain.is_null() { return false; }
    unsafe {
        let d = match CStr::from_ptr(domain).to_str() { Ok(s) => s, Err(_) => return false };
        (*engine).tree.insert(d)
    }
}

/// Insert a whitelist (allow) rule: do NOT block `domain` when loaded from `origin`.
/// Returns true if the rule was added.
#[no_mangle]
pub extern "C" fn blocker_engine_insert_with_whitelist(
    engine: *mut BlockerEngine,
    domain: *const c_char,
    origin: *const c_char,
) -> bool {
    if engine.is_null() || domain.is_null() || origin.is_null() { return false; }
    unsafe {
        let d = match CStr::from_ptr(domain).to_str() { Ok(s) => s, Err(_) => return false };
        let o = match CStr::from_ptr(origin).to_str() { Ok(s) => s, Err(_) => return false };
        (*engine).tree.insert_with_whitelist(d, o)
    }
}

/// Insert a context block rule: block `domain` ONLY when loaded from `origin`.
/// Returns true if the rule was added.
#[no_mangle]
pub extern "C" fn blocker_engine_insert_with_blacklist(
    engine: *mut BlockerEngine,
    domain: *const c_char,
    origin: *const c_char,
) -> bool {
    if engine.is_null() || domain.is_null() || origin.is_null() { return false; }
    unsafe {
        let d = match CStr::from_ptr(domain).to_str() { Ok(s) => s, Err(_) => return false };
        let o = match CStr::from_ptr(origin).to_str() { Ok(s) => s, Err(_) => return false };
        (*engine).tree.insert_with_blacklist(d, o)
    }
}

/// Bulk-insert multiple universal block rules.
/// `domains` is a C array of `count` null-terminated strings.
/// Returns true on success (individual bad entries are silently skipped).
#[no_mangle]
pub extern "C" fn blocker_engine_bulk_insert(
    engine: *mut BlockerEngine,
    domains: *const *const c_char,
    count: usize,
) -> bool {
    if engine.is_null() || domains.is_null() { return false; }
    unsafe {
        for &dp in std::slice::from_raw_parts(domains, count) {
            if dp.is_null() { continue; }
            if let Ok(s) = CStr::from_ptr(dp).to_str() {
                (*engine).tree.insert(s);
            }
        }
    }
    true
}

// ── Query ─────────────────────────────────────────────────────────────────────

/// Check if `domain` is blocked (no origin context).
#[no_mangle]
pub extern "C" fn blocker_engine_is_blocked(
    engine: *const BlockerEngine,
    domain: *const c_char,
) -> bool {
    if engine.is_null() || domain.is_null() { return false; }
    unsafe {
        let d = match CStr::from_ptr(domain).to_str() { Ok(s) => s, Err(_) => return false };
        (*engine).is_blocked(d)
    }
}

/// Check if `domain` is blocked given an `origin` page context.
/// Pass NULL for origin to check without context.
#[no_mangle]
pub extern "C" fn blocker_engine_is_blocked_with_origin(
    engine: *const BlockerEngine,
    domain: *const c_char,
    origin: *const c_char,
) -> bool {
    if engine.is_null() || domain.is_null() { return false; }
    unsafe {
        let d = match CStr::from_ptr(domain).to_str() { Ok(s) => s, Err(_) => return false };
        let o = if origin.is_null() { None } else {
            match CStr::from_ptr(origin).to_str() { Ok(s) => Some(s), Err(_) => None }
        };
        (*engine).is_blocked_with_origin(d, o)
    }
}

/// Extended check with resource type bitmask and explicit third-party flag.
/// `resource_mask`: OR of `resource_type::*` constants (0 = any resource type).
/// `is_third_party`: true when the request crosses an eTLD+1 boundary.
#[no_mangle]
pub extern "C" fn blocker_engine_is_blocked_ex(
    engine: *const BlockerEngine,
    domain: *const c_char,
    origin: *const c_char,
    resource_mask: u16,
    is_third_party: bool,
) -> bool {
    if engine.is_null() || domain.is_null() { return false; }
    unsafe {
        let d = match CStr::from_ptr(domain).to_str() { Ok(s) => s, Err(_) => return false };
        let o = if origin.is_null() { None } else {
            match CStr::from_ptr(origin).to_str() { Ok(s) => Some(s), Err(_) => None }
        };
        (*engine).is_blocked_ex(d, o, resource_mask, is_third_party)
    }
}

// ── Stats & enumeration ───────────────────────────────────────────────────────

/// Total number of rules currently in the active tree.
#[no_mangle]
pub extern "C" fn blocker_engine_total_rules(
    engine: *const BlockerEngine,
) -> usize {
    if engine.is_null() { return 0; }
    unsafe { (*engine).total_rule_count() }
}

/// Count of all blocked-domain entries in the tree (all rule types).
/// Equivalent to the old `domain_tree_count_blocked`.
#[no_mangle]
pub extern "C" fn blocker_engine_count_blocked(
    engine: *const BlockerEngine,
) -> usize {
    if engine.is_null() { return 0; }
    unsafe { (*engine).tree.count_blocked() }
}

/// Return all blocked domains as a null-terminated C string array.
/// `*out_count` is set to the number of strings.
/// The caller must free the array with `blocker_engine_free_string_array`.
#[no_mangle]
pub extern "C" fn blocker_engine_get_all_blocked(
    engine: *const BlockerEngine,
    out_domains: *mut *mut c_char,
    out_count: *mut usize,
) -> bool {
    if engine.is_null() || out_domains.is_null() || out_count.is_null() { return false; }
    unsafe {
        let domains = (*engine).tree.get_all_blocked();
        let count = domains.len();
        let c_strings: Vec<*mut c_char> = domains
            .into_iter()
            .map(|s| std::ffi::CString::new(s).unwrap().into_raw())
            .collect();
        let boxed = c_strings.into_boxed_slice();
        *out_domains = Box::into_raw(boxed) as *mut c_char;
        *out_count = count;
        true
    }
}

/// Free a string array returned by `blocker_engine_get_all_blocked`.
#[no_mangle]
pub extern "C" fn blocker_engine_free_string_array(
    domains: *mut *mut c_char,
    count: usize,
) {
    if domains.is_null() { return; }
    unsafe {
        let slice = std::slice::from_raw_parts_mut(domains, count);
        for &ptr in slice.iter() {
            if !ptr.is_null() { let _ = std::ffi::CString::from_raw(ptr); }
        }
        let _ = Box::from_raw(std::slice::from_raw_parts_mut(domains, count));
    }
}

// ── Metadata accessors ────────────────────────────────────────────────────────

/// Returns metadata for a single installed list as a JSON C string.
/// Returns NULL if not found. Free with `blocker_engine_free_string`.
#[no_mangle]
pub extern "C" fn blocker_engine_get_list_info(
    engine: *const BlockerEngine,
    name: *const c_char,
) -> *mut c_char {
    if engine.is_null() || name.is_null() { return std::ptr::null_mut(); }
    unsafe {
        let name_str = match CStr::from_ptr(name).to_str() { Ok(s) => s, Err(_) => return std::ptr::null_mut() };
        match (*engine).get_list_info(name_str) {
            Some(meta) => crate::filter_list::metadata_to_json_cstring(meta).unwrap_or(std::ptr::null_mut()),
            None => std::ptr::null_mut(),
        }
    }
}

/// Returns all installed lists as a JSON array C string.
/// Returns NULL on failure. Free with `blocker_engine_free_string`.
#[no_mangle]
pub extern "C" fn blocker_engine_get_all_lists(
    engine: *const BlockerEngine,
) -> *mut c_char {
    if engine.is_null() { return std::ptr::null_mut(); }
    unsafe {
        let lists = (*engine).get_all_lists();
        crate::filter_list::all_metadata_to_json_cstring(lists).unwrap_or(std::ptr::null_mut())
    }
}

/// Free a JSON C string returned by any `blocker_engine_get_*` function.
#[no_mangle]
pub extern "C" fn blocker_engine_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe { let _ = std::ffi::CString::from_raw(s); }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use super::resource_type as rt;

    // ── Backward-compatible tests (must all still pass) ──────────────────────

    #[test]
    fn test_basic_insert_and_query() {
        let mut tree = DomainTree::new();
        assert!(tree.insert("example.com"));
        assert!(tree.is_blocked("example.com"));
        assert!(!tree.is_blocked("other.com"));
    }

    #[test]
    fn test_subdomain_blocking() {
        let mut tree = DomainTree::new();
        tree.insert("example.com");
        assert!(tree.is_blocked("example.com"));
        assert!(tree.is_blocked("ads.example.com"));
        assert!(tree.is_blocked("tracker.ads.example.com"));
        assert!(!tree.is_blocked("notexample.com"));
        assert!(!tree.is_blocked("example.org"));
    }

    #[test]
    fn test_whitelist_origin() {
        let mut tree = DomainTree::new();
        tree.insert("doubleclick.net");
        tree.insert_with_whitelist("doubleclick.net", "example.com");
        assert!(tree.is_blocked("doubleclick.net"));
        assert!(!tree.is_blocked_with_origin("doubleclick.net", Some("example.com")));
        assert!(!tree.is_blocked_with_origin("doubleclick.net", Some("sub.example.com")));
        assert!(tree.is_blocked_with_origin("doubleclick.net", Some("badsite.com")));
    }

    #[test]
    fn test_blacklist_origin() {
        let mut tree = DomainTree::new();
        tree.insert_with_blacklist("amazon-adsystem.com", "annoyingsite.com");
        assert!(!tree.is_blocked("amazon-adsystem.com"));
        assert!(!tree.is_blocked_with_origin("amazon-adsystem.com", Some("goodsite.com")));
        assert!(tree.is_blocked_with_origin("amazon-adsystem.com", Some("annoyingsite.com")));
        assert!(tree.is_blocked_with_origin("amazon-adsystem.com", Some("sub.annoyingsite.com")));
    }

    #[test]
    fn test_multiple_domains() {
        let mut tree = DomainTree::new();
        tree.insert("ads.com");
        tree.insert("tracker.net");
        tree.insert("analytics.org");
        assert!(tree.is_blocked("ads.com"));
        assert!(tree.is_blocked("sub.ads.com"));
        assert!(tree.is_blocked("tracker.net"));
        assert!(tree.is_blocked("analytics.org"));
        assert!(!tree.is_blocked("google.com"));
    }

    #[test]
    fn test_serialization() {
        let mut tree = DomainTree::new();
        tree.insert("example.com");
        tree.insert_with_whitelist("ads.net", "goodsite.com");
        tree.insert_with_blacklist("tracker.org", "badsite.com");
        let path = "/tmp/test_tree_v3.bin";
        assert!(tree.save_to_file(path).is_ok());
        let loaded = DomainTree::load_from_file(path).unwrap();
        assert!(loaded.is_blocked("example.com"));
        assert!(loaded.is_blocked("sub.example.com"));
        assert!(!loaded.is_blocked_with_origin("ads.net", Some("goodsite.com")));
        assert!(loaded.is_blocked_with_origin("tracker.org", Some("badsite.com")));
        assert!(!loaded.is_blocked_with_origin("tracker.org", Some("othersite.com")));
    }

    #[test]
    fn test_bulk_load() {
        let mut tree = DomainTree::new();
        tree.bulk_load(&["ads.com", "tracker.net", "analytics.org"]);
        assert!(tree.is_blocked("ads.com"));
        assert!(tree.is_blocked("tracker.net"));
        assert!(tree.is_blocked("analytics.org"));
    }

    #[test]
    fn test_priority_whitelist_over_universal() {
        let mut tree = DomainTree::new();
        tree.insert("ads.com");
        tree.insert_with_whitelist("ads.com", "trustedsite.com");
        assert!(tree.is_blocked("ads.com"));
        assert!(!tree.is_blocked_with_origin("ads.com", Some("trustedsite.com")));
        assert!(tree.is_blocked_with_origin("ads.com", Some("othersite.com")));
    }

    #[test]
    fn test_blacklist_does_not_apply_universal() {
        let mut tree = DomainTree::new();
        tree.insert_with_blacklist("conditional.com", "badsite.com");
        assert!(!tree.is_blocked("conditional.com"));
        assert!(!tree.is_blocked_with_origin("conditional.com", Some("goodsite.com")));
        assert!(tree.is_blocked_with_origin("conditional.com", Some("badsite.com")));
    }

    // ── Resource type tests ──────────────────────────────────────────────────

    #[test]
    fn test_resource_type_script_only() {
        let mut tree = DomainTree::new();
        tree.insert_with_modifiers("tracker.com", Action::Block, OriginConstraint::Any, rt::SCRIPT, false);

        // Blocked for scripts
        assert!(tree.is_blocked_ex("tracker.com", None, rt::SCRIPT, false));
        // Not blocked for images
        assert!(!tree.is_blocked_ex("tracker.com", None, rt::IMAGE, false));
        // resource_type::ALL (0) on the request side bypasses the mask check → blocked
        assert!(tree.is_blocked_ex("tracker.com", None, rt::ALL, false));
    }

    #[test]
    fn test_resource_type_multi_mask() {
        let mut tree = DomainTree::new();
        tree.insert_with_modifiers("cdn.com", Action::Block, OriginConstraint::Any, rt::SCRIPT | rt::STYLESHEET, false);

        assert!(tree.is_blocked_ex("cdn.com", None, rt::SCRIPT, false));
        assert!(tree.is_blocked_ex("cdn.com", None, rt::STYLESHEET, false));
        assert!(!tree.is_blocked_ex("cdn.com", None, rt::IMAGE, false));
        assert!(!tree.is_blocked_ex("cdn.com", None, rt::XHR, false));
    }

    // ── Third-party tests ────────────────────────────────────────────────────

    #[test]
    fn test_third_party_only_blocks_correctly() {
        let mut tree = DomainTree::new();
        tree.insert_with_modifiers("widget.com", Action::Block, OriginConstraint::Any, rt::ALL, true);

        assert!(tree.is_blocked_ex("widget.com", Some("othersite.com"), rt::ALL, true));
        assert!(!tree.is_blocked_ex("widget.com", Some("widget.com"), rt::ALL, false));
        assert!(!tree.is_blocked_ex("widget.com", None, rt::ALL, false));
    }

    #[test]
    fn test_third_party_plus_resource_type() {
        let mut tree = DomainTree::new();
        tree.insert_with_modifiers("tracker.net", Action::Block, OriginConstraint::Any, rt::SCRIPT, true);

        assert!(tree.is_blocked_ex("tracker.net", Some("news.com"), rt::SCRIPT, true));
        assert!(!tree.is_blocked_ex("tracker.net", Some("news.com"), rt::IMAGE, true));
        assert!(!tree.is_blocked_ex("tracker.net", Some("tracker.net"), rt::SCRIPT, false));
    }

    // ── Combined rule tests ───────────────────────────────────────────────────

    #[test]
    fn test_universal_block_with_resource_exception() {
        let mut tree = DomainTree::new();
        tree.insert_with_modifiers("tracker.com", Action::Block, OriginConstraint::Any, rt::ALL, false);
        tree.insert_with_modifiers("tracker.com", Action::Allow, OriginConstraint::Any, rt::IMAGE, false);

        // Allow rule for IMAGE wins
        assert!(!tree.is_blocked_ex("tracker.com", None, rt::IMAGE, false));
        // Block still applies for scripts
        assert!(tree.is_blocked_ex("tracker.com", None, rt::SCRIPT, false));
        // Unknown resource type: block applies (ALL on request side bypasses mask check)
        assert!(tree.is_blocked_ex("tracker.com", None, rt::ALL, false));
    }

    #[test]
    fn test_origin_constraint_with_resource_type() {
        let mut tree = DomainTree::new();
        tree.insert_with_modifiers(
            "cdn.com", Action::Block,
            OriginConstraint::OnlyOn(vec!["evil.com".to_string()]),
            rt::SCRIPT, false,
        );

        assert!(tree.is_blocked_ex("cdn.com", Some("evil.com"), rt::SCRIPT, false));
        assert!(!tree.is_blocked_ex("cdn.com", Some("good.com"), rt::SCRIPT, false));
        assert!(!tree.is_blocked_ex("cdn.com", Some("evil.com"), rt::IMAGE, false));
    }

    #[test]
    fn test_except_on_origin_constraint() {
        let mut tree = DomainTree::new();
        // Block everywhere EXCEPT trusted.com
        tree.insert_with_modifiers(
            "ads.com", Action::Block,
            OriginConstraint::ExceptOn(vec!["trusted.com".to_string()]),
            rt::ALL, false,
        );

        assert!(tree.is_blocked_ex("ads.com", Some("evil.com"), rt::ALL, false));
        assert!(!tree.is_blocked_ex("ads.com", Some("trusted.com"), rt::ALL, false));
        assert!(!tree.is_blocked_ex("ads.com", Some("sub.trusted.com"), rt::ALL, false));
        // No origin → ExceptOn with no origin → blocked (can't match the exclusion)
        assert!(tree.is_blocked_ex("ads.com", None, rt::ALL, false));
    }

    #[test]
    fn test_serialization_preserves_modifiers() {
        let mut tree = DomainTree::new();
        tree.insert_with_modifiers("tracker.com", Action::Block, OriginConstraint::Any, rt::SCRIPT | rt::XHR, true);
        tree.insert_with_modifiers(
            "cdn.com", Action::Allow,
            OriginConstraint::OnlyOn(vec!["safe.com".to_string()]),
            rt::IMAGE, false,
        );

        let path = "/tmp/test_tree_v3_modifiers.bin";
        assert!(tree.save_to_file(path).is_ok());
        let loaded = DomainTree::load_from_file(path).unwrap();

        assert!(loaded.is_blocked_ex("tracker.com", Some("news.com"), rt::SCRIPT, true));
        assert!(!loaded.is_blocked_ex("tracker.com", Some("tracker.com"), rt::SCRIPT, false));
        assert!(!loaded.is_blocked_ex("cdn.com", Some("safe.com"), rt::IMAGE, false));
    }

    #[test]
    fn test_old_cache_format_rejected() {
        let path = "/tmp/old_format_domtree2.bin";
        std::fs::write(path, b"DOMTREE2\x00\x00\x00\x00").unwrap();
        assert!(DomainTree::load_from_file(path).is_err());
    }
}