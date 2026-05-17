<p align="center">
  <strong>﹏ hum ﹏</strong>
  <br>
  The only AI stack nestled on a biodiverse agentic kernel framework.
</p>


```
curl -fsSL https://raw.githubusercontent.com/adiled/hum/main/install | bash
```

**hum** (n.) The phenomena occuring upon perfect harmonization of all players in an AI composition.

```
hum update
hum status
hum logs
hum sessions
hum uninstall
```

## Run in 60 seconds (local dev — no install needed)

```bash
# terminal 1 — daemon binds $XDG_RUNTIME_DIR/hum/thrum.sock
cargo run -p humd

# terminal 2 — the openai-server nestler hellos humd
cd nestlings/openai-server && pnpm install && pnpm run build
node dist/index.js

# terminal 3 — talk to hum's nest via openai-compatible shape
curl http://127.0.0.1:14620/v1/models
```

Three terminals. No systemd. No `./install`. Stops when you Ctrl-C.

For persistent setups (`./install`) and mesh deployments
(ensemble + remote nestlers), see [nestlings/README.md](nestlings/) §
*Three paradigms for running a nestler*.

**Key buildings blocks**

it is all composed of biodiverse primitives, not an outcome of conventional linguistics, following are some nouns

- **thrum** the hum-native vibration protocol, built on a range of `chi`
- **nest** where netlers nestle
- **nestled** a nestler when it has nestled 
- **nestling** what a nestler conforms to, for becoming nestled
- **petal** a cohesive yet inconclusive evidence of a bloom
- **bloom** the one which wilts

Learn more by simply humming along..

**Config** `~/.config/hum/hum.json`

Refer to hum.schema.json
