// filter_list.rs
//
// FilterListManager: downloads, parses, and manages EasyList-style filter lists.
//
// Design decisions made here:
//
// 1. Raw list files are stored on disk per-list. This means rebuild-on-disable
//    is just a re-parse of the stored files — no extra memory needed to hold
//    every domain in a second data structure.
//
// 2. Parsing is streaming line-by-line. We never hold the full list in memory.
//
// 3. HTTP is done with reqwest blocking. No async runtime. This is called from
//    Dart via an Isolate.run() so blocking is fine.
//
// 4. Progress callbacks are a raw C function pointer passed in from Dart. Dart
//    must call this FFI function from an Isolate (not the main isolate) to
//    avoid blocking the UI. The callback is invoked from the same thread that
//    called the FFI function — no cross-thread issues.
//
// 5. Metadata is stored as lists.json alongside the cached tree. On load, we
//    read this file to know which lists exist and whether they are enabled.
//
// 6. The manager owns the DomainTree. All is_blocked queries go through the
//    manager, not the bare tree, so callers don't need to manage two handles.
//
// ABP rule subset we support:
//   ||domain.com^             → universal block of domain.com and all subdomains
//   @@||domain.com^           → exception / whitelist (don't block on any origin)
//   ||domain.com^$domain=x.com|y.com  → block domain.com only when origin is x or y
//   @@||domain.com^$domain=x.com      → whitelist domain.com only when on x.com
//
// We intentionally ignore:
//   - Cosmetic / element-hiding rules (##, #@#, etc.)
//   - Path-level rules (anything with / after the domain)
//   - Regex rules (/regex/)
//   - Script/resource-type rules that aren't $domain=
//   - IP address rules

use std::collections::HashMap;
use std::ffi::CStr;
use std::fmt;
use std::fs;
use std::io::{BufRead, BufReader};
use std::os::raw::{c_char, c_int};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::DomainTree;

// ─── Progress callback type ──────────────────────────────────────────────────

/// C-compatible progress callback. `percent` is 0–100.
/// Pass NULL if you don't want progress events.
pub type ProgressCallback = Option<unsafe extern "C" fn(percent: c_int)>;

fn report_progress(cb: ProgressCallback, percent: i32) {
    if let Some(f) = cb {
        unsafe { f(percent) };
    }
}

// ─── Metadata ────────────────────────────────────────────────────────────────

/// Persistent metadata for a single filter list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterListMetadata {
    /// User-facing name / identifier (also used as filename stem)
    pub name: String,
    /// Canonical URL to fetch from
    pub url: String,
    /// Last ETag received from server (used for conditional requests)
    pub etag: Option<String>,
    /// Last-Modified header received from server
    pub last_modified: Option<String>,
    /// Unix timestamp (seconds) of the last successful download
    pub last_fetched_secs: Option<u64>,
    /// Number of rules currently loaded from this list
    pub rule_count: usize,
    /// Whether this list contributes rules to the active tree
    pub enabled: bool,
}

impl FilterListMetadata {
    fn new(name: impl Into<String>, url: impl Into<String>) -> Self {
        FilterListMetadata {
            name: name.into(),
            url: url.into(),
            etag: None,
            last_modified: None,
            last_fetched_secs: None,
            rule_count: 0,
            enabled: true,
        }
    }
}

// ─── Manager ─────────────────────────────────────────────────────────────────

pub struct FilterListManager {
    /// The live domain tree — rebuilt from enabled lists on startup or toggle
    pub tree: DomainTree,
    /// Metadata indexed by list name
    pub lists: HashMap<String, FilterListMetadata>,
    /// Directory where list .txt files and lists.json are stored
    pub storage_dir: PathBuf,
}

// Error type for internal use
#[derive(Debug)]
pub enum FilterError {
    Io(std::io::Error),
    Http(String),
    Json(serde_json::Error),
    NotFound(String),
    AlreadyExists(String),
}

impl fmt::Display for FilterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FilterError::Io(e) => write!(f, "IO error: {}", e),
            FilterError::Http(s) => write!(f, "HTTP error: {}", s),
            FilterError::Json(e) => write!(f, "JSON error: {}", e),
            FilterError::NotFound(s) => write!(f, "Not found: {}", s),
            FilterError::AlreadyExists(s) => write!(f, "Already exists: {}", s),
        }
    }
}

impl From<std::io::Error> for FilterError {
    fn from(e: std::io::Error) -> Self {
        FilterError::Io(e)
    }
}

