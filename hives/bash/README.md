---
title: "bash"
description: "a builder: the host it runs on, lent to the blooms that build there"
---

# bash

> _a builder: the host it runs on, lent to the blooms that build there_

Its one bee is entered as a builder. A bloom builds here when its prompt's
`cwd` names this humd, `hum://<humdid>/<path>`, or when its forager says
`builds` and the prompt names none. Every nurse hands its cell the thrum's
`bash` tendril, and humd brings each reach here.

Each sigil gets one shell, `bash --noprofile --norc`, started where the
bloom builds (home, when the path is empty) and kept from one reach to the
next, so `cd` and `export` carry over. A command gets no stdin, its output
is stdout and stderr together, and a command past its `timeout` is stopped
with its shell. The shell runs with the builder's own environment, as the
user the builder runs as.

```
hum hive bash install
```

Installing this hive is how a humd lends its host. Every bloom that builds
here can run anything that user can, including blooms from peers this humd
trusts, so install it only where that is what you want.
