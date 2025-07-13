//! Builds the inventory: every reviewable unit of code in the repo.

use crate::lang::{self, Lang, Registry};
use crate::model::{INVENTORY_VERSION, Inventory, Node, NodeKind, Stamp};
use anyhow::Result;
use ignore::WalkBuilder;
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use tree_sitter::{Parser, QueryCursor, StreamingIterator};

const MAX_FILE_BYTES: u64 = 1_000_000;
/// A file node is only worth reviewing on its own when this many non-blank
/// lines sit outside every function.
const MIN_OWN_LINES: usize = 3;

const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "vendor",
    "target",
    "dist",
    "build",
    "out",
    ".next",
    ".nuxt",
    ".venv",
    "venv",
    "__pycache__",
    ".git",
    ".seam",
    "coverage",
    ".idea",
    ".vscode",
    "third_party",
    "Pods",
];

pub fn hash(text: &str) -> String {
    let digest = Sha256::digest(text.trim().as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn is_test_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    let in_test_dir = lower.split('/').any(|part| {
        matches!(
            part,
            "test"
                | "tests"
                | "__tests__"
                | "spec"
                | "specs"
                | "testdata"
                | "fixtures"
                | "e2e"
                | "mocks"
                | "__mocks__"
                | "bench"
                | "benches"
                | "benchmarks"
        )
    });
    let name = lower.rsplit('/').next().unwrap_or("");
    in_test_dir
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.contains(".bench.")
        || name.contains("_test.")
        || name.starts_with("test_")
}

/// A file's path, its stamp, whether it was parsed just now, and its units.
type Parsed = (String, Stamp, bool, Vec<Node>);

enum Job<'a> {
    /// Unchanged since the last scan: reuse what was parsed then.
    Cached(Vec<Node>),
    Parse {
        path: PathBuf,
        lang: Option<&'a Lang>,
    },
}