impl From<serde_json::Error> for FilterError {
    fn from(e: serde_json::Error) -> Self {
        FilterError::Json(e)
    }
}

impl FilterListManager {
    // ── Paths ────────────────────────────────────────────────────────────────

    fn metadata_path(&self) -> PathBuf {
        self.storage_dir.join("lists.json")
    }

    fn tree_cache_path(&self) -> PathBuf {
        self.storage_dir.join("domain_tree.cache")
    }

    fn list_file_path(&self, name: &str) -> PathBuf {
        // Sanitize name: only allow alphanumeric, dash, underscore, dot
        let safe_name: String = name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.storage_dir.join(format!("{}.txt", safe_name))
    }

    // ── Lifecycle ────────────────────────────────────────────────────────────

    /// Create a new manager for the given storage directory.
    /// If metadata exists, loads it but does NOT rebuild the tree — call
    /// `rebuild_tree()` explicitly after loading if needed, or rely on the
    /// cache loaded separately by the Dart layer.
    pub fn new(storage_dir: impl Into<PathBuf>) -> Self {
        let storage_dir = storage_dir.into();
        fs::create_dir_all(&storage_dir).ok();

        let mut manager = FilterListManager {
            tree: DomainTree::new(),
            lists: HashMap::new(),
            storage_dir,
        };

        // Load metadata if it exists; ignore errors (first run)
        manager.load_metadata().ok();

        manager
    }

    /// Load and rebuild the tree from all enabled lists.
    /// This is called after metadata is loaded, or after toggling a list.
    pub fn rebuild_tree(&mut self, progress: ProgressCallback) -> Result<usize, FilterError> {
        self.tree = DomainTree::new();

        let enabled_names: Vec<String> = self
            .lists
            .values()
            .filter(|m| m.enabled)
            .map(|m| m.name.clone())
            .collect();

        let total = enabled_names.len();
        if total == 0 {
            return Ok(0);
        }

        let mut total_rules = 0usize;

        for (idx, name) in enabled_names.iter().enumerate() {
            let path = self.list_file_path(name);
            if path.exists() {
                let count = parse_list_file_into_tree(&path, &mut self.tree)?;
                if let Some(meta) = self.lists.get_mut(name) {
                    meta.rule_count = count;
                }
                total_rules += count;
            }

            let pct = ((idx + 1) * 90 / total) as i32;
            report_progress(progress, pct);
        }

        // Save rebuilt tree
        self.tree
            .save_to_file(self.tree_cache_path().to_str().unwrap_or(""))
            .ok();

        self.save_metadata()?;
        report_progress(progress, 100);

        Ok(total_rules)
    }

    // ── Install / Update / Remove ─────────────────────────────────────────────

    /// Download and install a new filter list.
    /// Returns the number of rules loaded, or an error.
    pub fn install(
        &mut self,
        name: &str,
        url: &str,
        progress: ProgressCallback,
    ) -> Result<usize, FilterError> {
        if self.lists.contains_key(name) {
            return Err(FilterError::AlreadyExists(name.to_string()));
        }

        report_progress(progress, 0);

        let meta = FilterListMetadata::new(name, url);
        self.lists.insert(name.to_string(), meta);

        // Fetch and parse
        let rule_count = self.fetch_and_apply(name, progress)?;

        Ok(rule_count)
    }

    /// Update a single filter list by re-downloading it.
    /// Uses ETag / If-Modified-Since for conditional GET.
    pub fn update(
        &mut self,
        name: &str,
        progress: ProgressCallback,
    ) -> Result<usize, FilterError> {
        if !self.lists.contains_key(name) {
            return Err(FilterError::NotFound(name.to_string()));
        }

        report_progress(progress, 0);
        self.fetch_and_apply(name, progress)
    }

    /// Update all enabled lists, one by one.
    pub fn update_all(&mut self, progress: ProgressCallback) -> Result<usize, FilterError> {
        let names: Vec<String> = self.lists.keys().cloned().collect();
        let total = names.len();
        let mut total_rules = 0usize;

        for (idx, name) in names.iter().enumerate() {
            let _start_pct = (idx * 90 / total.max(1)) as i32;
            let end_pct = ((idx + 1) * 90 / total.max(1)) as i32;

            // Scale inner progress to [start_pct..end_pct]
            let scaler: ProgressCallback = None; // inner progress not scaled here for simplicity

            match self.fetch_and_apply(name, scaler) {
                Ok(count) => total_rules += count,
                Err(_) => {} // Don't fail the whole batch on one bad list
            }

            report_progress(progress, end_pct);
        }

        report_progress(progress, 100);
        Ok(total_rules)
    }

