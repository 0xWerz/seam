mod graph;
mod lang;
mod model;
mod proof;
mod scan;
mod view;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use graph::Graph;
use model::{Claim, ClaimKind, ClaimState, Inventory, Link, Map, Node, NodeKind, Note};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, IsTerminal};
use std::path::{Path, PathBuf};

const SKILL: &str = include_str!("../plugin/skills/seam/SKILL.md");
const NOTE_STATUSES: &[&str] = &["idea", "tested", "dead-end", "finding"];

#[derive(Parser)]
#[command(
    name = "seam",
    version,
    about = "Tracks a security review of a codebase and checks every claim against the source."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List every reviewable unit of code in the repo
    Scan {
        /// Repo root (defaults to the current directory)
        path: Option<PathBuf>,
        /// Parse every file again, ignoring what the last scan remembered
        #[arg(long)]
        fresh: bool,
    },
    /// Show how much has been reviewed
    Status {
        #[arg(long)]
        json: bool,
        /// Count test code too
        #[arg(long)]
        tests: bool,
    },
    /// List what still needs review
    Next {
        /// Only units under this path
        #[arg(long)]
        path: Option<String>,
        /// Only units this one reaches, nearest first
        #[arg(long, value_name = "UNIT")]
        reachable_from: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Include test code
        #[arg(long)]
        tests: bool,
        #[arg(long)]
        json: bool,
    },
    /// Print a unit's source with what is already known about it
    Show {
        /// Unit id, or `path:line`
        target: String,
        /// Leave the source out: only connections, claims and notes
        #[arg(long)]
        no_source: bool,
        #[arg(long)]
        json: bool,
    },
    /// Record what a unit is, with a quote from its source
    Claim {
        /// Unit id, or `path:line`
        target: Option<String>,
        /// entry, guard, sink, data or none
        #[arg(long)]
        kind: Option<String>,
        /// What the unit does, in one line
        #[arg(long)]
        summary: Option<String>,
        /// Exact code the claim rests on (not needed for `none`)
        #[arg(long)]
        quote: Option<String>,
        #[arg(long)]
        tag: Vec<String>,
        /// Read claims as JSON lines from standard input
        #[arg(long)]
        stdin: bool,
    },
    /// Remove the claims on a unit
    Unclaim {
        target: String,
        /// Only claims of this kind
        #[arg(long)]
        kind: Option<String>,
    },
    /// Record that one unit leads to another, with a quote from the first
    Link {
        from: Option<String>,
        to: Option<String>,
        #[arg(long)]
        why: Option<String>,
        #[arg(long)]
        quote: Option<String>,
        /// Read links as JSON lines from standard input
        #[arg(long)]
        stdin: bool,
    },

    /// Remove an asserted link, or cut an inferred connection that is wrong
    Unlink { from: String, to: String },
    /// List the connections seam sees, for checking them against the code
    Edges {
        /// Only connections starting under this path
        path: Option<String>,
        /// Include test code
        #[arg(long)]
        tests: bool,
        #[arg(long)]
        json: bool,
    },
    /// Attach a research note to a unit
    Note {
        target: String,
        /// idea, tested, dead-end or finding
        #[arg(long)]
        status: String,
        #[arg(long)]
        text: String,
    },
    /// List research notes
    Notes {
        /// Only notes with this status
        #[arg(long)]
        status: Option<String>,
    },
    /// Remove the research notes on a unit
    Unnote {
        target: String,
        /// Only notes with this status
        #[arg(long)]
        status: Option<String>,
    },
    /// Entry points that reach a sensitive operation with no check recorded
    Gaps {
        /// Also list the paths that count as checked, and by which guard
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// What changed since a git revision, and whether it has been reviewed
    Diff {
        /// A branch, tag or commit to compare against, such as `main`
        rev: String,
        /// Exit with an error if changed code has not been reviewed
        #[arg(long)]
        check: bool,
        /// Include test code
        #[arg(long)]
        tests: bool,
    },
    /// Re-check every claim against the code as it is now
    Verify {
        /// Drop claims whose node or quoted code no longer exists
        #[arg(long)]
        prune: bool,
    },
    /// Write the map as a web page and open it
    View {
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        no_open: bool,
    },
    /// Print the agent skill, for installing into an AI coding agent
    Skill,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("seam: {err:#}");
        std::process::exit(1);
    }
}

struct Project {
    root: PathBuf,
    inv: Inventory,
    map: Map,
    /// Held by commands that change the map, until they finish.
    _lock: Option<model::Lock>,
}

fn open() -> Result<Project> {
    open_with(false)
}

/// For commands that change the map: nothing else may write until this is dropped.
fn open_locked() -> Result<Project> {
    open_with(true)
}

fn open_with(lock: bool) -> Result<Project> {
    let cwd = std::env::current_dir()?;
    let root = model::find_root(&cwd)
        .ok_or_else(|| anyhow!("no .seam directory here or above; run `seam scan` first"))?;
    let _lock = if lock {
        Some(model::Lock::acquire(&root)?)
    } else {
        None
    };
    // Rescan every time so claims are always checked against the current code.
    // Only files that changed since the last run are parsed again.
    let inv = scan::scan(&root, model::load_inventory(&root))?;
    model::save_inventory(&root, &inv)?;
    let map = model::load_map(&root)?;
    Ok(Project {
        root,
        inv,
        map,
        _lock,
    })
}

fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Scan { path, fresh } => cmd_scan(path, fresh),
        Command::Status { json, tests } => cmd_status(json, tests),
        Command::Next {
            path,
            reachable_from,
            limit,
            tests,
            json,
        } => cmd_next(path, reachable_from, limit, tests, json),
        Command::Show {
            target,
            no_source,
            json,
        } => cmd_show(&target, no_source, json),
        Command::Claim {
            target,
            kind,
            summary,
            quote,
            tag,
            stdin,
        } => cmd_claim(target, kind, summary, quote, tag, stdin),
        Command::Unclaim { target, kind } => cmd_unclaim(&target, kind),
        Command::Link {
            from,
            to,
            why,
            quote,
            stdin,
        } => cmd_link(from, to, why, quote, stdin),
        Command::Note {
            target,
            status,
            text,
        } => cmd_note(&target, &status, &text),
        Command::Notes { status } => cmd_notes(status),
        Command::Unlink { from, to } => cmd_unlink(&from, &to),
        Command::Edges { path, tests, json } => cmd_edges(path, tests, json),
        Command::Unnote { target, status } => cmd_unnote(&target, status),
        Command::Gaps { all, json } => cmd_gaps(all, json),
        Command::Diff { rev, check, tests } => cmd_diff(&rev, check, tests),
        Command::Verify { prune } => cmd_verify(prune),
        Command::View { out, no_open } => cmd_view(out, no_open),
        Command::Skill => {
            print!("{SKILL}");
            Ok(())
        }
    }
}

/// Accepts a node id or `path:line`, where the line picks the innermost node.
fn resolve<'a>(inv: &'a Inventory, target: &str) -> Result<&'a Node> {
    if let Some(node) = inv.nodes.iter().find(|n| n.id == target) {
        return Ok(node);
    }
    if let Some((path, line)) = target.rsplit_once(':')
        && let Ok(line) = line.parse::<usize>()
    {
        let innermost = inv
            .nodes
            .iter()
            .filter(|n| n.path == path && n.start_line <= line && line <= n.end_line)
            .min_by_key(|n| n.end_line - n.start_line);
        if let Some(node) = innermost {
            return Ok(node);
        }
    }
    // A bare function name is enough when only one function has it.
    let mut named = inv
        .nodes
        .iter()
        .filter(|n| n.kind != NodeKind::File && n.name == target);
    if let (Some(node), None) = (named.next(), named.next()) {
        return Ok(node);
    }
    let close: Vec<&str> = inv
        .nodes
        .iter()
        .filter(|n| n.id.contains(target))
        .take(5)
        .map(|n| n.id.as_str())
        .collect();
    if close.is_empty() {
        bail!("no unit `{target}`; list candidates with `seam next`");
    }
    bail!(
        "no unit `{target}`; did you mean one of:\n  {}",
        close.join("\n  ")
    );
}