/// Scans the repo. Files unchanged since `previous` are not parsed again.
pub fn scan(root: &Path, previous: Option<Inventory>) -> Result<Inventory> {
    let registry = Registry::new();

    let (old_stamps, old_names) = previous
        .as_ref()
        .map(|p| (p.files.clone(), p.names.clone()))
        .unwrap_or_default();
    let mut old_nodes: HashMap<String, Vec<Node>> = HashMap::new();
    for node in previous.map(|p| p.nodes).unwrap_or_default() {
        old_nodes.entry(node.path.clone()).or_default().push(node);
    }

    let walker = WalkBuilder::new(root)
        .hidden(true)
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !(e.file_type().is_some_and(|t| t.is_dir()) && SKIP_DIRS.contains(&name.as_ref()))
        })
        .build();

    let mut paths = Vec::new();
    for entry in walker.flatten() {
        if entry.file_type().is_some_and(|t| t.is_file()) {
            paths.push(entry.into_path());
        }
    }
    paths.sort();

    let mut jobs: Vec<(String, Stamp, Job)> = Vec::new();
    for path in paths {
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        if meta.len() > MAX_FILE_BYTES || meta.len() == 0 {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if file_name.ends_with(".min.js") || file_name.ends_with(".d.ts") {
            continue;
        }

        let lang = registry.for_ext(&ext);
        if lang.is_none() && !lang::is_plain(&file_name, &ext) {
            continue;
        }
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let stamp = Stamp {
            modified,
            size: meta.len(),
        };
        let cached = (old_stamps.get(&rel) == Some(&stamp))
            .then(|| old_nodes.remove(&rel))
            .flatten();
        let job = match cached {
            Some(nodes) => Job::Cached(nodes),
            None => Job::Parse { path, lang },
        };
        jobs.push((rel, stamp, job));
    }

    // Parsing is the slow part and each file is independent.
    let parsed: Vec<Option<Parsed>> = jobs
        .into_par_iter()
        .map_init(Parser::new, |parser, (rel, stamp, job)| match job {
            Job::Cached(nodes) => Some((rel, stamp, false, nodes)),
            Job::Parse { path, lang } => {
                let source = fs::read_to_string(&path).ok()?;
                let nodes = parse_source(parser, lang, &rel, &source);
                Some((rel, stamp, true, nodes))
            }
        })
        .collect();

    let mut nodes = Vec::new();
    let mut files: BTreeMap<String, Stamp> = BTreeMap::new();
    let mut changed: HashSet<String> = HashSet::new();
    for (rel, stamp, was_parsed, file_nodes) in parsed.into_iter().flatten() {
        if was_parsed {
            changed.insert(rel.clone());
        }
        files.insert(rel, stamp);
        nodes.extend(file_nodes);
    }

    let mut names: Vec<&str> = nodes
        .iter()
        .filter(|n| n.kind != NodeKind::File)
        .map(|n| n.name.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    let names = hash(&names.join("\n"));
    // Mentions depend on every function name in the repo: a new or renamed
    // function can change them in files that did not change themselves.
    let only = (names == old_names).then_some(&changed);
    add_mentions(root, &mut nodes, only);
    Ok(Inventory {
        version: INVENTORY_VERSION,
        nodes,
        files,
        names,
    })
}

/// The units in one file's source. `lang` is `None` for files kept whole.
pub fn parse_source(
    parser: &mut Parser,
    lang: Option<&Lang>,
    rel: &str,
    source: &str,
) -> Vec<Node> {
    let mut nodes = Vec::new();
    match lang {
        Some(lang) => scan_parsed(parser, lang, rel, source, &mut nodes),
        None => nodes.push(file_node(rel, "plain", source, true)),
    }
    nodes
}

/// Parses one file as it was, for comparing against an older version.
pub fn parse_text(rel: &str, source: &str) -> Vec<Node> {
    let registry = Registry::new();
    let ext = rel.rsplit('.').next().unwrap_or("").to_lowercase();
    let file_name = rel.rsplit('/').next().unwrap_or(rel);
    let lang = registry.for_ext(&ext);
    if lang.is_none() && !lang::is_plain(file_name, &ext) {
        return Vec::new();
    }
    parse_source(&mut Parser::new(), lang, rel, source)
}

/// Whether a line is part of an import statement, and whether the statement
/// carries on to the next line (`import {` ... `} from "./x"`).
fn import_line(line: &str, inside: bool) -> (bool, bool) {
    let text = line.trim_start();
    if inside {
        let ends = text.contains('}') || text.contains(')') || text.contains(';');
        return (true, !ends);
    }
    let text = text
        .trim_start_matches("pub ")
        .trim_start_matches("export ")
        .trim_start_matches('#');
    let first = text
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .next()
        .unwrap_or("");
    let starts = matches!(
        first,
        "import"
            | "from"
            | "use"
            | "using"
            | "include"
            | "extern"
            | "mod"
            | "require"
            | "require_relative"
            | "load"
    );
    let is_import = starts || text.contains("require(") || text.contains("import(");
    if !is_import {
        return (false, false);
    }
    let opens = text.matches('{').count() + text.matches('(').count();
    let closes = text.matches('}').count() + text.matches(')').count();
    (true, opens > closes)
}

/// The line with the contents of its quoted strings removed. A word inside a
/// string (`"POST, OPTIONS"`) is not a reference to a function of that name.
fn without_strings(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in line.chars() {
        match quote {
            Some(q) => {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == q {
                    quote = None;
                    out.push(ch);
                }
            }
            None => {
                if matches!(ch, '"' | '\'' | '`') {
                    quote = Some(ch);
                }
                out.push(ch);
            }
        }
    }
    out
}

/// A name shorter than this is too likely to be a local variable.
const MIN_MENTION_CHARS: usize = 4;

/// Second pass: now that every function name is known, record where one is
/// named without being called. That is how a decorator, a handler passed to
/// a router or a callback connects to the function it refers to, in any
/// language and any framework.
fn add_mentions(root: &Path, nodes: &mut [Node], only: Option<&HashSet<String>>) {
    let names: std::collections::HashSet<String> = nodes
        .iter()
        .filter(|n| n.kind != NodeKind::File && n.name.chars().count() >= MIN_MENTION_CHARS)
        .map(|n| n.name.clone())
        .collect();
    if names.is_empty() {
        return;
    }
    // Nodes of one file sit together, file node first.
    nodes
        .par_chunk_by_mut(|a, b| a.path == b.path)
        .for_each(|group| {
            let path = group[0].path.clone();
            if only.is_some_and(|changed| !changed.contains(&path)) {
                return;
            }
            let Ok(source) = fs::read_to_string(root.join(&path)) else {
                return;
            };
            let lines: Vec<&str> = source.lines().collect();
            let mut in_function = vec![false; lines.len() + 2];
            for node in group.iter().filter(|n| n.kind != NodeKind::File) {
                let last = node.end_line.min(lines.len());
                in_function[node.start_line..=last].fill(true);
            }
            // What each line says once its string literals and import
            // statements are taken out: neither is a use of a function.
            let mut code: Vec<String> = Vec::with_capacity(lines.len());
            let mut importing = false;
            for line in &lines {
                let (is_import, continues) = import_line(line, importing);
                importing = continues;
                code.push(if is_import {
                    String::new()
                } else {
                    without_strings(line)
                });
            }
            for node in group.iter_mut() {
                let is_file = node.kind == NodeKind::File;
                let mut found: BTreeSet<String> = BTreeSet::new();
                let last = node.end_line.min(lines.len());
                for number in node.start_line..=last {
                    // A file node only owns the lines outside its functions.
                    if is_file && in_function[number] {
                        continue;
                    }
                    for token in code[number - 1]
                        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
                    {
                        if names.contains(token)
                            && token != node.name
                            && !node.calls.iter().any(|c| bare_name(c) == token)
                        {
                            found.insert(token.to_string());
                        }
                    }
                }
                node.mentions = found.into_iter().collect();
            }
        });
}

fn file_node(rel: &str, lang: &str, source: &str, reviewable: bool) -> Node {
    Node {
        id: rel.to_string(),
        kind: NodeKind::File,
        name: rel.rsplit('/').next().unwrap_or(rel).to_string(),
        path: rel.to_string(),
        lang: lang.to_string(),
        start_line: 1,
        end_line: source.lines().count().max(1),
        hash: hash(source),
        reviewable,
        is_test: is_test_path(rel),
        calls: Vec::new(),
        mentions: Vec::new(),
        imports: import_hints(source),
    }
}

/// Words that introduce a dependency on another file, across languages.
const IMPORT_WORDS: &[&str] = &[
    "import", "from", "require", "use", "using", "include", "mod", "extern", "load",
];
const MAX_HINTS: usize = 600;

/// Collects the words on a file's import lines, and the parts of any
/// `a::b::c` path. This is not a parser for any one language's imports: it
/// only needs to answer "is that other file named anywhere in this one's
/// dependencies".
fn import_hints(source: &str) -> Vec<String> {
    let mut hints: BTreeSet<String> = BTreeSet::new();
    let word = |c: char| c.is_alphanumeric() || c == '_';
    for line in source.lines() {
        let mut tokens = line.split(|c: char| !word(c)).filter(|t| t.len() >= 2);
        let imports = tokens.clone().any(|t| IMPORT_WORDS.contains(&t));
        if imports {
            hints.extend(
                tokens
                    .by_ref()
                    .filter(|t| !IMPORT_WORDS.contains(t))
                    .map(str::to_lowercase),
            );
        } else if line.contains("::") {
            for chunk in line.split(|c: char| !(word(c) || c == ':')) {
                if chunk.contains("::") {
                    hints.extend(
                        chunk
                            .split("::")
                            .filter(|t| t.len() >= 2)
                            .map(str::to_lowercase),
                    );
                }
            }
        }
        if hints.len() > MAX_HINTS {
            break;
        }
    }
    hints.into_iter().collect()
}

struct Def {
    name: String,
    is_method: bool,
    start_byte: usize,
    end_byte: usize,
    start_line: usize,
    end_line: usize,
    qualifier: Vec<String>,
    /// Inside a class, impl block or other type, as opposed to a function.
    in_type: bool,
}

fn scan_parsed(parser: &mut Parser, lang: &Lang, rel: &str, source: &str, out: &mut Vec<Node>) {
    if parser.set_language(&lang.language).is_err() {
        out.push(file_node(rel, lang.name, source, true));
        return;
    }
    let Some(tree) = parser.parse(source, None) else {
        out.push(file_node(rel, lang.name, source, true));
        return;
    };
    let bytes = source.as_bytes();
    let names = lang.query.capture_names();
    let name_idx = names.iter().position(|n| *n == "name");

    let mut defs: Vec<Def> = Vec::new();
    // (byte offset, callee name)
    let mut calls: Vec<(usize, String)> = Vec::new();

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&lang.query, tree.root_node(), bytes);
    while let Some(m) = matches.next() {
        let name_node = m
            .captures()
            .iter()
            .find(|c| Some(c.index as usize) == name_idx)
            .map(|c| c.node);
        for cap in m.captures() {
            let cap_name = names[cap.index as usize];
            let Some(name_node) = name_node else { continue };
            let Ok(name) = name_node.utf8_text(bytes) else {
                continue;
            };
            if cap_name == "reference.call" || cap_name == "reference.class" {
                calls.push((
                    cap.node.start_byte(),
                    format!("{}{name}", call_style(name_node)),
                ));
            } else if cap_name == "definition.function" || cap_name == "definition.method" {
                let Some(node) = body_node(cap.node) else {
                    continue;
                };
                let node = with_leading_markers(node);
                let (qualifier, in_type) = qualifier(cap.node, bytes);
                defs.push(Def {
                    name: name.to_string(),
                    is_method: cap_name == "definition.method",
                    start_byte: node.0,
                    end_byte: node.1,
                    start_line: node.2,
                    end_line: node.3,
                    qualifier,
                    in_type,
                });
            }
        }
    }

    collect_callbacks(tree.root_node(), bytes, &mut defs);

    // The same function can match more than one pattern.
    defs.sort_by_key(|d| (d.start_byte, std::cmp::Reverse(d.end_byte)));
    defs.dedup_by_key(|d| (d.start_byte, d.end_byte));

    // Lines covered by any function, to see what the file itself still holds.
    let total_lines = source.lines().count();
    let mut covered = vec![false; total_lines + 2];
    for d in &defs {
        covered[d.start_line..=d.end_line.min(total_lines)].fill(true);
    }
    let own_lines = source
        .lines()
        .enumerate()
        .filter(|(i, l)| !covered[i + 1] && !l.trim().is_empty())
        .count();
    out.push(file_node(
        rel,
        lang.name,
        source,
        own_lines >= MIN_OWN_LINES || defs.is_empty(),
    ));

    let is_test = is_test_path(rel);
    let mut seen: HashMap<String, usize> = HashMap::new();
    for d in &defs {
        let mut qualified = d.qualifier.clone();
        qualified.push(d.name.clone());
        let mut id = format!("{rel}#{}", qualified.join("."));
        let count = seen.entry(id.clone()).or_insert(0);
        *count += 1;
        if *count > 1 {
            id = format!("{id}~{count}");
        }
        // Hashed by whole lines, the same text `node_text` hands to the proof check.
        let text = source
            .lines()
            .skip(d.start_line - 1)
            .take(d.end_line - d.start_line + 1)
            .collect::<Vec<_>>()
            .join("\n");
        let mut callees: BTreeSet<String> = BTreeSet::new();
        for (offset, callee) in &calls {
            if *offset >= d.start_byte && *offset < d.end_byte && bare_name(callee) != d.name {
                callees.insert(callee.clone());
            }
        }
        out.push(Node {
            id,
            kind: if d.is_method || d.in_type {
                NodeKind::Method
            } else {
                NodeKind::Function
            },
            name: d.name.clone(),
            path: rel.to_string(),
            lang: lang.name.to_string(),
            start_line: d.start_line,
            end_line: d.end_line,
            hash: hash(&text),
            reviewable: true,
            // Tests often live beside the code: a `tests` module, or a test name.
            is_test: is_test
                || d.name.starts_with("test_")
                || d.name.starts_with("Test")
                || d.qualifier.iter().any(|q| q == "tests" || q == "test"),
            calls: callees.into_iter().collect(),
            mentions: Vec::new(),
            imports: Vec::new(),
        });
    }
}