    /// Remove a filter list entirely — deletes the stored file and rebuilds.
    pub fn remove(&mut self, name: &str, progress: ProgressCallback) -> Result<(), FilterError> {
        if !self.lists.contains_key(name) {
            return Err(FilterError::NotFound(name.to_string()));
        }

        // Delete the raw file
        let path = self.list_file_path(name);
        if path.exists() {
            fs::remove_file(&path)?;
        }

        self.lists.remove(name);
        self.save_metadata()?;
        self.rebuild_tree(progress)?;

        Ok(())
    }

    /// Enable or disable a list, then rebuild the tree from scratch.
    pub fn set_enabled(
        &mut self,
        name: &str,
        enabled: bool,
        progress: ProgressCallback,
    ) -> Result<(), FilterError> {
        let meta = self
            .lists
            .get_mut(name)
            .ok_or_else(|| FilterError::NotFound(name.to_string()))?;

        if meta.enabled == enabled {
            return Ok(()); // No-op
        }

        meta.enabled = enabled;
        self.save_metadata()?;
        self.rebuild_tree(progress)?;

        Ok(())
    }

    // ── Query ────────────────────────────────────────────────────────────────

    /// Delegate to the inner tree
    pub fn is_blocked(&self, domain: &str) -> bool {
        self.tree.is_blocked(domain)
    }

    /// Delegate to the inner tree with origin
    pub fn is_blocked_with_origin(&self, domain: &str, origin: Option<&str>) -> bool {
        self.tree.is_blocked_with_origin(domain, origin)
    }

    /// Full check with resource type and third-party flag.
    pub fn is_blocked_ex(
        &self,
        domain: &str,
        origin: Option<&str>,
        resource_mask: u16,
        is_third_party: bool,
    ) -> bool {
        self.tree.is_blocked_ex(domain, origin, resource_mask, is_third_party)
    }

    // ── Metadata accessors ───────────────────────────────────────────────────

    pub fn get_list_info(&self, name: &str) -> Option<&FilterListMetadata> {
        self.lists.get(name)
    }

    pub fn get_all_lists(&self) -> Vec<&FilterListMetadata> {
        self.lists.values().collect()
    }

    pub fn total_rule_count(&self) -> usize {
        self.tree.count_blocked()
    }

    // ── Internal helpers ─────────────────────────────────────────────────────

    /// Core download → save → parse → tree-insert → persist flow.
    fn fetch_and_apply(
        &mut self,
        name: &str,
        progress: ProgressCallback,
    ) -> Result<usize, FilterError> {
        // Clone what we need to avoid holding a reference into self while mutating
        let (url, etag, last_modified) = {
            let meta = self.lists.get(name).unwrap();
            (
                meta.url.clone(),
                meta.etag.clone(),
                meta.last_modified.clone(),
            )
        };

        report_progress(progress, 10);

        // ── HTTP GET ─────────────────────────────────────────────────────────
        let client = reqwest::blocking::Client::builder()
            .gzip(true)
            .deflate(true)
            .user_agent("BlockerEngine/1.0")
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| FilterError::Http(e.to_string()))?;

        let mut req = client.get(&url);

        if let Some(etag_val) = &etag {
            req = req.header("If-None-Match", etag_val.as_str());
        }
        if let Some(lm_val) = &last_modified {
            req = req.header("If-Modified-Since", lm_val.as_str());
        }

        let response = req
            .send()
            .map_err(|e| FilterError::Http(e.to_string()))?;

        // 304 Not Modified → skip download, but still re-parse what we have
        let not_modified = response.status().as_u16() == 304;

        let (new_etag, new_last_modified) = if !not_modified {
            let etag_hdr = response
                .headers()
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let lm_hdr = response
                .headers()
                .get("last-modified")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            (etag_hdr, lm_hdr)
        } else {
            (etag, last_modified)
        };

        report_progress(progress, 30);

        // ── Save to disk ──────────────────────────────────────────────────────
        let list_path = self.list_file_path(name);

        if !not_modified {
            let text = response
                .text()
                .map_err(|e| FilterError::Http(e.to_string()))?;

            fs::write(&list_path, &text)?;
        }