fn state_of(root: &Path, nodes: &BTreeMap<&str, &Node>, claim: &Claim) -> ClaimState {
    let Some(node) = nodes.get(claim.node.as_str()) else {
        return ClaimState::Orphaned;
    };
    if node.hash == claim.hash {
        return ClaimState::Ok;
    }
    if claim.kind == ClaimKind::None {
        return ClaimState::Changed;
    }
    match scan::node_text(root, node) {
        Ok(text) if proof::locate(&text, &claim.quote).is_ok() => ClaimState::Changed,
        _ => ClaimState::Stale,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Review {
    Unreviewed,
    Current,
    /// Reviewed, but the code changed since.
    Outdated,
}

fn review_states(p: &Project) -> HashMap<&str, Review> {
    let nodes = model::index(&p.inv);
    let mut out: HashMap<&str, Review> = HashMap::new();
    for claim in &p.map.claims {
        let state = state_of(&p.root, &nodes, claim);
        if state == ClaimState::Orphaned {
            continue;
        }
        let review = if state == ClaimState::Ok {
            Review::Current
        } else {
            Review::Outdated
        };
        let slot = out.entry(claim.node.as_str()).or_insert(review);
        if review == Review::Outdated {
            *slot = Review::Outdated;
        }
    }
    out
}

fn counted(node: &Node, tests: bool) -> bool {
    node.reviewable && (tests || !node.is_test)
}

/// The directory a node is grouped under in status output.
fn area(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    match parts.len() {
        1 => ".".to_string(),
        2 => parts[0].to_string(),
        _ => format!("{}/{}", parts[0], parts[1]),
    }
}

fn cmd_scan(path: Option<PathBuf>, fresh: bool) -> Result<()> {
    let root = match path {
        Some(p) => p
            .canonicalize()
            .with_context(|| format!("no such directory: {}", p.display()))?,
        None => {
            let cwd = std::env::current_dir()?;
            model::find_root(&cwd).unwrap_or(cwd)
        }
    };
    let previous = if fresh {
        None
    } else {
        model::load_inventory(&root)
    };
    let inv = scan::scan(&root, previous)?;
    model::save_inventory(&root, &inv)?;
    let files = inv
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::File)
        .count();
    let functions = inv.nodes.len() - files;
    let reviewable = inv.nodes.iter().filter(|n| counted(n, false)).count();
    let mut langs: BTreeMap<&str, usize> = BTreeMap::new();
    for node in inv.nodes.iter().filter(|n| n.kind == NodeKind::File) {
        *langs.entry(node.lang.as_str()).or_default() += 1;
    }
    let mut langs: Vec<_> = langs.into_iter().collect();
    langs.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    let langs: Vec<String> = langs.iter().map(|(l, c)| format!("{l} {c}")).collect();
    println!("{files} files, {functions} functions: {reviewable} units to review");
    println!("files by language: {}", langs.join(", "));
    println!("inventory written to {}", root.join(model::DIR).display());
    Ok(())
}

fn cmd_status(json: bool, tests: bool) -> Result<()> {
    let p = open()?;
    let reviews = review_states(&p);
    let nodes = model::index(&p.inv);

    let mut total = 0;
    let mut current = 0;
    let mut outdated = 0;
    let mut areas: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for node in p.inv.nodes.iter().filter(|n| counted(n, tests)) {
        total += 1;
        let slot = areas.entry(area(&node.path)).or_default();
        slot.1 += 1;
        match reviews.get(node.id.as_str()) {
            Some(Review::Current) => {
                current += 1;
                slot.0 += 1;
            }
            Some(Review::Outdated) => outdated += 1,
            _ => {}
        }
    }

    let mut kinds: BTreeMap<ClaimKind, usize> = BTreeMap::new();
    let mut stale = 0;
    let mut orphaned = 0;
    for claim in &p.map.claims {
        *kinds.entry(claim.kind).or_default() += 1;
        match state_of(&p.root, &nodes, claim) {
            ClaimState::Stale => stale += 1,
            ClaimState::Orphaned => orphaned += 1,
            _ => {}
        }
    }

    if json {
        let areas: BTreeMap<&String, serde_json::Value> = areas
            .iter()
            .map(|(a, (done, all))| (a, serde_json::json!({ "reviewed": done, "total": all })))
            .collect();
        let kinds: BTreeMap<&str, usize> = kinds.iter().map(|(k, c)| (k.as_str(), *c)).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "total": total, "reviewed": current, "outdated": outdated,
                "stale_claims": stale, "orphaned_claims": orphaned,
                "claims": kinds, "links": p.map.links.len(), "notes": p.map.notes.len(),
                "areas": areas,
            }))?
        );
        return Ok(());
    }

    let percent = if total == 0 { 0 } else { current * 100 / total };
    println!("{current} of {total} reviewed ({percent}%)");
    if outdated > 0 {
        println!("{outdated} reviewed but changed since: see `seam next`");
    }
    if stale + orphaned > 0 {
        println!(
            "{stale} claims whose quoted code is gone, {orphaned} whose unit is gone: see `seam verify`"
        );
    }
    let count = |k| kinds.get(&k).copied().unwrap_or(0);
    println!(
        "entries {}  guards {}  sinks {}  data {}  nothing {}  links {}  notes {}",
        count(ClaimKind::Entry),
        count(ClaimKind::Guard),
        count(ClaimKind::Sink),
        count(ClaimKind::Data),
        count(ClaimKind::None),
        p.map.links.len(),
        p.map.notes.len()
    );
    if !areas.is_empty() {
        println!();
        let width = areas.keys().map(|a| a.len()).max().unwrap_or(0);
        for (name, (done, all)) in &areas {
            println!("  {name:<width$}  {done}/{all}");
        }
    }
    Ok(())
}

