//! The web view: one self-contained HTML file with the map embedded.

use crate::graph::{self, Graph};
use crate::model::{ClaimKind, ClaimState, Inventory, Map};
use anyhow::Result;
use serde_json::{Map as JsonMap, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const TEMPLATE: &str = include_str!("../assets/view.html");
/// Above this many units the page shows coverage per directory only: one
/// square per unit stops being readable, and the file stops being small.
const MAX_UNITS_DRAWN: usize = 6000;
/// Connections listed for one unit.
const MAX_NEIGHBOURS: usize = 40;

pub fn render(root: &Path, inv: &Inventory, map: &Map, states: &[ClaimState]) -> Result<String> {
    let graph = Graph::build(inv, map);
    let kinds = graph::kinds_by_node(map);
    let index = crate::model::index(inv);

    // Every route from an entry point to a sensitive operation.
    let reaches = graph::reaches(&graph, map);
    let mut paths: Vec<Value> = Vec::new();
    let mut busy: BTreeSet<&str> = BTreeSet::new();
    for id in graph::bare_entry_sinks(&graph, map) {
        busy.insert(id);
        paths.push(
            json!({ "entry": id, "sink": id, "stops": [id], "guards": [], "checked": false }),
        );
    }
    for reach in &reaches {
        busy.insert(reach.entry);
        paths.push(json!({
            "entry": reach.entry, "sink": reach.sink, "stops": reach.path,
            "guards": reach.guards, "checked": reach.guarded,
        }));
    }
    // Unchecked first: they are what the page is opened for.
    paths.sort_by_key(|p| p["checked"].as_bool().unwrap_or(false));

    // Entry points from which nothing sensitive is reached.
    let idle: Vec<&str> = kinds
        .iter()
        .filter(|(id, k)| k.contains(&ClaimKind::Entry) && !busy.contains(*id))
        .map(|(id, _)| *id)
        .collect();

    // Units the page has something to say about: claimed as more than
    // "nothing here", standing on a path, or carrying a note.
    let mut wanted: BTreeSet<&str> = kinds
        .iter()
        .filter(|(_, k)| k.iter().any(|k| *k != ClaimKind::None))
        .map(|(id, _)| *id)
        .collect();
    for reach in &reaches {
        wanted.extend(reach.path.iter().copied());
        wanted.extend(reach.guards.iter().copied());
    }
    wanted.extend(map.notes.iter().map(|n| n.node.as_str()));

    let mut nodes = JsonMap::new();
    for id in wanted.iter().filter(|id| index.contains_key(*id)) {
        let node = index[id];
        let claims: Vec<Value> = map
            .claims
            .iter()
            .zip(states)
            .filter(|(c, _)| c.node == *id)
            .map(|(c, state)| {
                json!({
                    "kind": c.kind.as_str(), "summary": c.summary, "quote": c.quote,
                    "line": c.line, "tags": c.tags, "state": state.as_str(),
                })
            })
            .collect();
        let notes: Vec<Value> = map
            .notes
            .iter()
            .filter(|n| n.node == *id)
            .map(|n| json!({ "status": n.status, "text": n.text }))
            .collect();
        let uses: Vec<&str> = graph.callees(id).into_iter().take(MAX_NEIGHBOURS).collect();
        let used_by: Vec<&str> = graph.callers(id).into_iter().take(MAX_NEIGHBOURS).collect();
        nodes.insert(
            id.to_string(),
            json!({
                "name": node.name, "path": node.path,
                "start": node.start_line, "end": node.end_line,
                "kinds": kinds.get(id).map(|k| k.iter().map(|k| k.as_str()).collect::<Vec<_>>()).unwrap_or_default(),
                "claims": claims, "notes": notes, "uses": uses, "used_by": used_by,
            }),
        );
    }

    // Coverage, counted the same way `seam status` does.
    let mut reviewed: BTreeMap<&str, bool> = BTreeMap::new();
    for (claim, state) in map.claims.iter().zip(states) {
        if *state == ClaimState::Orphaned {
            continue;
        }
        let current = reviewed.entry(claim.node.as_str()).or_insert(true);
        *current = *current && *state == ClaimState::Ok;
    }
    let mut areas: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    let (mut total, mut done, mut outdated) = (0, 0, 0);
    // One entry per unit: its id and 0 (not reviewed), 1 (reviewed) or 2
    // (reviewed, but the code changed since).
    let mut units: Vec<Value> = Vec::new();
    for node in inv.nodes.iter().filter(|n| n.reviewable && !n.is_test) {
        total += 1;
        let slot = areas.entry(area(&node.path)).or_default();
        slot.2 += 1;
        let state = match reviewed.get(node.id.as_str()) {
            Some(true) => {
                done += 1;
                slot.0 += 1;
                1
            }
            Some(false) => {
                outdated += 1;
                slot.1 += 1;
                2
            }
            None => 0,
        };
        units.push(json!([node.id, state]));
    }
    let areas: Vec<Value> = areas
        .iter()
        .map(|(name, (done, outdated, total))| {
            json!({ "name": name, "reviewed": done, "outdated": outdated, "total": total })
        })
        .collect();
    let notes: Vec<Value> = map
        .notes
        .iter()
        .map(|n| json!({ "node": n.node, "status": n.status, "text": n.text }))
        .collect();

    let data = json!({
        "project": root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        "coverage": { "total": total, "reviewed": done, "outdated": outdated },
        "areas": areas,
        "units": if units.len() <= MAX_UNITS_DRAWN { Value::Array(units) } else { Value::Null },
        "nodes": nodes,
        "paths": paths,
        "idle": idle,
        "notes": notes,
    });
    // The payload sits inside a script element and holds quoted source code,
    // which can contain anything. Escaping every "<" keeps "</script>" and
    // "<!--" in a quote from ending or commenting out the script.
    let payload = serde_json::to_string(&data)?.replace('<', "\\u003c");
    Ok(TEMPLATE.replace("\"__SEAM_DATA__\"", &payload))
}

fn area(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    match parts.len() {
        1 => ".".to_string(),
        2 => parts[0].to_string(),
        _ => format!("{}/{}", parts[0], parts[1]),
    }
}