        report_progress(progress, 60);

        // ── Parse file into tree ──────────────────────────────────────────────
        // The list file now exists on disk; parse it streaming.
        let rule_count = if list_path.exists() {
            parse_list_file_into_tree(&list_path, &mut self.tree)?
        } else {
            0
        };

        report_progress(progress, 85);

        // ── Update metadata ───────────────────────────────────────────────────
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if let Some(meta) = self.lists.get_mut(name) {
            meta.etag = new_etag;
            meta.last_modified = new_last_modified;
            meta.last_fetched_secs = Some(now_secs);
            meta.rule_count = rule_count;
        }

        // ── Persist ───────────────────────────────────────────────────────────
        self.tree
            .save_to_file(self.tree_cache_path().to_str().unwrap_or(""))
            .ok();
        self.save_metadata()?;

        report_progress(progress, 100);

        Ok(rule_count)
    }

    fn save_metadata(&self) -> Result<(), FilterError> {
        let lists_vec: Vec<&FilterListMetadata> = self.lists.values().collect();
        let json = serde_json::to_string_pretty(&lists_vec)?;
        fs::write(self.metadata_path(), json)?;
        Ok(())
    }

    fn load_metadata(&mut self) -> Result<(), FilterError> {
        let path = self.metadata_path();
        if !path.exists() {
            return Ok(());
        }
        let content = fs::read_to_string(&path)?;
        let list: Vec<FilterListMetadata> = serde_json::from_str(&content)?;
        for meta in list {
            self.lists.insert(meta.name.clone(), meta);
        }
        Ok(())
    }
}

// ─── ABP Streaming Parser ─────────────────────────────────────────────────────

/// Parse a locally-stored filter list file into the tree.
/// Processes one line at a time — no full-file allocation.
pub fn parse_list_file_into_tree(
    path: &Path,
    tree: &mut DomainTree,
) -> Result<usize, FilterError> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut count = 0usize;

    for line_result in reader.lines() {
        let line = match line_result {
            Ok(l) => l,
            Err(_) => continue, // Skip unreadable lines
        };

        let line = line.trim();

        // Skip empty lines and comments
        if line.is_empty() || line.starts_with('!') || line.starts_with('#') {
            continue;
        }

        // Skip cosmetic rules (element hiding)
        if line.contains("##") || line.contains("#@#") || line.contains("#?#") {
            continue;
        }

        // Skip regex rules
        if line.starts_with('/') && line.ends_with('/') {
            continue;
        }

        if let Some(rule) = parse_abp_line(line) {
            use crate::{Action, OriginConstraint};

            // Determine origin constraint from positive/negative origin lists:
            //   positive only       → OnlyOn(positive)
            //   negative only       → ExceptOn(negative)
            //   positive + negative → OnlyOn(positive); mixed form is uncommon
            //                         and we conservatively prefer positive list
            //   neither             → Any
            let origin_constraint = match (rule.positive_origins.is_empty(), rule.negative_origins.is_empty()) {
                (false, _)    => OriginConstraint::OnlyOn(rule.positive_origins),
                (true, false) => OriginConstraint::ExceptOn(rule.negative_origins),
                (true, true)  => OriginConstraint::Any,
            };

            let action = if rule.is_exception { Action::Allow } else { Action::Block };

            tree.insert_with_modifiers(
                &rule.domain,
                action,
                origin_constraint,
                rule.resource_mask,
                rule.third_party_only,
            );
            count += 1;
        }
    }

    Ok(count)
}

// ─── ABP Line Parser ──────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
struct ParsedRule {
    domain: String,
    is_exception: bool,
    /// Positive origins from $domain= (block/allow only on these).
    /// Empty = no positive constraint.
    positive_origins: Vec<String>,
    /// Negated origins from $domain=~x (block/allow except on these).
    /// Empty = no negated constraint.
    negative_origins: Vec<String>,
    /// Bitmask of resource types, 0 = all.
    resource_mask: u16,
    third_party_only: bool,
}

/// Map a single ABP modifier token to a resource type bit.
/// Returns 0 for unknown modifiers (caller should ignore those).
fn modifier_to_resource_bit(token: &str) -> u16 {
    use crate::resource_type as rt;
    match token {
        "script"                        => rt::SCRIPT,
        "image" | "background"         => rt::IMAGE,
        "stylesheet"                    => rt::STYLESHEET,
        "xmlhttprequest" | "xhr"        => rt::XHR,
        "media"                         => rt::MEDIA,
        "font"                          => rt::FONT,
        "document"                      => rt::DOCUMENT,
        "subdocument"                   => rt::SUBDOCUMENT,
        "other" | "object" | "ping"
        | "beacon" | "websocket"       => rt::OTHER,
        _                               => 0,
    }
}

