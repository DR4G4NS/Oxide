//! Exact DashMap 5.5.3 method -> locking-effect table.
//!
//! Derived from the actual installed registry source:
//!   dashmap-5.5.3 (Cargo.lock pins 5.5.3, checksum 978747c1d849a7d2...)
//!   <CARGO_HOME>/registry/src/index.crates.io-*/dashmap-5.5.3/src/lib.rs
//!   and the same revision's mapref/{one,entry}.rs + iter.rs + lock.rs.
//!
//! See DASHMAP_55_EFFECTS.md for the authoritative prose table.

use std::collections::BTreeSet;

/// What kind of shard lock a DashMap operation needs on the affected shard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LockKind {
    /// `RwLockReadGuard` on the shard (`parking_lot`-style shared lock).
    Shared,
    /// `RwLockWriteGuard` on the shard (exclusive lock).
    Exclusive,
}

/// Kinds of DashMap effects (used both for direct ops and call-graph summaries).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EffectKind {
    /// Acquired a shared read guard (get/try_get/view-closure).
    ReadLock,
    /// Acquired an exclusive write guard (get_mut/try_get_mut/entry/iter_mut-closure).
    WriteLock,
    /// Key/value mutation that takes an exclusive shard lock: insert/alter/alter_all.
    ExclusiveMutation,
    /// Remove-family under an exclusive shard lock.
    Remove,
    /// Clear/retain/shrink_to_fit: exclusive lock on all shards.
    ClearRetain,
    /// Iterator over the map holding the shard shared lock while advancing.
    IterRead,
    /// Mutable iterator holding the shard exclusive lock while advancing.
    IterWrite,
}

/// The guard kind produced by a DashMap call that stays alive in the user code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GuardKind {
    /// Immutable value guard (`Ref`) — shared shard lock held.
    Read,
    /// Mutable value guard (`RefMut`) or `Entry` — exclusive shard lock held.
    Write,
    /// Immutable iterator (`Iter`) — per-shard shared lock while advancing.
    IterRead,
    /// Mutable iterator (`IterMut`) — per-shard exclusive lock while advancing.
    IterWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DmCode {
    Dm001,
    Dm002,
    Dm003,
    Dm004,
    Dm005,
    Dm900,
    Dm901,
}

/// Internal analyzer / tool failure codes (always blocking).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ToolCode {
    Tool001,
    Tool002,
    Tool003,
    Tool004,
}

impl ToolCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ToolCode::Tool001 => "TOOL001",
            ToolCode::Tool002 => "TOOL002",
            ToolCode::Tool003 => "TOOL003",
            ToolCode::Tool004 => "TOOL004",
        }
    }

    pub fn is_blocking(self) -> bool {
        true
    }
}

