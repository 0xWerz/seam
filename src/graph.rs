//! Call edges between nodes, and the question the map exists to answer:
//! which entry points reach a sensitive operation with no check on the way.

use crate::model::{ClaimKind, Inventory, Map, Node, NodeKind};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

/// A callee name that resolves to more functions than this is too ambiguous
/// to draw an edge for (`get`, `new`, `run`).
const MAX_CANDIDATES: usize = 3;
const MAX_DEPTH: usize = 8;

pub struct Graph<'a> {
    pub forward: HashMap<&'a str, BTreeSet<&'a str>>,
    pub reverse: HashMap<&'a str, BTreeSet<&'a str>>,
    /// Edges asserted by a link, as opposed to inferred from a call by name.
    pub asserted: HashSet<(&'a str, &'a str)>,
    /// Method calls matched by name alone across files. Without types these
    /// are guesses (`x.is_empty()` could be anyone's `is_empty`), so they are
    /// offered as hints and kept out of paths and gaps.
    pub maybe: HashMap<&'a str, BTreeSet<&'a str>>,
}

impl<'a> Graph<'a> {
    pub fn build(inv: &'a Inventory, map: &'a Map) -> Self {
        let mut by_name: HashMap<&str, Vec<&Node>> = HashMap::new();
        for node in inv.nodes.iter().filter(|n| n.kind != NodeKind::File) {
            by_name.entry(node.name.as_str()).or_default().push(node);
        }

        let mut graph = Graph {
            forward: HashMap::new(),
            reverse: HashMap::new(),
            asserted: HashSet::new(),
            maybe: HashMap::new(),
        };
        // The import words of each file, for judging calls that leave it.
        let empty = HashSet::new();
        let mut imports: HashMap<&str, HashSet<&str>> = HashMap::new();
        for file in inv.nodes.iter().filter(|n| n.kind == NodeKind::File) {
            imports.insert(
                &file.path,
                file.imports.iter().map(String::as_str).collect(),
            );
        }
        let cut: HashSet<(&str, &str)> = map
            .cuts
            .iter()
            .map(|c| (c.from.as_str(), c.to.as_str()))
            .collect();
        for node in &inv.nodes {
            let hints = imports.get(node.path.as_str()).unwrap_or(&empty);
            let calls = node
                .calls
                .iter()
                .map(|c| (crate::scan::bare_name(c), Style::of(c)));
            let mentions = node.mentions.iter().map(|m| (m.as_str(), Style::Mention));
            for (name, style) in calls.chain(mentions) {
                let Some(candidates) = by_name.get(name) else {
                    continue;
                };
                for target in resolve(node, name, style, candidates, hints) {
                    if cut.contains(&(node.id.as_str(), target.id.as_str())) {
                        continue;
                    }
                    let guess = style == Style::OnValue
                        && target.path != node.path
                        && target.kind == NodeKind::Method;
                    if guess {
                        graph.maybe.entry(&node.id).or_default().insert(&target.id);
                    } else {
                        graph.add(&node.id, &target.id);
                    }
                }
            }
        }
        let known: HashSet<&str> = inv.nodes.iter().map(|n| n.id.as_str()).collect();
        for link in &map.links {
            if known.contains(link.from.as_str()) && known.contains(link.to.as_str()) {
                graph.add(&link.from, &link.to);
                graph.asserted.insert((&link.from, &link.to));
            }
        }
        graph
    }

    fn add(&mut self, from: &'a str, to: &'a str) {
        self.forward.entry(from).or_default().insert(to);
        self.reverse.entry(to).or_default().insert(from);
    }

    pub fn callees(&self, id: &str) -> Vec<&'a str> {
        self.forward
            .get(id)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default()
    }

    pub fn may_call(&self, id: &str) -> Vec<&'a str> {
        self.maybe
            .get(id)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default()
    }

    pub fn callers(&self, id: &str) -> Vec<&'a str> {
        self.reverse
            .get(id)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    /// `name()`
    Plain,
    /// `value.name()`
    OnValue,
    /// `Type::name()`
    Path,
    /// Named without being called.
    Mention,
}

impl Style {
    fn of(call: &str) -> Self {
        if call.starts_with("::") {
            Self::Path
        } else if call.starts_with('.') {
            Self::OnValue
        } else {
            Self::Plain
        }
    }
}

