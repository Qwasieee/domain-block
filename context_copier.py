#!/usr/bin/env python3
"""
Context Copier with Content Search
Supports both file-pattern collections and content-search definitions.
"""

import os
import sys
import glob
import subprocess
import mimetypes
import re
import argparse
import zipfile
from pathlib import Path
from typing import List, Set, Optional

# =============================================================================
# SEARCH HELPERS
# =============================================================================


def exact(query: str, base_path: str = "lib") -> dict:
    return {"mode": "strict", "query": query, "base_path": base_path}


def fuzzy(query: str, base_path: str = "lib") -> dict:
    return {"mode": "fuzzy", "query": query, "base_path": base_path}


def fromFile(file_path: str, recursive: bool = False) -> dict:
    """
    Collect imports from a Dart file.

    Args:
        file_path: Path to the entry Dart file
        recursive: If True, recursively collect all imports. If False, only direct imports.

    Returns a dict indicating this is a fromFile collection.
    """
    return {"mode": "fromFile", "entry_file": file_path, "recursive": recursive}

# =============================================================================
# MY CUSTOM PATHS
# =============================================================================


MY_PATHS = {
    "Project": [
        "src/**",
        "examples/**",
        "build.sh",
        "Cargo.toml",
        "migrate_to_flutter.sh",
        "setup.sh",
    ],
    "src": [
        "src/**",
    ]
}


# =============================================================================
# CONFIGURATION
# =============================================================================


class Config:
    def __init__(self):
        self.max_file_size = 5 * 1024 * 1024  # 5MB
        self.exclusion_patterns = [
            ".git", "node_modules", "__pycache__", ".pytest_cache",
            ".venv", "venv", "env", ".env", "dist", "build", ".next",
            ".nuxt", "target", ".cargo", ".gradle", "vendor", ".bundle",
            "*.log", "*.tmp", "*.cache", "*.pyc", "*.pyo", "*.pyd",
            "*.so", "*.dll", "*.dylib", "*.exe", "*.bin", "*.class",
            "*.jar", "*.war", "*.ear", "*.zip", "*.tar", "*.gz", "*.rar",
            "*.7z", "*.img", "*.iso", "*.dmg", "*.pkg", "*.deb", "*.rpm"
        ]
        self.include_line_numbers = False
        self.include_folder_tree = True  # Always append project structure

# =============================================================================
# FOLDER TREE
# =============================================================================

# Common source root folder names to look for, in priority order
_SOURCE_ROOTS = ["lib", "src", "app", "packages", "source", "core"]

# Folders to skip entirely when building the tree (on top of dot-folders)
_TREE_IGNORED_DIRS = {
    "node_modules", "__pycache__", ".dart_tool", "build", "dist",
    ".gradle", ".idea", ".vscode", "vendor", ".bundle", "target",
    ".next", ".nuxt", ".cargo", "venv", ".venv", "env", ".env",
}


def _gather_tree(startpath: str, prefix: str = "") -> List[str]:
    """
    Recursively gather lines of a directory tree, skipping dot-folders and
    common build/dependency directories.
    Ported from folder_tree_printer.py.
    """
    lines = []
    try:
        all_entries = sorted(os.listdir(startpath))
    except PermissionError:
        lines.append(f"{prefix}[Permission Denied]")
        return lines
    except FileNotFoundError:
        lines.append(f"{prefix}[Not Found]")
        return lines

    # Filter out dot-folders and ignored dirs
    entries = []
    for entry in all_entries:
        fullpath = os.path.join(startpath, entry)
        if os.path.isdir(fullpath):
            if entry.startswith('.') or entry in _TREE_IGNORED_DIRS:
                continue
        entries.append(entry)

    for i, entry in enumerate(entries):
        path = os.path.join(startpath, entry)
        connector = "└── " if i == len(entries) - 1 else "├── "
        lines.append(f"{prefix}{connector}{entry}")
        if os.path.isdir(path):
            extension = "    " if i == len(entries) - 1 else "│   "
            lines.extend(_gather_tree(path, prefix + extension))

    return lines


