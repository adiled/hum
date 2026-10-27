---
title: "web2"
description: "an attendant: the web2 world as one tendril, wrepl"
---

# web2

> _an attendant: the web2 world as one tendril, wrepl_

`web2 serve` enters one bee, an attendant, and serves one fixed tool,
`wrepl`. A reach (`chi:"tendril-reach"`) carries one command line in
`line`, every API and MCP server web2 knows answers it as if it were a
CLI, and web2 sends the answer back as `chi:"tendril-result"`:

```
help
github help pulls
github pulls/list rust-lang rust --state open --jq '.[].title'
github pulls/list --help
sentry help issues
```

The thrum sees none of it: `wrepl` is a tendril like any other
([THRUM.md, The tendril](../../THRUM.md#the-tendril)), so humd hands it
to every bloom on its humd, every nurse hands it to its cells, and a
harness that takes tools only over MCP gets it as one tool among `bash`
and the forager's own. OpenAPI, MCP and OAuth stop here.

```
hum hive web2 install
```

The install builds `web2`, keeps `web2 serve` running under orchd, and
prints where the binary is; it puts nothing on the PATH. The bee enters
itself each time it starts.

## Services

web2 reaches what `~/.config/hum/hives/web2.json` names. Each service is
an OpenAPI 3 spec, from a URL or a file (a relative path is beside
web2.json), in JSON or YAML, or an MCP server, over stdio or streamable
HTTP. An `mcpServers` block pasted from a harness's config works as it
is, and `web2 import` copies in the servers the harnesses on this machine
already know (below).

```json
{
  "services": {
    "github": {
      "openapi": "https://raw.githubusercontent.com/github/rest-api-description/main/descriptions/api.github.com/api.github.com.json",
      "auth": "bearer",
      "allow": ["issues/create"]
    },
    "sentry": { "writes": true }
  },
  "mcpServers": {
    "sentry": { "type": "http", "url": "https://mcp.sentry.dev/mcp" },
    "everything": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-everything"] }
  }
}
```

A service in `services` with no `openapi` or `mcp` of its own sets the
policy of the `mcpServers` entry of the same name. A service's name is
letters, digits, `-` and `_`. web2.json is read again whenever it
changes, so a service added there is reached on the next call, with no
restart; while the file is broken, every reach is answered with what is
wrong with it, and nothing is reached. `${VAR}` in `base`, in an MCP
server's `url`, `headers` or `env`, or in an `auth`'s user name or OAuth
app reads the environment web2 runs in, and one not set there breaks
web2.json.

| key | |
|---|---|
| `openapi` | the spec's URL or path; `base` overrides the server it names |
| `mcp` | `{ "command", "args", "env", "cwd" }` for stdio, `{ "url", "headers" }` for HTTP; `"type": "stdio"` or `"http"` when the shape does not say |
| `description` | one line for `help`; an OpenAPI spec's title stands in |
| `writes` | `true` turns every write on |
| `allow` | the write commands on, by name or glob (`create-*`, a `*` matching any run of characters), when `writes` is not |
| `deny` | commands off, by name or glob, whatever else says |
| `auth` | how web2 signs in (below); a stdio MCP server takes none |
| `timeoutMs` | how long a call may take, 120000 by default |

At the top level, `maxOutput` (32 KiB) bounds an answer, keeping its
start and its end; `timeoutMs` sets every service's default; `idleMs`
(ten minutes) is how long an MCP server stays connected unused;
`redirectPort` (8976) is where a sign-in comes back to; and `servePeers`
lends web2 to peers' blooms (below).

## Commands

An OpenAPI operation is a command named by its `operationId`,
kebab-cased (`listPets` is `list-pets`), or by its method and path when
it has none (`DELETE /pets/{petId}` is `delete-pets-pet-id`), grouped by
its first tag. Its path, query and header parameters and the fields of
its JSON body are its flags; a body that is not a JSON object is
`--body`. An MCP tool is a command named by its tool name, kebab-cased,
with its input schema's properties as flags. Spelling is forgiven across
kebab, snake and camel case.

`--name value` and `--name=value` set a flag, `--flag` and `--no-flag` a
boolean, and a flag repeated builds an array. Words with no flag fill
the command's required inputs in order. `--json '{…}'` gives the whole
input, flags laid over it winning, and `--jq '<filter>'` narrows a JSON
result as jq would; an MCP tool's structured content, or a lone text
block that is JSON, counts as JSON.

`help` lists the services, `<service> help` a service's commands, and
`<service> <command> --help` a command's flags. A service with more than
120 commands lists its groups; `<service> help <group>` or
`<service> help <words>` finds the rest.

## Reading and writing

A service only reads until web2.json says otherwise. An OpenAPI
operation reads when its method is GET, HEAD or OPTIONS. An MCP tool
reads only when its annotations say `readOnlyHint: true`, as the MCP
spec has it; most servers do not say, so their tools count as writes
until `writes` or `allow` turns them on. A refused call answers with the
key that would turn it on, and a service's help counts what is off.
Which writes a bloom may make is the config's to say, never the bloom's.

```
web2 review deepwiki             each write that is off, turned on or left, one by one
web2 allow deepwiki 'read-*'     on, by name or glob
web2 deny github 'repos/delete*' off, whatever else says
```

These edit web2.json in place, keeping its order, and refuse a change
that would break it. `web2 deny` also takes the names out of `allow`. A
command in `deny` stays off whatever `allow` says, so `web2 review` does
not offer it, and `review` and `allow` both name what `deny` keeps off.

## Signing in

```
web2 login github              a token, typed at a prompt that does not show it
web2 login github --token-stdin
web2 login sentry              an MCP server's own OAuth, in a browser
web2 logout github
```

`auth` is one of:

| `auth` | |
|---|---|
| `"bearer"` | a token, sent as `Authorization: Bearer` |
| `{ "header": "X-Api-Key" }` | a token in that header |
| `{ "query": "api_key" }` | a token in that query parameter; OpenAPI only |
| `{ "basic": "<user>" }` | HTTP basic, the token as the password |
| `{ "oauth": { "clientId", "clientSecret", "scopes", "authorizeUrl", "tokenUrl", "redirectPort" } }` | an OAuth app; only `clientId` is needed when the spec names the rest |

To read the token from the environment instead of storing it, add
`"env": "VAR"` beside `header`, `query` or `basic`, or write
`{ "bearer": { "env": "VAR" } }`.

A token is stored in `secrets/credentials.json` in web2's state
directory, mode 0600 in a 0700 directory, and never put in an
environment, logged or answered back. The bee reads the file again when
it changes, so a sign-in counts from the next call, with no restart.

An OpenAPI service with an `oauth` app signs in with the authorization
code flow and PKCE, through a callback on `127.0.0.1:8976`, and its
token is renewed before it expires; the authorization and token URLs and
the scopes come from the spec when it names them. An MCP server over HTTP
with no `auth` that asks to be signed in is signed in by its own
discovery and client registration. A stdio MCP server takes no `auth`:
it reads its credentials from its environment, which its `env` sets
(`${VAR}` there reads one from web2's), and an `auth` on one breaks
web2.json.

An OpenAPI service with no `auth` signs in the way its spec names: a
bearer token, or a key in a header or the query, stored by `web2 login`.
Until one is stored, web2 calls without one, so an API that answers
anonymous reads still answers, and a 401 or a 403 says how to sign in. A
spec that names HTTP basic or OAuth needs an `auth` in web2.json, for the
user or the client id, and `web2 login` says which, with the `auth` to
add. A service whose `auth` is in web2.json is not called until it is
signed in.

Signing in is a person's to do, at this machine: a bloom cannot open a
browser or type at a prompt, so a service that needs it answers with the
command to run. A sign-in link is printed, opened in the browser, and
waited on for five minutes. `WEB2_BROWSER` names the program the link is
handed to, for a machine with no browser of its own.

## For people

`web2` alone is the same REPL a bloom reaches (lines piped to it are run
one by one), and `web2 run '<line>'` one line of it. `web2 status` names
the config and the state directory and lists the services, what each
may write, and how each is signed in. `web2 refresh [service]` reads
specs and tool lists again; the commands read from a spec are kept for a
day, and an MCP server's tools are listed again when it says they
changed.

`web2 import` reads Claude Code (`~/.claude.json`, and `.mcp.json` in
the directory it runs in), Claude Desktop, Cursor, VS Code and Codex,
and adds each MCP server it finds to `mcpServers`, except hum's own and
any name web2.json already has; `--dry-run` shows what it would add and
leave out.
Each one only reads until web2.json turns its writes on. A server web2
does not read, such as a legacy SSE server or a name with a dot, is left
out and named with the reason, and so is one with a `${…}` in its `url`,
`headers` or `env`, VS Code's `${input:…}` among them, since `web2 serve`
may not run in the environment the import does; a person adds such a
server by hand. Like `allow`, an import refuses a change that would
break web2.json.

## Environment

| env | default | what |
|---|---|---|
| `WEB2_CONFIG` | `$XDG_CONFIG_HOME/hum/hives/web2.json` | the config |
| `WEB2_STATE` | `$XDG_STATE_HOME/hum/hives/web2` | the tokens, and the commands read from specs |
| `WEB2_BROWSER` | the system's browser | the program a sign-in link is handed to |
| `WEB2_SERVER_STDERR` | unset | set, a stdio MCP server's stderr is web2's; unset, it is dropped |
| `RUST_LOG` | `info` for `web2 serve`, `warn` otherwise | what web2 logs, to stderr |

The bee and `web2` in a terminal share services and sign-ins only when
they agree on `WEB2_CONFIG` and `WEB2_STATE`.

## Test

```bash
cd hives/web2 && cargo test
```

`tests/wrepl.rs` runs the `web2` binary against an OpenAPI service the
test serves and `examples/mock_mcp.rs`, an MCP server over stdio, given
its token in its `env`, and over HTTP, signed in with a bearer token and
with OAuth. `tests/bee.rs` has a
forager's prompt reach `wrepl` through a real humd and a nurse, and back.

## What it does not do

- It offers an MCP server no capabilities of its own: no sampling, no
  elicitation, no roots. A tool that asks for input mid-call, or turns
  into a long task, answers that wrepl does not follow it.
- Legacy SSE MCP servers and Swagger 2.0 specs are not read, and a
  spec's `$ref`s to other files are not followed.
- A peer's blooms reach it only when both sides say so: the peer's humd
  trusts remote attendants (`humd.trustRemoteAttendants` in its
  hum.json, or `HUM_TRUST_REMOTE_ATTENDANTS`) and has the web2 hive
  installed, and this web2.json says `servePeers: true`. `servePeers`
  lends them every service web2 is signed in to here, so it is a
  person's choice, off by default; until then a peer's reach is refused,
  saying so.
- The tokens are as safe as the user web2 runs as: any process of that
  user can read `secrets/`, and so can a bloom that builds on a
  [`bash`](../bash/README.md) builder running as that user.
