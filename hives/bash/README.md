---
title: "bash"
description: "a builder: the host it runs on, lent to the blooms that build there"
---

# bash

> _a builder: the host it runs on, lent to the blooms that build there_

Its one bee is entered as a builder. A bloom builds here when its
prompt's `cwd` names this humd, `hum://<humdid>/<path>` (`hum:///<path>`
from a forager on this humd), or when it names none and its forager, on
this humd, says `builds`. A bloom that builds carries the thrum's `bash`
tendril among its tools, and humd brings each reach for it to the
builder on the humd where the bloom builds, this one or a peer.

Each sigil gets one shell, `bash --noprofile --norc`, started where the
bloom builds (home, when the path is empty) and kept from one reach to
the next, so `cd` and `export` carry over; a reach from another
directory starts it again there. A command gets no stdin, and its result
is stdout and stderr together with the exit code, the first third and
last two thirds of it when it runs past 30,000 bytes. A command runs for
its `timeout`, two minutes unless it says otherwise and ten at most;
past that it is stopped with its shell and answered with exit code 124.
A sigil's shell ends with the sigil's `cleanup`, or after thirty minutes
without a command.

The shell runs with the builder's own environment, as the user the
builder runs as. Installed, the builder runs under orchd, so that
environment is the service manager's, not your login shell's, and the
shell reads no profile: on macOS its `PATH` is
`/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin`.

```
hum hive bash install
```

Installing this hive is how a humd lends its host. Every bloom that builds
here can run anything that user can, including blooms from this humd's
peers (no setting gates a peer's reach for `bash` the way
`trustRemoteNurses` gates its nurses), so install it only where that is
what you want.

```
cd hives/bash && cargo test    # the shell, against a real bash
```