def _detect_tree_root(cwd: Path) -> Path:
    """
    Return the best folder to use as the tree root.
    Checks for common source roots (lib, src, app, …) under cwd;
    falls back to cwd itself if none are found.
    """
    for name in _SOURCE_ROOTS:
        candidate = cwd / name
        if candidate.is_dir():
            return candidate
    return cwd


def build_folder_tree(cwd: Optional[Path] = None) -> str:
    """
    Build a project-structure tree string and return it.
    The tree root is auto-detected from common source folders.
    """
    if cwd is None:
        cwd = Path.cwd()

    tree_root = _detect_tree_root(cwd)
    label = tree_root.name + "/"

    lines = [label]
    lines.extend(_gather_tree(str(tree_root)))

    tree_str = "\n".join(lines)

    header = f"\n\n{'=' * 60}\n📁 Project Structure ({label})\n{'=' * 60}\n"
    return header + tree_str

# =============================================================================
# DART IMPORT RESOLVER
# =============================================================================


class DartImportResolver:
    def __init__(self, project_root: Path):
        self.project_root = project_root
        self.package_name = self._detect_package_name()
        self.visited_files: Set[Path] = set()

    def _detect_package_name(self) -> Optional[str]:
        """Detect the package name from pubspec.yaml"""
        pubspec_path = self.project_root / "pubspec.yaml"
        if not pubspec_path.exists():
            print(f"⚠️  Warning: pubspec.yaml not found at {pubspec_path}")
            return None

        try:
            with open(pubspec_path, 'r', encoding='utf-8') as f:
                for line in f:
                    if line.strip().startswith('name:'):
                        name = line.split(':', 1)[1].strip()
                        name = name.strip('"\'')
                        return name
        except Exception as e:
            print(f"⚠️  Warning: Could not read pubspec.yaml: {e}")

        return None

    def _extract_imports(self, file_path: Path) -> List[str]:
        """Extract all import and export statements from a Dart file"""
        imports = []

        try:
            with open(file_path, 'r', encoding='utf-8', errors='ignore') as f:
                content = f.read()

            content_single_line = re.sub(
                r"'([^']*)\n\s*([^']*)'", r"'\1\2'", content)
            content_single_line = re.sub(
                r'"([^"]*)\n\s*([^"]*)"', r'"\1\2"', content_single_line)

            import_pattern = r'''(?:import|export|part)\s+['"](.*?)['"]'''
            matches = re.findall(import_pattern, content_single_line)
            imports.extend(matches)

            conditional_pattern = r'''(?:import|export)\s+['"].*?['"]\s+if\s*\([^)]+\)\s+['"](.*?)['"]'''
            conditional_matches = re.findall(
                conditional_pattern, content_single_line)
            imports.extend(conditional_matches)

        except Exception as e:
            print(f"⚠️  Warning: Could not read {file_path}: {e}")

        return imports

    def _is_project_import(self, import_path: str) -> bool:
        """Check if import is a project-internal import"""
        if not self.package_name:
            return import_path.startswith('./') or import_path.startswith('../')

        if import_path.startswith(f'package:{self.package_name}/'):
            return True

        if import_path.startswith('./') or import_path.startswith('../'):
            return True

        if not import_path.startswith('package:') and not import_path.startswith('dart:'):
            return True

        return False

    def _resolve_import_path(self, import_path: str, current_file: Path) -> Optional[Path]:
        """Resolve an import path to an absolute file path"""

        if self.package_name and import_path.startswith(f'package:{self.package_name}/'):
            relative_path = import_path.replace(
                f'package:{self.package_name}/', '')
            resolved = self.project_root / 'lib' / relative_path
            return resolved if resolved.exists() else None

        if import_path.startswith('./') or import_path.startswith('../'):
            import_path_clean = import_path.replace(
                './', '').replace('../', '')
            levels_up = import_path.count('../')

            base_dir = current_file.parent
            for _ in range(levels_up):
                base_dir = base_dir.parent

            resolved = base_dir / import_path_clean
            return resolved if resolved.exists() else None

        if not import_path.startswith('package:') and not import_path.startswith('dart:'):
            resolved = current_file.parent / import_path
            if resolved.exists():
                return resolved

            resolved = self.project_root / 'lib' / import_path
            if resolved.exists():
                return resolved

        return None

    def collect_files(self, entry_file_path: str, recursive: bool = False) -> List[Path]:
        """
        Collect all files imported by the entry file.

        Args:
            entry_file_path: Path to the entry file
            recursive: If True, recursively collect all imports. If False, only direct imports.

        Returns a list of unique file paths.
        """
        entry_file = Path(entry_file_path).resolve()

        if not entry_file.exists():
            print(f"❌ Error: Entry file not found: {entry_file}")
            return []

        if not hasattr(self, 'project_root') or self.project_root is None:
            current = entry_file.parent
            while current != current.parent:
                if (current / 'pubspec.yaml').exists():
                    self.project_root = current
                    self.package_name = self._detect_package_name()
                    break
                current = current.parent

        self.visited_files.clear()

        if recursive:
            self._collect_recursive(entry_file)
        else:
            self._collect_direct(entry_file)

        return sorted(list(self.visited_files))

    def _collect_direct(self, file_path: Path):
        """Collect only direct imports from a file (non-recursive)"""
        if not file_path.exists() or not file_path.is_file():
            return

        self.visited_files.add(file_path)

        imports = self._extract_imports(file_path)

        for import_path in imports:
            if not self._is_project_import(import_path):
                continue

            resolved_path = self._resolve_import_path(import_path, file_path)

            if resolved_path is None:
                continue

            if resolved_path.exists() and resolved_path.is_file():
                self.visited_files.add(resolved_path)

    def _collect_recursive(self, file_path: Path):
        """Recursively collect imports from a file"""
        if file_path in self.visited_files:
            return

        if not file_path.exists() or not file_path.is_file():
            return

        self.visited_files.add(file_path)

        imports = self._extract_imports(file_path)

        for import_path in imports:
            if not self._is_project_import(import_path):
                continue

            resolved_path = self._resolve_import_path(import_path, file_path)

            if resolved_path is None:
                continue

            self._collect_recursive(resolved_path)