/// Parse a single ABP filter line. Returns None for unsupported/ignored rules.
fn parse_abp_line(line: &str) -> Option<ParsedRule> {
    let is_exception = line.starts_with("@@");
    let rule = if is_exception { &line[2..] } else { line };

    // Must start with || to be a domain-level rule
    if !rule.starts_with("||") {
        return None;
    }
    let rule = &rule[2..];

    // Split on $ to separate domain from options
    let (domain_part, options_part) = if let Some(dollar_pos) = rule.find('$') {
        (&rule[..dollar_pos], Some(&rule[dollar_pos + 1..]))
    } else {
        (rule, None)
    };

    // Strip trailing ^ and |
    let domain = domain_part.trim_end_matches('^').trim_end_matches('|');

    // Reject path rules, IP addresses, and invalid domains
    if domain.contains('/') { return None; }
    if domain.parse::<std::net::IpAddr>().is_ok() { return None; }
    if domain.is_empty() || !domain.contains('.') { return None; }

    let domain = domain.to_lowercase();

    // Parse modifiers
    let mut resource_mask: u16 = 0;
    let mut third_party_only = false;
    let mut positive_origins: Vec<String> = Vec::new();
    let mut negative_origins: Vec<String> = Vec::new();

    if let Some(opts) = options_part {
        for opt in opts.split(',') {
            let opt = opt.trim();

            if opt == "third-party" || opt == "3p" {
                third_party_only = true;
            } else if opt == "~third-party" || opt == "~3p" {
                // "first-party only" — we don't block third-party, so this
                // rule can still be used but without the third_party_only flag.
                // The rule will apply to all requests (first and third party).
                // This is the safe conservative behaviour.
            } else if opt.starts_with("domain=") {
                // Parse pipe-separated origins; ~ prefix = negated
                for raw in opt["domain=".len()..].split('|').filter(|s| !s.is_empty()) {
                    if let Some(neg) = raw.strip_prefix('~') {
                        let o = neg.to_lowercase();
                        if o.contains('.') { negative_origins.push(o); }
                    } else {
                        let o = raw.to_lowercase();
                        if o.contains('.') { positive_origins.push(o); }
                    }
                }
            } else {
                // Try to match as a resource type modifier
                let bit = modifier_to_resource_bit(opt);
                if bit != 0 {
                    resource_mask |= bit;
                }
                // Unknown modifiers are silently ignored (safe default)
            }
        }
    }

    Some(ParsedRule {
        domain,
        is_exception,
        positive_origins,
        negative_origins,
        resource_mask,
        third_party_only,
    })
}

// ─── JSON output helpers ──────────────────────────────────────────────────────

/// Serialize a single FilterListMetadata to a JSON C string.
/// Caller must free via `filter_list_free_string`.
pub fn metadata_to_json_cstring(meta: &FilterListMetadata) -> Option<*mut c_char> {
    let json = serde_json::to_string(meta).ok()?;
    let c = std::ffi::CString::new(json).ok()?;
    Some(c.into_raw())
}

/// Serialize all metadata to a JSON array C string.
pub fn all_metadata_to_json_cstring(lists: Vec<&FilterListMetadata>) -> Option<*mut c_char> {
    let json = serde_json::to_string(&lists).ok()?;
    let c = std::ffi::CString::new(json).ok()?;
    Some(c.into_raw())
}

// ─── FFI ─────────────────────────────────────────────────────────────────────
//
// FFI design notes:
//
// - All functions that can fail return bool (false = error).
// - JSON is used for structured return values (list metadata) to avoid
//   complex pointer-to-pointer patterns.
// - Dart must free any *mut c_char returned here via filter_list_free_string.
// - Caller must pass a storage_dir that already exists (or we create it).
// - Progress callback is optional (pass NULL to ignore).

/// Create a new FilterListManager for the given storage directory.
/// Returns an opaque pointer; free with `filter_list_manager_free`.
#[no_mangle]
pub extern "C" fn filter_list_manager_new(
    storage_dir: *const c_char,
) -> *mut FilterListManager {
    if storage_dir.is_null() {
        return std::ptr::null_mut();
    }
    let dir_str = unsafe {
        match CStr::from_ptr(storage_dir).to_str() {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        }
    };
    Box::into_raw(Box::new(FilterListManager::new(dir_str)))
}