fn cmd_next(
    path: Option<String>,
    reachable_from: Option<String>,
    limit: usize,
    tests: bool,
    json: bool,
) -> Result<()> {
    let p = open()?;
    let reviews = review_states(&p);
    // With a starting unit, the order is distance from it rather than file order.
    let graph = Graph::build(&p.inv, &p.map);
    let distance: Option<HashMap<&str, usize>> = match &reachable_from {
        Some(target) => {
            let start = resolve(&p.inv, target)?;
            Some(
                graph::reachable(&graph, &start.id)
                    .into_iter()
                    .enumerate()
                    .map(|(i, id)| (id, i))
                    .collect(),
            )
        }
        None => None,
    };
    let mut pending: Vec<(&Node, Review)> = p
        .inv
        .nodes
        .iter()
        .filter(|n| counted(n, tests))
        .filter(|n| {
            distance
                .as_ref()
                .is_none_or(|d| d.contains_key(n.id.as_str()))
        })
        .filter(|n| {
            path.as_deref()
                .is_none_or(|prefix| n.path.starts_with(prefix))
        })
        .filter_map(|n| match reviews.get(n.id.as_str()) {
            Some(Review::Current) => None,
            Some(r) => Some((n, *r)),
            None => Some((n, Review::Unreviewed)),
        })
        .collect();
    // Code that changed after review goes first: it was judged once already.
    pending.sort_by_key(|(n, r)| {
        let near = distance
            .as_ref()
            .and_then(|d| d.get(n.id.as_str()).copied())
            .unwrap_or(0);
        (*r != Review::Outdated, near, n.path.clone(), n.start_line)
    });
    let remaining = pending.len();
    pending.truncate(limit);

    if json {
        let items: Vec<_> = pending
            .iter()
            .map(|(n, r)| {
                serde_json::json!({
                    "id": n.id, "kind": n.kind, "path": n.path,
                    "start_line": n.start_line, "end_line": n.end_line,
                    "changed_since_review": *r == Review::Outdated,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({ "remaining": remaining, "nodes": items })
            )?
        );
        return Ok(());
    }
    if pending.is_empty() {
        println!(
            "nothing left to review{}",
            path.map(|p| format!(" under {p}")).unwrap_or_default()
        );
        return Ok(());
    }
    for (node, review) in &pending {
        let lines = node.end_line - node.start_line + 1;
        let flag = if *review == Review::Outdated {
            "  (changed since review)"
        } else {
            ""
        };
        println!("{}  {lines} lines{flag}", node.id);
    }
    if remaining > pending.len() {
        println!(
            "... and {} more not shown: raise --limit to see them",
            remaining - pending.len()
        );
    }
    Ok(())
}

fn cmd_show(target: &str, no_source: bool, json: bool) -> Result<()> {
    let p = open()?;
    let node = resolve(&p.inv, target)?;
    let graph = Graph::build(&p.inv, &p.map);
    let nodes = model::index(&p.inv);

    if json {
        let claims: Vec<_> = p
            .map
            .claims
            .iter()
            .filter(|c| c.node == node.id)
            .map(|c| {
                serde_json::json!({
                    "kind": c.kind.as_str(), "summary": c.summary, "quote": c.quote, "line": c.line,
                    "tags": c.tags, "state": state_of(&p.root, &nodes, c).as_str(),
                })
            })
            .collect();
        let notes: Vec<_> = p.map.notes.iter().filter(|n| n.node == node.id).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "id": node.id, "kind": node.kind, "lang": node.lang, "path": node.path,
                "start_line": node.start_line, "end_line": node.end_line, "is_test": node.is_test,
                "used_by": graph.callers(&node.id), "uses": graph.callees(&node.id),
                "may_call": graph.may_call(&node.id),
                "claims": claims, "notes": notes,
                "source": scan::node_text(&p.root, node)?,
            }))?
        );
        return Ok(());
    }

    let kind = match node.kind {
        NodeKind::Function => "function",
        NodeKind::Method => "method",
        NodeKind::File => "file",
    };
    println!("{}", node.id);
    println!(
        "{kind}, {}, lines {}-{}",
        node.lang, node.start_line, node.end_line
    );
    // Test callers say little about how production code reaches a node.
    let callers: Vec<&str> = graph
        .callers(&node.id)
        .into_iter()
        .filter(|id| node.is_test || !nodes.get(id).is_some_and(|n| n.is_test))
        .collect();
    print_ids("used by", &callers);
    print_ids("uses", &graph.callees(&node.id));
    print_ids(
        "may call (method matched by name only)",
        &graph.may_call(&node.id),
    );
    for claim in p.map.claims.iter().filter(|c| c.node == node.id) {
        let state = state_of(&p.root, &nodes, claim);
        let flag = if state == ClaimState::Ok {
            String::new()
        } else {
            format!(" [{}]", state.as_str())
        };
        println!("claimed {}{flag}: {}", claim.kind.as_str(), claim.summary);
    }
    for note in p.map.notes.iter().filter(|n| n.node == node.id) {
        println!("note ({}): {}", note.status, note.text);
    }
    if no_source {
        return Ok(());
    }
    println!();
    let text = scan::node_text(&p.root, node)?;
    let width = node.end_line.to_string().len();
    // A file unit is what is left outside its functions. Each function is
    // shown as one line pointing at its own unit, not printed a second time.
    let mut inner: Vec<&Node> = if node.kind == NodeKind::File {
        p.inv
            .nodes
            .iter()
            .filter(|n| n.path == node.path && n.kind != NodeKind::File)
            .collect()
    } else {
        Vec::new()
    };
    inner.sort_by_key(|n| (n.start_line, std::cmp::Reverse(n.end_line)));
    let mut skip_until = 0;
    for (i, line) in text.lines().enumerate() {
        let number = node.start_line + i;
        if number <= skip_until {
            continue;
        }
        if let Some(unit) = inner.iter().find(|n| n.start_line == number) {
            let range = format!("{}-{}", unit.start_line, unit.end_line);
            println!("{range:>width$} | [own unit: {}]", unit.id);
            skip_until = unit.end_line;
            continue;
        }
        println!("{number:>width$} | {line}");
    }
    Ok(())
}