/// Languages that can call each other directly.
fn family(lang: &str) -> &str {
    match lang {
        "javascript" | "typescript" | "tsx" => "js",
        "c" | "cpp" => "c",
        other => other,
    }
}

/// In these languages `value.name()` can only be a method, never a free function.
fn methods_only_on_values(lang: &str) -> bool {
    matches!(lang, "rust" | "java" | "csharp" | "cpp" | "php" | "ruby")
}

/// A name made of several words (`require_login`, `showOrder`) is unlikely to
/// be a local variable or a coincidence. A single common word is.
fn distinctive(name: &str) -> bool {
    name.chars().count() >= 6
        && (name.trim_matches('_').contains('_') || name.chars().skip(1).any(|c| c.is_uppercase()))
}

/// In these languages a function in another file is out of reach unless that
/// file is imported. Elsewhere, files in one directory share a package.
fn needs_import(lang: &str) -> bool {
    matches!(family(lang), "js" | "python")
}

/// File names that say nothing: the directory is the module.
const GENERIC_STEMS: &[&str] = &["mod", "index", "__init__", "lib", "main"];

/// How strongly `hints` (the words on a file's import lines) point at `path`:
/// the number of trailing path parts they name, counted from the file name
/// upwards. `"../src/scanner"` gives `src/scanner.ts` two and
/// `web/lib/scanner.ts` one.
fn import_score(path: &str, hints: &HashSet<&str>) -> usize {
    let mut parts: Vec<String> = path.split('/').map(str::to_lowercase).collect();
    if let Some(file) = parts.last_mut()
        && let Some((stem, _)) = file.rsplit_once('.')
    {
        *file = stem.to_string();
    }
    if parts
        .last()
        .is_some_and(|stem| GENERIC_STEMS.contains(&stem.as_str()))
    {
        parts.pop();
    }
    parts
        .iter()
        .rev()
        .take(3)
        .take_while(|part| hints.contains(part.as_str()))
        .count()
}

/// How many leading directories two paths have in common.
fn shared_dirs(a: &str, b: &str) -> usize {
    let a: Vec<&str> = a.split('/').collect();
    let b: Vec<&str> = b.split('/').collect();
    a.iter()
        .zip(&b)
        .take(a.len().min(b.len()).saturating_sub(1))
        .take_while(|(x, y)| x == y)
        .count()
}

fn directory(path: &str) -> &str {
    path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("")
}

/// Picks what a name most plausibly refers to. Matching by name alone is
/// noisy in a large repo, so every rule here removes candidates; an edge is
/// only drawn when few candidates are left and the caller's file has a
/// reason to reach them.
fn resolve<'a>(
    from: &Node,
    name: &str,
    style: Style,
    candidates: &[&'a Node],
    hints: &HashSet<&str>,
) -> Vec<&'a Node> {
    let mut left: Vec<&Node> = candidates
        .iter()
        .copied()
        .filter(|c| c.id != from.id && family(&c.lang) == family(&from.lang))
        // Production code does not call into tests.
        .filter(|c| from.is_test || !c.is_test)
        .filter(|c| {
            let same_file = c.path == from.path;
            match style {
                Style::Plain => c.kind == NodeKind::Function || same_file,
                Style::OnValue => {
                    if c.kind == NodeKind::Method {
                        true
                    } else {
                        // `x.json()` is never the plain function `json` defined
                        // beside it. It can only be a function reached through
                        // an imported module (`scanner.scanDiff()`).
                        !same_file && !methods_only_on_values(&from.lang)
                    }
                }
                Style::Path => true,
                Style::Mention => same_file || distinctive(name),
            }
        })
        .collect();

    let local: Vec<&Node> = left
        .iter()
        .copied()
        .filter(|c| c.path == from.path)
        .collect();
    if !local.is_empty() {
        left = local;
    } else {
        let best = left
            .iter()
            .map(|c| import_score(&c.path, hints))
            .max()
            .unwrap_or(0);
        if best > 0 {
            left.retain(|c| import_score(&c.path, hints) == best);
            // `import "./diff"` names a file without saying which directory.
            // Between equally named files, the import means the nearest one.
            let nearest = left
                .iter()
                .map(|c| shared_dirs(&c.path, &from.path))
                .max()
                .unwrap_or(0);
            left.retain(|c| shared_dirs(&c.path, &from.path) == nearest);
        } else if needs_import(&from.lang) {
            left.clear();
        } else {
            left.retain(|c| directory(&c.path) == directory(&from.path));
        }
    }
    if left.len() > MAX_CANDIDATES {
        Vec::new()
    } else {
        left
    }
}

