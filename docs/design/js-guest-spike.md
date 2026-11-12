# JavaScript guest spike (ECO-05)

Date: 2026-11-11. Review: pending user review (approved to proceed).

## Decision

**Go with changes for a constrained authoring toolchain; this is not approval
to ship JavaScript support.** A componentize-js guest built with `--disable all`
runs `create` and `list` through Terrarium's existing storage imports, linker,
transactions, fuel, deadlines and memory limits. No runtime linker change or
WASI implementation is needed. The default component does not link.

The two operations fit default limits on this Mac under both Cranelift and
Pulley. However, the component is about 80 times the Rust todo component's
size, dev-profile compilation takes tens of seconds, and Pulley operation
latency is hundreds of milliseconds. Keep Rust as the default authoring path.
Before making JS available, build a pinned, restricted toolchain and SDK,
measure release hosts and physical Android devices, and complete the licence
inventory. Do not raise default limits or reuse live instances to hide costs.

This task adds only this document to the host repository. Experiments ran in
`/tmp/eco-05-js-spike`; no branch was created or switched, no runtime change is
proposed here, and no plugin package source was changed. Guest JS compiled to
Wasm does not introduce JavaScript into JSON UI templates.

## Environment and scope of evidence

| Item | Tested value |
| --- | --- |
| Host source | `a3df59a09cbe7ae54a17c64b2c06de9939f3e1b0`, `feature/ecosystem` |
| Plugins source | `db79793bf47266f536056cb6cf4b4e1cb5357dae`, `feature/ecosystem` |
| Machine | macOS 26.5.1 (25F80), arm64; CPU model/RAM sysctl unavailable in sandbox |
| Rust / Cargo | 1.96.0 (`ac68faa20`) / 1.96.0 (`30a34c682`) |
| Host Wasmtime | 49.0.5, workspace lockfile, dev profile |
| Node / npm | v23.6.0 / 11.2.0 |
| jco / componentize-js | 1.41.0 / 0.23.0, StarlingMonkey backend, no `--aot` |
| Wizer | 10.0.0; local dev build with its Wasmtime 36.0.2 |
| wasm-tools | 1.252.0 |
| Android | `adb devices -l`: empty list; no device measurements |

The npm StarlingMonkey embedding SHA-256 was
`6094cd93d966f9bec9b01332db6e0eedbaf22f60b745c9f823f21e33cbd4d365`.
The generated component metadata identifies ComponentizeJS 0.23.0 and
wit-component 0.259.0. This identifies the binary tested without pretending
that an upstream `main` revision identifies the packaged engine.

The npm cache needed `--cache /tmp/eco-05-js-spike/npm-cache`. Bundled Wizer
failed before building a component with:

```text
Error: failed to create cache directory: /Users/rahul.pan/Library/Caches/BytecodeAlliance.wasmtime
Caused by: Operation not permitted (os error 1)
```

The sole Wizer source adjustment was selecting an explicit cache config in
`/tmp`, instead of `Cache::from_file(None)`. This is a **build-tool cache
location change**, not a guest WASI mitigation or host linker change. Its
reproduction is below. No global configuration or home directory was changed.

## Guest and component imports

The guest targets the unchanged `wit/plugin.wit`, world `plugin`, and imports
`terrarium:plugin/storage@0.1.0`. `create` inserts a synthetic task title;
`list` queries the same plugin's rows. UUIDs come from `storage.insert`, not
JavaScript randomness. The manifest reuses the Rust todo schema so both
implementations exercise the same Store API. JS lists only id/title; Rust's
closest exported read operation is `open_tasks`, which also computes dates,
filters and projects more columns. These are useful baselines, not equivalent
implementations or evidence that JS outperforms Rust.

Each row below was separately built and inspected using
`wasm-tools component wit <file>`. All four retain the same Terrarium imports:
`types`, `storage`, `clock`, `log`, `interop`, `speech`, `ai`, `schedule`, each
under `terrarium:plugin/…@0.1.0`. `types` is a type interface, not an ambient
service. Every build exports `handle` with the existing result/error contract.

| Build | Disabled features (cumulative) | Bytes | Build wall time | Remaining WASI imports |
| --- | --- | ---: | ---: | --- |
| default | none | 12,153,189 | 42.31 s | A + B + C + D below |
| no-http | `http fetch-event` | 12,139,255 | 33.74 s | B + C + D |
| no-stdio | `http fetch-event stdio` | 12,123,168 | 11.04 s | C + D |
| pure | `all` (`http fetch-event stdio random clocks`) | 12,112,357 | 11.26 s | **None** |

Exact groups, including versions:

- A: `wasi:http/types@0.2.10`, `wasi:http/outgoing-handler@0.2.10`.
- B: `wasi:cli/{stdin,stdout,terminal-input,terminal-output,terminal-stdin,terminal-stdout,terminal-stderr}@0.2.12`,
  `wasi:filesystem/{types,preopens}@0.2.12`.
- C: `wasi:io/{poll,error,streams}@0.2.12`, `wasi:cli/stderr@0.2.12`,
  `wasi:clocks/{monotonic-clock,wall-clock}@0.2.12`.
- D: `wasi:random/random@0.2.12`.

Disabling stdio alone does **not** remove all stderr/IO imports in this
version. The complete disable set does remove them. Build times include
snapshotting and component assembly, use a debug Wizer, and have different
cache warmth; the first two overlapped. They are reproducible observations,
not a comparison of optimization benefits.

The unchanged host rejected the default build after compilation:

```text
LOAD_ERROR Wasm("component imports instance `wasi:io/poll@0.2.12`, but a matching implementation was not found in the linker: instance export `pollable` has the wrong type: resource implementation is missing")
```

The pure component loaded and both operations succeeded against the unchanged
host in native and interpreter modes. Five alternating create/list calls in
each measured run left five synthetic rows and ten Store change records. A
fresh Wasm instance was created for every operation; persistence came from
SQLite. No WASI interfaces were ever added to either test linker.

### Alternatives if a future build retains WASI

| Option | Behavior to require | Evaluation in this spike |
| --- | --- | --- |
| Separate JS-only deny linker | Explicit implementations for exact interface versions and resources. Trap on HTTP requests, stream/poll operations and clock reads; filesystem preopens may return an empty list, filesystem operations return permission-denied; terminal getters may return none. No process stdio or environment passthrough. Trap on random requests, or supply host CSPRNG bytes if a separately reviewed policy permits randomness. Never return predictable bytes as cryptographic randomness. | Design evaluation only; not implemented or measured. Unnecessary for `--disable all`. Would expand the runtime trusted surface and obscure unsupported APIs. |
| wasi-virt | Compose virtualized interfaces into the component; default deny semantics trap on use. Inspect the resulting outer imports again and reject any remaining ambient interfaces. Pin compatible WIT versions and measure resulting size/cost. | Design evaluation only; not installed or composed. No claim of compatibility with these 0.2.12 imports. Adds a component and maintenance burden without a benefit for the pure guest. |
| No-go for that artifact | Reject unresolved imports; retain the existing Terrarium linker. | Default artifact was rejected as expected. This remains the policy for guests that cannot be built pure. |