const ANONYMOUS: &[&str] = &[
    "arrow_function",
    "function_expression",
    "func_literal",
    "anonymous_function",
    "closure_expression",
    "lambda_expression",
    "do_block",
];
/// Shorter callbacks stay part of whatever contains them.
const MIN_CALLBACK_LINES: usize = 3;

/// An anonymous function passed to a call is often where the real work is:
/// `app.get("/users", (req, res) => { ... })`. It has no name of its own, so
/// it is named after the call it is passed to and that call's first string:
/// `app.get:/users`. This holds for any library, in any language: routes,
/// event names, queue topics, command names.
fn collect_callbacks(root: tree_sitter::Node, bytes: &[u8], defs: &mut Vec<Def>) {
    let mut cursor = root.walk();
    let mut done = false;
    while !done {
        let node = cursor.node();
        if ANONYMOUS.contains(&node.kind())
            && let Some(def) = callback(node, bytes)
        {
            defs.push(def);
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                done = true;
                break;
            }
        }
    }
}

fn callback(node: tree_sitter::Node, bytes: &[u8]) -> Option<Def> {
    let lines = node.end_position().row - node.start_position().row + 1;
    if lines < MIN_CALLBACK_LINES {
        return None;
    }
    // Find the call this function is an argument of (or, in Ruby, a block of).
    let mut holder = node.parent()?;
    let mut arguments = None;
    if holder.kind() == "argument" {
        holder = holder.parent()?;
    }
    if holder.kind().contains("argument") {
        arguments = Some(holder);
        holder = holder.parent()?;
    }
    if arguments.is_none() && node.kind() != "do_block" {
        return None;
    }
    let callee = holder
        .child_by_field_name("function")
        .or_else(|| holder.child_by_field_name("method"))
        .or_else(|| holder.child_by_field_name("name"))
        .and_then(|c| c.utf8_text(bytes).ok())?;
    let callee: String = callee
        .chars()
        .filter(|c| !c.is_whitespace())
        .take(48)
        .collect();
    if callee.is_empty() {
        return None;
    }
    let arguments = arguments.or_else(|| holder.child_by_field_name("arguments"));
    let label = arguments.and_then(|args| {
        let mut walk = args.walk();
        args.named_children(&mut walk)
            .find(|c| c.kind().contains("string") || c.kind().contains("symbol"))
            .and_then(|c| c.utf8_text(bytes).ok())
            .map(|text| {
                text.trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | ':'))
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .take(48)
                    .collect::<String>()
            })
    });
    // Only a callback registered under a string gets its own unit. The rest
    // (`.then(...)`, `.map(...)`, timers) are part of the code around them,
    // and splitting them out would bury a repo in fragments.
    let label = label.filter(|l| !l.is_empty())?;
    let name = format!("{callee}:{label}");
    Some(Def {
        name,
        is_method: false,
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        start_line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
        qualifier: Vec::new(),
        in_type: false,
    })
}