# =============================================================================
# FILE PROCESSOR
# =============================================================================


class FileProcessor:
    def __init__(self, config: Config):
        self.config = config
        self.text_extensions = {
            '.py', '.dart', '.js', '.ts', '.html', '.css', '.json',
            '.yaml', '.yml', '.toml', '.ini', '.xml', '.md', '.txt',
            '.java', '.c', '.cpp', '.h', '.hpp', '.cs', '.go', '.rs',
            '.rb', '.php', '.swift', '.kt', '.scala', '.r', '.sql'
        }

    def is_binary_file(self, filepath: Path) -> bool:
        if filepath.suffix.lower() in self.text_extensions:
            return False
        mime_type, _ = mimetypes.guess_type(str(filepath))
        if mime_type and not mime_type.startswith('text/'):
            text_types = {'application/json',
                          'application/xml', 'application/javascript'}
            if mime_type not in text_types:
                return True
        try:
            with open(filepath, 'rb') as f:
                chunk = f.read(1024)
                if b'\x00' in chunk:
                    return True
        except Exception:
            return True
        return False

    def should_exclude_path(self, path: Path) -> bool:
        for pattern in self.config.exclusion_patterns:
            for part in path.parts:
                if Path(part).match(pattern):
                    return True
        return False

    def find_files(self, patterns: List[str], base_dir: str = ".") -> List[Path]:
        base_path = Path(base_dir).resolve()
        found_files = set()
        for pattern in patterns:
            direct_pattern = str(base_path / pattern)
            matches = glob.glob(direct_pattern, recursive=True)
            if '**' not in pattern:
                recursive_pattern = str(base_path / "**" / pattern)
                matches.extend(glob.glob(recursive_pattern, recursive=True))
            for match in matches:
                path = Path(match).resolve()
                if path.is_file() and not self.should_exclude_path(path):
                    found_files.add(path)
        return sorted(list(found_files))

    def process_files(self, patterns: List[str], base_dir: str = ".") -> str:
        files = self.find_files(patterns, base_dir)
        if not files:
            return "No files found matching the specified patterns."
        base_path = Path(base_dir).resolve()
        output_parts = []
        for file_path in files:
            try:
                if file_path.stat().st_size > self.config.max_file_size:
                    output_parts.append(
                        f"<{file_path.name}> [SKIPPED - Too large]")
                    continue
                if self.is_binary_file(file_path):
                    output_parts.append(
                        f"<{file_path.name}> [SKIPPED - Binary]")
                    continue
                rel_path = file_path.relative_to(base_path)
                with open(file_path, 'r', encoding='utf-8', errors='replace') as f:
                    content = f.read()
                output_parts.append(f"<{file_path.name}> [{rel_path}]:")
                output_parts.append(content)
                output_parts.append("")
            except Exception as e:
                output_parts.append(f"<{file_path.name}> [ERROR]: {e}")
        return '\n'.join(output_parts)

    def process_files_from_paths(self, file_paths: List[Path]) -> str:
        """Process a list of Path objects directly"""
        if not file_paths:
            return "No files found."

        try:
            common_base = Path(os.path.commonpath(
                [str(p) for p in file_paths]))
        except ValueError:
            common_base = Path.cwd()

        output_parts = []
        for file_path in file_paths:
            try:
                if file_path.stat().st_size > self.config.max_file_size:
                    output_parts.append(
                        f"<{file_path.name}> [SKIPPED - Too large]")
                    continue
                if self.is_binary_file(file_path):
                    output_parts.append(
                        f"<{file_path.name}> [SKIPPED - Binary]")
                    continue

                try:
                    rel_path = file_path.relative_to(common_base)
                except ValueError:
                    rel_path = file_path

                with open(file_path, 'r', encoding='utf-8', errors='replace') as f:
                    content = f.read()
                output_parts.append(f"<{file_path.name}> [{rel_path}]:")
                output_parts.append(content)
                output_parts.append("")
            except Exception as e:
                output_parts.append(f"<{file_path.name}> [ERROR]: {e}")
        return '\n'.join(output_parts)