/// Which kinds each node has been claimed as.
pub fn kinds_by_node(map: &Map) -> BTreeMap<&str, BTreeSet<ClaimKind>> {
    let mut out: BTreeMap<&str, BTreeSet<ClaimKind>> = BTreeMap::new();
    for claim in &map.claims {
        out.entry(claim.node.as_str())
            .or_default()
            .insert(claim.kind);
    }
    out
}

pub struct Reach<'a> {
    pub entry: &'a str,
    pub sink: &'a str,
    pub path: Vec<&'a str>,
    /// True when a guard is recorded on the path or called from it.
    pub guarded: bool,
    /// The guards that were found, for explaining why a path counts as checked.
    pub guards: Vec<&'a str>,
}

/// Every entry-to-sink path the graph can see, shortest first per pair.
pub fn reaches<'a>(graph: &Graph<'a>, map: &'a Map) -> Vec<Reach<'a>> {
    let kinds = kinds_by_node(map);
    let has = |id: &str, kind: ClaimKind| kinds.get(id).is_some_and(|k| k.contains(&kind));
    // A node counts as checked if it is a guard itself or calls one directly.
    let guards_at = |id: &'a str| -> Vec<&'a str> {
        let mut found: Vec<&str> = graph
            .callees(id)
            .into_iter()
            .filter(|c| has(c, ClaimKind::Guard))
            .collect();
        if has(id, ClaimKind::Guard) {
            found.insert(0, id);
        }
        found
    };

    let mut out = Vec::new();
    for (&entry, _) in kinds.iter().filter(|(_, k)| k.contains(&ClaimKind::Entry)) {
        let mut seen: HashSet<&str> = HashSet::from([entry]);
        let mut queue: VecDeque<Vec<&str>> = VecDeque::from([vec![entry]]);
        while let Some(path) = queue.pop_front() {
            let last = *path.last().unwrap();
            if last != entry && has(last, ClaimKind::Sink) {
                let mut guards: Vec<&str> = path.iter().flat_map(|id| guards_at(id)).collect();
                guards.dedup();
                out.push(Reach {
                    entry,
                    sink: last,
                    guarded: !guards.is_empty(),
                    guards,
                    path: path.clone(),
                });
            }
            if path.len() > MAX_DEPTH {
                continue;
            }
            for next in graph.callees(last) {
                if seen.insert(next) {
                    let mut longer = path.clone();
                    longer.push(next);
                    queue.push_back(longer);
                }
            }
        }
    }
    out
}

/// Everything reachable from `start` by following edges, nearest first.
pub fn reachable<'a>(graph: &Graph<'a>, start: &'a str) -> Vec<&'a str> {
    let mut seen: HashSet<&str> = HashSet::from([start]);
    let mut order = vec![start];
    let mut queue: VecDeque<&str> = VecDeque::from([start]);
    while let Some(id) = queue.pop_front() {
        for next in graph.callees(id) {
            if seen.insert(next) {
                order.push(next);
                queue.push_back(next);
            }
        }
    }
    order
}

/// Entries that are also sinks themselves, with no guard on the node or called from it.
pub fn bare_entry_sinks<'a>(graph: &Graph<'a>, map: &'a Map) -> Vec<&'a str> {
    let kinds = kinds_by_node(map);
    kinds
        .iter()
        .filter(|(id, k)| {
            k.contains(&ClaimKind::Entry)
                && k.contains(&ClaimKind::Sink)
                && !k.contains(&ClaimKind::Guard)
                && !graph
                    .callees(id)
                    .iter()
                    .any(|c| kinds.get(c).is_some_and(|k| k.contains(&ClaimKind::Guard)))
        })
        .map(|(id, _)| *id)
        .collect()
}