/// Free a FilterListManager created by `filter_list_manager_new`.
#[no_mangle]
pub extern "C" fn filter_list_manager_free(manager: *mut FilterListManager) {
    if !manager.is_null() {
        unsafe {
            let _ = Box::from_raw(manager);
        }
    }
}

/// Rebuild the active tree from all enabled lists stored on disk.
/// Call this once after creating the manager to apply saved lists.
/// Returns the total rule count, or 0 on failure.
#[no_mangle]
pub extern "C" fn filter_list_manager_rebuild(
    manager: *mut FilterListManager,
    progress: ProgressCallback,
) -> usize {
    if manager.is_null() {
        return 0;
    }
    unsafe {
        match (*manager).rebuild_tree(progress) {
            Ok(count) => count,
            Err(_) => 0,
        }
    }
}

/// Install a new filter list (download + parse + add to metadata).
/// Returns the number of rules inserted, or 0 on failure.
/// `name` must be unique; returns 0 if it already exists.
#[no_mangle]
pub extern "C" fn filter_list_manager_install(
    manager: *mut FilterListManager,
    name: *const c_char,
    url: *const c_char,
    progress: ProgressCallback,
) -> usize {
    if manager.is_null() || name.is_null() || url.is_null() {
        return 0;
    }
    unsafe {
        let name_str = match CStr::from_ptr(name).to_str() {
            Ok(s) => s,
            Err(_) => return 0,
        };
        let url_str = match CStr::from_ptr(url).to_str() {
            Ok(s) => s,
            Err(_) => return 0,
        };
        match (*manager).install(name_str, url_str, progress) {
            Ok(count) => count,
            Err(_) => 0,
        }
    }
}

/// Update a single filter list (conditional GET, re-parse).
/// Returns rule count, or 0 on failure.
#[no_mangle]
pub extern "C" fn filter_list_manager_update(
    manager: *mut FilterListManager,
    name: *const c_char,
    progress: ProgressCallback,
) -> usize {
    if manager.is_null() || name.is_null() {
        return 0;
    }
    unsafe {
        let name_str = match CStr::from_ptr(name).to_str() {
            Ok(s) => s,
            Err(_) => return 0,
        };
        match (*manager).update(name_str, progress) {
            Ok(count) => count,
            Err(_) => 0,
        }
    }
}

/// Update all filter lists.
/// Returns total rule count across all lists, or 0 on total failure.
#[no_mangle]
pub extern "C" fn filter_list_manager_update_all(
    manager: *mut FilterListManager,
    progress: ProgressCallback,
) -> usize {
    if manager.is_null() {
        return 0;
    }
    unsafe {
        match (*manager).update_all(progress) {
            Ok(count) => count,
            Err(_) => 0,
        }
    }
}

/// Remove a filter list entirely (deletes file, rebuilds tree).
/// Returns true on success.
#[no_mangle]
pub extern "C" fn filter_list_manager_remove(
    manager: *mut FilterListManager,
    name: *const c_char,
    progress: ProgressCallback,
) -> bool {
    if manager.is_null() || name.is_null() {
        return false;
    }
    unsafe {
        let name_str = match CStr::from_ptr(name).to_str() {
            Ok(s) => s,
            Err(_) => return false,
        };
        (*manager).remove(name_str, progress).is_ok()
    }
}

/// Enable or disable a filter list, rebuilding the tree if changed.
/// Returns true on success.
#[no_mangle]
pub extern "C" fn filter_list_manager_set_enabled(
    manager: *mut FilterListManager,
    name: *const c_char,
    enabled: bool,
    progress: ProgressCallback,
) -> bool {
    if manager.is_null() || name.is_null() {
        return false;
    }
    unsafe {
        let name_str = match CStr::from_ptr(name).to_str() {
            Ok(s) => s,
            Err(_) => return false,
        };
        (*manager).set_enabled(name_str, enabled, progress).is_ok()
    }
}

/// Check if a domain is blocked (no origin context).
#[no_mangle]
pub extern "C" fn filter_list_manager_is_blocked(
    manager: *const FilterListManager,
    domain: *const c_char,
) -> bool {
    if manager.is_null() || domain.is_null() {
        return false;
    }
    unsafe {
        let domain_str = match CStr::from_ptr(domain).to_str() {
            Ok(s) => s,
            Err(_) => return false,
        };
        (*manager).is_blocked(domain_str)
    }
}

