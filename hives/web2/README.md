---
title: "web2"
description: "an attendant: the web2 world as one tendril, wrepl"
---

# web2

> _an attendant: the web2 world as one tendril, wrepl_

Its one bee is entered as an attendant and serves one fixed tool,
`wrepl`. A reach carries one command line, and every API and MCP server
web2 knows answers it as if it were a CLI:

```
help
github help pulls
github pulls/list rust-lang rust --state open --jq '.[].title'
sentry search-issues --query 'is:unresolved' --jq '.[0]'
```

The thrum sees none of it: `wrepl` is a tendril like any other, so
every nurse hands it to its cells, and a harness that takes tools only
over MCP gets it as one tool among `bash` and the forager's own.
OpenAPI, MCP and OAuth stop here.

```
hum hive web2 install
```

## Services

web2 reaches what `~/.config/hum/hives/web2.json` names. Each service is
an OpenAPI 3 spec, from a URL or a file beside web2.json, in JSON or
YAML, or an MCP server, over stdio or streamable HTTP. An `mcpServers`
block pasted verbatim from any harness's config works as it is, and
`web2 import` copies in the servers the harnesses on this machine
already know (Claude Code, Claude Desktop, Cursor, VS Code, Codex).

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
policy of the `mcpServers` entry of the same name. web2.json is read
again whenever it changes, so a service added there is reached on the
next call, with no restart. `${VAR}` in a base URL, a header or an env
value reads the environment web2 runs in.

| key | |
|---|---|
| `openapi` | the spec's URL or path; `base` overrides the server it names |
| `mcp` | `{ "command", "args", "env", "cwd" }` for stdio, `{ "url", "headers" }` for HTTP |
| `description` | one line for `help`; an OpenAPI spec's title stands in |
| `writes` | `true` turns every write on |
| `allow` | the write commands on, by name or glob (`create-*`), when `writes` is not |
| `deny` | commands off, by name or glob, whatever else says |
| `auth` | `"bearer"`, `{ "header": "X-Api-Key" }`, `{ "query": "api_key" }`, `{ "basic": "<user>" }`, or `{ "oauth": { "clientId", "clientSecret", "scopes" } }`; add `"env": "VAR"` to read the token from the environment instead of storing it |
| `timeoutMs` | how long a call may take, 120000 by default |

At the top level, `maxOutput` (32 KiB) bounds an answer, keeping its
start and its end, `timeoutMs` sets every service's default, `idleMs`
(ten minutes) is how long an MCP server stays connected unused, and
`redirectPort` (8976) is where a sign-in comes back to.

## Commands

An OpenAPI operation is a command named by its `operationId`,
kebab-cased (`listPets` is `list-pets`), grouped by its first tag. Its
parameters and the fields of its JSON body are its flags. An MCP tool
is a command named by its tool name, kebab-cased, with its input
schema's properties as flags. Spelling is forgiven across kebab, snake
and camel case.

`--name value` and `--name=value` set a flag, `--flag` and `--no-flag` a
boolean, and a flag repeated builds an array. Words with no flag fill
the command's required inputs in order. `--json '{…}'` gives the whole
input, flags laid over it winning, and `--jq '<filter>'` narrows a JSON
result as jq would. A service with many commands lists its groups;
`<service> help <group>` or `<service> help <words>` finds the rest.

## Reading and writing

A service only reads until web2.json says otherwise. An OpenAPI
operation reads when its method is GET, HEAD or OPTIONS. An MCP tool
reads only when its annotations say `readOnlyHint: true`, as the MCP
spec has it; most servers do not say, so their tools count as writes
until `writes` or `allow` turns them on. A refused call answers with the
key that would turn it on, and a service's help counts what is off.
Which writes a bloom may make is the config's to say, never the bloom's.

```
web2 review deepwiki             each command that is off, turned on or left, one by one
web2 allow deepwiki 'read-*'     on, by name or glob
web2 deny github 'repos/delete*' off, whatever else says
```

These edit web2.json in place, keeping its order, and refuse a change
that would break it.

## Signing in

```
web2 login github              a token, typed at a prompt that does not show it
web2 login github --token-stdin
web2 login sentry              an MCP server's own OAuth, in a browser
web2 logout github
```

A token is stored in `secrets/credentials.json` in web2's state
directory, mode 0600 in a 0700 directory, and never put in an
environment, logged or answered back. An
OpenAPI service with an `oauth` app signs in with the authorization code
flow and PKCE, through a callback on `127.0.0.1:8976`, and its token is
renewed before it expires; the authorization and token URLs come from
the spec when it names them. An MCP server over HTTP that asks to be
signed in is signed in by its own discovery and client registration.
When a spec names a way to sign in and nothing is stored, web2 calls
without one, so an API that answers anonymous reads still answers; a 401
or a 403 says how to sign in.

Signing in is a person's to do, at this machine: a bloom cannot open a
browser or type at a prompt, so a service that needs it answers with the
command to run. `WEB2_BROWSER` names the program a sign-in link is handed
to, for a machine with no browser of its own.

## For people

`web2` alone is the same REPL a bloom reaches, and `web2 run '<line>'`
one line of it. `web2 status` lists the services and how each is signed
in, and `web2 refresh` reads specs and tool lists again (a spec is kept
for a day). The hive installs nothing on the PATH; the install prints
where its binary is.

## What it does not do

- It offers an MCP server no capabilities of its own: no sampling, no
  elicitation, no roots. A tool that asks for input mid-call, or turns
  into a long task, answers that wrepl does not follow it.
- Legacy SSE MCP servers and Swagger 2.0 specs are not read.
- Its tools reach blooms grown on its own humd; humd does not hand a
  peer's blooms a local attendant's tools yet.
- The tokens are as safe as the user web2 runs as: a process of that
  user can read `secrets/`. A builder that keeps its shells out of hum's
  secrets closes that to blooms.