# =============================================================================
# CONTENT SEARCH
# =============================================================================


def get_all_dart_files(base_path: Path) -> List[Path]:
    dart_files = []
    for root, dirs, files in os.walk(base_path):
        for file in files:
            if file.endswith('.dart'):
                dart_files.append(Path(root) / file)
    return sorted(dart_files)


def find_files_with_content(query: str, mode: str, base_path: str) -> List[Path]:
    """Return a list of files whose content matches the query."""
    base = Path(base_path).resolve()
    all_files = get_all_dart_files(base)
    case_sensitive = (mode == "strict")
    matched: List[Path] = []
    for file_path in all_files:
        try:
            with open(file_path, 'r', encoding='utf-8', errors='ignore') as f:
                content = f.read()
            hit = query in content if case_sensitive else query.lower() in content.lower()
            if hit:
                matched.append(file_path)
        except Exception:
            pass
    return matched


def run_content_search(query: str, mode: str, base_path: str, processor: FileProcessor) -> str:
    """Search for files containing query and return their CONTENTS (not just paths)."""
    matched_files = find_files_with_content(query, mode, base_path)
    if not matched_files:
        return f"// No files found containing: {query}"
    return processor.process_files_from_paths(matched_files)

# =============================================================================
# CLIPBOARD UTILITIES
# =============================================================================