fn print_ids(label: &str, ids: &[&str]) {
    const SHOWN: usize = 20;
    if ids.is_empty() {
        return;
    }
    println!("{label}:");
    for id in ids.iter().take(SHOWN) {
        println!("  {id}");
    }
    if ids.len() > SHOWN {
        println!(
            "  ... and {} more: `seam show --json` lists all",
            ids.len() - SHOWN
        );
    }
}

#[derive(Deserialize)]
struct ClaimInput {
    node: String,
    kind: String,
    summary: String,
    #[serde(default)]
    quote: String,
    #[serde(default)]
    tags: Vec<String>,
}

fn cmd_claim(
    target: Option<String>,
    kind: Option<String>,
    summary: Option<String>,
    quote: Option<String>,
    tags: Vec<String>,
    stdin: bool,
) -> Result<()> {
    let mut p = open_locked()?;
    let mut inputs = Vec::new();
    if stdin {
        if std::io::stdin().is_terminal() {
            bail!("--stdin expects JSON lines piped in, one claim per line");
        }
        for (i, line) in std::io::stdin().lock().lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let input: ClaimInput = serde_json::from_str(&line)
                .with_context(|| format!("line {}: not a valid claim", i + 1))?;
            inputs.push(input);
        }
    } else {
        inputs.push(ClaimInput {
            node: target.ok_or_else(|| anyhow!("which unit? give an id or `path:line`"))?,
            kind: kind
                .ok_or_else(|| anyhow!("--kind is required: entry, guard, sink, data or none"))?,
            summary: summary
                .ok_or_else(|| anyhow!("--summary is required: what the unit does, in one line"))?,
            quote: quote.unwrap_or_default(),
            tags,
        });
    }

    let mut accepted = 0;
    let mut rejected = Vec::new();
    for input in inputs {
        match accept_claim(&p, &input) {
            Ok(claim) => {
                // A real claim replaces "nothing here"; the same claim again replaces itself.
                p.map.claims.retain(|c| {
                    !(c.node == claim.node
                        && (c.kind == ClaimKind::None
                            || claim.kind == ClaimKind::None
                            || (c.kind == claim.kind && c.quote == claim.quote)))
                });
                let line = claim
                    .line
                    .map(|l| format!(" (line {l})"))
                    .unwrap_or_default();
                println!("accepted: {} is {}{line}", claim.node, claim.kind.as_str());
                p.map.claims.push(claim);
                accepted += 1;
            }
            Err(err) => rejected.push(format!("rejected: {}: {err}", input.node)),
        }
    }
    if accepted > 0 {
        model::save_map(&p.root, &p.map)?;
    }
    if !rejected.is_empty() {
        bail!("{}", rejected.join("\n"));
    }
    Ok(())
}

fn accept_claim(p: &Project, input: &ClaimInput) -> Result<Claim> {
    let node = resolve(&p.inv, &input.node)?;
    let kind = ClaimKind::parse(&input.kind).ok_or_else(|| {
        anyhow!(
            "unknown kind `{}`: use entry, guard, sink, data or none",
            input.kind
        )
    })?;
    let summary = input.summary.trim();
    if summary.chars().count() < 10 {
        bail!("summary is too short: say what the unit does in one line");
    }
    let mut line = None;
    let mut quote = String::new();
    if kind == ClaimKind::None {
        if p.map
            .claims
            .iter()
            .any(|c| c.node == node.id && c.kind != ClaimKind::None)
        {
            bail!("unit already has claims; `seam unclaim` them first if they were wrong");
        }
    } else {
        if input.quote.trim().is_empty() {
            bail!(
                "--quote is required for `{}`: the exact code the claim rests on",
                kind.as_str()
            );
        }
        let text = scan::node_text(&p.root, node)?;
        let offset = proof::locate(&text, &input.quote).map_err(|e| anyhow!(e.message()))?;
        line = Some(node.start_line + offset);
        quote = input.quote.trim().to_string();
    }
    Ok(Claim {
        node: node.id.clone(),
        kind,
        summary: summary.to_string(),
        quote,
        line,
        tags: input.tags.clone(),
        hash: node.hash.clone(),
    })
}

fn cmd_unclaim(target: &str, kind: Option<String>) -> Result<()> {
    let mut p = open_locked()?;
    // Works on nodes that no longer exist too, so orphaned claims can be removed.
    let id = resolve(&p.inv, target)
        .map(|n| n.id.clone())
        .unwrap_or_else(|_| target.to_string());
    let kind = match kind {
        Some(k) => Some(ClaimKind::parse(&k).ok_or_else(|| anyhow!("unknown kind `{k}`"))?),
        None => None,
    };
    let before = p.map.claims.len();
    p.map
        .claims
        .retain(|c| !(c.node == id && kind.is_none_or(|k| c.kind == k)));
    let removed = before - p.map.claims.len();
    if removed == 0 {
        bail!("no matching claims on `{id}`");
    }
    model::save_map(&p.root, &p.map)?;
    println!("removed {removed} claim(s) from {id}");
    Ok(())
}

#[derive(Deserialize)]
struct LinkInput {
    from: String,
    to: String,
    why: String,
    quote: String,
}

