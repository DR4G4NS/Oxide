# Agent instructions (Oxide)

This file is for **coding agents** (Cursor, Copilot, Codex, Claude, Aider, …) working in a contributor’s clone or PR. Humans: start with [CONTRIBUTING.md](CONTRIBUTING.md). Architecture detail: [ARCHITECTURE.md](ARCHITECTURE.md).

You are a contributor to an independently authored **authoritative Mindustry server**. Stay on the rails below. Inventing a “cleaner” architecture, copying Java from Mindustry, or greenwashing CI is out of bounds.

## Read first

1. [README.md](README.md) — product scope, **documented gaps**, how to run.
2. [ARCHITECTURE.md](ARCHITECTURE.md) — modules, protocol contract, building recipe.
3. [CONTRIBUTING.md](CONTRIBUTING.md) — PR size, tests, license.
4. [THIRD_PARTY.md](THIRD_PARTY.md) — what may be committed.

If a gap is already listed in the README, **do not silently “finish” it** in the same bugfix PR. Either stay inside the requested change, or say the gap is out of scope.

## Non-negotiable

- **One binary, one target:** Mindustry v8 **159.7** (`compat/current.toml`). Do not retarget, dual-build, or infer layouts from 158.1 source order.
- **Do not copy Mindustry/Arc Java into `src/`.** Independently reimplement. GPL Java stays outside the tree (JAR as oracle only).
- **Do not commit JARs, extra Mindustry assets, `.cache/mindustry/`, `world-*.json`, `admin-data.json`, or agent session dirs.** Official campaign maps already live in `third_party/mindustry-maps/` under **GPLv3 (Anuken)**. Do not relicense them, copy more GPL trees into `src/`, or `include_bytes!` those maps into the production binary.
- **Do not invent wire.** Packet IDs, field order, and widths come from `compat/<build>/` + `javap` / TypeIO. `byte` / `short` / `int` are not interchangeable.
- **Do not speed snapshots** (`~50 ms` entities, `blockSyncTime` = 6 s) to hide missing simulation. If the client predicts a block, Rust must simulate it.
- **Do not hold `DashMap::Ref` / `RefMut`** while querying or mutating another tile. Snapshot keys, drop guards, then apply.
- **Do not add mods, plugins, custom packets, event-bus, or asset-stream work** unless the issue/PR explicitly asks. That is out of the 0.1 line.
- **Do not bump `Cargo.toml` version or rewrite `CERTIFIED_RUNTIME_SHA`** to make CI green. Recertification is a dedicated change.
- **Do not disable or skip CI gates** (`architecture_guard`, `dashmap_guard`, clippy `-D warnings`, cert ledger, low-core tests).
- **Do not add new markdown diaries** (audit dumps, status novels). Update README / ARCHITECTURE / this file only when the change requires it.
- **Do not drive-by reformat or rename** unrelated modules. Match the style of the file you are in.

## Where code goes

| Change | Put it here | Do not |
|---|---|---|
| Framing, session, tick, emit | `src/network/listener` | Per-block gameplay rules |
| Encode / decode | `src/network/wire` | Import listener |
| TypeIO config | `src/network/buildings/config` | Concatenate `extra_data` / Building tails |
| Placement lifecycle | `src/network/buildings/placement` | Skip `after_placement` on any build path |
| Power / sandbox / reactor / economy / combat / units | matching `src/network/...` domain | Treat TypeIO tag 5/14 as a raw ID |
| Operator UI | `src/tui` | Claim TUI is vanilla-parity |
| Content IDs / balance tables | `src/game/*.tsv` then codegen | Hand-edit giant ID lists in prose |

Valid handler: **decode with limits → authenticate → drop map guards → domain mutation → persist/broadcast**. Do not mutate the world mid-decode.

Domain must not import listener, runtime, console, or TUI (**ARCH001–ARCH002**). Emit through `&dyn FrameEmit`, not connection maps or outbound helpers (**ARCH003–ARCH006**).

Oversized files in `migration-reports/architecture-size-registry.tsv` are already classified. Do not “split for cleanliness” without an explicit architecture task.

## Porting a building (recipe)

Follow [ARCHITECTURE.md](ARCHITECTURE.md) § Recipe. Short form:

1. Pin class, ID, revision on the **159.7** JAR.
2. Extract `updateTile` / accept / config / `write`+`read`; confirm wire with the JAR.
3. State + transition in the **domain** module. Do not reuse a `DynamicTile` field without updating the legacy-field table in ARCHITECTURE.
4. Hook **once** in the scheduler and the accept boundary.
5. Byte-exact `writeSync` + full TypeIO config case.
6. Negative tests. If the client predicts the block, a test that spans **> 360** game ticks and crosses a `BlockSnapshot`.
7. `fmt`, clippy, full suite. JAR smoke when the change is on the wire.
8. Residual gap → README. Do not revive audit diaries.

## Verify before you stop

Always (no JAR):

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets -- --test-threads=1
git diff --check
```

`--test-threads=1` is required (DashMap shard collisions). Do not “fix” hangs by raising parallelism.

When you touch modules or concurrency:

```bash
bash tools/architecture_guard.sh
cargo run --quiet --manifest-path tools/dashmap_guard/Cargo.toml -- check . --paths src,tests --deny-warnings
```

When you touch `compat/`, protocol IDs, or save/stream codecs: run the matching fixture tests and do not hand-wave ledger rows to `PASS`.

Wire change without **exact payload fixture + framing (id, length, no trailing bytes) + mutation cases** is incomplete. The label “byte-exact” requires the target JAR to have read the full fixture.

## PR the agent should produce

Keep the diff **small and on-intent**. In the PR body (see `.github/pull_request_template.md`):

- **What changed** (user-visible vs internal).
- **Tests run** (commands + result). Name any JAR smoke.
- **Gaps** if behaviour is still short of vanilla.

Do not bundle unrelated refactors, dependency bumps, or README rewrites with a protocol fix.
