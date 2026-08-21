//! Intra-crate function index and transitive DashMap-effect propagation.
//!
//! Effects are propagated through the call graph using SCC condensation and
//! bottom-up fixed-point iteration. Non-convergence or resource exhaustion
//! surfaces as TOOL001/TOOL003 diagnostics (fail-closed).

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::ast::CrateIndex;
use crate::diagnostics::Diagnostic;
use crate::effects::{EffectKind, ToolCode};
use crate::identity::{apply_subst, MapId};

/// Safety abort thresholds (not convergence heuristics).
pub const MAX_FUNCTIONS: usize = 50_000;
pub const MAX_MAP_IDENTITIES: usize = 100_000;
pub const MAX_SUMMARY_ENTRIES: usize = 500_000;
pub const MAX_SCC_ROUNDS: usize = 128;
pub const MAX_GLOBAL_ROUNDS: usize = 256;

#[derive(Clone, Copy)]
pub struct BuildLimits {
    pub max_functions: usize,
    pub max_map_identities: usize,
    pub max_summary_entries: usize,
    pub max_scc_rounds: usize,
    pub max_global_rounds: usize,
}

impl Default for BuildLimits {
    fn default() -> Self {
        BuildLimits {
            max_functions: MAX_FUNCTIONS,
            max_map_identities: MAX_MAP_IDENTITIES,
            max_summary_entries: MAX_SUMMARY_ENTRIES,
            max_scc_rounds: MAX_SCC_ROUNDS,
            max_global_rounds: MAX_GLOBAL_ROUNDS,
        }
    }
}

#[derive(Default, Debug, Clone, PartialEq)]
pub struct EffectSummary {
    pub map_effects: HashMap<MapId, crate::effects::EffectSet>,
    pub witness: HashMap<MapId, Vec<String>>,
    pub unresolved_effect: bool,
    pub has_unresolved_call: bool,
    pub unresolved_roots: BTreeSet<String>,
    pub calls: Vec<CallEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallEdge {
    pub callee: usize,
    pub subst: Vec<(String, String)>,
}

impl EffectSummary {
    pub fn exclusive_on(&self, guard_map: &MapId) -> Option<(MapId, EffectKind)> {
        for (map, set) in &self.map_effects {
            if map.exact(guard_map) {
                for e in set.iter() {
                    if crate::effects::effect_requires_exclusive(e) {
                        return Some((map.clone(), e));
                    }
                }
            }
        }
        None
    }

    pub fn exclusive_on_field_only(&self, guard_map: &MapId) -> Option<(MapId, EffectKind)> {
        for (map, set) in &self.map_effects {
            if map.same_field(guard_map) {
                for e in set.iter() {
                    if crate::effects::effect_requires_exclusive(e) {
                        return Some((map.clone(), e));
                    }
                }
            }
        }
        None
    }

    pub fn any_effect_on(&self, guard_map: &MapId) -> bool {
        self.map_effects.keys().any(|m| m.exact(guard_map))
    }
}

#[derive(Default, Debug, Clone)]
pub struct DirectEffects {
    pub map_effects: Vec<(MapId, EffectKind)>,
    pub calls: Vec<CallEdge>,
    pub unresolved_effect: bool,
    pub has_unresolved_call: bool,
    pub unresolved_roots: BTreeSet<String>,
}

pub struct CallGraph {
    pub direct: Vec<DirectEffects>,
    pub summary: Vec<EffectSummary>,
}

pub struct CallGraphBuild {
    pub graph: Option<CallGraph>,
    pub tool_errors: Vec<Diagnostic>,
}

impl CallGraph {
    pub fn build(index: &CrateIndex, direct: Vec<DirectEffects>) -> CallGraphBuild {
        Self::build_with_limits(index, direct, BuildLimits::default())
    }