fn cmd_link(
    from: Option<String>,
    to: Option<String>,
    why: Option<String>,
    quote: Option<String>,
    stdin: bool,
) -> Result<()> {
    let mut p = open_locked()?;
    let mut inputs = Vec::new();
    if stdin {
        if std::io::stdin().is_terminal() {
            bail!("--stdin expects JSON lines piped in, one link per line");
        }
        for (i, line) in std::io::stdin().lock().lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let input: LinkInput = serde_json::from_str(&line)
                .with_context(|| format!("line {}: not a valid link", i + 1))?;
            inputs.push(input);
        }
    } else {
        inputs.push(LinkInput {
            from: from.ok_or_else(|| anyhow!("which unit does the link start from?"))?,
            to: to.ok_or_else(|| anyhow!("which unit does the link lead to?"))?,
            why: why.ok_or_else(|| {
                anyhow!("--why is required: how the first unit leads to the second")
            })?,
            quote: quote.ok_or_else(|| {
                anyhow!("--quote is required: code in the first unit that shows it")
            })?,
        });
    }

    let mut accepted = 0;
    let mut rejected = Vec::new();
    for input in inputs {
        match accept_link(&p, &input) {
            Ok(link) => {
                println!("linked: {} -> {} (line {})", link.from, link.to, link.line);
                p.map
                    .links
                    .retain(|l| !(l.from == link.from && l.to == link.to));
                // Asserting a connection overrides an earlier decision that it was wrong.
                p.map
                    .cuts
                    .retain(|c| !(c.from == link.from && c.to == link.to));
                p.map.links.push(link);
                accepted += 1;
            }
            Err(err) => rejected.push(format!("rejected: {} -> {}: {err}", input.from, input.to)),
        }
    }
    if accepted > 0 {
        model::save_map(&p.root, &p.map)?;
    }
    if !rejected.is_empty() {
        bail!("{}", rejected.join("\n"));
    }
    Ok(())
}

fn accept_link(p: &Project, input: &LinkInput) -> Result<Link> {
    let from = resolve(&p.inv, &input.from)?;
    let to = resolve(&p.inv, &input.to)?;
    if input.why.trim().chars().count() < 10 {
        bail!("--why is too short: say how the first unit leads to the second");
    }
    let text = scan::node_text(&p.root, from)?;
    let offset = proof::locate(&text, &input.quote).map_err(|e| anyhow!(e.message()))?;
    Ok(Link {
        from: from.id.clone(),
        to: to.id.clone(),
        why: input.why.trim().to_string(),
        quote: input.quote.trim().to_string(),
        line: from.start_line + offset,
    })
}

fn cmd_note(target: &str, status: &str, text: &str) -> Result<()> {
    let mut p = open_locked()?;
    let node = resolve(&p.inv, target)?;
    if !NOTE_STATUSES.contains(&status) {
        bail!(
            "unknown status `{status}`: use {}",
            NOTE_STATUSES.join(", ")
        );
    }
    if text.trim().is_empty() {
        bail!("--text is empty");
    }
    p.map.notes.push(Note {
        node: node.id.clone(),
        status: status.to_string(),
        text: text.trim().to_string(),
    });
    println!("noted on {}", node.id);
    model::save_map(&p.root, &p.map)
}

fn cmd_notes(status: Option<String>) -> Result<()> {
    let p = open()?;
    let notes: Vec<&Note> = p
        .map
        .notes
        .iter()
        .filter(|n| status.as_deref().is_none_or(|s| n.status == s))
        .collect();
    if notes.is_empty() {
        println!("no notes");
        return Ok(());
    }
    for note in notes {
        println!("[{}] {}\n    {}", note.status, note.node, note.text);
    }
    Ok(())
}

fn cmd_unlink(from: &str, to: &str) -> Result<()> {
    let mut p = open_locked()?;
    let from = resolve(&p.inv, from)
        .map(|n| n.id.clone())
        .unwrap_or_else(|_| from.to_string());
    let to = resolve(&p.inv, to)
        .map(|n| n.id.clone())
        .unwrap_or_else(|_| to.to_string());
    let before = p.map.links.len();
    p.map.links.retain(|l| !(l.from == from && l.to == to));
    let removed_link = p.map.links.len() < before;

    // An edge seam inferred by name cannot be deleted from the code, so the
    // decision that it is wrong is recorded and applied on every later run.
    let inferred = {
        let graph = Graph::build(&p.inv, &p.map);
        graph.callees(&from).contains(&to.as_str())
    };
    if inferred {
        p.map.cuts.push(model::Cut {
            from: from.clone(),
            to: to.clone(),
        });
    }
    if !removed_link && !inferred {
        bail!("no connection from `{from}` to `{to}`");
    }
    model::save_map(&p.root, &p.map)?;
    match (removed_link, inferred) {
        (true, _) => println!("unlinked {from} -> {to}"),
        (false, _) => println!("cut the inferred connection {from} -> {to}"),
    }
    Ok(())
}