impl DmCode {
    pub fn as_str(self) -> &'static str {
        match self {
            DmCode::Dm001 => "DM001",
            DmCode::Dm002 => "DM002",
            DmCode::Dm003 => "DM003",
            DmCode::Dm004 => "DM004",
            DmCode::Dm005 => "DM005",
            DmCode::Dm900 => "DM900",
            DmCode::Dm901 => "DM901",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<DmCode> {
        match s {
            "DM001" => Some(DmCode::Dm001),
            "DM002" => Some(DmCode::Dm002),
            "DM003" => Some(DmCode::Dm003),
            "DM004" => Some(DmCode::Dm004),
            "DM005" => Some(DmCode::Dm005),
            "DM900" => Some(DmCode::Dm900),
            "DM901" => Some(DmCode::Dm901),
            _ => None,
        }
    }

    /// Blocking codes cause a non-zero exit. DM900/DM901 are warnings.
    pub fn is_blocking(self) -> bool {
        matches!(
            self,
            DmCode::Dm001 | DmCode::Dm002 | DmCode::Dm003 | DmCode::Dm004 | DmCode::Dm005
        )
    }
}

/// One DashMap 5.5.3 method's behaviour.
#[derive(Debug, Clone)]
pub struct MethodSpec {
    pub name: &'static str,
    pub lock: LockKind,
    /// If the call yields a guard that can stay alive in user code.
    pub guard: Option<GuardKind>,
    pub effects: &'static [EffectKind],
}

/// Exact methods pulled from the installed DashMap 5.5.3.
pub const DASHMAP_METHODS: &[MethodSpec] = &[
    // ---- guard-producing ----
    MethodSpec {
        name: "get",
        lock: LockKind::Shared,
        guard: Some(GuardKind::Read),
        effects: &[EffectKind::ReadLock],
    },
    MethodSpec {
        name: "try_get",
        lock: LockKind::Shared,
        guard: Some(GuardKind::Read),
        effects: &[EffectKind::ReadLock],
    },
    MethodSpec {
        name: "get_mut",
        lock: LockKind::Exclusive,
        guard: Some(GuardKind::Write),
        effects: &[EffectKind::WriteLock],
    },
    MethodSpec {
        name: "try_get_mut",
        lock: LockKind::Exclusive,
        guard: Some(GuardKind::Write),
        effects: &[EffectKind::WriteLock],
    },
    MethodSpec {
        name: "entry",
        lock: LockKind::Exclusive,
        guard: Some(GuardKind::Write),
        effects: &[EffectKind::WriteLock],
    },
    MethodSpec {
        name: "try_entry",
        lock: LockKind::Exclusive,
        guard: Some(GuardKind::Write),
        effects: &[EffectKind::WriteLock],
    },
    MethodSpec {
        name: "iter",
        lock: LockKind::Shared,
        guard: Some(GuardKind::IterRead),
        effects: &[EffectKind::IterRead],
    },
    MethodSpec {
        name: "iter_mut",
        lock: LockKind::Exclusive,
        guard: Some(GuardKind::IterWrite),
        effects: &[EffectKind::IterWrite],
    },
    // ---- exclusive mutators ----
    MethodSpec {
        name: "insert",
        lock: LockKind::Exclusive,
        guard: None,
        effects: &[EffectKind::ExclusiveMutation],
    },
    MethodSpec {
        name: "remove",
        lock: LockKind::Exclusive,
        guard: None,
        effects: &[EffectKind::Remove],
    },
    MethodSpec {
        name: "remove_if",
        lock: LockKind::Exclusive,
        guard: None,
        effects: &[EffectKind::Remove],
    },
    MethodSpec {
        name: "remove_if_mut",
        lock: LockKind::Exclusive,
        guard: None,
        effects: &[EffectKind::Remove],
    },
    MethodSpec {
        name: "retain",
        lock: LockKind::Exclusive,
        guard: None,
        effects: &[EffectKind::ClearRetain],
    },
    MethodSpec {
        name: "clear",
        lock: LockKind::Exclusive,
        guard: None,
        effects: &[EffectKind::ClearRetain],
    },
    MethodSpec {
        name: "shrink_to_fit",
        lock: LockKind::Exclusive,
        guard: None,
        effects: &[EffectKind::ClearRetain],
    },
    MethodSpec {
        name: "alter",
        lock: LockKind::Exclusive,
        guard: None,
        effects: &[EffectKind::ExclusiveMutation],
    },
    MethodSpec {
        name: "alter_all",
        lock: LockKind::Exclusive,
        guard: None,
        effects: &[EffectKind::ExclusiveMutation],
    },
    // ---- read-only (brief shared lock) ----
    MethodSpec {
        name: "view",
        lock: LockKind::Shared,
        guard: None,
        effects: &[EffectKind::ReadLock],
    },
    MethodSpec {
        name: "contains_key",
        lock: LockKind::Shared,
        guard: None,
        effects: &[EffectKind::ReadLock],
    },
    MethodSpec {
        name: "contains",
        lock: LockKind::Shared,
        guard: None,
        effects: &[EffectKind::ReadLock],
    },
    MethodSpec {
        name: "hasher",
        lock: LockKind::Shared,
        guard: None,
        effects: &[EffectKind::ReadLock],
    },
    MethodSpec {
        name: "len",
        lock: LockKind::Shared,
        guard: None,
        effects: &[EffectKind::ReadLock],
    },
    MethodSpec {
        name: "is_empty",
        lock: LockKind::Shared,
        guard: None,
        effects: &[EffectKind::ReadLock],
    },
    MethodSpec {
        name: "capacity",
        lock: LockKind::Shared,
        guard: None,
        effects: &[EffectKind::ReadLock],
    },
];

/// Look up the spec for a method name (only if it is a `&self` DashMap method).
pub fn dashmap_spec(name: &str) -> Option<&'static MethodSpec> {
    DASHMAP_METHODS.iter().find(|m| m.name == name)
}