def copy_to_clipboard(text: str) -> bool:
    try:
        if sys.platform == "darwin":
            subprocess.run(["pbcopy"], input=text, text=True, check=True)
        elif sys.platform == "linux":
            try:
                subprocess.run(["xclip", "-selection", "clipboard"],
                               input=text, text=True, check=True)
            except FileNotFoundError:
                try:
                    subprocess.run(["xsel", "--clipboard", "--input"],
                                   input=text, text=True, check=True)
                except FileNotFoundError:
                    if os.environ.get("WAYLAND_DISPLAY"):
                        subprocess.run(["wl-copy"], input=text,
                                       text=True, check=True)
                    else:
                        raise
        elif sys.platform == "win32":
            subprocess.run(["clip"], input=text, text=True, check=True)
        else:
            return False
        return True
    except Exception:
        return False


def save_to_file(text: str, filename: str = "llm_context.txt") -> bool:
    try:
        with open(filename, 'w', encoding='utf-8') as f:
            f.write(text)
        return True
    except Exception:
        return False


def save_to_zip(file_paths: List[Path], zip_name: str, include_tree: bool = True) -> bool:
    """
    Zip the resolved file paths, preserving their relative structure from cwd.
    Optionally includes a project_structure.txt file generated from the folder tree.
    Returns True on success, False on failure.
    """
    cwd = Path.cwd()
    try:
        with zipfile.ZipFile(zip_name, 'w', compression=zipfile.ZIP_DEFLATED) as zf:
            for file_path in file_paths:
                try:
                    arcname = file_path.relative_to(cwd)
                except ValueError:
                    arcname = file_path
                zf.write(file_path, arcname)

            if include_tree:
                tree_root = _detect_tree_root(cwd)
                label = tree_root.name + "/"
                tree_lines = [label] + _gather_tree(str(tree_root))
                tree_content = "\n".join(tree_lines)
                zf.writestr("project_structure.txt", tree_content)

        return True
    except Exception as e:
        print(f"❌ Zip error: {e}")
        return False


def _slugify(name: str) -> str:
    """Convert a collection name to a safe filename slug."""
    slug = re.sub(r'[^\w\s-]', '', name).strip()
    slug = re.sub(r'[\s-]+', '_', slug)
    return slug.lower()

# =============================================================================
# ENTRY PROCESSING HELPERS
# =============================================================================


def _describe_entry(entry_list: list) -> str:
    """Build a short human-readable description of what a collection contains."""
    parts = []
    string_count = sum(1 for x in entry_list if isinstance(x, str))
    if string_count:
        parts.append(
            f"{string_count} glob pattern{'s' if string_count != 1 else ''}")
    for item in entry_list:
        if isinstance(item, dict):
            mode = item.get("mode", "")
            if mode == "fromFile":
                suffix = " (recursive)" if item.get(
                    "recursive") else " (direct imports)"
                parts.append(
                    f"imports of {Path(item['entry_file']).name}{suffix}")
            elif mode in ("strict", "fuzzy"):
                label = "exact" if mode == "strict" else "fuzzy"
                parts.append(f'{label} search for "{item["query"]}"')
    return ", ".join(parts) if parts else "unknown"


def _process_fromfile_entry(entry: dict, processor: FileProcessor) -> tuple[str, List[Path]]:
    """Resolve a fromFile entry and return (context_str, file_paths)."""
    recursive = entry.get('recursive', False)
    entry_path = Path(entry['entry_file']).resolve()
    project_root = entry_path.parent

    current = entry_path.parent
    while current != current.parent:
        if (current / 'pubspec.yaml').exists():
            project_root = current
            break
        current = current.parent

    resolver = DartImportResolver(project_root)
    file_paths = resolver.collect_files(
        entry['entry_file'], recursive=recursive)

    if not file_paths:
        return "No files found.", []

    return processor.process_files_from_paths(file_paths), file_paths