fn cmd_edges(path: Option<String>, tests: bool, json: bool) -> Result<()> {
    let p = open()?;
    let graph = Graph::build(&p.inv, &p.map);
    let mut rows: Vec<(&str, &str, &str)> = Vec::new();
    for node in p.inv.nodes.iter().filter(|n| tests || !n.is_test) {
        if path
            .as_deref()
            .is_some_and(|prefix| !node.path.starts_with(prefix))
        {
            continue;
        }
        for to in graph.callees(&node.id) {
            let how = if graph.asserted.contains(&(node.id.as_str(), to)) {
                "asserted"
            } else {
                "inferred"
            };
            rows.push((&node.id, to, how));
        }
        for to in graph.may_call(&node.id) {
            rows.push((&node.id, to, "guess"));
        }
    }
    if json {
        let items: Vec<_> = rows
            .iter()
            .map(|(from, to, how)| serde_json::json!({ "from": from, "to": to, "how": how }))
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!(
            "no connections{}",
            path.map(|p| format!(" under {p}")).unwrap_or_default()
        );
        return Ok(());
    }
    let mut last = "";
    for (from, to, how) in &rows {
        if *from != last {
            println!("{from}");
            last = from;
        }
        let tag = if *how == "inferred" {
            String::new()
        } else {
            format!("  [{how}]")
        };
        println!("    -> {to}{tag}");
    }
    println!(
        "\n{} connection(s). Inferred ones are matched by name: cut a wrong one with `seam unlink`.",
        rows.len()
    );
    Ok(())
}

fn cmd_unnote(target: &str, status: Option<String>) -> Result<()> {
    let mut p = open_locked()?;
    let id = resolve(&p.inv, target)
        .map(|n| n.id.clone())
        .unwrap_or_else(|_| target.to_string());
    let before = p.map.notes.len();
    p.map
        .notes
        .retain(|n| !(n.node == id && status.as_deref().is_none_or(|s| n.status == s)));
    let removed = before - p.map.notes.len();
    if removed == 0 {
        bail!("no matching notes on `{id}`");
    }
    model::save_map(&p.root, &p.map)?;
    println!("removed {removed} note(s) from {id}");
    Ok(())
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| anyhow!("could not run git: {e}"))?;
    if !out.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn cmd_diff(rev: &str, check: bool, tests: bool) -> Result<()> {
    let p = open()?;
    git(
        &p.root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ],
    )
    .map_err(|_| anyhow!("`{rev}` is not a git revision here"))?;
    // Paths come back relative to the repo top, which may sit above the seam root.
    let prefix = git(&p.root, &["rev-parse", "--show-prefix"])?
        .trim()
        .to_string();

    let mut touched: Vec<String> = git(
        &p.root,
        &["diff", "--name-only", "--no-renames", "--relative", rev],
    )?
    .lines()
    .map(str::to_string)
    .collect();
    touched.extend(
        git(&p.root, &["ls-files", "--others", "--exclude-standard"])?
            .lines()
            .map(str::to_string),
    );
    touched.sort();
    touched.dedup();

    let reviews = review_states(&p);
    let kinds = graph::kinds_by_node(&p.map);
    let describe = |id: &str| -> String {
        let claimed = kinds
            .get(id)
            .map(|k| k.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(", "));
        match (reviews.get(id), claimed) {
            (Some(Review::Current), Some(k)) => format!("reviewed ({k})"),
            (Some(Review::Outdated), Some(k)) => format!("was {k}, changed since: review again"),
            _ => "not reviewed".to_string(),
        }
    };

    let (mut added, mut changed, mut removed) = (Vec::new(), Vec::new(), Vec::new());
    for path in &touched {
        let old_text =
            git(&p.root, &["show", &format!("{rev}:{prefix}{path}")]).unwrap_or_default();
        let old: BTreeMap<String, String> = scan::parse_text(path, &old_text)
            .into_iter()
            .filter(|n| counted(n, tests))
            .map(|n| (n.id, n.hash))
            .collect();
        let new: Vec<&Node> = p
            .inv
            .nodes
            .iter()
            .filter(|n| &n.path == path && counted(n, tests))
            .collect();
        for node in &new {
            match old.get(&node.id) {
                None => added.push(node.id.as_str()),
                Some(hash) if *hash != node.hash => changed.push(node.id.as_str()),
                _ => {}
            }
        }
        for id in old.keys() {
            if !new.iter().any(|n| &n.id == id) {
                removed.push(id.clone());
            }
        }
    }

    if added.is_empty() && changed.is_empty() && removed.is_empty() {
        println!("no reviewable code changed since {rev}");
        return Ok(());
    }
    let mut unreviewed = 0;
    for (label, ids) in [("new", &added), ("changed", &changed)] {
        if ids.is_empty() {
            continue;
        }
        println!("{label} since {rev}:");
        for id in ids {
            let state = describe(id);
            if !state.starts_with("reviewed") {
                unreviewed += 1;
            }
            println!("  {id}\n      {state}");
        }
        println!();
    }
    if !removed.is_empty() {
        println!("removed since {rev}:");
        for id in &removed {
            let had = if kinds.contains_key(id.as_str()) {
                "  (has claims: see `seam verify`)"
            } else {
                ""
            };
            println!("  {id}{had}");
        }
        println!();
    }
    println!(
        "{} new, {} changed, {} removed. {unreviewed} still to review.",
        added.len(),
        changed.len(),
        removed.len()
    );
    if check && unreviewed > 0 {
        std::process::exit(2);
    }
    Ok(())
}

