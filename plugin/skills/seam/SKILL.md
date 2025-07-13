---
name: seam
description: Build and use a security map of a codebase with the seam CLI. Use when asked to map, review or audit a repository's security, to find its entry points, permission checks and sensitive operations, to see what has and has not been reviewed, or to continue earlier security research on a repo.
allowed-tools: Bash(seam *)
---

# Seam

Seam keeps a security map of a repository in `.seam/map.json`. You do the
reading and the judging. Seam keeps the list of everything that exists, checks
that each thing you say is backed by real code, and remembers it between
sessions.

The map is only useful if it is true. Seam rejects any claim whose quote is not
in the code, so read a unit before you say anything about it, and copy the
quote from what `seam show` printed rather than from memory.

## The loop

1. `seam scan` once per repo. It lists every function, method and file.
2. `seam status` to see coverage by directory, and `seam notes` to read what an
   earlier session already tried. Do both before starting work, so you do not
   repeat it.
3. `seam next --path <dir> --limit 20` to get units that still need review.
   Pick the area yourself: start where requests arrive (routes, handlers,
   controllers, consumers). Once an entry point is claimed, follow it with
   `seam next --reachable-from <id>`, which lists what it reaches, nearest
   first.
4. `seam show <id>` to read one unit. It prints what uses it, what it uses,
   any existing claims and notes, and the source with line numbers.
5. `seam claim` to record what the unit is.
6. Repeat from 3. Run `seam gaps` when a group of entry points and the code
   they reach has been claimed.

Code that changed after it was reviewed comes back in `seam next`, marked
"changed since review". Read it again and claim it again.

A unit is named `path#Name`, for example `src/auth.py#Login.check`. Quote the
id in the shell, because of the `#`. `path:line` also works, and so does a bare
function name when only one function has it. A handler passed inline to a call
is named after that call and its first string: `server.js#app.post:/users`.

A file is a unit too: it stands for the code outside its functions (imports,
constants, route tables, start-up code), and `seam show <file>` prints only
that, with one line pointing at each function. Claim the file for what its
top-level code does, and each function separately. Do not claim a file and a
function inside it as the same entry point, or every path from it is counted
twice.

Lists are cut short for reading: `seam next` shows 20 units unless you pass
`--limit`, and `seam show` shows the first 20 connections. `status`, `next`,
`show`, `edges` and `gaps` accept `--json`, which is never cut short.
`seam show <id> --no-source` prints the connections, claims and notes alone.

## Claims

Every unit you review gets at least one claim. The kinds:

- `entry`: untrusted input enters here. A route handler, a webhook, a message
  consumer, a command-line argument, an uploaded file, a deserialised payload.
- `guard`: something that gates access or validates input. A login or
  permission check, a signature check, an allow list, a rate limit.
- `sink`: an operation that must not run unchecked. An outbound request, a
  database query, a file read or write, command execution, a redirect, sending
  a message, changing money or permissions. Ask what an attacker gains if it
  runs with input they chose. A helper that only formats or serialises a
  response is not a sink: claim it `none`. Calling something a sink when it is
  not creates paths in `seam gaps` that mean nothing.
- `data`: sensitive data is defined, stored or returned here. Credentials,
  tokens, personal data.
- `none`: you read it and nothing in it matters for security.

```sh
seam claim "src/api/orders.py#show_order" --kind entry \
  --summary "Returns an order by the id in the URL path" \
  --quote '@app.route("/orders/<order_id>")' --tag http
```

- `--summary` says what the unit does in one plain line. Put the weakness in
  the summary when a check is incomplete: "Blocks private IP literals only; a
  hostname that resolves to a private address passes".
- `--quote` is the exact code the claim rests on, copied from `seam show`.
  Whitespace may differ; nothing else may. Quote the decisive line, not the
  whole function. `none` needs no quote.
- One unit can hold several claims. A route with a permission decorator is an
  `entry` and a `guard`: make both, each with its own quote.
- A quote must come from inside the unit you are claiming. If the decisive
  code lives in a function it calls, claim that function instead.

To record many at once, pipe JSON lines:

```sh
seam claim --stdin <<'EOF'
{"node":"src/fetch.py#download","kind":"sink","summary":"Requests an arbitrary URL from the server","quote":"requests.get(url, timeout=5)","tags":["outbound-request"]}
{"node":"src/util.py#slugify","kind":"none","summary":"Pure string formatting, no input from the request"}
EOF
```

If seam rejects a claim, the code does not say what you thought it said. Run
`seam show` again and correct the claim. Never reword a quote to make it pass.
Use `seam unclaim <id>` to remove a claim that turned out to be wrong.

## Links

Seam connects units when one names the other, and only believes a call into
another file when that file is imported. Where a connection is real but
invisible to that, assert it. The usual cases are a route table, a queue, a
plugin registry, and dynamic dispatch such as a loop that calls `rule.match()`
on every item of a list: link the loop to each function it can end up calling.

```sh
seam link "src/routes.py#register" "src/api/orders.py#show_order" \
  --why "Registered as the handler for GET /orders/<id>" \
  --quote 'router.add("/orders/<id>", show_order)'
```

The quote must be in the first unit. For many links at once, pipe JSON lines:

```sh
seam link --stdin <<'EOF'
{"from":"src/scanner.ts#scanDiff","to":"src/rules.ts#rules.sql-injection.match","why":"scanDiff calls match on every rule in the list","quote":"rule.match(file, line)"}
EOF
```

"Uses" means the unit names the other one. Usually that is a call, but it is
also a decorator, a function passed as a handler, or a type such as
`ReturnType<typeof parse>`.

A connection seam inferred can be wrong. `seam edges <path>` lists every
connection that starts under a path, so you can check them against the code
in one pass. When one is not supported by the code, remove it with
`seam unlink <from> <to>`. Seam remembers the decision. The same command
removes a link you asserted.

`seam show` may also print "may call (method matched by name only)". Those are
guesses: a method call cannot be resolved without types. Check them by reading
the code, and assert the ones that are real with `seam link`.

## Gaps

`seam gaps` lists entry points that reach a sink with no guard recorded on the
way. Each one is a place to look, not a finding. Read the path. Then either
claim the guard you find, or keep the gap and write a note about it.

A path counts as checked as soon as any guard is recorded on it. That says a
check exists, not that it is the right check or a complete one. Run
`seam gaps --all` to see every checked path with the guard that covers it, and
judge each: a guard that only runs in some configurations, or that validates
something other than what the sink needs, should be said so in its summary and
in a note.

## Notes

Record research as you go, so the next session starts where this one stopped:

```sh
seam note "src/fetch.py#is_public_host" --status idea \
  --text "Only IP literals are checked. Try a hostname that resolves to 127.0.0.1."
```

Statuses: `idea` (worth trying), `tested` (tried, with the result), `dead-end`
(tried, does not work, and why), `finding` (confirmed, with the evidence).
Write a `dead-end` note as carefully as a `finding`: it is what stops the same
path being walked twice. `seam unnote <id>` removes the notes on a unit.

## Reviewing a change

`seam diff <rev>` lists the units that are new or changed since a branch, tag
or commit, and whether each has been reviewed. To review a pull request, run
`seam diff main`, then show and claim each unit it lists.

## Reporting back

- `seam status` gives the coverage figure. Report it as it is. Do not describe
  a repo as reviewed when part of it is not.
- `seam view` writes the map as a page with a diagram and opens it.
- Tell the person what you claimed, what `seam gaps` shows, and what is left.