The [WASI-Virt project](https://github.com/bytecodealliance/WASI-Virt)
describes deny-by-default virtualization. It is a fallback to investigate,
not a reason to permit ambient WASI. There is no measured post-virtualization
import list because this route was not needed or executed.

## Execution measurements

The instrumented runner uses a **temporary copy** of `terrarium-runtime`,
with timing prints around `Component::from_binary`, `pre.instantiate` and
`call_handle`, plus `Store::get_fuel`. A ResourceLimiter wrapper logs requested
memory/table sizes and delegates every decision and count limit to the
original StoreLimits. No limits, engine settings, epoch ticker, transaction,
storage implementation or linker registration were changed. The plain
control runner depends directly on the real worktree crate.

Each instrumented run compiles once, then performs five create/list pairs on
a temporary database containing zero through five synthetic rows. Values
below include the first operation; timings are medians unless a range is
shown. Instantiation includes start/data initialization and its fuel;
`handle` includes host storage calls; total call also includes the transaction.
Diagnostic printing and a dev host add overhead. Native and Pulley JS
compilations overlapped with each other and the rejection control, so cold
compile times are particularly sensitive to load. They are not release
performance forecasts. The later unchanged-host native load took 27.92 s.

| Guest / backend | Compile, once | Fresh instantiate create / read | Handle create / read | Total create / read |
| --- | ---: | ---: | ---: | ---: |
| Rust todo / Cranelift | 461.15 ms | 2.408 / 0.203 ms | 0.312 / 0.198 ms | 4.611 / 4.115 ms |
| JS pure / Cranelift | 57,801.67 ms | 1.040 / 1.055 ms | 0.583 / 0.480 ms | 2.039 / 1.927 ms |
| Rust todo / Pulley | 646.61 ms | 0.276 / 0.277 ms | 7.823 / 20.735 ms | 10.633 / 25.827 ms |
| JS pure / Pulley | 63,199.68 ms | 1.253 / 1.319 ms | 331.366 / 486.732 ms | 333.733 / 488.735 ms |

The noisy Rust native instantiate-create sample ranged 0.175–4.390 ms;
JS native ranged 0.977–2.317 ms. JS Pulley total create ranged
306.704–339.341 ms and list 343.465–537.787 ms. Do not infer a speedup from
the smaller JS native medians in this small, differently shaped workload.

| Guest / backend | Instantiate fuel | Create fuel, including instantiate | Read fuel, 1–5 rows, including instantiate |
| --- | ---: | ---: | ---: |
| Rust todo / Cranelift | 32,770 | 65,524 | 69,714–161,531 |
| JS pure / Cranelift | 8,470,530 | 9,831,720 | 9,972,069–10,706,020 |
| Rust todo / Pulley | 65,538 | 98,292 | 102,482–194,299 |
| JS pure / Pulley | 8,519,682 | 9,880,872 | 10,021,221–10,755,172 |

Fuel is Wasmtime accounting, not elapsed CPU time; backend totals differ.
These numbers cover only tiny operations, not large queries or long-running
JavaScript. No AOT, large-data, exception-stress or malicious-guest benchmark
was performed.

| Budget | Default / ceiling | Rust observed | JS pure observed | Result |
| --- | --- | --- | --- | --- |
| Component/package bytes | Package cap 32 MiB (33,554,432 bytes) | 151,391-byte component | 12,112,357-byte component (11.55 MiB) | Component plus this small manifest fits; package measurement below |
| Linear memory, per memory | 32 / 128 MiB | 1,114,112 initially, 1,179,648 peak (1.125 MiB) | 9,502,720 initially and peak (9.0625 MiB), both backends | Fits without raising limits |
| Fuel per operation | 200,000,000 / 2,000,000,000 | At most 194,299 here | At most 10,755,172 here | Under 5.4% of default |
| Timeout per operation | 2,000 / 10,000 ms | All calls succeeded | All calls succeeded | No deadline changes |
| Tables / elements | 32 tables, 20,000 elements per table | Initial tables 11 and 56 elements | Initial tables 23 and 7,714 elements | Fits existing table limits |
| Instances / memories | 32 / 4 | Loaded and called | Loaded and called | Fits existing count limits |

Memory above is the observed Wasm linear-memory allocation, not heap liveness
or resident process memory. The JS run had one reported linear memory and no
memory growth. Rust grew by one 64 KiB page per fresh call. Compiled code,
compiler allocations, SQLite and host bookkeeping are outside this limit.
**Compilation occurs during prepare, outside the per-operation epoch/fuel
budget**; large components therefore remain a load-time cost even when calls
fit. This spike does not establish a safe worst-case compile budget.

Separate **unchanged-host** runs measured whole-process peak RSS with Python
`getrusage`, including compilation, load, database setup and five operation
pairs. These are single-process peaks, not per-call retained allocations or
an estimate of mobile memory. Each Python wrapper launched only one child.

| Guest / backend | Plain host load (compile + setup) | Whole-process peak RSS |
| --- | ---: | ---: |
| Rust todo / Cranelift | 695.46 ms | 64,700,416 bytes (61.70 MiB) |
| JS pure / Cranelift | 21,949.26 ms | 621,068,288 bytes (592.30 MiB) |
| Rust todo / Pulley | 768.11 ms | 71,204,864 bytes (67.91 MiB) |
| JS pure / Pulley | 40,425.69 ms | 496,353,280 bytes (473.36 MiB) |

`terra pack pure --unsigned -o pure.terra` used the unchanged CLI, validated
the package and produced 3,984,030 bytes. Its expanded entries total
12,115,131 bytes (12,112,357 Wasm + 2,774 manifest), below the 32 MiB package
cap. `wasm-tools validate pure.wasm` passed. The pack output was:

```text
ok: js-spike 0.1.0 (2 operations, 0 queries, 0 pages, 0 widgets)
packed pure.terra (3890 KB)
digest: ab10661fdac1ce942259646c717db270bd81b9790c44f88adaa7d8e79e759824
unsigned
```

Representative instrumented native output (first create; generated synthetic
ID omitted):

```text
COMPILE mode=native bytes=12112357 ms=57801.671
TABLE current=0 desired=23
MEMORY current=0 desired=9502720
TABLE current=0 desired=7714
INSTANTIATE op=create ms=2.317 fuel=8470530
HANDLE op=create ms=2.775 fuel_total=9831720
CHANGES 10
```

Representative plain-host interpreter summary:

```text
LOAD ms=40425.686
CHANGES 10
PROCESS {"elapsed_s": 44.37538304110058, "peak_rss_bytes_macos": 496353280, "exit": 0}
```

The large compiler/process peak is a separate resource concern even though
the guest fits its Store limits. Release and device measurements must include
this peak, not only the smaller linear-memory figure.

Package cap and defaults were read from `crates/runtime/src/package.rs` and
`manifest.rs`; table/count limits from `host.rs`. Any later package requesting
higher manifest limits must go through the existing `capabilities` /
`added_capabilities` consent flow. No raised limit is required or recommended
for this fixture; ceilings must never be silently changed for JS.

## Author experience and proposed SDK

`jco types wit --world-name plugin -o types` successfully generated the world
and eight interface declaration files. For example, `handle` is typed as
`(operation: string, args: Json) => Json`, `insert` returns a string ID and
`query` returns JSON text. WIT result errors are represented through thrown
error payloads in JS, rather than returning `{tag: 'ok'}` from `handle`.
The synthetic guest exercises successful result lowering; general error,
rollback and type-safety coverage belongs in the follow-up SDK work.

A thin SDK should supply ambient module declarations for the exact versioned
WIT import specifiers (or suitable generated path mappings), object/JSON
wrappers for storage and handle arguments, a typed plugin-error helper, and
wrappers for Terrarium clock/log. The generated files alone do not declare
all these specifiers as ambient TypeScript modules. Use `storage.insert` for
IDs, Terrarium's clock for time and `log.info` for logging. Test thrown errors
and rollback before promising Rust SDK parity. No browser DOM, Node runtime,
arbitrary npm compatibility or direct network/file access should be promised.

The [ComponentizeJS feature documentation](https://github.com/bytecodealliance/ComponentizeJS#api)
warns that disabling random makes randomness deterministic and disabling clocks
breaks timers; the [component-model JS guide](https://component-model.bytecodealliance.org/language-support/building-a-simple-component/javascript.html)
also warns about misleading date/random outputs. The toolchain must document
these semantic differences and the SDK must direct authors to host services.
A component with no ambient imports is not proof that its built-ins have
normal JS behavior. Random-dependent or timer-dependent packages need their
own review; deterministic output must never be used for secrets.

Today `terra build` requires Cargo.toml, invokes cargo for
`wasm32-unknown-unknown`, then wraps a Rust core module with wit-component
(`crates/runtime/src/builder.rs`). It cannot build this JS package. Proposed
future detection: preserve the Cargo.toml path; add an explicit package.json
Terrarium build declaration with a JS entry point, reject ambiguous dual
configurations, and require a lockfile/pinned local jco. Invoke the pure build
profile, inspect/validate the resulting component, and emit the same
`dist/plugin.wasm` alongside the existing manifest and JSON templates. Do not
send an already componentized JS binary through the Rust core-module wrapper.
Treat npm dependency installation/build scripts and top-level snapshot
execution as author-machine build activity, never an app runtime service.

## Security, licences and compatibility

The runtime security boundary remains the outer component's imports and
Terrarium's existing quotas. `--disable all` removes ambient WASI bindings;
it does not grant storage across plugins, speech or AI permissions. The
runtime still enforces those permissions and all writes still use Store APIs.
The component contains a much larger JS parser, interpreter, GC and bindings
implementation, increasing untrusted code size and guest complexity. Wasmtime
and the host import/ABI boundary remain trusted attack surfaces. Pure mode
also loses stdio-based diagnostic output; plugin errors and `log.info` need
intentional handling.

There is no guest native-code JIT inside Wasm: StarlingMonkey's
[SpiderMonkey build configuration](https://github.com/bytecodealliance/StarlingMonkey/blob/main/cmake/spidermonkey.cmake)
disables JIT and enables the portable interpreter. This is distinct from
Wasmtime compiling Wasm to native code in Cranelift mode. Optional Weval AOT
is a build-time route and was not measured. A packaged engine's provenance
must still be pinned and checked for a release.

For REL-01, inventory the actual shipped engine and dependency notices:
jco/componentize-js/Wizer tooling declares Apache-2.0 with LLVM exception;
[StarlingMonkey](https://github.com/bytecodealliance/StarlingMonkey/blob/main/LICENSE)
is Apache-2.0; [Mozilla SpiderMonkey](https://github.com/mozilla-firefox/firefox/blob/main/js/src/vm/Interpreter.cpp) includes MPL-2.0-covered sources and
additional third-party notices. Preserve applicable notices and arrange source
availability for covered distributed code and any modifications, with an
artifact-specific review. This is an inventory handoff, not completed legal
approval or an assertion that every bundled object shares one licence.

Upgrade policy: this fixture uses the existing API/WIT and no new manifest
fields. A runtime that already supports that contract needs no JS-specific
linker upgrade; older published host binaries were not tested. Building from
JS requires the proposed newer author CLI. If later work adds a manifest
field or changes the contract, bump/gate the authoring target and require a
host upgrade before a synced package is accepted. Do not weaken old-host
unknown-field rejection or change defaults across synced devices. Pin build
tool and engine versions; inspect imports and rerun sandbox/limit tests on
every upgrade, since the disable profile and size may change.

## Follow-up tasks ready for planning

These are proposed tasks, not work implemented by this spike. Put them in
`tasks/12-ecosystem/` if the coordinator schedules JS support.

| Proposed task | Size | Acceptance gate |
| --- | --- | --- |
| Pin JS author toolchain and SDK | M | Locked tools/engine provenance; regenerated WIT types; storage/clock/log wrappers; error and rollback parity; warnings about disabled built-ins; minimal example with JSON UI only |
| Add opt-in JS builds to terra | M | Explicit entry/detection and ambiguity errors; pure profile; same package layout; versioned types; no npm installation in runtime; integration tests for unresolved WASI rejection and old Rust packages |
| Establish release/mobile performance budget | M | Repeat compile, fresh instantiate, fuel, linear-memory and process-RSS measurements with release hosts, realistic row counts, native Android and device Pulley; decide acceptable install/start/call latency before release |
| Validate the JS sandbox and upgrade policy | M | Malicious/unresolved imports fail closed; quotas, timeout, rollback, permissions and resource counts tested; tool updates cannot silently add ambient access |
| REL-01 JS-engine notice/source inventory | S | Actual engine revision, bundled dependency licences/notices and covered-source delivery recorded for distributed artifacts |

If release/device results cannot meet the agreed budgets with the same
sandbox and fresh-instance model, change the shipping decision to **no-go**.
WASI stubs, wasi-virt, AOT and engine sharing are separate experiments, not
implicit fallback implementation tasks.

## Validation

The task changes documentation only; runtime-contract/template/plugin changes
were not made, so the conditional cross-repository checks are not required.
All required host checks ran with `terrarium-app` excluded where applicable:

```text
cargo fmt --all -- --check                                      PASS
cargo clippy --locked --workspace --exclude terrarium-app --all-targets -- -D warnings  PASS
cargo test --locked --workspace --exclude terrarium-app           PASS
node --test ui/tests/*.test.cjs                                  PASS (90)
python3 -m unittest discover -s scripts/tests -v                  PASS (24)
```

No socket-related test failure occurred in this run. Native app builds were
not run. No tests or experimental code are added to the repositories.

## Reproduction

The scripts below are experimental source embedded in this report, not
shipping build machinery. Run from the host worktree at the revisions above.
All guest data is synthetic. Keep source worktrees unchanged. Use the shared
Cargo target directory specified for this worktree.

### Install tools and build the guest

```sh
export TERRA_HOST_SOURCE="$PWD"
export TERRA_PLUGIN_SOURCE="$(cd ../terrarium-plugins/packages && pwd)"
export CARGO_TARGET_DIR=/Users/rahul.pan/Code/po/ai_build_app/rust/wt/target-hist
mkdir -p /tmp/eco-05-js-spike
cd /tmp/eco-05-js-spike
npm init -y
npm --cache /tmp/eco-05-js-spike/npm-cache install --save-exact \
  @bytecodealliance/jco@1.41.0 @bytecodealliance/componentize-js@0.23.0
cargo install wasm-tools --version 1.252.0 --locked --root "$PWD/tools"
```

Save this as `/tmp/eco-05-js-spike/plugin.js`:

```js
import { insert, query } from 'terrarium:plugin/storage@0.1.0';
export function handle(operation, argsJson) {
  const args = JSON.parse(argsJson);
  if (operation === 'create') {
    const title = args.title.trim();
    if (!title) throw { code: 'invalid', message: 'Enter a title' };
    return JSON.stringify({ id: insert('tasks', JSON.stringify({ title })) });
  }
  if (operation === 'list') {
    return query('SELECT id, title FROM tasks WHERE _deleted IS NULL ORDER BY id', '[]');
  }
  throw { code: 'unknown-operation', message: 'Unknown operation' };
}
```

For a normal author machine, use the bundled Wizer. For this sandbox,
reproduce the cache-only adjustment (no HOME override):

```sh
curl -fsSL https://crates.io/api/v1/crates/wizer/10.0.0/download -o wizer.tgz
tar -xzf wizer.tgz
python3 - <<'PY'
from pathlib import Path
p = Path('wizer-10.0.0/src/lib.rs')
s = p.read_text().replace('wasmtime::Cache::from_file(None)',
    'wasmtime::Cache::from_file(Some(std::path::Path::new("/tmp/eco-05-js-spike/wizer-cache.toml")))')
p.write_text(s)
Path('wizer-cache.toml').write_text('[cache]\ndirectory = "/tmp/eco-05-js-spike/wizer-cache"\n')
PY
cargo build --locked --manifest-path wizer-10.0.0/Cargo.toml \
  --features env_logger,structopt --bin wizer
```

The four build/inspection commands (run one at a time for clean timings):

```sh
for variant in default no-http no-stdio pure; do
  case "$variant" in
    default) features=() ;;
    no-http) features=(--disable http fetch-event) ;;
    no-stdio) features=(--disable http fetch-event stdio) ;;
    pure) features=(--disable all) ;;
  esac
  ./node_modules/.bin/jco componentize plugin.js --wit "$TERRA_HOST_SOURCE/wit" \
    --world-name plugin --wizer-bin "$CARGO_TARGET_DIR/debug/wizer" \
    -o "$variant.wasm" "${features[@]}"
  ./tools/bin/wasm-tools component wit "$variant.wasm" > "$variant.wit"
done
./tools/bin/wasm-tools validate pure.wasm
./tools/bin/wasm-tools metadata show pure.wasm
./node_modules/.bin/jco types "$TERRA_HOST_SOURCE/wit" --world-name plugin -o types
```

Use bash or zsh for arrays. On an unrestricted machine, omit `--wizer-bin`
to use bundled Wizer; record that difference in timing comparisons. Snapshot
entropy and dependency resolution can change hashes across builds. Record a
lockfile and binary hashes for each rerun. This run produced:

```text
6f46969147cc628753da9c87b189a200409a03d7b26cc025b47dc6b80f42f522  plugin.js
aaca2e637ff777238b69f449f17b6835842a7eff4e9e0cd03db5a2ce03347b67  default.wasm
d6e5ae6c933a48723e8870f61f4a1dcff64985af8bc8272a3e263ba3e713e151  no-http.wasm
b8cbd63afa1f1d88c4eea05c09d49fe4633f78d555949d7b150e0c9195271ccd  no-stdio.wasm
204633b349cd33d0d259f36977412b1f4a638767f9fc9720796b40da8888c47e  pure.wasm
dad1eb0f706fbe9638f6f52ec2e4b13aa66de111130082b183c0637ac78e980e  package-lock.json
```

### Prepare synthetic packages and runner

Create the fixture directories from the unchanged todo schema:

```python
from pathlib import Path
import json, os, shutil
spike = Path('/tmp/eco-05-js-spike')
todo = json.loads((Path(os.environ['TERRA_PLUGIN_SOURCE']) / 'todo/manifest.json').read_text())
manifest = {
    'id': 'js-spike', 'name': 'JS spike', 'version': '0.1.0',
    'api': todo['api'], 'storage': todo['storage'],
    'operations': {
        'create': {'description': 'Create a synthetic task', 'effect': 'write',
                   'params': {'title': {'type': 'string', 'required': True}}},
        'list': {'description': 'List synthetic tasks'},
    },
}
for name in ['default', 'no-http', 'no-stdio', 'pure']:
    package = spike / name
    (package / 'dist').mkdir(parents=True, exist_ok=True)
    (package / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    shutil.copy(spike / (name + '.wasm'), package / 'dist/plugin.wasm')
```

Save this runner as `/tmp/eco-05-js-spike/spike.rs`. It is shared by the
unchanged-host control and the instrumented copy:

```rust
use terrarium_runtime::{Execution, HostConfig, PluginHost};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let execution = if a[2] == "pulley" { Execution::Interpreter } else { Execution::Native };
    let data = tempfile::tempdir().unwrap();
    let mut host = PluginHost::new(HostConfig {
        data_dir: data.path().join("plugins"), spaces_dir: None, space: "spike".into(),
        device: "spike-device".into(), user: "synthetic".into(), timezone: "UTC".into(),
        execution, speech: None,
    }).unwrap();
    let start = std::time::Instant::now();
    let id = match host.load(std::path::Path::new(&a[1])) {
        Ok(m) => m.id.clone(), Err(e) => { eprintln!("LOAD_ERROR {e:?}"); return; }
    };
    eprintln!("LOAD ms={:.3}",start.elapsed().as_secs_f64()*1000.0);
    let n: usize = a.get(3).map(|s| s.parse().unwrap()).unwrap_or(5);
    for _ in 0..n {
        for (op,args) in [("create",serde_json::json!({"title":"Synthetic task"})),
            (if id=="todo" {"open_tasks"} else {"list"},serde_json::json!({"today":"2026-11-11","end":"2026-11-18"}))] {
            let start = std::time::Instant::now();
            let result=host.call(&id,op,&args);
            eprintln!("CALL op={op} ms={:.3} result={result:?}",start.elapsed().as_secs_f64()*1000.0);
        }
    }
    eprintln!("CHANGES {}",host.store(&id).unwrap().changes(100).unwrap().len());
}
```

Save and run the following Python setup script **from the host worktree**:

```python
# Run from the host worktree. All experimental source goes under /tmp.
from pathlib import Path
import shutil
host = Path.cwd()
spike = Path('/tmp/eco-05-js-spike')
runtime = spike / 'runtime'
shutil.copytree(host / 'crates/runtime', runtime, dirs_exist_ok=True,
                ignore=shutil.ignore_patterns('target'))
s = (runtime / 'Cargo.toml').read_text()
values = {'version': '"0.1.0-alpha.1"', 'edition': '"2021"',
          'license': '"Apache-2.0"', 'publish': 'false',
          'repository': '"https://github.com/RAHUL445/terrarium"',
          'homepage': '"https://github.com/RAHUL445/terrarium"',
          'serde': '{ version = "1", features = ["derive"] }',
          'serde_json': '"1"', 'thiserror': '"2"', 'tempfile': '"3"'}
for key, value in values.items():
    s = s.replace(f'{key}.workspace = true', f'{key} = {value}')
for dep in ['core', 'ai', 'scheduler']:
    s = s.replace(f'path = "../{dep}"', f'path = "{host}/crates/{dep}"')
s += f'\n[workspace]\n[patch.crates-io]\nloro-internal = {{ path = "{host}/vendor/loro-internal" }}\n'
(runtime / 'Cargo.toml').write_text(s)
shutil.copy(host / 'Cargo.lock', runtime / 'Cargo.lock')
(runtime / 'src/bin').mkdir(exist_ok=True)
shutil.copy(spike / 'spike.rs', runtime / 'src/bin/spike.rs')
# For the unchanged-host control, use the same runner against the real crate.
plain = spike / 'plain'
(plain / 'src').mkdir(parents=True, exist_ok=True)
(plain / 'Cargo.toml').write_text(f'''[package]
name = "eco05-plain"
version = "0.0.0"
edition = "2021"
[workspace]
[dependencies]
terrarium-runtime = {{ path = "{host}/crates/runtime" }}
serde_json = "1"
tempfile = "3"
[patch.crates-io]
loro-internal = {{ path = "{host}/vendor/loro-internal" }}
''')
shutil.copy(host / 'Cargo.lock', plain / 'Cargo.lock')
shutil.copy(spike / 'spike.rs', plain / 'src/main.rs')
```

Save this patch as `/tmp/eco-05-js-spike/instrumentation.patch`, then apply it
to the temporary copy with the explicit target below. Adjust the WIT path in
the first hunk for a different checkout. It contains no linker changes.

```diff
--- host.rs
+++ host.rs
@@ -19 +19 @@
-    wasmtime::component::bindgen!({ path: "../../wit", world: "plugin" });
+    wasmtime::component::bindgen!({ path: "/Users/rahul.pan/Code/po/ai_build_app/rust/wt/feature-ecosystem/terrarium/wit", world: "plugin" });
@@ -191,0 +192 @@
+        let start = std::time::Instant::now();
@@ -192,0 +194 @@
+        eprintln!("COMPILE mode={} bytes={} ms={:.3}", self.mode(), bytes.len(), start.elapsed().as_secs_f64()*1000.0);
@@ -216 +218 @@
-            store.limiter(|s| &mut s.limits);
+            store.limiter(|s| s);
@@ -229,0 +232 @@
+                let start = std::time::Instant::now();
@@ -230,0 +234,2 @@
+                eprintln!("INSTANTIATE op={operation} ms={:.3} fuel={}", start.elapsed().as_secs_f64()*1000.0, limits.fuel-store.get_fuel().unwrap());
+                let start = std::time::Instant::now();
@@ -233,0 +239 @@
+                eprintln!("HANDLE op={operation} ms={:.3} fuel_total={}", start.elapsed().as_secs_f64()*1000.0, limits.fuel-store.get_fuel().unwrap());
@@ -253,0 +260 @@
+            eprintln!("END op={operation} fuel_total={} result={result:?}", limits.fuel-store.get_fuel().unwrap());
@@ -926,0 +934,17 @@
+
+// Diagnostic pass-through: the original StoreLimits still decides every limit.
+impl wasmtime::ResourceLimiter for HostState {
+    fn memory_growing(&mut self, current: usize, desired: usize, maximum: Option<usize>) -> wasmtime::Result<bool> {
+        eprintln!("MEMORY current={current} desired={desired}");
+        wasmtime::ResourceLimiter::memory_growing(&mut self.limits, current, desired, maximum)
+    }
+    fn table_growing(&mut self, current: usize, desired: usize, maximum: Option<usize>) -> wasmtime::Result<bool> {
+        eprintln!("TABLE current={current} desired={desired}");
+        wasmtime::ResourceLimiter::table_growing(&mut self.limits, current, desired, maximum)
+    }
+    fn memory_grow_failed(&mut self, e: wasmtime::Error) -> wasmtime::Result<()> { wasmtime::ResourceLimiter::memory_grow_failed(&mut self.limits, e) }
+    fn table_grow_failed(&mut self, e: wasmtime::Error) -> wasmtime::Result<()> { wasmtime::ResourceLimiter::table_grow_failed(&mut self.limits, e) }
+    fn instances(&self) -> usize { wasmtime::ResourceLimiter::instances(&self.limits) }
+    fn tables(&self) -> usize { wasmtime::ResourceLimiter::tables(&self.limits) }
+    fn memories(&self) -> usize { wasmtime::ResourceLimiter::memories(&self.limits) }
+}
```

```sh
patch /tmp/eco-05-js-spike/runtime/src/host.rs < /tmp/eco-05-js-spike/instrumentation.patch
cargo build --offline --manifest-path /tmp/eco-05-js-spike/runtime/Cargo.toml --bin spike
cargo build --offline --manifest-path /tmp/eco-05-js-spike/plain/Cargo.toml
cd /tmp/eco-05-js-spike
for mode in native pulley; do
  "$CARGO_TARGET_DIR/debug/spike" pure "$mode" 5 > "js-$mode.log" 2>&1
  "$CARGO_TARGET_DIR/debug/spike" "$TERRA_PLUGIN_SOURCE/todo" "$mode" 5 > "rust-$mode.log" 2>&1
done
"$CARGO_TARGET_DIR/debug/eco05-plain" default native 1 > default-load.log 2>&1
"$CARGO_TARGET_DIR/debug/eco05-plain" pure native 5 > plain-native.log 2>&1
"$CARGO_TARGET_DIR/debug/eco05-plain" pure pulley 5 > plain-pulley.log 2>&1
"$CARGO_TARGET_DIR/debug/terra" pack pure --unsigned -o pure.terra
```

The Rust baseline used the existing built todo component, then was verified
byte-for-byte against a locked source rebuild and the unchanged runtime's
component encoder. This preserves the specified shared target directory:

```sh
cd "$TERRA_HOST_SOURCE"
cargo build --locked --release --target wasm32-unknown-unknown \
  --manifest-path "$TERRA_PLUGIN_SOURCE/todo/Cargo.toml"
mkdir -p /tmp/eco-05-js-spike/plain/src/bin
cat > /tmp/eco-05-js-spike/plain/src/bin/wrap.rs <<'RS'
fn main() {
    let a: Vec<_> = std::env::args().collect();
    let core = std::fs::read(&a[1]).unwrap();
    let component = terrarium_runtime::componentize(&core).unwrap();
    std::fs::write(&a[2], component).unwrap();
}
RS
cargo run --offline --manifest-path /tmp/eco-05-js-spike/plain/Cargo.toml --bin wrap -- \
  "$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/todo.wasm" \
  /tmp/eco-05-js-spike/verified-todo.wasm
shasum -a 256 /tmp/eco-05-js-spike/verified-todo.wasm \
  "$TERRA_PLUGIN_SOURCE/todo/dist/plugin.wasm"
```

Both component hashes were
`7e729ef72395d290653c8bb5ca0d052d474336be14a96c429997e6f566552906`.
The core module hash was
`ee115cc560a6c193acf6ae26859557d2ae39e69e7dee6df84961313c67b445a3`.
If a rebuild differs, use the newly wrapped component in a temporary copy of
the todo package and record the changed baseline. Do not silently benchmark
stale dist output. The source build and wrapper both succeeded in this run.

For process RSS on macOS, save this as `measure.py` and prefix each **plain**
runner invocation with `python3 measure.py`:

```python
import json,resource,subprocess,sys,time
start=time.monotonic()
r=subprocess.run(sys.argv[1:])
print('PROCESS '+json.dumps({'elapsed_s':time.monotonic()-start,'peak_rss_bytes_macos':resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss,'exit':r.returncode}),flush=True)
raise SystemExit(r.returncode)
```

`getrusage(RUSAGE_CHILDREN).ru_maxrss` is bytes on macOS; run a new Python
process per sample. An attempted `/usr/bin/time -l` could run the guest but
could not read `kern.clockrate` in the sandbox, so its status was 1 and it
supplied no usable RSS result. The Python wrapper avoids that sysctl; its
runner exit statuses were 0. This time-tool error was not a guest trap.