pub fn is_dashmap_method(name: &str) -> bool {
    dashmap_spec(name).is_some()
}

/// Iterator methods that end a temporary chain: the iterator is exhausted and
/// its shard lock is released before the enclosing statement completes.
pub fn iterator_chain_terminal(method: &str) -> bool {
    matches!(
        method,
        "collect"
            | "count"
            | "sum"
            | "product"
            | "fold"
            | "reduce"
            | "for_each"
            | "try_fold"
            | "try_for_each"
            | "all"
            | "any"
            | "find"
            | "find_map"
            | "position"
            | "rposition"
            | "max"
            | "min"
            | "last"
            | "nth"
            | "partition"
            | "unzip"
            | "min_by"
            | "max_by"
            | "min_by_key"
            | "max_by_key"
            | "fold_first"
            | "next_chunk"
            | "count_eq"
    )
}

/// True when a fresh call to `method` would leave a live guard if bound.
pub fn produces_guard(method: &str) -> Option<GuardKind> {
    dashmap_spec(method).and_then(|m| m.guard)
}

/// Post-fix method names that *consume/destroy* a DashMap guard. Used to
/// decide whether `let x = <guard expr>.NAME(...)` still holds the guard.
pub fn consumed_by_followup(kind: GuardKind, method: &str) -> bool {
    match kind {
        // Option<Ref> / TryResult<Ref> consumers.
        GuardKind::Read | GuardKind::Write => matches!(
            method,
            "map"
                | "and_then"
                | "filter_map"
                | "flat_map"
                | "into_pair"
                | "into_pair_mut"
                | "into_key"
                | "is_some"
                | "is_none"
                | "is_occupied"
                | "is_some_and"
                | "is_none_or"
                | "map_or"
                | "map_or_else"
                | "clone"
                | "cloned"
                | "copied"
        ),
        // Iterator methods that take `self` by value and fully consume the
        // iterator object. Methods taking `&self` / `&mut self` (next, nth,
        // find, any, …) do NOT release the bound iterator guard.
        GuardKind::IterRead | GuardKind::IterWrite => matches!(
            method,
            "collect"
                | "count"
                | "sum"
                | "product"
                | "fold"
                | "reduce"
                | "for_each"
                | "partition"
                | "unzip"
                | "last"
                | "fold_first"
                | "compare"
                | "partial_cmp"
                | "eq"
                | "ne"
                | "count_eq"
        ),
    }
}

/// Iterator adapters that keep the iterator (and therefore the per-shard lock)
/// alive: guard stays held.
pub fn is_lazy_iterator_adapter(method: &str) -> bool {
    matches!(
        method,
        "map"
            | "filter"
            | "filter_map"
            | "flat_map"
            | "flatten"
            | "take"
            | "skip"
            | "take_while"
            | "skip_while"
            | "step_by"
            | "enumerate"
            | "zip"
            | "chain"
            | "cloned"
            | "copied"
            | "rev"
            | "inspect"
            | "fuse"
            | "peekable"
            | "by_ref"
            | "cycle"
            | "intersperse"
            | "interleave"
            | "scan"
            | "array_chunks"
            | "chunks"
            | "windows"
            | "duplicates"
            | "unique"
            | "unique_by"
            | "tee"
            | "group_by"
            | "dedup"
            | "dedup_by"
    )
}