/// A call is stored with a prefix saying how it was written, because that
/// narrows what it can refer to: `.name` was called on a value (`x.name()`),
/// `::name` through a path (`Type::name()`), and a bare `name` on its own.
fn call_style(name_node: tree_sitter::Node) -> &'static str {
    let Some(parent) = name_node.parent() else {
        return "";
    };
    let kind = parent.kind();
    if kind.contains("scoped") || kind.contains("qualified") {
        return "::";
    }
    let on_value = ["field", "member", "attribute", "selector"]
        .iter()
        .any(|k| kind.contains(k))
        || parent.child_by_field_name("object").is_some()
        || parent.child_by_field_name("receiver").is_some();
    if on_value { "." } else { "" }
}

pub fn bare_name(call: &str) -> &str {
    call.trim_start_matches("::").trim_start_matches('.')
}

/// Some grammars tag only the declarator of a function (C, C++). Climb to the
/// node that holds the body; a bare prototype has none and is skipped.
fn body_node(node: tree_sitter::Node) -> Option<tree_sitter::Node> {
    if node.kind() != "function_declarator" {
        return Some(node);
    }
    let mut current = node;
    while let Some(parent) = current.parent() {
        if parent.kind() == "function_definition" {
            return Some(parent);
        }
        if !parent.kind().ends_with("declarator") {
            return None;
        }
        current = parent;
    }
    None
}

