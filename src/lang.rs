//! Language registry. Each grammar ships a generic "tags" query that marks
//! definitions and calls; seam uses those and knows nothing about frameworks.

use tree_sitter::{Language, Query};

pub struct Lang {
    pub name: &'static str,
    pub language: Language,
    pub query: Query,
}

struct Spec {
    name: &'static str,
    exts: &'static [&'static str],
    language: fn() -> Language,
    /// Tried in order; the first that compiles for the grammar is used.
    queries: fn() -> Vec<String>,
}

fn specs() -> Vec<Spec> {
    vec![
        Spec {
            name: "rust",
            exts: &["rs"],
            language: || tree_sitter_rust::LANGUAGE.into(),
            queries: || vec![tree_sitter_rust::TAGS_QUERY.to_string()],
        },
        Spec {
            name: "python",
            exts: &["py", "pyi"],
            language: || tree_sitter_python::LANGUAGE.into(),
            queries: || vec![tree_sitter_python::TAGS_QUERY.to_string()],
        },
        Spec {
            name: "javascript",
            exts: &["js", "jsx", "mjs", "cjs"],
            language: || tree_sitter_javascript::LANGUAGE.into(),
            queries: || vec![tree_sitter_javascript::TAGS_QUERY.to_string()],
        },
        Spec {
            name: "typescript",
            exts: &["ts", "mts", "cts"],
            language: || tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            queries: ts_queries,
        },
        Spec {
            name: "tsx",
            exts: &["tsx"],
            language: || tree_sitter_typescript::LANGUAGE_TSX.into(),
            queries: ts_queries,
        },
        Spec {
            name: "go",
            exts: &["go"],
            language: || tree_sitter_go::LANGUAGE.into(),
            queries: || vec![tree_sitter_go::TAGS_QUERY.to_string()],
        },
        Spec {
            name: "java",
            exts: &["java"],
            language: || tree_sitter_java::LANGUAGE.into(),
            queries: || vec![tree_sitter_java::TAGS_QUERY.to_string()],
        },
        Spec {
            name: "ruby",
            exts: &["rb"],
            language: || tree_sitter_ruby::LANGUAGE.into(),
            queries: || vec![tree_sitter_ruby::TAGS_QUERY.to_string()],
        },
        Spec {
            name: "php",
            exts: &["php"],
            language: || tree_sitter_php::LANGUAGE_PHP.into(),
            queries: || vec![tree_sitter_php::TAGS_QUERY.to_string()],
        },
        Spec {
            name: "c",
            exts: &["c", "h"],
            language: || tree_sitter_c::LANGUAGE.into(),
            queries: || vec![tree_sitter_c::TAGS_QUERY.to_string()],
        },
        Spec {
            name: "cpp",
            exts: &["cc", "cpp", "cxx", "hpp", "hh", "hxx"],
            language: || tree_sitter_cpp::LANGUAGE.into(),
            queries: || {
                vec![
                    format!(
                        "{}\n{}",
                        tree_sitter_c::TAGS_QUERY,
                        tree_sitter_cpp::TAGS_QUERY
                    ),
                    tree_sitter_cpp::TAGS_QUERY.to_string(),
                ]
            },
        },
        Spec {
            name: "csharp",
            exts: &["cs"],
            language: || tree_sitter_c_sharp::LANGUAGE.into(),
            queries: || vec![tree_sitter_c_sharp::TAGS_QUERY.to_string()],
        },
    ]
}

/// TypeScript's own tags only cover what it adds to JavaScript, so the
/// JavaScript tags are tried on top of them first.
fn ts_queries() -> Vec<String> {
    vec![
        format!(
            "{}\n{}",
            tree_sitter_javascript::TAGS_QUERY,
            tree_sitter_typescript::TAGS_QUERY
        ),
        tree_sitter_typescript::TAGS_QUERY.to_string(),
    ]
}

/// Files that are worth reviewing but have no grammar here: they become a
/// single file node each.
const PLAIN_EXTS: &[&str] = &[
    "kt", "kts", "swift", "scala", "dart", "ex", "exs", "erl", "lua", "pl", "r", "m", "mm", "zig",
    "sol", "vue", "svelte", "astro", "sql", "sh", "bash", "zsh", "ps1", "tf", "hcl", "yaml", "yml",
    "toml", "ini", "conf", "env", "graphql", "gql", "proto", "html", "htm", "ejs", "erb", "hbs",
    "j2", "jinja", "tpl", "twig",
];
const PLAIN_NAMES: &[&str] = &[
    "Dockerfile",
    "Makefile",
    "Jenkinsfile",
    "Caddyfile",
    "nginx.conf",
];

pub struct Registry {
    langs: Vec<(&'static [&'static str], Lang)>,
}

impl Registry {
    pub fn new() -> Self {
        let mut langs = Vec::new();
        for spec in specs() {
            let language = (spec.language)();
            let query = (spec.queries)()
                .into_iter()
                .find_map(|q| Query::new(&language, &q).ok());
            match query {
                Some(query) => langs.push((
                    spec.exts,
                    Lang {
                        name: spec.name,
                        language,
                        query,
                    },
                )),
                None => eprintln!(
                    "seam: no usable tags query for {}, treating as plain",
                    spec.name
                ),
            }
        }
        Self { langs }
    }

    pub fn for_ext(&self, ext: &str) -> Option<&Lang> {
        self.langs
            .iter()
            .find(|(exts, _)| exts.contains(&ext))
            .map(|(_, l)| l)
    }
}

pub fn is_plain(file_name: &str, ext: &str) -> bool {
    PLAIN_EXTS.contains(&ext) || PLAIN_NAMES.contains(&file_name)
}
