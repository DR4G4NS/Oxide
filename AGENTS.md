# Agent instructions (Oxide)

Oxide is an independently authored, authoritative Mindustry server. One binary targets **v8 159.7**, pinned in [compat/current.toml](compat/current.toml).

## Read for the task

Read the relevant sections; a documentation or typo fix does not require a full repository tour.

| Task | Reference |
|---|---|
| Product scope, running the server, unported content | [README.md](README.md) |
| Locating behaviour, packet paths, JAR queries, client harnesses | [navigation.md](navigation.md) |
| Gameplay, state, protocol, module boundaries | [ARCHITECTURE.md](ARCHITECTURE.md) |
| Investigating a vanilla divergence | Check [gaps.md](gaps.md) and the README's gap sections before calling it new |
| Validation and PR requirements | [CONTRIBUTING.md](CONTRIBUTING.md#validation) |
| Adding or redistributing third-party material | [THIRD_PARTY.md](THIRD_PARTY.md) |

## Constraints that change implementation decisions

- Independently implement behaviour. Do not copy Mindustry/Arc Java into `src/`; use the target JAR as an external oracle. Do not infer 159.7 wire layouts from 158.1 source order.
- Do not commit JARs, extra Mindustry assets, `.cache/mindustry/`, runtime saves (`world-*.json`, `admin-data.json`), or agent session directories. Existing official maps in `third_party/mindustry-maps/` remain **GPLv3 (Anuken)**; do not relicense or embed them in the production binary.
- Packet IDs, field order, widths, and TypeIO references come from `compat/<build>/` and the target JAR. Config objects, Building tails, and sync codecs are separate formats.
- Decode with limits, authenticate, release map guards, then delegate mutation and persist/broadcast. Do not mutate the world during decoding.
- Never hold a `DashMap::Ref` / `RefMut` while querying or mutating another tile. Snapshot keys, drop guards, then apply.
- Keep gameplay in its domain. Domains must not import listener, runtime, console, or TUI; emit through `&dyn FrameEmit`, not connection maps or outbound helpers (**ARCH001–ARCH006**). See the [module map](ARCHITECTURE.md#modules).
- Simulate predicted state on one authoritative path. Preserve entity snapshots at ~50 ms and `blockSyncTime` at 6 s; faster snapshots do not repair missing simulation. Every construction path runs `after_placement`.
- For a building port, use the [building recipe](ARCHITECTURE.md#recipe-for-porting-a-building). Update the legacy-field table before reusing a `DynamicTile` field. Change content IDs/balance in `src/game/*.tsv` and the relevant generator or consumer, not hand-written duplicate lists.
- Mods, plugins, custom packets, event-bus, and asset-stream work require an explicit task; they are outside the 0.1 scope. Do not close unrelated documented gaps or split modules for cleanliness in a bugfix.
- Do not disable compatibility, Rust, architecture, or concurrency checks or bump `Cargo.toml` version to make checks pass. Compatibility is validated by current tests and target-JAR probes, without a runtime-SHA certification gate.

## Completion

Carry the requested change through implementation, applicable validation, and fixes for failures it causes. Local edits and reruns within that scope do not need repeated approval. For manual server trials, use a separate save under `target/` and stop only the process started for the trial.

Use the [validation matrix](CONTRIBUTING.md#validation); documentation-only changes need documentation checks, while runtime changes need the Rust suite. Rust test commands require `--test-threads=1` because of DashMap shard collisions. A skipped JAR smoke is not a pass; report missing evidence and do not claim byte-exactness without the target JAR reading the complete fixture.

Update the existing document that owns the changed fact. Product limitations belong in README; located divergences belong in `gaps.md`. Delete a closed gap's section, retaining any narrower residual gap. Do not add audit diaries or duplicate checklists.

Keep the diff on the requested concern. Report what changed, validation commands and outcomes, and residual gaps using the [PR template](.github/pull_request_template.md).