/// Decorators, attributes and annotations often sit just outside the function
/// node, and they are exactly where permission checks get declared. Pull them
/// (and doc comments) into the node's range.
fn with_leading_markers(node: tree_sitter::Node) -> (usize, usize, usize, usize) {
    let mut outer = node;
    while let Some(parent) = outer.parent() {
        let kind = parent.kind();
        // `const f = () => {}`: the declaration holds this one function.
        let sole_declaration = (kind == "lexical_declaration" || kind == "variable_declaration")
            && parent.named_child_count() == 1;
        if kind.contains("decorated") || kind == "export_statement" || sole_declaration {
            outer = parent;
        } else {
            break;
        }
    }
    let mut start = outer;
    while let Some(prev) = start.prev_sibling() {
        let kind = prev.kind();
        let is_marker = kind.contains("attribute")
            || kind.contains("decorator")
            || kind.contains("annotation")
            || kind.contains("comment");
        // Only markers directly above, with no blank line in between.
        if !is_marker
            || start
                .start_position()
                .row
                .saturating_sub(prev.end_position().row)
                > 1
        {
            break;
        }
        start = prev;
    }
    (
        start.start_byte(),
        outer.end_byte(),
        start.start_position().row + 1,
        outer.end_position().row + 1,
    )
}

/// Names of the enclosing classes, modules, traits or impl blocks.
fn qualifier(node: tree_sitter::Node, bytes: &[u8]) -> (Vec<String>, bool) {
    let mut parts = Vec::new();
    let mut in_type = false;
    let clean = |name: &str| -> String {
        // `Graph<'a>` and `Vec<T>` read better, and stay stable, as the bare type name.
        let bare = name.split('<').next().unwrap_or(name);
        bare.chars()
            .filter(|c| !c.is_whitespace())
            .take(60)
            .collect()
    };
    // `func (s *Server) handle()`: the type is the last word of the receiver.
    if let Some(receiver) = node
        .child_by_field_name("receiver")
        .and_then(|r| r.utf8_text(bytes).ok())
    {
        let type_name = receiver
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .rfind(|t| !t.is_empty());
        if let Some(type_name) = type_name {
            parts.push(type_name.to_string());
            in_type = true;
        }
    }
    let mut current = node.parent();
    while let Some(n) = current {
        let kind = n.kind();
        let is_type = [
            "class",
            "module",
            "mod_item",
            "interface",
            "struct",
            "trait",
            "namespace",
            "impl",
            "object_",
            "enum",
        ]
        .iter()
        .any(|k| kind.contains(k));
        // A function inside a function, or inside `const name = ...`, is
        // named after what holds it: `require_admin.wrapper`.
        let is_function = (kind.contains("function") || kind.contains("method"))
            && (kind.ends_with("_definition")
                || kind.ends_with("_declaration")
                || kind.ends_with("_item")
                || kind == "method");
        let is_wrapper =
            kind.ends_with("body") || kind.ends_with("list") || kind.ends_with("block");
        if kind == "object" {
            // An object literal with its own `id` or `name` is named by it:
            // `rules.sql-injection.match` rather than `rules.match~4`.
            let mut walk = n.walk();
            let label = n.named_children(&mut walk).find_map(|pair| {
                let key = pair.child_by_field_name("key")?.utf8_text(bytes).ok()?;
                let value = pair.child_by_field_name("value")?;
                (matches!(key, "id" | "name" | "key") && value.kind().contains("string"))
                    .then(|| value.utf8_text(bytes).ok())
                    .flatten()
            });
            if let Some(label) = label {
                let label = label.trim_matches(|c: char| matches!(c, '"' | '\'' | '`'));
                if !label.is_empty() {
                    parts.push(clean(label));
                }
            }
        } else if (is_type || is_function || kind == "variable_declarator") && !is_wrapper {
            // Only a type block is named by its `type` field (`impl Store`). On a
            // function that field is the return type.
            let name = n
                .child_by_field_name("name")
                .or_else(|| is_type.then(|| n.child_by_field_name("type")).flatten())
                .and_then(|c| c.utf8_text(bytes).ok());
            if let Some(name) = name {
                parts.push(clean(name));
                in_type |= is_type;
            }
        }
        current = n.parent();
    }
    parts.reverse();
    (parts, in_type)
}

/// Returns the node's source text.
pub fn node_text(root: &Path, node: &Node) -> Result<String> {
    let source = fs::read_to_string(root.join(&node.path))?;
    if node.kind == NodeKind::File {
        return Ok(source);
    }
    let text: Vec<&str> = source
        .lines()
        .skip(node.start_line - 1)
        .take(node.end_line - node.start_line + 1)
        .collect();
    Ok(text.join("\n"))
}