def _print_file_list(file_paths: List[Path], label: str = "Files included"):
    """Pretty-print a list of resolved file paths."""
    for fp in file_paths:
        try:
            rel = fp.relative_to(Path.cwd())
        except ValueError:
            rel = fp
        print(f"    • {rel}")


# =============================================================================
# MAIN
# =============================================================================


def main():
    parser = argparse.ArgumentParser(
        description='Context Copier - Collect files for LLM context')
    parser.add_argument('-s', '--save', action='store_true',
                        help='Save output to llm_context.txt file')
    parser.add_argument('-v', '--verbose', action='store_true',
                        help='Print the collected content to stdout')
    parser.add_argument('-p', '--paths', action='store_true',
                        help='Copy only the file paths to clipboard instead of file contents')
    parser.add_argument('-z', '--zip', action='store_true',
                        help='Zip the resolved files (preserving relative paths) instead of copying to clipboard')
    parser.add_argument('--no-tree', action='store_true',
                        help='Skip appending the project folder tree to the copied context')
    args = parser.parse_args()

    print("\n" + "=" * 60)
    print("🔍 Context Copier — Select a collection")
    print("=" * 60)

    path_names = list(MY_PATHS.keys())
    max_idx_width = len(str(len(path_names)))

    for i, name in enumerate(path_names, 1):
        entry_list = MY_PATHS[name]
        if not isinstance(entry_list, list):
            entry_list = [entry_list]
        desc = _describe_entry(entry_list)
        print(
            f"  {str(i).rjust(max_idx_width)}. {name}  \033[2m({desc})\033[0m")

    print(f"  {str(len(path_names) + 1).rjust(max_idx_width)}. Exit")
    print()

    try:
        choice = int(input("Select collection: ")) - 1
        if choice == len(path_names):
            print("Goodbye! 👋")
            return 0
        if choice < 0 or choice >= len(path_names):
            print("❌ Invalid selection.")
            return 1

        selected = path_names[choice]
        entry = MY_PATHS[selected]
        config = Config()
        processor = FileProcessor(config)

        print(f"\n📦 Collection: {selected}")

        context = None
        all_collected_paths: List[Path] = []

        if isinstance(entry, list):
            # Unwrap single-item list containing a lone fromFile dict
            if len(entry) == 1 and isinstance(entry[0], dict) and entry[0].get("mode") == "fromFile":
                entry = entry[0]
            else:
                string_patterns = [x for x in entry if isinstance(x, str)]
                dict_entries = [x for x in entry if isinstance(x, dict)]
                context_parts = []

                if string_patterns:
                    print(
                        f"  → Resolving {len(string_patterns)} glob pattern(s)...")
                    result = processor.process_files(string_patterns)
                    matched_files = processor.find_files(string_patterns)
                    all_collected_paths.extend(matched_files)
                    print(f"    ✓ {len(matched_files)} file(s) matched")
                    context_parts.append(result)

                for dict_entry in dict_entries:
                    mode = dict_entry.get("mode", "")

                    if mode == "fromFile":
                        recursive = dict_entry.get('recursive', False)
                        suffix = "recursively" if recursive else "direct imports only"
                        print(
                            f"  → Resolving imports from {Path(dict_entry['entry_file']).name} ({suffix})...")
                        result, file_paths = _process_fromfile_entry(
                            dict_entry, processor)
                        all_collected_paths.extend(file_paths)
                        context_parts.append(result)

                    elif mode in ("strict", "fuzzy"):
                        label = "exact" if mode == "strict" else "fuzzy"
                        print(
                            f"  → {label.capitalize()} search: \"{dict_entry['query']}\"...")
                        matched = find_files_with_content(
                            dict_entry["query"], mode, dict_entry["base_path"])
                        all_collected_paths.extend(matched)
                        print(
                            f"    ✓ {len(matched)} file(s) contain this term")
                        result = run_content_search(
                            dict_entry["query"], mode, dict_entry["base_path"], processor)
                        context_parts.append(result)

                context = "\n\n".join(
                    context_parts) if context_parts else "No content collected."

        # Handle bare dict (after possible unwrap above)
        if isinstance(entry, dict):
            mode = entry.get("mode", "")

            if mode == "fromFile":
                recursive = entry.get('recursive', False)
                suffix = "recursively" if recursive else "direct imports only"
                print(
                    f"  → Resolving imports from {Path(entry['entry_file']).name} ({suffix})...")
                context, file_paths = _process_fromfile_entry(entry, processor)
                all_collected_paths.extend(file_paths)

            elif mode in ("strict", "fuzzy"):
                label = "exact" if mode == "strict" else "fuzzy"
                print(
                    f"  → {label.capitalize()} search: \"{entry['query']}\"...")
                matched = find_files_with_content(
                    entry["query"], mode, entry["base_path"])
                all_collected_paths.extend(matched)
                print(f"    ✓ {len(matched)} file(s) contain this term")
                context = run_content_search(
                    entry["query"], mode, entry["base_path"], processor)

        if context is None:
            context = "No content collected."

        # Deduplicate paths while preserving order
        seen = set()
        unique_paths: List[Path] = []
        for p in all_collected_paths:
            if p not in seen:
                seen.add(p)
                unique_paths.append(p)

        # ── Always print all resolved paths ───────────────────────────────────
        print(f"\n  📁 Included files ({len(unique_paths)}):")
        _print_file_list(unique_paths)

        # ── Append folder tree (unless --no-tree or --paths mode) ────────────
        include_tree = config.include_folder_tree and not args.no_tree and not args.paths
        if include_tree:
            tree_str = build_folder_tree(Path.cwd())
            context += tree_str
            tree_root = _detect_tree_root(Path.cwd())
            print(f"\n  🌲 Project tree appended ({tree_root.name}/)")

        # ── Summary line ──────────────────────────────────────────────────────
        char_count = len(context)
        line_count = context.count('\n')
        print(
            f"\n  📊 {len(unique_paths)} file(s) · {line_count:,} lines · {char_count:,} chars")

        if args.verbose:
            print("\n" + "=" * 60)
            print(f"📋 Content for: {selected}")
            print("=" * 60)
            print(f"\n{context}\n")

        # ── Zip mode (standalone — skips clipboard) ───────────────────────────
        if args.zip:
            zip_name = f"{_slugify(selected)}.zip"
            include_tree_in_zip = not args.no_tree
            print()
            if save_to_zip(unique_paths, zip_name, include_tree=include_tree_in_zip):
                tree_note = " + project_structure.txt" if include_tree_in_zip else ""
                print(
                    f"✅ Saved zip to {zip_name}  ({len(unique_paths)} file(s){tree_note})")
            return 0

        # ── Determine what to copy ─────────────────────────────────────────────
        if args.paths:
            to_copy = "\n".join(
                str(p.relative_to(Path.cwd())) if p.is_relative_to(
                    Path.cwd()) else str(p)
                for p in unique_paths
            )
            copy_label = "paths"
        else:
            to_copy = context
            copy_label = "content"

        print()
        if copy_to_clipboard(to_copy):
            print(f"✅ Copied {copy_label} to clipboard!")
        else:
            print("⚠️  Could not copy to clipboard.")

        if args.save:
            if save_to_file(to_copy):
                print("✅ Saved to llm_context.txt")

        return 0

    except ValueError:
        print("❌ Invalid input. Please enter a number.")
        return 1
    except KeyboardInterrupt:
        print("\n\n👋 Cancelled.")
        return 0
    except Exception as e:
        print(f"❌ Error: {e}")
        import traceback
        traceback.print_exc()
        return 1


if __name__ == "__main__":
    sys.exit(main())