/// Iterator adapters that run the closure *while* the shard lock is held
/// (for_each/all/any/find/fold/position/... and lazy adapters' closures).
pub fn should_walk_iterator_closure(method: &str) -> bool {
    matches!(
        method,
        "map"
            | "filter"
            | "filter_map"
            | "flat_map"
            | "for_each"
            | "take_while"
            | "skip_while"
            | "all"
            | "any"
            | "find"
            | "find_map"
            | "fold"
            | "try_fold"
            | "position"
            | "inspect"
            | "max_by"
            | "min_by"
            | "max_by_key"
            | "min_by_key"
            | "scan"
            | "try_for_each"
            | "group_by"
            | "dedup_by"
            | "enumerate"
            | "chain"
            | "zip"
    )
}

/// Which argument of the iterator adapter is the user closure (index within the
/// method call's argument list, 0-based, excluding the receiver).
pub fn iterator_closure_arg_index(method: &str) -> usize {
    match method {
        "all" | "any" | "find" | "find_map" | "fold" | "try_fold" | "position" | "max_by"
        | "min_by" | "max_by_key" | "min_by_key" | "try_for_each" | "inspect" | "for_each" => 0,
        "map" | "filter" | "filter_map" | "flat_map" | "take_while" | "skip_while" | "scan"
        | "dedup_by" | "group_by" => 0,
        _ => usize::MAX,
    }
}

/// DashMap `&self` methods that take a closure run while the shard is locked.
pub fn dashmap_closure_guard(method: &str) -> Option<GuardKind> {
    match method {
        "view" => Some(GuardKind::Read),
        "retain" | "remove_if" | "remove_if_mut" | "alter" | "alter_all" => Some(GuardKind::Write),
        _ => None,
    }
}

/// The closure argument index (excluding receiver) for DashMap closure methods.
pub fn dashmap_closure_arg_index(method: &str) -> Option<usize> {
    match method {
        "view" => Some(0),
        "retain" => Some(0),
        "remove_if" => Some(1),
        "remove_if_mut" => Some(1),
        "alter" => Some(1),
        "alter_all" => Some(0),
        _ => None,
    }
}

/// Whether an effect requires the *exclusive* shard lock (deadlocks against
/// any live guard on the same shard).
pub fn effect_requires_exclusive(effect: EffectKind) -> bool {
    matches!(
        effect,
        EffectKind::WriteLock
            | EffectKind::ExclusiveMutation
            | EffectKind::Remove
            | EffectKind::ClearRetain
            | EffectKind::IterWrite
    )
}

/// Whether an effect needs the shared lock only (deadlocks only against a
/// live exclusive guard).
pub fn effect_is_shared(effect: EffectKind) -> bool {
    !effect_requires_exclusive(effect)
}

/// Direct (same-function) conflict rule: given a live guard kind and a direct
/// DashMap operation effect, does an in-place re-entry deadlock?
///
/// Returns the DM code to report for an in-function re-entry.
pub fn direct_conflict(guard: GuardKind, effect: EffectKind) -> Option<DmCode> {
    match guard {
        // Immutable guard (shared lock held). Exclusive ops on the same shard
        // park forever -> DM001. Another shared read is re-entrant-safe.
        GuardKind::Read => {
            if effect_requires_exclusive(effect) {
                Some(DmCode::Dm001)
            } else {
                None
            }
        }
        // Mutable guard (exclusive lock held) -> ANY DashMap op on the same
        // shard deadlocks -> DM002.
        GuardKind::Write => Some(DmCode::Dm002),
        // Iterator holding shared lock -> exclusive ops deadlock -> DM003.
        GuardKind::IterRead => {
            if effect_requires_exclusive(effect) {
                Some(DmCode::Dm003)
            } else {
                None
            }
        }
        // Mutable iterator holding exclusive lock -> ANY op deadlocks -> DM003.
        GuardKind::IterWrite => Some(DmCode::Dm003),
    }
}