    pub fn build_with_limits(
        index: &CrateIndex,
        direct: Vec<DirectEffects>,
        limits: BuildLimits,
    ) -> CallGraphBuild {
        let mut tool_errors = Vec::new();
        if index.fns.len() > limits.max_functions {
            tool_errors.push(tool_diag(
                ToolCode::Tool003,
                "",
                1,
                1,
                format!(
                    "resource limit exceeded: {} functions (max {})",
                    index.fns.len(),
                    limits.max_functions
                ),
            ));
            return CallGraphBuild {
                graph: None,
                tool_errors,
            };
        }

        let mut summary: Vec<EffectSummary> = (0..index.fns.len())
            .map(|id| direct_to_summary(&direct[id]))
            .collect();

        let mut identity_count: HashSet<String> = summary
            .iter()
            .flat_map(|s| s.map_effects.keys().map(|m| m.concrete.clone()))
            .collect();

        if identity_count.len() > limits.max_map_identities {
            tool_errors.push(tool_diag(
                ToolCode::Tool003,
                "",
                1,
                1,
                format!(
                    "resource limit exceeded: {} map identities (max {})",
                    identity_count.len(),
                    limits.max_map_identities
                ),
            ));
            return CallGraphBuild {
                graph: None,
                tool_errors,
            };
        }

        let sccs = tarjan_scc(index.fns.len(), |id| {
            summary[id].calls.iter().map(|e| e.callee).collect()
        });

        let condensed_edges = build_condensed_edges(&summary, &sccs);
        let mut condensed_order = condensed_topo(&sccs, &condensed_edges);
        condensed_order.reverse();

        let mut global_round = 0;
        let mut changed_any = true;
        let mut last_growth: Vec<String> = Vec::new();

        while changed_any && global_round < limits.max_global_rounds {
            changed_any = false;
            global_round += 1;

            for component in &condensed_order {
                let members: Vec<usize> = sccs[*component].clone();
                let mut inner_round = 0;
                let mut inner_changed = true;
                while inner_changed && inner_round < limits.max_scc_rounds {
                    inner_changed = false;
                    inner_round += 1;
                    for id in &members {
                        let before = summary[*id].clone();
                        let next = propagate_one(*id, &summary, index);
                        if next != before {
                            inner_changed = true;
                            changed_any = true;
                            for (map, _) in &next.map_effects {
                                if !identity_count.contains(&map.concrete) {
                                    last_growth.push(map.concrete.clone());
                                    identity_count.insert(map.concrete.clone());
                                }
                            }
                            summary[*id] = next;
                        }
                    }
                }
                if inner_changed {
                    let witness = build_nonconvergence_witness(index, &members, &summary);
                    tool_errors.push(tool_diag(
                        ToolCode::Tool001,
                        &witness.file,
                        witness.line,
                        witness.column,
                        format!(
                            "callgraph effects did not converge in SCC after {} rounds (global round {global_round})\n{}",
                            limits.max_scc_rounds,
                            witness.detail
                        ),
                    ));
                    return CallGraphBuild {
                        graph: None,
                        tool_errors,
                    };
                }
            }

            let total_entries: usize = summary.iter().map(|s| s.map_effects.len()).sum();
            if total_entries > limits.max_summary_entries {
                tool_errors.push(tool_diag(
                    ToolCode::Tool003,
                    "",
                    1,
                    1,
                    format!(
                        "resource limit exceeded: {total_entries} summary entries (max {})",
                        limits.max_summary_entries
                    ),
                ));
                return CallGraphBuild {
                    graph: None,
                    tool_errors,
                };
            }
            if identity_count.len() > limits.max_map_identities {
                tool_errors.push(tool_diag(
                    ToolCode::Tool003,
                    "",
                    1,
                    1,
                    format!(
                        "resource limit exceeded: {} map identities (max {})",
                        identity_count.len(),
                        limits.max_map_identities
                    ),
                ));
                return CallGraphBuild {
                    graph: None,
                    tool_errors,
                };
            }
        }

        if changed_any {
            let growth = last_growth.into_iter().take(8).collect::<Vec<_>>().join("\n  ");
            tool_errors.push(tool_diag(
                ToolCode::Tool001,
                "",
                1,
                1,
                format!(
                    "callgraph effects did not converge after {} global rounds\nlast map identities added:\n  {growth}",
                    limits.max_global_rounds
                ),
            ));
            return CallGraphBuild {
                graph: None,
                tool_errors,
            };
        }

        CallGraphBuild {
            graph: Some(CallGraph { direct, summary }),
            tool_errors,
        }
    }