/// Check if a domain is blocked given an origin context.
/// Pass NULL for origin to check without context.
#[no_mangle]
pub extern "C" fn filter_list_manager_is_blocked_with_origin(
    manager: *const FilterListManager,
    domain: *const c_char,
    origin: *const c_char,
) -> bool {
    if manager.is_null() || domain.is_null() {
        return false;
    }
    unsafe {
        let domain_str = match CStr::from_ptr(domain).to_str() {
            Ok(s) => s,
            Err(_) => return false,
        };
        let origin_str = if origin.is_null() {
            None
        } else {
            match CStr::from_ptr(origin).to_str() {
                Ok(s) => Some(s),
                Err(_) => None,
            }
        };
        (*manager).is_blocked_with_origin(domain_str, origin_str)
    }
}

/// Extended block check with resource type bitmask and third-party flag.
/// `resource_mask`: bitmask from `resource_type::*` constants in lib.rs (0 = any).
/// `is_third_party`: pass true if the request crosses an eTLD+1 boundary.
#[no_mangle]
pub extern "C" fn filter_list_manager_is_blocked_ex(
    manager: *const FilterListManager,
    domain: *const c_char,
    origin: *const c_char,
    resource_mask: u16,
    is_third_party: bool,
) -> bool {
    if manager.is_null() || domain.is_null() {
        return false;
    }
    unsafe {
        let domain_str = match CStr::from_ptr(domain).to_str() {
            Ok(s) => s,
            Err(_) => return false,
        };
        let origin_str = if origin.is_null() {
            None
        } else {
            match CStr::from_ptr(origin).to_str() {
                Ok(s) => Some(s),
                Err(_) => None,
            }
        };
        (*manager).is_blocked_ex(domain_str, origin_str, resource_mask, is_third_party)
    }
}

/// Get metadata for a single list as a JSON string.
/// Returns NULL if not found. Caller must free via `filter_list_free_string`.
#[no_mangle]
pub extern "C" fn filter_list_manager_get_list_info(
    manager: *const FilterListManager,
    name: *const c_char,
) -> *mut c_char {
    if manager.is_null() || name.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        let name_str = match CStr::from_ptr(name).to_str() {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        };
        match (*manager).get_list_info(name_str) {
            Some(meta) => metadata_to_json_cstring(meta).unwrap_or(std::ptr::null_mut()),
            None => std::ptr::null_mut(),
        }
    }
}

/// Get all filter lists as a JSON array string.
/// Returns NULL on failure. Caller must free via `filter_list_free_string`.
#[no_mangle]
pub extern "C" fn filter_list_manager_get_all_lists(
    manager: *const FilterListManager,
) -> *mut c_char {
    if manager.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        let lists = (*manager).get_all_lists();
        all_metadata_to_json_cstring(lists).unwrap_or(std::ptr::null_mut())
    }
}

/// Get the total number of rules currently in the tree.
#[no_mangle]
pub extern "C" fn filter_list_manager_total_rules(
    manager: *const FilterListManager,
) -> usize {
    if manager.is_null() {
        return 0;
    }
    unsafe { (*manager).total_rule_count() }
}