/// Transitive (through a helper function) conflict rule. Read/Write guards map
/// to DM004; iterator guards keep DM003 (per the required fixtures).
pub fn transitive_conflict(guard: GuardKind, effect: EffectKind) -> Option<DmCode> {
    match guard {
        GuardKind::Read => {
            if effect_requires_exclusive(effect) {
                Some(DmCode::Dm004)
            } else {
                None
            }
        }
        GuardKind::Write => Some(DmCode::Dm004),
        // Mutable iterator: ANY same-map DashMap op deadlocks (including reads).
        GuardKind::IterWrite => Some(DmCode::Dm003),
        GuardKind::IterRead => {
            if effect_requires_exclusive(effect) {
                Some(DmCode::Dm003)
            } else {
                None
            }
        }
    }
}

/// Direct conflict for IterWrite guards: any DashMap effect on the same map.
pub fn direct_conflict_any(guard: GuardKind, effect: EffectKind) -> Option<DmCode> {
    match guard {
        GuardKind::IterWrite => Some(DmCode::Dm003),
        _ => direct_conflict(guard, effect),
    }
}

/// Transitive conflict: IterWrite guard conflicts with any effect on same map.
pub fn transitive_conflict_any(guard: GuardKind, effect: EffectKind) -> Option<DmCode> {
    match guard {
        GuardKind::IterWrite => Some(DmCode::Dm003),
        _ => transitive_conflict(guard, effect),
    }
}

/// Effect set (deduplicated, ordered) for a map identity.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffectSet {
    pub k: BTreeSet<EffectKind>,
}

impl EffectSet {
    pub fn from_list(list: &[EffectKind]) -> EffectSet {
        EffectSet {
            k: list.iter().copied().collect(),
        }
    }
    pub fn union(&mut self, other: &EffectSet) -> bool {
        let before = self.k.len();
        self.k.extend(other.k.iter().copied());
        self.k.len() != before
    }
    pub fn is_empty(&self) -> bool {
        self.k.is_empty()
    }
    pub fn has_exclusive(&self) -> bool {
        self.k.iter().any(|e| effect_requires_exclusive(*e))
    }
    pub fn iter(&self) -> impl Iterator<Item = EffectKind> + '_ {
        self.k.iter().copied()
    }
    pub fn list(&self) -> Vec<EffectKind> {
        self.k.iter().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_guard_plus_insert_is_dm001() {
        assert_eq!(
            direct_conflict(GuardKind::Read, EffectKind::ExclusiveMutation),
            Some(DmCode::Dm001)
        );
    }

    #[test]
    fn read_guard_plus_get_is_safe() {
        assert_eq!(direct_conflict(GuardKind::Read, EffectKind::ReadLock), None);
    }

    #[test]
    fn write_guard_plus_any_is_dm002() {
        assert_eq!(
            direct_conflict(GuardKind::Write, EffectKind::ReadLock),
            Some(DmCode::Dm002)
        );
        assert_eq!(
            direct_conflict(GuardKind::Write, EffectKind::ExclusiveMutation),
            Some(DmCode::Dm002)
        );
    }

    #[test]
    fn iter_plus_mutation_is_dm003() {
        assert_eq!(
            direct_conflict(GuardKind::IterRead, EffectKind::Remove),
            Some(DmCode::Dm003)
        );
    }

    #[test]
    fn transitive_read_is_dm004() {
        assert_eq!(
            transitive_conflict(GuardKind::Read, EffectKind::ExclusiveMutation),
            Some(DmCode::Dm004)
        );
    }
}
