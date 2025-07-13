//! End-to-end checks against the built binary, on a small throwaway repo.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const APP: &str = r#"from flask import Flask, request
import requests
from auth import require_admin

app = Flask(__name__)


@app.route("/fetch")
def fetch_preview():
    url = request.args["url"]
    return download(url)


@app.route("/admin/export")
@require_admin
def export_users():
    return dump_users()


def download(url):
    return requests.get(url, timeout=5).text


def dump_users():
    return db.query("select * from users")
"#;

const AUTH: &str = r#"def require_admin(view):
    def wrapper(*args, **kwargs):
        if not current_user.is_admin:
            abort(403)
        return view(*args, **kwargs)
    return wrapper
"#;

struct Repo(PathBuf);

impl Repo {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("seam-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("app.py"), APP).unwrap();
        fs::write(dir.join("auth.py"), AUTH).unwrap();
        let repo = Repo(dir);
        repo.ok(&["scan", "."]);
        repo
    }

    fn run(&self, args: &[&str]) -> (bool, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_seam"))
            .args(args)
            .current_dir(&self.0)
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.success(), text)
    }

    fn ok(&self, args: &[&str]) -> String {
        let (success, text) = self.run(args);
        assert!(success, "`seam {}` failed:\n{text}", args.join(" "));
        text
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn lists_functions_and_files() {
    let repo = Repo::new("list");
    let next = repo.ok(&["next", "--limit", "50"]);
    for id in [
        "app.py#fetch_preview",
        "app.py#export_users",
        "app.py#download",
        "auth.py#require_admin",
    ] {
        assert!(next.contains(id), "missing {id} in:\n{next}");
    }
}

#[test]
fn a_decorator_above_a_function_belongs_to_it() {
    let repo = Repo::new("decorator");
    let out = repo.ok(&[
        "claim",
        "app.py#export_users",
        "--kind",
        "guard",
        "--summary",
        "Admin-only: wrapped in require_admin",
        "--quote",
        "@require_admin",
    ]);
    assert!(out.contains("accepted"), "{out}");
}

#[test]
fn a_quote_that_is_not_in_the_code_is_rejected() {
    let repo = Repo::new("reject");
    let (success, out) = repo.run(&[
        "claim",
        "app.py#fetch_preview",
        "--kind",
        "guard",
        "--summary",
        "Checks the URL against an allow list",
        "--quote",
        "if url not in ALLOWED_HOSTS:",
    ]);
    assert!(!success);
    assert!(out.contains("not found"), "{out}");
    assert!(
        !repo.path().join(".seam/map.json").exists(),
        "a rejected claim must not be saved"
    );
}

#[test]
fn a_quote_from_another_function_is_rejected() {
    let repo = Repo::new("scope");
    let (success, _) = repo.run(&[
        "claim",
        "app.py#fetch_preview",
        "--kind",
        "sink",
        "--summary",
        "Fetches the URL over HTTP",
        "--quote",
        "requests.get(url, timeout=5)",
    ]);
    assert!(
        !success,
        "the quote lives in download(), not fetch_preview()"
    );
}

fn claim_everything(repo: &Repo) {
    repo.ok(&[
        "claim",
        "app.py#fetch_preview",
        "--kind",
        "entry",
        "--summary",
        "Public route that takes a URL from the query string",
        "--quote",
        r#"url = request.args["url"]"#,
    ]);
    repo.ok(&[
        "claim",
        "app.py#download",
        "--kind",
        "sink",
        "--summary",
        "Fetches an arbitrary URL from the server",
        "--quote",
        "requests.get(url, timeout=5)",
    ]);
    repo.ok(&[
        "claim",
        "app.py#export_users",
        "--kind",
        "entry",
        "--summary",
        "Route that exports every user",
        "--quote",
        r#"@app.route("/admin/export")"#,
    ]);
    repo.ok(&[
        "claim",
        "app.py#export_users",
        "--kind",
        "guard",
        "--summary",
        "Admin-only: wrapped in require_admin",
        "--quote",
        "@require_admin",
    ]);
    repo.ok(&[
        "claim",
        "app.py#dump_users",
        "--kind",
        "sink",
        "--summary",
        "Reads the whole users table",
        "--quote",
        r#"db.query("select * from users")"#,
    ]);
}

#[test]
fn gaps_reports_only_the_unchecked_path() {
    let repo = Repo::new("gaps");
    claim_everything(&repo);
    let gaps = repo.ok(&["gaps"]);
    assert!(gaps.contains("app.py#fetch_preview"), "{gaps}");
    assert!(gaps.contains("app.py#download"), "{gaps}");
    assert!(
        !gaps.contains("export_users"),
        "the guarded route must not be reported:\n{gaps}"
    );
}

#[test]
fn claims_go_stale_when_the_quoted_code_is_removed() {
    let repo = Repo::new("stale");
    claim_everything(&repo);
    assert!(repo.ok(&["verify"]).contains("5 ok"));

    let changed = APP.replace("requests.get(url, timeout=5).text", "safe_fetch(url)");
    fs::write(repo.path().join("app.py"), changed).unwrap();

    let (success, out) = repo.run(&["verify"]);
    assert!(!success, "verify must fail when a claim is stale");
    assert!(out.contains("stale: app.py#download"), "{out}");
    let next = repo.ok(&["next"]);
    assert!(
        next.contains("app.py#download") && next.contains("changed since review"),
        "{next}"
    );
}

#[test]
fn view_embeds_the_map() {
    let repo = Repo::new("view");
    claim_everything(&repo);
    repo.ok(&["view", "--no-open"]);
    let html = fs::read_to_string(repo.path().join(".seam/view.html")).unwrap();
    assert!(html.contains("app.py#fetch_preview"));
    assert!(!html.contains("__SEAM_DATA__"));
}

/// A repo holding one small file per supported language.
fn languages() -> Repo {
    let dir = std::env::temp_dir().join(format!(
        "seam-test-langs-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/langs");
    for entry in fs::read_dir(fixtures).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
    }
    let repo = Repo(dir);
    repo.ok(&["scan", "."]);
    repo
}

#[test]
fn every_language_yields_its_functions() {
    let repo = languages();
    let listed = repo.ok(&["next", "--limit", "200", "--tests"]);
    for id in [
        "server.js#createUser",
        "server.js#listOrders",
        "server.js#Mailer.send",
        "auth.js#requireAuth",
        "api.ts#OrdersController.findOne",
        "api.ts#handler",
        "api.ts#parse",
        "main.go#Server.handleExport",
        "main.go#requireAdmin",
        "lib.rs#Store.load",
        "lib.rs#show_order",
        "lib.rs#tests.loads",
        "Orders.java#Orders.show",
        "orders.rb#OrdersController.show",
        "orders.php#OrdersController.show",
        "orders.php#helper_fn",
        "util.c#copy_name",
        "Svc.cs#Shop.OrdersController.Show",
    ] {
        assert!(
            listed.lines().any(|l| l.starts_with(&format!("{id}  "))),
            "missing {id} in:\n{listed}"
        );
    }
}

#[test]
fn tests_beside_the_code_are_left_out_by_default() {
    let repo = languages();
    assert!(
        !repo
            .ok(&["next", "--limit", "200"])
            .contains("lib.rs#tests.loads")
    );
}

#[test]
fn inline_handlers_are_named_after_their_route() {
    let repo = languages();
    let shown = repo.ok(&["show", "server.js#app.post:/users"]);
    assert!(
        shown.contains("auth.js#requireAuth"),
        "the handler names its middleware:\n{shown}"
    );
    assert!(shown.contains("server.js#createUser"), "{shown}");
}

#[test]
fn markers_above_a_function_can_be_quoted_in_every_language() {
    let repo = languages();
    for (id, quote) in [
        ("lib.rs#show_order", r#"#[require_role("admin")]"#),
        (
            "Orders.java#Orders.show",
            r#"@PreAuthorize("hasRole('ADMIN')")"#,
        ),
        (
            "Svc.cs#Shop.OrdersController.Show",
            r#"[Authorize(Roles = "Admin")]"#,
        ),
        ("api.ts#OrdersController.findOne", "@UseGuards(AdminGuard)"),
        (
            "orders.php#OrdersController.show",
            "#[Route('/orders/{id}')]",
        ),
    ] {
        let (success, out) = repo.run(&[
            "claim",
            id,
            "--kind",
            "guard",
            "--summary",
            "Declares who may call this handler",
            "--quote",
            quote,
        ]);
        assert!(success, "{id} should include the marker above it:\n{out}");
    }
}

#[test]
fn a_bare_function_name_is_enough_when_it_is_unique() {
    let repo = languages();
    assert!(
        repo.ok(&["show", "copy_name"])
            .starts_with("util.c#copy_name")
    );
}

#[test]
fn claims_made_at_the_same_time_are_all_kept() {
    let repo = Repo::new("parallel");
    let jobs = [
        (
            "app.py#fetch_preview",
            "entry",
            r#"url = request.args["url"]"#,
        ),
        ("app.py#download", "sink", "requests.get(url, timeout=5)"),
        ("app.py#export_users", "guard", "@require_admin"),
        (
            "app.py#dump_users",
            "sink",
            r#"db.query("select * from users")"#,
        ),
        (
            "auth.py#require_admin",
            "guard",
            "if not current_user.is_admin:",
        ),
    ];
    let children: Vec<_> = jobs
        .iter()
        .map(|(id, kind, quote)| {
            Command::new(env!("CARGO_BIN_EXE_seam"))
                .args([
                    "claim",
                    id,
                    "--kind",
                    kind,
                    "--summary",
                    "Claimed while others were writing",
                    "--quote",
                    quote,
                ])
                .current_dir(repo.path())
                .output()
        })
        .collect::<Vec<_>>();
    // Started one after another above; run a second wave truly in parallel.
    assert!(
        children
            .iter()
            .all(|c| c.as_ref().unwrap().status.success())
    );
    repo.ok(&["unclaim", "app.py#download"]);
    let handles: Vec<_> = jobs
        .iter()
        .map(|(id, kind, quote)| {
            let dir = repo.path().to_path_buf();
            let (id, kind, quote) = (id.to_string(), kind.to_string(), quote.to_string());
            std::thread::spawn(move || {
                Command::new(env!("CARGO_BIN_EXE_seam"))
                    .args([
                        "claim",
                        &id,
                        "--kind",
                        &kind,
                        "--summary",
                        "Claimed while others were writing",
                        "--quote",
                        &quote,
                    ])
                    .current_dir(dir)
                    .output()
                    .unwrap()
            })
        })
        .collect();
    for handle in handles {
        assert!(handle.join().unwrap().status.success());
    }
    assert!(
        repo.ok(&["verify"]).contains("5 ok"),
        "a concurrent write was lost"
    );
}

#[test]
fn links_and_notes_can_be_removed() {
    let repo = Repo::new("remove");
    repo.ok(&[
        "link",
        "app.py#fetch_preview",
        "app.py#download",
        "--why",
        "Passes the URL straight through",
        "--quote",
        "return download(url)",
    ]);
    repo.ok(&["unlink", "app.py#fetch_preview", "app.py#download"]);
    repo.ok(&[
        "note",
        "app.py#download",
        "--status",
        "idea",
        "--text",
        "Try a redirect to an internal host",
    ]);
    assert!(repo.ok(&["notes"]).contains("redirect"));
    repo.ok(&["unnote", "app.py#download"]);
    assert!(repo.ok(&["notes"]).contains("no notes"));
}

#[test]
fn gaps_and_show_speak_json() {
    let repo = Repo::new("json");
    claim_everything(&repo);
    let gaps = repo.ok(&["gaps", "--json"]);
    assert!(
        gaps.contains(r#""entry": "app.py#fetch_preview""#),
        "{gaps}"
    );
    let shown = repo.ok(&["show", "app.py#download", "--json"]);
    assert!(
        shown.contains(r#""used_by""#) && shown.contains("requests.get"),
        "{shown}"
    );
}

#[test]
fn code_in_a_quote_cannot_break_out_of_the_page() {
    let repo = Repo::new("escape");
    let hostile = "def render():\n    return \"</script><script>alert(1)</script><!--\"\n";
    fs::write(repo.path().join("page.py"), hostile).unwrap();
    repo.ok(&[
        "claim",
        "page.py#render",
        "--kind",
        "sink",
        "--summary",
        "Returns markup that must never reach the page raw",
        "--quote",
        r#"return "</script><script>alert(1)</script><!--""#,
    ]);
    repo.ok(&["view", "--no-open"]);
    let html = fs::read_to_string(repo.path().join(".seam/view.html")).unwrap();
    assert!(
        !html.contains("<script>alert(1)"),
        "a quote was written into the page unescaped"
    );
    assert_eq!(
        html.matches("</script>").count(),
        1,
        "only the page's own script may close"
    );
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "seam")
        .env("GIT_AUTHOR_EMAIL", "seam@example.com")
        .env("GIT_COMMITTER_NAME", "seam")
        .env("GIT_COMMITTER_EMAIL", "seam@example.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn diff_lists_what_changed_and_whether_it_was_reviewed() {
    let repo = Repo::new("diff");
    claim_everything(&repo);
    git(repo.path(), &["init", "-q", "-b", "main"]);
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-q", "-m", "base"]);
    assert!(
        repo.ok(&["diff", "main"])
            .contains("no reviewable code changed")
    );

    let edited = APP.replace(
        "requests.get(url, timeout=5).text",
        "requests.get(url, timeout=9).text",
    ) + "\n\ndef purge_orders():\n    return db.query(\"delete from orders\")\n";
    fs::write(repo.path().join("app.py"), edited).unwrap();

    let out = repo.ok(&["diff", "main"]);
    assert!(
        out.contains("app.py#purge_orders") && out.contains("not reviewed"),
        "{out}"
    );
    assert!(
        out.contains("app.py#download") && out.contains("review again"),
        "{out}"
    );
    assert!(
        !out.contains("fetch_preview"),
        "untouched code must not be listed:\n{out}"
    );

    let (success, _) = repo.run(&["diff", "main", "--check"]);
    assert!(
        !success,
        "--check must fail while changed code is unreviewed"
    );
}

/// Two copies of the same module, as in a repo that ships a server build and
/// a web build, plus a helper that nothing imports.
fn twin_modules() -> Repo {
    let dir = std::env::temp_dir().join(format!(
        "seam-test-twins-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&dir);
    for sub in ["api", "src", "web/lib"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    let scanner = "export function scanDiff(diff) {\n  return diff.split(\"\\n\");\n}\n";
    fs::write(dir.join("src/scanner.js"), scanner).unwrap();
    fs::write(dir.join("web/lib/scanner.js"), scanner).unwrap();
    fs::write(
        dir.join("api/scan.js"),
        "import { scanDiff } from \"../src/scanner\";\n\nexport function handler(req) {\n  return json(scanDiff(req.body));\n}\n\nfunction json(value) {\n  return JSON.stringify(value);\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("api/packs.js"),
        "export function listPacks() {\n  return json([\"default\"]);\n}\n",
    )
    .unwrap();
    let repo = Repo(dir);
    repo.ok(&["scan", "."]);
    repo
}

#[test]
fn a_call_resolves_to_the_file_that_is_imported() {
    let repo = twin_modules();
    let shown = repo.ok(&["show", "api/scan.js#handler"]);
    assert!(shown.contains("src/scanner.js#scanDiff"), "{shown}");
    assert!(
        !shown.contains("web/lib/scanner.js#scanDiff"),
        "the twin that is not imported must not be linked:\n{shown}"
    );
}

#[test]
fn a_helper_in_a_file_that_is_not_imported_is_not_linked() {
    let repo = twin_modules();
    let shown = repo.ok(&["show", "api/packs.js#listPacks"]);
    assert!(!shown.contains("api/scan.js#json"), "{shown}");
}

#[test]
fn a_function_inside_a_function_is_named_after_it() {
    let repo = Repo::new("nested");
    let listed = repo.ok(&["next", "--limit", "50"]);
    assert!(listed.contains("auth.py#require_admin.wrapper"), "{listed}");
}

#[test]
fn a_file_unit_shows_only_what_is_outside_its_functions() {
    let repo = Repo::new("fileunit");
    let shown = repo.ok(&["show", "app.py"]);
    assert!(
        shown.contains("[own unit: app.py#fetch_preview]"),
        "{shown}"
    );
    assert!(
        shown.contains("app = Flask(__name__)"),
        "top-level code must still be shown:\n{shown}"
    );
    assert!(
        !shown.contains("url = request.args"),
        "a function body was printed again:\n{shown}"
    );
}

#[test]
fn next_can_follow_what_an_entry_point_reaches() {
    let repo = Repo::new("reachable");
    let near = repo.ok(&["next", "--reachable-from", "app.py#fetch_preview"]);
    assert!(near.contains("app.py#download"), "{near}");
    assert!(
        !near.contains("export_users") && !near.contains("dump_users"),
        "unreachable units were listed:\n{near}"
    );
}

#[test]
fn a_wrong_inferred_connection_can_be_cut_and_stays_cut() {
    let repo = Repo::new("cut");
    claim_everything(&repo);
    assert!(repo.ok(&["gaps"]).contains("app.py#download"));

    let out = repo.ok(&["unlink", "app.py#fetch_preview", "app.py#download"]);
    assert!(out.contains("cut the inferred connection"), "{out}");
    assert!(
        !repo
            .ok(&["show", "app.py#fetch_preview"])
            .contains("app.py#download")
    );
    assert!(!repo.ok(&["gaps"]).contains("reaches app.py#download"));

    // Asserting the connection with a quote brings it back.
    repo.ok(&[
        "link",
        "app.py#fetch_preview",
        "app.py#download",
        "--why",
        "Passes the URL straight through",
        "--quote",
        "return download(url)",
    ]);
    assert!(repo.ok(&["gaps"]).contains("app.py#download"));
}

#[test]
fn gaps_can_explain_which_guard_covers_a_path() {
    let repo = Repo::new("explain");
    claim_everything(&repo);
    let brief = repo.ok(&["gaps"]);
    assert!(brief.contains("count as checked"), "{brief}");
    let all = repo.ok(&["gaps", "--all"]);
    assert!(
        all.contains("checked by") && all.contains("app.py#export_users: Admin-only"),
        "{all}"
    );
}

#[test]
fn links_can_be_piped_in_and_bad_ones_are_rejected() {
    let repo = Repo::new("links");
    let mut child = Command::new(env!("CARGO_BIN_EXE_seam"))
        .args(["link", "--stdin"])
        .current_dir(repo.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(stdin, r#"{{"from":"app.py#fetch_preview","to":"app.py#download","why":"Passes the URL straight through","quote":"return download(url)"}}"#).unwrap();
        writeln!(stdin, r#"{{"from":"app.py#export_users","to":"app.py#download","why":"Claimed without any basis","quote":"return download(everything)"}}"#).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out.status.success(),
        "a rejected link must fail the command"
    );
    assert!(
        text.contains("linked: app.py#fetch_preview -> app.py#download"),
        "{text}"
    );
    assert!(
        text.contains("rejected: app.py#export_users -> app.py#download"),
        "{text}"
    );
    let map = fs::read_to_string(repo.path().join(".seam/map.json")).unwrap();
    assert_eq!(
        map.matches("\"from\"").count(),
        1,
        "only the proven link may be saved:\n{map}"
    );
}

#[test]
fn a_relative_import_means_the_nearest_file_of_that_name() {
    let repo = twin_modules();
    fs::write(
        repo.path().join("src/cli.js"),
        "import { scanDiff } from \"./scanner\";\n\nexport function main(argv) {\n  return scanDiff(argv[2]);\n}\n",
    )
    .unwrap();
    let shown = repo.ok(&["show", "src/cli.js#main"]);
    assert!(shown.contains("src/scanner.js#scanDiff"), "{shown}");
    assert!(!shown.contains("web/lib/scanner.js"), "{shown}");
    // The import line itself is not a use of the function.
    assert!(
        !repo
            .ok(&["show", "src/cli.js", "--no-source"])
            .contains("scanDiff")
    );
}

#[test]
fn a_word_inside_a_string_is_not_a_connection() {
    let repo = twin_modules();
    fs::write(
        repo.path().join("api/route.js"),
        "export function OPTIONS() {\n  return { allow: \"POST, OPTIONS\" };\n}\n\nexport function POST(req) {\n  return req.body;\n}\n",
    )
    .unwrap();
    assert!(
        !repo
            .ok(&["show", "api/route.js#OPTIONS"])
            .contains("api/route.js#POST")
    );
}

#[test]
fn a_method_call_is_not_a_plain_function_beside_it() {
    let repo = twin_modules();
    fs::write(
        repo.path().join("api/body.js"),
        "export function parseBody(request) {\n  return request.json();\n}\n\nfunction json(value) {\n  return JSON.stringify(value);\n}\n",
    )
    .unwrap();
    assert!(
        !repo
            .ok(&["show", "api/body.js#parseBody"])
            .contains("api/body.js#json")
    );
}

#[test]
fn edges_lists_connections_for_auditing() {
    let repo = Repo::new("edges");
    let out = repo.ok(&["edges", "app.py"]);
    assert!(
        out.contains("app.py#fetch_preview") && out.contains("-> app.py#download"),
        "{out}"
    );
    repo.ok(&["unlink", "app.py#fetch_preview", "app.py#download"]);
    assert!(!repo.ok(&["edges", "app.py"]).contains("-> app.py#download"));
}

#[test]
fn show_can_leave_the_source_out() {
    let repo = Repo::new("nosource");
    let out = repo.ok(&["show", "app.py#fetch_preview", "--no-source"]);
    assert!(
        out.contains("app.py#download") && !out.contains("request.args"),
        "{out}"
    );
}