/// Free a C string returned by any `filter_list_manager_get_*` function.
#[no_mangle]
pub extern "C" fn filter_list_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe {
            let _ = std::ffi::CString::from_raw(s);
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Parser unit tests ───────────────────────────────────────────────────────

    fn parsed(line: &str) -> Option<ParsedRule> {
        parse_abp_line(line)
    }

    #[test]
    fn test_parse_universal_block() {
        let r = parsed("||ads.example.com^").unwrap();
        assert_eq!(r.domain, "ads.example.com");
        assert!(!r.is_exception);
        assert!(r.positive_origins.is_empty());
        assert!(r.negative_origins.is_empty());
        assert_eq!(r.resource_mask, 0);
        assert!(!r.third_party_only);
    }

    #[test]
    fn test_parse_exception() {
        let r = parsed("@@||safe.example.com^").unwrap();
        assert_eq!(r.domain, "safe.example.com");
        assert!(r.is_exception);
        assert!(r.positive_origins.is_empty());
        assert_eq!(r.resource_mask, 0);
    }

    #[test]
    fn test_parse_block_with_domain_option() {
        let r = parsed("||tracker.com^$domain=bad1.com|bad2.com").unwrap();
        assert_eq!(r.domain, "tracker.com");
        assert!(!r.is_exception);
        assert_eq!(r.positive_origins, vec!["bad1.com", "bad2.com"]);
        assert!(r.negative_origins.is_empty());
    }

    #[test]
    fn test_parse_whitelist_with_domain_option() {
        let r = parsed("@@||cdn.net^$domain=trusted.com").unwrap();
        assert_eq!(r.domain, "cdn.net");
        assert!(r.is_exception);
        assert_eq!(r.positive_origins, vec!["trusted.com"]);
    }

    #[test]
    fn test_parse_negated_domain_option() {
        let r = parsed("||ads.com^$domain=~good.com|~safe.org").unwrap();
        assert_eq!(r.domain, "ads.com");
        assert!(r.positive_origins.is_empty());
        assert_eq!(r.negative_origins, vec!["good.com", "safe.org"]);
    }

    #[test]
    fn test_parse_resource_type_script() {
        let r = parsed("||tracker.com^$script").unwrap();
        assert_eq!(r.resource_mask, crate::resource_type::SCRIPT);
        assert!(!r.third_party_only);
    }

    #[test]
    fn test_parse_resource_type_multi() {
        use crate::resource_type as rt;
        let r = parsed("||tracker.com^$script,image").unwrap();
        assert_eq!(r.resource_mask, rt::SCRIPT | rt::IMAGE);
    }

    #[test]
    fn test_parse_third_party() {
        let r = parsed("||tracker.com^$third-party").unwrap();
        assert!(r.third_party_only);
    }

    #[test]
    fn test_parse_combined_modifiers() {
        use crate::resource_type as rt;
        let r = parsed("||ads.net^$script,third-party,domain=evil.com").unwrap();
        assert_eq!(r.domain, "ads.net");
        assert!(!r.is_exception);
        assert_eq!(r.resource_mask, rt::SCRIPT);
        assert!(r.third_party_only);
        assert_eq!(r.positive_origins, vec!["evil.com"]);
    }

    #[test]
    fn test_parse_ignores_cosmetic() {
        assert!(parsed("example.com##.ad-banner").is_none());
        assert!(parsed("##.some-class").is_none());
    }

    #[test]
    fn test_parse_ignores_path_rules() {
        assert!(parsed("||example.com/ads/*").is_none());
    }

    #[test]
    fn test_parse_ignores_no_double_pipe() {
        assert!(parsed("ads.example.com").is_none());
    }

    #[test]
    fn test_parse_ignores_no_dot() {
        assert!(parsed("||localhost^").is_none());
    }

    #[test]
    fn test_manager_install_and_query() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let list_path = dir.path().join("test_list.txt");

        // Write a minimal filter list to a temp file
        let mut f = std::fs::File::create(&list_path).unwrap();
        writeln!(f, "! Test list").unwrap();
        writeln!(f, "||ads.example.com^").unwrap();
        writeln!(f, "||tracker.net^").unwrap();
        writeln!(f, "||cdn.safe.com^$domain=evil.com").unwrap();
        writeln!(f, "@@||safe.example.com^").unwrap();
        drop(f);

        let mut manager = FilterListManager::new(dir.path());

        // Manually add metadata and copy file into place
        let meta = FilterListMetadata {
            name: "test".to_string(),
            url: "https://example.com/list.txt".to_string(),
            etag: None,
            last_modified: None,
            last_fetched_secs: None,
            rule_count: 0,
            enabled: true,
        };
        manager.lists.insert("test".to_string(), meta);

        // Copy the list file to where the manager expects it
        let dest = manager.list_file_path("test");
        std::fs::copy(&list_path, &dest).unwrap();

        // Rebuild tree
        manager.rebuild_tree(None).unwrap();

        assert!(manager.is_blocked("ads.example.com"));
        assert!(manager.is_blocked("sub.ads.example.com"));
        assert!(manager.is_blocked("tracker.net"));
        assert!(!manager.is_blocked("safe.example.com"));

        // cdn.safe.com blocked only on evil.com
        assert!(!manager.is_blocked("cdn.safe.com"));
        assert!(manager.is_blocked_with_origin("cdn.safe.com", Some("evil.com")));
        assert!(!manager.is_blocked_with_origin("cdn.safe.com", Some("good.com")));
    }
}