    pub fn summary(&self, fn_id: usize) -> &EffectSummary {
        &self.summary[fn_id]
    }
}

struct NonconvergenceWitness {
    file: String,
    line: usize,
    column: usize,
    detail: String,
}

fn build_nonconvergence_witness(
    index: &CrateIndex,
    members: &[usize],
    summary: &[EffectSummary],
) -> NonconvergenceWitness {
    let mut scc_names: Vec<String> = members
        .iter()
        .map(|id| index.fns[*id].display_name.clone())
        .collect();
    scc_names.sort();
    let mut detail = format!("SCC:\n");
    for n in &scc_names {
        detail.push_str(&format!("  {n}\n"));
    }
    if let Some(&id) = members.first() {
        let meta = &index.fns[id];
        for edge in &summary[id].calls {
            if members.contains(&edge.callee) {
                detail.push_str(&format!(
                    "edge:\n  {} -> {}\n  subst: {:?}\n",
                    meta.display_name, index.fns[edge.callee].display_name, edge.subst
                ));
            }
        }
        return NonconvergenceWitness {
            file: index.files[meta.file_idx].path.clone(),
            line: meta.line,
            column: 1,
            detail,
        };
    }
    NonconvergenceWitness {
        file: String::new(),
        line: 1,
        column: 1,
        detail,
    }
}

fn direct_to_summary(d: &DirectEffects) -> EffectSummary {
    let mut s = EffectSummary {
        unresolved_effect: d.unresolved_effect,
        has_unresolved_call: d.has_unresolved_call,
        unresolved_roots: d.unresolved_roots.clone(),
        calls: d.calls.clone(),
        ..Default::default()
    };
    for (map, effect) in &d.map_effects {
        s.map_effects
            .entry(map.clone())
            .or_default()
            .k
            .insert(*effect);
        s.witness
            .entry(map.clone())
            .or_insert_with(|| vec![effect_name(*effect).to_string()]);
    }
    s
}

fn propagate_one(id: usize, current: &[EffectSummary], index: &CrateIndex) -> EffectSummary {
    let mut next = current[id].clone();
    for edge in current[id].calls.clone() {
        let callee = edge.callee;
        if callee >= current.len() {
            continue;
        }
        let cs = &current[callee];
        for (map, set) in &cs.map_effects {
            let remapped = apply_subst(map, &edge.subst);
            let target = next.map_effects.entry(remapped.clone()).or_default();
            let before = target.k.len();
            target.k.extend(set.iter());
            if let Some(w) = cs.witness.get(map) {
                let mut chain = vec![index.fns[callee].display_name.clone()];
                chain.extend(w.iter().cloned());
                next.witness.entry(remapped).or_insert(chain);
            }
            let _ = before;
        }
        if cs.unresolved_effect {
            next.unresolved_effect = true;
        }
        if cs.has_unresolved_call {
            next.has_unresolved_call = true;
        }
        for root in &cs.unresolved_roots {
            next.unresolved_roots.insert(remap_root(root, &edge.subst));
        }
    }
    next
}

fn remap_root(root: &str, subst: &[(String, String)]) -> String {
    for (from, to) in subst {
        if root == from {
            return to.split('.').next().unwrap_or(to).to_string();
        }
    }
    root.to_string()
}

fn tool_diag(code: ToolCode, file: &str, line: usize, column: usize, message: String) -> Diagnostic {
    Diagnostic::new_tool(code, file, line, column, message)
}

fn tarjan_scc(n: usize, successors: impl Fn(usize) -> Vec<usize>) -> Vec<Vec<usize>> {
    let mut index = 0usize;
    let mut stack = Vec::new();
    let mut on_stack = vec![false; n];
    let mut indices = vec![None; n];
    let mut lowlink = vec![0usize; n];
    let mut sccs = Vec::new();

    fn strongconnect(
        v: usize,
        index: &mut usize,
        stack: &mut Vec<usize>,
        on_stack: &mut [bool],
        indices: &mut [Option<usize>],
        lowlink: &mut [usize],
        sccs: &mut Vec<Vec<usize>>,
        successors: &impl Fn(usize) -> Vec<usize>,
    ) {
        indices[v] = Some(*index);
        lowlink[v] = *index;
        *index += 1;
        stack.push(v);
        on_stack[v] = true;

        for w in successors(v) {
            if indices[w].is_none() {
                strongconnect(
                    w, index, stack, on_stack, indices, lowlink, sccs, successors,
                );
                lowlink[v] = lowlink[v].min(lowlink[w]);
            } else if on_stack[w] {
                lowlink[v] = lowlink[v].min(indices[w].unwrap());
            }
        }

        if lowlink[v] == indices[v].unwrap() {
            let mut component = Vec::new();
            loop {
                let w = stack.pop().unwrap();
                on_stack[w] = false;
                component.push(w);
                if w == v {
                    break;
                }
            }
            sccs.push(component);
        }
    }

    for v in 0..n {
        if indices[v].is_none() {
            strongconnect(
                v,
                &mut index,
                &mut stack,
                &mut on_stack,
                &mut indices,
                &mut lowlink,
                &mut sccs,
                &successors,
            );
        }
    }
    sccs
}

fn condensed_topo(sccs: &[Vec<usize>], edges: &[(usize, usize)]) -> Vec<usize> {
    let n = sccs.len();
    let mut indeg = vec![0usize; n];
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(from, to) in edges {
        if from != to {
            adj[from].push(to);
            indeg[to] += 1;
        }
    }
    let mut queue: Vec<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    let mut order = Vec::new();
    let mut qi = 0;
    while qi < queue.len() {
        let v = queue[qi];
        qi += 1;
        order.push(v);
        for &w in &adj[v] {
            indeg[w] -= 1;
            if indeg[w] == 0 {
                queue.push(w);
            }
        }
    }
    if order.len() != n {
        return (0..n).collect();
    }
    order
}

fn build_condensed_edges(summary: &[EffectSummary], sccs: &[Vec<usize>]) -> Vec<(usize, usize)> {
    let mut node_of = HashMap::new();
    for (i, comp) in sccs.iter().enumerate() {
        for &n in comp {
            node_of.insert(n, i);
        }
    }
    let mut edges = BTreeSet::new();
    for (id, s) in summary.iter().enumerate() {
        let from = node_of.get(&id).copied().unwrap_or(0);
        for edge in &s.calls {
            if let Some(&to) = node_of.get(&edge.callee) {
                if from != to {
                    edges.insert((from, to));
                }
            }
        }
    }
    edges.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::CrateIndex;
    use crate::effects::EffectKind;
    use crate::identity::MapId;

    #[test]
    fn nonconvergence_emits_tool001() {
        let mut index = CrateIndex::new();
        for (id, name) in [(0, "a"), (1, "b")] {
            index.fns.push(crate::ast::FnMetadata {
                id,
                name: name.into(),
                display_name: format!("R::{name}"),
                file_idx: 0,
                line: id + 1,
                is_method: true,
                param_names: vec![],
                has_self: true,
                param_is_map: vec![],
                param_types: vec![],
                impl_owner: Some("R".into()),
            });
        }
        index.files.push(crate::ast::FileInfo {
            path: "callgraph_nonconverge.rs".into(),
            ..Default::default()
        });

        let direct = vec![
            DirectEffects {
                map_effects: vec![(MapId::new("self.seed", "seed"), EffectKind::ReadLock)],
                calls: vec![CallEdge {
                    callee: 1,
                    subst: vec![("self".into(), "self.link".into())],
                }],
                unresolved_effect: false,
                has_unresolved_call: false,
                unresolved_roots: BTreeSet::new(),
            },
            DirectEffects {
                map_effects: vec![(MapId::new("self.tail", "tail"), EffectKind::ReadLock)],
                calls: vec![CallEdge {
                    callee: 0,
                    subst: vec![("self".into(), "self.link".into())],
                }],
                unresolved_effect: false,
                has_unresolved_call: false,
                unresolved_roots: BTreeSet::new(),
            },
        ];

        let limits = BuildLimits {
            max_scc_rounds: 1,
            ..BuildLimits::default()
        };
        let build = CallGraph::build_with_limits(&index, direct, limits);
        assert!(
            build.graph.is_none(),
            "pathological identity growth should not converge"
        );
        assert!(
            build.tool_errors.iter().any(|d| {
                d.code == ToolCode::Tool001.as_str() || d.code == ToolCode::Tool003.as_str()
            }),
            "expected TOOL001 or TOOL003, got {:?}",
            build.tool_errors
        );
    }
}

fn effect_name(e: EffectKind) -> &'static str {
    match e {
        EffectKind::ReadLock => "get/view",
        EffectKind::WriteLock => "get_mut/entry",
        EffectKind::ExclusiveMutation => "insert/alter",
        EffectKind::Remove => "remove",
        EffectKind::ClearRetain => "retain/clear",
        EffectKind::IterRead => "iter",
        EffectKind::IterWrite => "iter_mut",
    }
}