fn cmd_gaps(all: bool, json: bool) -> Result<()> {
    let p = open()?;
    let graph = Graph::build(&p.inv, &p.map);
    let reaches = graph::reaches(&graph, &p.map);
    let bare = graph::bare_entry_sinks(&graph, &p.map);
    let open_paths: Vec<_> = reaches.iter().filter(|r| !r.guarded).collect();
    let checked: Vec<_> = reaches.iter().filter(|r| r.guarded).collect();

    if json {
        let describe = |r: &&graph::Reach| serde_json::json!({ "entry": r.entry, "sink": r.sink, "path": r.path, "guards": r.guards });
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "paths_traced": reaches.len(),
                "unchecked": open_paths.iter().map(describe).collect::<Vec<_>>(),
                "checked": checked.iter().map(describe).collect::<Vec<_>>(),
                "entry_is_sink": bare,
            }))?
        );
        return Ok(());
    }

    if p.map.claims.iter().all(|c| c.kind != ClaimKind::Entry) {
        println!("no entry points claimed yet, so there is nothing to trace");
        return Ok(());
    }
    let through = |r: &graph::Reach| {
        if r.path.len() > 2 {
            println!("    through {}", r.path[1..r.path.len() - 1].join(" -> "));
        }
    };
    if open_paths.is_empty() && bare.is_empty() {
        println!(
            "no unchecked path found across {} entry-to-sink path(s). This only covers what has been claimed so far.",
            reaches.len()
        );
    } else {
        println!(
            "No check is recorded on these paths. Each is a place to look, not a confirmed flaw.\n"
        );
        for id in &bare {
            println!("{id}\n    takes untrusted input and performs a sensitive operation itself\n");
        }
        for reach in &open_paths {
            println!("{}\n    reaches {}", reach.entry, reach.sink);
            through(reach);
            println!();
        }
    }
    if all && !checked.is_empty() {
        // A recorded guard only says a check exists on the path, not that it
        // is the right check or a complete one. Listing them lets a reader judge.
        println!("\nPaths that count as checked, and the guard recorded on each:\n");
        for reach in &checked {
            println!("{}\n    reaches {}", reach.entry, reach.sink);
            through(reach);
            println!("    checked by");
            for guard in &reach.guards {
                let what = p
                    .map
                    .claims
                    .iter()
                    .find(|c| c.node == *guard && c.kind == ClaimKind::Guard)
                    .map(|c| c.summary.as_str())
                    .unwrap_or("");
                println!("      {guard}: {what}");
            }
            println!();
        }
    } else if !checked.is_empty() {
        println!(
            "{} path(s) count as checked: list them with `seam gaps --all`.",
            checked.len()
        );
    }
    Ok(())
}

fn cmd_verify(prune: bool) -> Result<()> {
    let mut p = open_locked()?;
    let nodes = model::index(&p.inv);
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut keep = Vec::new();
    for claim in &p.map.claims {
        let state = state_of(&p.root, &nodes, claim);
        *counts.entry(state.as_str()).or_default() += 1;
        match state {
            ClaimState::Stale => println!(
                "stale: {} ({}): quoted code is gone",
                claim.node,
                claim.kind.as_str()
            ),
            ClaimState::Orphaned => println!(
                "orphaned: {} ({}): unit no longer exists",
                claim.node,
                claim.kind.as_str()
            ),
            _ => {}
        }
        keep.push(!matches!(state, ClaimState::Stale | ClaimState::Orphaned));
    }
    let get = |k| counts.get(k).copied().unwrap_or(0);
    println!(
        "{} ok, {} on changed code, {} stale, {} orphaned",
        get("ok"),
        get("changed"),
        get("stale"),
        get("orphaned")
    );
    let dropped = keep.iter().filter(|k| !**k).count();
    drop(nodes);
    if prune && dropped > 0 {
        let mut keep = keep.into_iter();
        p.map.claims.retain(|_| keep.next().unwrap_or(true));
        model::save_map(&p.root, &p.map)?;
        println!("removed {dropped} claim(s)");
    } else if dropped > 0 {
        // A non-zero exit lets this run as a check in CI.
        std::process::exit(2);
    }
    Ok(())
}

fn cmd_view(out: Option<PathBuf>, no_open: bool) -> Result<()> {
    let p = open()?;
    let nodes = model::index(&p.inv);
    let states: Vec<ClaimState> = p
        .map
        .claims
        .iter()
        .map(|c| state_of(&p.root, &nodes, c))
        .collect();
    let html = view::render(&p.root, &p.inv, &p.map, &states)?;
    let path = out.unwrap_or_else(|| p.root.join(model::DIR).join("view.html"));
    std::fs::write(&path, html)?;
    println!("wrote {}", path.display());
    if !no_open {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else if cfg!(target_os = "windows") {
            "explorer"
        } else {
            "xdg-open"
        };
        let _ = std::process::Command::new(opener).arg(&path).spawn();
    }
    Ok(())
}
