//! Guard-liveness / scope walker.
//!
//! Walks every function body tracking which DashMap guards are *live* at each
//! point (lexical scope, `drop`, snapshot extraction, scope end), and reports
//! a DM diagnostic whenever an operation or a helper call can re-enter the
//! same map while its guard is still held.

use std::collections::HashMap;

use syn::spanned::Spanned;
use syn::{Block, Expr, Pat, Stmt};

use crate::ast::CrateIndex;
use crate::callgraph::{CallEdge, CallGraph, DirectEffects};
use crate::diagnostics::{Diagnostic, Loc};
use crate::effects::{
    consumed_by_followup, dashmap_closure_arg_index, dashmap_closure_guard, dashmap_spec,
    direct_conflict_any, is_lazy_iterator_adapter, iterator_chain_terminal, produces_guard,
    transitive_conflict_any, DmCode, EffectKind, GuardKind,
};
use crate::identity::{apply_subst, canonicalize, resolve_root, Binding, Bindings, MapId};

#[derive(Debug, Clone)]
pub struct LiveGuard {
    pub map: MapId,
    pub kind: GuardKind,
    pub name: String,
    pub acquired_line: usize,
}

#[derive(Debug, Default)]
pub struct LintOutput {
    pub direct: Vec<DirectEffects>,
    pub diags: Vec<Diagnostic>,
    /// file path -> (start, end) lines of statements/functions (suppression targets).
    pub node_spans: HashMap<String, Vec<(usize, usize)>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    /// Pass 1: record direct effects + calls; report direct DM001/002/003 and DM005.
    RecordAndDirect,
    /// Pass 3: report transitive DM003/004 + DM900 using the propagated graph.
    Transitive,
}

/// Result of walking an expression: whether it ends (still) holding a guard.
#[derive(Default, Debug)]
pub struct Outcome {
    pub guard: Option<(MapId, GuardKind)>,
}

fn is_deferred_invocation(method: &str) -> bool {
    matches!(
        method,
        "spawn"
            | "spawn_blocking"
            | "spawn_local"
            | "spawn_scoped"
            | "spawn_scope"
            | "scope"
            | "block_in_place"
            | "detach"
            | "set_join_handler"
            | "spawn_on"
            | "spawn_on_with"
    )
}

fn is_mem_drop(p: &syn::Path) -> bool {
    let segs: Vec<String> = p.segments.iter().map(|s| s.ident.to_string()).collect();
    matches!(
        segs.iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .as_slice(),
        ["drop"] | ["mem", "drop"] | ["std", "mem", "drop"] | ["core", "mem", "drop"]
    )
}

fn is_known_safe_path(p: &syn::Path) -> bool {
    if let Some(first) = p.segments.first() {
        let f = first.ident.to_string();
        if matches!(
            f.as_str(),
            "std"
                | "core"
                | "alloc"
                | "Arc"
                | "Rc"
                | "Box"
                | "RefCell"
                | "Cell"
                | "Mutex"
                | "RwLock"
                | "OnceLock"
                | "LazyLock"
                | "Once"
                | "SyncLazy"
                | "Vec"
                | "HashMap"
                | "HashSet"
                | "BTreeMap"
                | "BTreeSet"
                | "String"
                | "Path"
                | "PathBuf"
                | "Option"
                | "Result"
                | "Some"
                | "None"
                | "Ok"
                | "Err"
                | "dashmap"
                | "DashMap"
                | "DashSet"
                | "anyhow"
                | "thiserror"
                | "serde_json"
                | "tracing"
                | "log"
                | "regex"
                | "std::sync"
                | "tokio"
        ) {
            return true;
        }
    }
    false
}

pub struct Walker<'a> {
    pub index: &'a CrateIndex,
    pub callgraph: Option<&'a CallGraph>,
    pub mode: Mode,
    pub src_lines: &'a [Vec<String>],
    pub file_paths: &'a [String],
    pub diags: Vec<Diagnostic>,
    pub node_spans: Vec<(usize, usize, usize)>,
    pub cur_file: usize,
    pub cur_fn: usize,
    pub guards: Vec<LiveGuard>,
    pub bindings: Bindings,
    pub cur_direct: DirectEffects,
    pub output_direct: Vec<DirectEffects>,
}

impl<'a> Walker<'a> {
    pub fn new(
        index: &'a CrateIndex,
        callgraph: Option<&'a CallGraph>,
        mode: Mode,
        src_lines: &'a [Vec<String>],
        file_paths: &'a [String],
        n_fns: usize,
    ) -> Walker<'a> {
        Walker {
            index,
            callgraph,
            mode,
            src_lines,
            file_paths,
            diags: Vec::new(),
            node_spans: Vec::new(),
            cur_file: 0,
            cur_fn: 0,
            guards: Vec::new(),
            bindings: Bindings::new(),
            cur_direct: DirectEffects::default(),
            output_direct: vec![DirectEffects::default(); n_fns],
        }
    }

    pub fn run_all(&mut self) {
        let ids: Vec<usize> = (0..self.index.fns.len()).collect();
        for id in ids {
            self.walk_fn(id);
        }
    }

    fn file(&self) -> &str {
        &self.file_paths[self.cur_file]
    }

    fn line_text(&self, file: usize, line: usize) -> String {
        self.src_lines
            .get(file)
            .and_then(|l| l.get(line.wrapping_sub(1)))
            .cloned()
            .unwrap_or_default()
    }

    // ------------------------------------------------------------------
    // function entry
    // ------------------------------------------------------------------

    fn walk_fn(&mut self, fn_id: usize) {
        if fn_id >= self.index.fn_bodies.len() {
            return;
        }
        let meta = &self.index.fns[fn_id];
        let Some(body) = &self.index.fn_bodies[fn_id] else {
            return;
        };
        self.cur_fn = fn_id;
        self.cur_file = meta.file_idx;
        self.guards.clear();
        self.bindings = Bindings::new();
        self.bindings.push_scope();
        for (i, p) in meta.param_names.iter().enumerate() {
            if meta.param_is_map.get(i).copied().unwrap_or(false) {
                self.bindings
                    .bind(p, Binding::Map(MapId::single(p.clone())));
            } else if let Some(ty) = meta.param_types.get(i).and_then(|t| t.clone()) {
                self.bindings.bind(p, Binding::Typed(ty));
            } else {
                self.bindings.bind(p, Binding::Root);
            }
        }
        if meta.has_self {
            if let Some(owner) = &meta.impl_owner {
                self.bindings
                    .bind("self", Binding::Typed(owner.clone()));
            } else {
                self.bindings.bind("self", Binding::Root);
            }
        }
        let bspan = body.span();
        self.node_spans
            .push((self.cur_file, meta.line, bspan.end().line));

        self.walk_block(body);

        if self.mode == Mode::RecordAndDirect {
            self.output_direct[fn_id] = std::mem::take(&mut self.cur_direct);
        }
    }

    // ------------------------------------------------------------------
    // statement / block walking
    // ------------------------------------------------------------------

    fn walk_block(&mut self, block: &Block) {
        self.bindings.push_scope();
        let base = self.guards.len();
        for stmt in &block.stmts {
            let st = stmt.span().start().line;
            let en = stmt.span().end().line;
            if st > 0 {
                self.node_spans.push((self.cur_file, st, en));
            }
            self.walk_stmt(stmt);
        }
        self.bindings.pop_scope();
        self.guards.truncate(base);
    }

    fn walk_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Local(local) => self.stmt_local(local),
            Stmt::Expr(e, _semi) => {
                let _ = self.walk_expr(e);
            }
            Stmt::Macro(m) => {
                if let Ok(e) = syn::parse2::<Expr>(m.mac.tokens.clone()) {
                    let _ = self.walk_expr(&e);
                }
            }
            _ => {}
        }
    }

    fn pattern_idents(pat: &Pat) -> Vec<String> {
        let mut out = Vec::new();
        fn walk(p: &Pat, out: &mut Vec<String>) {
            match p {
                Pat::Ident(id) => out.push(id.ident.to_string()),
                Pat::Tuple(t) => t.elems.iter().for_each(|e| walk(e, out)),
                Pat::TupleStruct(ts) => ts.elems.iter().for_each(|e| walk(e, out)),
                Pat::Struct(ps) => ps.fields.iter().for_each(|f| walk(&f.pat, out)),
                Pat::Slice(s) => s.elems.iter().for_each(|e| walk(e, out)),
                Pat::Or(o) => o.cases.iter().for_each(|c| walk(c, out)),
                Pat::Paren(p) => walk(&p.pat, out),
                Pat::Reference(r) => walk(&r.pat, out),
                Pat::Type(pt) => walk(&pt.pat, out),
                _ => {}
            }
        }
        walk(pat, &mut out);
        out
    }

    fn is_wildcard_pat(pat: &Pat) -> bool {
        matches!(pat, Pat::Wild(_))
    }

    fn bind_let_guard(
        &mut self,
        map: MapId,
        kind: GuardKind,
        names: Vec<String>,
        acquired_line: usize,
    ) {
        for n in &names {
            self.bindings.bind(n, Binding::Other);
        }
        if names.is_empty() {
            return; // `_` pattern: value dropped at end of the let statement
        }
        let name = names[0].clone();
        self.guards.push(LiveGuard {
            map,
            kind,
            name,
            acquired_line,
        });
    }

    fn drop_guard(&mut self, expr: &Expr) {
        if let Expr::Path(p) = expr {
            if p.path.segments.len() == 1 {
                let name = p.path.segments[0].ident.to_string();
                self.guards.retain(|g| g.name != name);
            }
        }
    }

    fn is_dashmap_constructor(&self, expr: &Expr, map_name: &str) -> Option<MapId> {
        fn find(expr: &Expr) -> bool {
            match expr {
                Expr::Call(c) => {
                    if let Expr::Path(p) = &*c.func {
                        let last = p
                            .path
                            .segments
                            .last()
                            .map(|s| s.ident.to_string())
                            .unwrap_or_default();
                        let has_dashmap = p
                            .path
                            .segments
                            .iter()
                            .any(|s| s.ident == "DashMap" || s.ident == "DashSet");
                        if has_dashmap
                            && matches!(
                                last.as_str(),
                                "new"
                                    | "with_shard_amount"
                                    | "with_capacity"
                                    | "with_capacity_and_shard_amount"
                                    | "with_hasher"
                                    | "with_capacity_and_hasher"
                                    | "with_hasher_and_shard_amount"
                                    | "with_capacity_and_hasher_and_shard_amount"
                                    | "default"
                            )
                        {
                            return true;
                        }
                    }
                    find(&c.func) || c.args.iter().any(find)
                }
                Expr::Path(p) => p
                    .path
                    .segments
                    .iter()
                    .any(|s| s.ident == "DashMap" || s.ident == "DashSet"),
                Expr::Reference(r) => find(&r.expr),
                Expr::Paren(p) => find(&p.expr),
                _ => false,
            }
        }
        if find(expr) {
            Some(MapId::single(map_name))
        } else {
            None
        }
    }

    fn stmt_local(&mut self, local: &syn::Local) {
        let Some(init) = &local.init else { return };
        let expr: &Expr = &init.expr;
        let div_opt = &init.diverge;
        let acquired_line = expr.span().start().line;

        // explicit drop() / std::mem::drop()
        if let Expr::Call(c) = expr {
            if let Expr::Path(p) = &*c.func {
                if is_mem_drop(&p.path) {
                    if let Some(arg) = c.args.first() {
                        self.drop_guard(arg);
                    }
                    return;
                }
            }
        }

        let wildcard = Self::is_wildcard_pat(&local.pat);
        let pat_idents = Self::pattern_idents(&local.pat);

        if let Some((map, gk)) = if wildcard {
            let outcome = self.walk_expr(expr);
            let _ = outcome.guard;
            None
        } else if let Some((map, gk)) = self.expr_is_iter_collect(expr) {
            Some((map, gk))
        } else {
            self.walk_expr(expr).guard
        } {
            if is_direct_iter_let(expr, gk)
                || self.expr_is_iter_collect(expr).is_some()
                || !matches!(gk, GuardKind::IterRead | GuardKind::IterWrite)
            {
                self.bind_let_guard(map.clone(), gk, pat_idents.clone(), acquired_line);
                let _ = div_opt;
                return;
            }
        }

        // Not a guard binding: classify the binding for alias canonicalization.
        let Some(name) = pat_idents.first().cloned() else {
            return;
        };
        if let Some(id) = canonicalize(expr, &self.bindings, &self.index.statics) {
            self.bindings.bind(&name, Binding::Map(id));
        } else if let Some(id) = self.is_dashmap_constructor(expr, &name) {
            self.bindings.bind(&name, Binding::Map(id));
        } else {
            self.bindings.bind(&name, Binding::Root);
        }
        // Rebinding a root object names a NEW object (shadowing) — any live
        // guard on a field of the old object (same spelling) can no longer be
        // reached or deadlock against the new one.
        self.rebind_drop_guards(&name);
        for n in pat_idents.iter().skip(1) {
            self.bindings.bind(n, Binding::Other);
        }
        let _ = div_opt;
    }

    fn expr_is_iter_collect(&self, expr: &Expr) -> Option<(MapId, GuardKind)> {
        if self.chain_snapshots_values(expr) {
            return None;
        }
        fn inner(
            e: &Expr,
            bindings: &Bindings,
            statics: &crate::identity::StaticMaps,
        ) -> Option<(MapId, GuardKind)> {
            match e {
                Expr::MethodCall(mc) if mc.method == "collect" => {
                    inner(&mc.receiver, bindings, statics)
                }
                Expr::MethodCall(mc)
                    if is_lazy_iterator_adapter(&mc.method.to_string()) =>
                {
                    inner(&mc.receiver, bindings, statics)
                }
                Expr::MethodCall(mc) if mc.method == "iter" => canonicalize(
                    &mc.receiver,
                    bindings,
                    statics,
                )
                .map(|m| (m, GuardKind::IterRead)),
                Expr::MethodCall(mc) if mc.method == "iter_mut" => canonicalize(
                    &mc.receiver,
                    bindings,
                    statics,
                )
                .map(|m| (m, GuardKind::IterWrite)),
                Expr::Reference(r) => inner(&r.expr, bindings, statics),
                Expr::Paren(p) => inner(&p.expr, bindings, statics),
                _ => None,
            }
        }
        inner(expr, &self.bindings, &self.index.statics)
    }

    fn chain_snapshots_values(&self, expr: &Expr) -> bool {
        match expr {
            Expr::MethodCall(mc) if mc.method == "collect" => {
                self.chain_snapshots_values(&mc.receiver)
            }
            Expr::MethodCall(mc)
                if matches!(
                    mc.method.to_string().as_str(),
                    "map" | "cloned" | "copied" | "filter_map" | "flat_map"
                ) =>
            {
                true
            }
            Expr::MethodCall(mc) if is_lazy_iterator_adapter(&mc.method.to_string()) => {
                self.chain_snapshots_values(&mc.receiver)
            }
            Expr::Reference(r) => self.chain_snapshots_values(&r.expr),
            Expr::Paren(p) => self.chain_snapshots_values(&p.expr),
            _ => false,
        }
    }

    fn rebind_drop_guards(&mut self, name: &str) {
        self.guards.retain(|g| walk_root_of(&g.map) != name);
    }

    // ------------------------------------------------------------------
    // expression walking
    // ------------------------------------------------------------------

    /// True when `recv` is a receiver that can be a DashMap: a field access
    /// whose field is declared as a DashMap/DashSet somewhere (world.tiles,
    /// self.vars are NOT — vars is a Vec), a bound map alias, or a static
    /// DashMap. This is what keeps `state.vars.get_mut` (a Vec op) from being
    /// misread as a DashMap guard.
    fn is_map_op_receiver(&self, recv: &Expr) -> bool {
        match recv {
            Expr::Field(f) => match &f.member {
                syn::Member::Named(m) => self.index.map_fields.contains(&m.to_string()),
                syn::Member::Unnamed(_) => false,
            },
            Expr::Path(p) if p.path.segments.len() == 1 => {
                let n = p.path.segments[0].ident.to_string();
                matches!(self.bindings.lookup(&n), Some(Binding::Map(_)))
                    || self.index.statics.contains(&n)
            }
            Expr::Reference(r) => self.is_map_op_receiver(&r.expr),
            Expr::Paren(p) => self.is_map_op_receiver(&p.expr),
            Expr::Unary(u) => {
                matches!(u.op, syn::UnOp::Deref(_)) && self.is_map_op_receiver(&u.expr)
            }
            _ => false,
        }
    }

    fn walk_expr(&mut self, expr: &Expr) -> Outcome {
        use syn::Expr::*;
        match expr {
            MethodCall(mc) => {
                let method = mc.method.to_string();
                if consumed_by_followup(GuardKind::IterRead, &method)
                    || consumed_by_followup(GuardKind::IterWrite, &method)
                {
                    let moved = collect_moved_idents(&mc.receiver);
                    if !moved.is_empty() {
                        self.guards.retain(|g| !moved.contains(&g.name));
                    }
                }
                let inner = self.walk_expr(&mc.receiver);

                if let Some((map, gk)) = inner.guard {
                    if matches!(gk, GuardKind::IterRead | GuardKind::IterWrite)
                        && iterator_chain_terminal(&method)
                    {
                        let guard = LiveGuard {
                            map: map.clone(),
                            kind: gk,
                            name: "<temp>".to_string(),
                            acquired_line: mc.receiver.span().start().line,
                        };
                        self.walk_each_arg_with_guard(&mc.args, guard);
                        return Outcome { guard: None };
                    }
                    // post-fix on a guard-producing call
                    if matches!(gk, GuardKind::IterRead | GuardKind::IterWrite)
                        && is_lazy_iterator_adapter(&method)
                    {
                        // iterator stays alive; closures run while shard locked
                        let guard = LiveGuard {
                            map: map.clone(),
                            kind: gk,
                            name: "<iter>".to_string(),
                            acquired_line: mc.receiver.span().start().line,
                        };
                        self.walk_each_arg_with_guard(&mc.args, guard);
                        return Outcome {
                            guard: Some((map, gk)),
                        };
                    }
                    if consumed_by_followup(gk, &method) {
                        // guard dies after this expression, but sync closures
                        // still run while it is held.
                        let guard = LiveGuard {
                            map: map.clone(),
                            kind: gk,
                            name: "<temp>".to_string(),
                            acquired_line: mc.receiver.span().start().line,
                        };
                        self.walk_each_arg_with_guard(&mc.args, guard);
                        return Outcome { guard: None };
                    }
                    // preserving post-fix: unwrap/expect/key/value/and_modify/
                    // or_insert on Entry (returns RefMut = still Write guard)
                    self.walk_plain_args(&mc.args);
                    return Outcome {
                        guard: Some((map, gk)),
                    };
                }

                // Is `mc.receiver` a DashMap (a known DashMap-typed field, a bound
                // map alias, or a static DashMap)?
                if self.is_map_op_receiver(&mc.receiver) {
                    if let Some(map) =
                        canonicalize(&mc.receiver, &self.bindings, &self.index.statics)
                    {
                        if let Some(spec) = dashmap_spec(&method) {
                            let op_loc = Loc {
                                file: self.file().to_string(),
                                line: mc.span().start().line,
                                column: mc.span().start().column + 1,
                            };
                            for effect in spec.effects.iter().copied() {
                                self.record_effect(map.clone(), effect);
                                self.check_direct_conflict(&map, effect, op_loc.clone());
                            }
                            // closure forms (view/retain/remove_if/alter/alter_all)
                            if let Some(gk) = dashmap_closure_guard(&method) {
                                if let Some(ci) = dashmap_closure_arg_index(&method) {
                                    let guard = LiveGuard {
                                        map: map.clone(),
                                        kind: gk,
                                        name: "<closure-lock>".to_string(),
                                        acquired_line: mc.span().start().line,
                                    };
                                    self.walk_arg_with_guard(&mc.args, ci, guard);
                                }
                                self.walk_plain_args(&mc.args);
                                return Outcome { guard: None };
                            }
                            if let Some(gk) = produces_guard(&method) {
                                return Outcome {
                                    guard: Some((map, gk)),
                                };
                            }
                            self.walk_plain_args(&mc.args);
                            return Outcome { guard: None };
                        }
                        // Known DashMap receiver but a non-DashMap method (e.g.
                        // `tiles.clone()`): no guard/effect semantics we model.
                        self.walk_plain_args(&mc.args);
                        return Outcome { guard: None };
                    }
                    // A known DashMap field whose root object we could not
                    // canonicalize: record an unresolved effect (DM900 driver).
                    self.mark_unresolved_effect();
                    self.walk_plain_args(&mc.args);
                    return Outcome { guard: None };
                }
                // Not a DashMap op: method on a root object or unknown type.
                self.handle_non_map_method(mc, &method);
                Outcome { guard: None }
            }
            Call(c) => {
                if let Expr::Path(p) = &*c.func {
                    if is_mem_drop(&p.path) {
                        if let Some(arg) = c.args.first() {
                            self.drop_guard(arg);
                            self.walk_plain_args(&c.args);
                        }
                        return Outcome { guard: None };
                    }
                }
                self.handle_call(c);
                Outcome { guard: None }
            }
            Try(t) => self.walk_expr(&t.expr),
            Await(a) => {
                let line = a.span().start().line;
                let column = a.span().start().column + 1;
                if self.mode == Mode::RecordAndDirect && !self.guards.is_empty() {
                    let snapshot = self.guards.clone();
                    let op_loc = Loc {
                        file: self.file().to_string(),
                        line,
                        column,
                    };
                    for g in &snapshot {
                        self.push_diag(
                            DmCode::Dm005,
                            line,
                            column,
                            format!(
                                "DashMap guard `{}` on `{}` survives across `.await`",
                                g.name, g.map.concrete
                            ),
                            g,
                            Some(op_loc.clone()),
                            Vec::new(),
                        );
                    }
                }
                let _ = self.walk_expr(&a.base);
                Outcome { guard: None }
            }
            If(i) => {
                self.bindings.push_scope();
                let then_guard = self.walk_let_condition(&i.cond);
                if let Some(g) = &then_guard {
                    self.guards.push(g.clone());
                }
                self.walk_block(&i.then_branch);
                if then_guard.is_some() {
                    self.guards.pop();
                }
                self.bindings.pop_scope();
                if let Some((_, els)) = &i.else_branch {
                    self.bindings.push_scope();
                    self.walk_expr(els);
                    self.bindings.pop_scope();
                }
                Outcome { guard: None }
            }
            Match(m) => {
                let cond_outcome = self.walk_expr(&m.expr);
                let cond_guard = cond_outcome.guard;
                let cond_line = m.expr.span().start().line;
                for arm in &m.arms {
                    self.bindings.push_scope();
                    let binds = Self::pattern_idents(&arm.pat);
                    let base = self.guards.len();
                    if let Some((map, gk)) = &cond_guard {
                        if !binds.is_empty() && !Self::is_wildcard_pat(&arm.pat) {
                            self.guards.push(LiveGuard {
                                map: map.clone(),
                                kind: *gk,
                                name: binds[0].clone(),
                                acquired_line: cond_line,
                            });
                            for b in &binds {
                                self.bindings.bind(b, Binding::Other);
                            }
                        }
                    }
                    if let Some((_, g)) = &arm.guard {
                        let _ = self.walk_expr(g);
                    }
                    self.walk_expr(&arm.body);
                    self.guards.truncate(base);
                    self.bindings.pop_scope();
                }
                Outcome { guard: None }
            }
            While(w) => {
                self.bindings.push_scope();
                let loop_guard = self.walk_let_condition(&w.cond);
                if let Some(g) = &loop_guard {
                    self.guards.push(g.clone());
                }
                self.walk_block(&w.body);
                if loop_guard.is_some() {
                    self.guards.pop();
                }
                self.bindings.pop_scope();
                Outcome { guard: None }
            }
            ForLoop(f) => {
                self.bindings.push_scope();
                let inner = self.walk_expr(&f.expr);
                let pushed = inner.guard.is_some();
                if let Some((map, gk)) = inner.guard {
                    let guard = LiveGuard {
                        map: map.clone(),
                        kind: gk,
                        name: "<loop-iter>".to_string(),
                        acquired_line: f.expr.span().start().line,
                    };
                    self.guards.push(guard);
                }
                for b in Self::pattern_idents(&f.pat) {
                    self.bindings.bind(&b, Binding::Other);
                }
                self.walk_block(&f.body);
                if pushed {
                    self.guards.pop();
                }
                self.bindings.pop_scope();
                Outcome { guard: None }
            }
            Loop(l) => {
                self.walk_block(&l.body);
                Outcome { guard: None }
            }
            Block(b) => {
                self.walk_block(&b.block);
                Outcome { guard: None }
            }
            Closure(c) => {
                // synchronous closure: run with current live guards
                let _ = self.walk_expr(&c.body);
                Outcome { guard: None }
            }
            Async(a) => {
                self.walk_block(&a.block);
                Outcome { guard: None }
            }
            Reference(r) => self.walk_expr(&r.expr),
            Paren(p) => self.walk_expr(&p.expr),
            Unary(u) => {
                if matches!(u.op, syn::UnOp::Deref(_)) {
                    let _ = self.walk_expr(&u.expr);
                }
                Outcome { guard: None }
            }
            Field(f) => {
                let _ = self.walk_expr(&f.base);
                Outcome { guard: None }
            }
            Index(i) => {
                let _ = self.walk_expr(&i.expr);
                let _ = self.walk_expr(&i.index);
                Outcome { guard: None }
            }
            Binary(b) => {
                let _ = self.walk_expr(&b.left);
                let _ = self.walk_expr(&b.right);
                Outcome { guard: None }
            }
            Assign(a) => {
                let _ = self.walk_expr(&a.left);
                let _ = self.walk_expr(&a.right);
                Outcome { guard: None }
            }
            Tuple(t) => {
                for e in &t.elems {
                    let _ = self.walk_expr(e);
                }
                Outcome { guard: None }
            }
            Array(a) => {
                for e in &a.elems {
                    let _ = self.walk_expr(e);
                }
                Outcome { guard: None }
            }
            Macro(m) => {
                if let Ok(e) = syn::parse2::<Expr>(m.mac.tokens.clone()) {
                    let _ = self.walk_expr(&e);
                }
                Outcome { guard: None }
            }
            _ => Outcome { guard: None },
        }
    }

    /// `if let` / `while let` condition: return the guard bound by the pattern
    /// (live during the body). Consumed/`_` patterns release the guard.
    fn walk_let_condition(&mut self, cond: &Expr) -> Option<LiveGuard> {
        if let Expr::Let(l) = cond {
            let outcome = self.walk_expr(&l.expr);
            let line = l.expr.span().start().line;
            if let Some((map, gk)) = outcome.guard {
                let binds = Self::pattern_idents(&l.pat);
                if binds.is_empty() || Self::is_wildcard_pat(&l.pat) {
                    return None;
                }
                for b in &binds {
                    self.bindings.bind(b, Binding::Other);
                }
                return Some(LiveGuard {
                    map: map.clone(),
                    kind: gk,
                    name: binds[0].clone(),
                    acquired_line: line,
                });
            }
        } else {
            let _ = self.walk_expr(cond);
        }
        None
    }

    // ------------------------------------------------------------------
    // DashMap closing methods (view/retain/...), direct conflicts
    // ------------------------------------------------------------------

    fn record_effect(&mut self, map: MapId, effect: EffectKind) {
        if self.mode != Mode::RecordAndDirect {
            return;
        }
        if !self
            .cur_direct
            .map_effects
            .iter()
            .any(|(m, e)| m == &map && e == &effect)
        {
            self.cur_direct.map_effects.push((map, effect));
        }
    }

    fn record_call_edge(&mut self, callee: usize, subst: Vec<(String, String)>) {
        if self.mode != Mode::RecordAndDirect {
            return;
        }
        let edge = CallEdge { callee, subst };
        if !self.cur_direct.calls.contains(&edge) {
            self.cur_direct.calls.push(edge);
        }
    }

    fn subst_for_call(
        &self,
        callee: usize,
        receiver: Option<&Expr>,
        args: &[&Expr],
    ) -> Vec<(String, String)> {
        let meta = &self.index.fns[callee];
        let mut subst = Vec::new();
        if meta.has_self {
            if let Some(recv) = receiver {
                if let Some(map) = canonicalize(recv, &self.bindings, &self.index.statics) {
                    subst.push(("self".to_string(), map.concrete));
                } else if let Some(root) = resolve_root(recv, &self.bindings, &self.index.statics) {
                    subst.push(("self".to_string(), root));
                }
            }
        }
        for (i, pname) in meta.param_names.iter().enumerate() {
            let Some(arg) = args.get(i) else { continue };
            if let Some(map) = canonicalize(arg, &self.bindings, &self.index.statics) {
                subst.push((pname.clone(), map.concrete));
            } else if let Some(root) = resolve_root(arg, &self.bindings, &self.index.statics) {
                subst.push((pname.clone(), root));
            }
        }
        subst
    }

    fn mark_unresolved_effect(&mut self) {
        if self.mode == Mode::RecordAndDirect {
            self.cur_direct.unresolved_effect = true;
        }
    }

    fn caller_impl_owner(&self) -> Option<&str> {
        self.index.fns[self.cur_fn].impl_owner.as_deref()
    }

    fn resolve_method_call(&self, receiver: &Expr, method: &str) -> Option<usize> {
        let receiver_is_self = match receiver {
            Expr::Path(p) => p.path.segments.len() == 1 && p.path.segments[0].ident == "self",
            _ => false,
        };
        if receiver_is_self {
            if let Some(owner) = self.caller_impl_owner() {
                return self.index.resolve_method_for_type(method, owner);
            }
        }
        if let Some(ty) = self.index.resolve_receiver_type(
            receiver,
            &self.bindings,
            self.caller_impl_owner(),
        ) {
            if let Some(id) = self.index.resolve_method_for_type(method, &ty) {
                return Some(id);
            }
        }
        None
    }

    /// Only an explicitly recognized standard/container type is exempt from
    /// unresolved-call uncertainty. Method names are never sufficient.
    fn is_known_external_receiver(&self, receiver: &Expr) -> bool {
        let Some(ty) = self.index.resolve_receiver_type(
            receiver,
            &self.bindings,
            self.caller_impl_owner(),
        ) else {
            return false;
        };
        ty.starts_with("std::")
            || ty.starts_with("core::")
            || ty.starts_with("alloc::")
            || matches!(
                ty.as_str(),
                "Arc"
                    | "Rc"
                    | "Box"
                    | "Vec"
                    | "HashMap"
                    | "HashSet"
                    | "BTreeMap"
                    | "BTreeSet"
                    | "String"
                    | "Path"
                    | "PathBuf"
                    | "Option"
                    | "Result"
            )
    }

    fn mark_unresolved_call_with_roots(&mut self, roots: &[String]) {
        if self.mode == Mode::RecordAndDirect {
            self.cur_direct.has_unresolved_call = true;
            for r in roots {
                self.cur_direct.unresolved_roots.insert(r.clone());
            }
        }
    }

    fn check_direct_conflict(&mut self, map: &MapId, effect: EffectKind, op_loc: Loc) {
        if self.mode != Mode::RecordAndDirect {
            return;
        }
        let hits: Vec<(usize, DmCode)> = self
            .guards
            .iter()
            .enumerate()
            .filter_map(|(i, g)| {
                if g.map.exact(map) {
                    direct_conflict_any(g.kind, effect).map(|code| (i, code))
                } else {
                    None
                }
            })
            .collect();
        for (i, code) in hits {
            let g = self.guards[i].clone();
            let msg = format!(
                "live DashMap guard may re-enter `{}` ({} while holding a {} guard)",
                map.concrete,
                effect_desc(effect),
                guard_kind_desc(g.kind)
            );
            self.push_diag(
                code,
                op_loc.line,
                op_loc.column,
                msg,
                &g,
                Some(op_loc.clone()),
                Vec::new(),
            );
        }
    }

    // ------------------------------------------------------------------
    // calls and transitive checks
    // ------------------------------------------------------------------

    fn handle_non_map_method(&mut self, mc: &syn::ExprMethodCall, method: &str) {
        let receiver_root = resolve_root(&mc.receiver, &self.bindings, &self.index.statics);
        let op_loc = Loc {
            file: self.file().to_string(),
            line: mc.span().start().line,
            column: mc.span().start().column + 1,
        };
        if let Some(callee) = self.resolve_method_call(&mc.receiver, method) {
            let args: Vec<&Expr> = mc.args.iter().collect();
            let subst = self.subst_for_call(callee, Some(&mc.receiver), &args);
            self.record_call_edge(callee, subst.clone());
            let name = self.index.fns[callee].display_name.clone();
            self.check_transitive_callee(callee, &name, &op_loc, &subst);
            self.walk_plain_args(&mc.args);
            return;
        }

        if is_deferred_invocation(method) {
            let saved = std::mem::take(&mut self.guards);
            self.walk_plain_args(&mc.args);
            self.guards = saved;
            return;
        }
        let mut roots = Vec::new();
        if let Some(r) = &receiver_root {
            roots.push(r.clone());
        }
        for a in &mc.args {
            if let Some(r) = resolve_root(a, &self.bindings, &self.index.statics) {
                roots.push(r);
            }
        }
        self.mark_unresolved_call_with_roots(&roots);
        if self.mode == Mode::Transitive && !self.is_known_external_receiver(&mc.receiver) {
            self.check_unresolved_call(roots, &op_loc);
        }
        self.walk_plain_args(&mc.args);
    }

    fn handle_call(&mut self, c: &syn::ExprCall) {
        let func = &c.func;
        let op_loc = Loc {
            file: self.file().to_string(),
            line: c.span().start().line,
            column: c.span().start().column + 1,
        };
        let args: Vec<&Expr> = c.args.iter().collect();
        if let Expr::Path(p) = &**func {
            if is_known_safe_path(&p.path) {
                self.walk_plain_args(&c.args);
                return;
            }
            let leaf = p
                .path
                .segments
                .last()
                .map(|s| s.ident.to_string())
                .unwrap_or_default();
            if is_deferred_invocation(&leaf) {
                let saved = std::mem::take(&mut self.guards);
                self.walk_plain_args(&c.args);
                self.guards = saved;
                return;
            }
            if let Some(callee) =
                self.index
                    .resolve_call(func, self.cur_file, self.caller_impl_owner())
            {
                let subst = self.subst_for_call(callee, None, &args);
                self.record_call_edge(callee, subst.clone());
                let name = self.index.fns[callee].display_name.clone();
                self.check_transitive_callee(callee, &name, &op_loc, &subst);
                self.walk_plain_args(&c.args);
                return;
            }
            let roots: Vec<String> = args
                .iter()
                .filter_map(|a| resolve_root(a, &self.bindings, &self.index.statics))
                .collect();
            self.mark_unresolved_call_with_roots(&roots);
            if self.mode == Mode::Transitive && !is_known_safe_path(&p.path) {
                self.check_unresolved_call(roots, &op_loc);
            }
            self.walk_plain_args(&c.args);
            return;
        }
        let _ = self.walk_expr(func);
        self.walk_plain_args(&c.args);
    }

    fn check_transitive_callee(
        &mut self,
        callee: usize,
        name: &str,
        op_loc: &Loc,
        subst: &[(String, String)],
    ) {
        if self.mode != Mode::Transitive {
            return;
        }
        let Some(cg) = self.callgraph else { return };
        let summary = cg.summary(callee);
        let guard_snapshot = self.guards.clone();
        for g in &guard_snapshot {
            let mut conflict: Option<(MapId, EffectKind, DmCode)> = None;
            let mut maps: Vec<_> = summary.map_effects.iter().collect();
            maps.sort_by(|(a, _), (b, _)| a.concrete.cmp(&b.concrete));
            for (map, set) in maps {
                let remapped = apply_subst(map, subst);
                if !remapped.exact(&g.map) {
                    continue;
                }
                for e in set.iter() {
                    let relevant = match g.kind {
                        GuardKind::IterWrite | GuardKind::Write => true,
                        GuardKind::Read | GuardKind::IterRead => {
                            crate::effects::effect_requires_exclusive(e)
                        }
                    };
                    if !relevant {
                        continue;
                    }
                    if let Some(code) = transitive_conflict_any(g.kind, e) {
                        conflict = Some((remapped.clone(), e, code));
                        break;
                    }
                }
                if conflict.is_some() {
                    break;
                }
            }
            if let Some((eff_map, effect, code)) = conflict {
                let mut path = vec![name.to_string()];
                // Witness is stored under the callee-side identity.
                if let Some(orig) = summary
                    .map_effects
                    .keys()
                    .find(|m| apply_subst(m, subst).exact(&eff_map))
                {
                    if let Some(w) = summary.witness.get(orig) {
                        if !w.is_empty() {
                            path.extend(w[..w.len() - 1].iter().cloned());
                        }
                    }
                }
                path.push(format!("{}.{}", eff_map.concrete, effect_op(effect)));
                let msg = format!(
                    "live DashMap guard may be re-entered transitively via `{name}` on `{}`",
                    eff_map.concrete
                );
                self.push_diag(
                    code,
                    op_loc.line,
                    op_loc.column,
                    msg,
                    g,
                    Some(op_loc.clone()),
                    path,
                );
            } else if summary.has_unresolved_call {
                let guard_root = walk_root_of(&g.map);
                let relevant = summary.unresolved_roots.contains(&guard_root)
                    || summary
                        .unresolved_roots
                        .iter()
                        .any(|r| g.map.concrete.starts_with(&format!("{r}.")));
                if relevant {
                    let msg = format!(
                        "could not prove whether `{name}` re-enters `{}` (callee has an unresolved call that may access the same root)",
                        g.map.concrete
                    );
                    self.push_diag(
                        DmCode::Dm900,
                        op_loc.line,
                        op_loc.column,
                        msg,
                        g,
                        None,
                        vec![
                            name.to_string(),
                            "<unresolved callee>".to_string(),
                        ],
                    );
                }
            } else if summary.unresolved_effect && !g.map.is_root_map() {
                let msg = format!(
                    "could not prove whether `{name}` re-enters `{}` (it performs an unidentifiable DashMap operation)",
                    g.map.concrete
                );
                self.push_diag(
                    DmCode::Dm900,
                    op_loc.line,
                    op_loc.column,
                    msg,
                    g,
                    None,
                    vec![
                        name.to_string(),
                        "<unidentified dashmap effect>".to_string(),
                    ],
                );
            } else if let Some((eff_map, effect)) = summary.exclusive_on_field_only(&g.map) {
                let remapped = apply_subst(&eff_map, subst);
                if remapped.exact(&g.map) {
                    continue;
                }
                let msg = format!(
                    "could not prove whether `{name}` re-enters `{0}` (matches guard map `{1}` only by field `{2}`; cannot prove they are the same DashMap)",
                    remapped.concrete, g.map.concrete, g.map.field
                );
                self.push_diag(
                    DmCode::Dm900,
                    op_loc.line,
                    op_loc.column,
                    msg,
                    g,
                    None,
                    vec![
                        name.to_string(),
                        format!("{}.{}", remapped.concrete, effect_op(effect)),
                    ],
                );
            }
        }
    }

    /// DM900 for a call we could not resolve, when the receiver/args carry a
    /// root object that owns a live guard's map. `roots` lists the canonical
    /// root-object paths seen as receiver/arguments.
    fn check_unresolved_call(&mut self, roots: Vec<String>, op_loc: &Loc) {
        if self.mode != Mode::Transitive {
            return;
        }
        let mut seen = std::collections::BTreeSet::new();
        let snapshot = self.guards.clone();
        for g in &snapshot {
            let root = walk_root_of(&g.map);
            if root.is_empty() {
                continue;
            }
            if roots.contains(&root) && seen.insert((root.clone(), g.map.concrete.clone())) {
                let msg = format!(
                    "could not prove whether the unresolved call re-enters `{}` (root object `{root}` could own the map)",
                    g.map.concrete
                );
                self.push_diag(
                    DmCode::Dm900,
                    op_loc.line,
                    op_loc.column,
                    msg,
                    g,
                    None,
                    vec![
                        "<unresolved callee>".to_string(),
                        format!("re-entry of `{root}.*`"),
                    ],
                );
            }
        }
    }

    // ------------------------------------------------------------------
    // diagnostics + arg walking helpers
    // ------------------------------------------------------------------

    fn push_diag(
        &mut self,
        code: DmCode,
        line: usize,
        column: usize,
        message: String,
        guard: &LiveGuard,
        conflict: Option<Loc>,
        effect_path: Vec<String>,
    ) {
        let hint = match code {
            DmCode::Dm001 | DmCode::Dm002 | DmCode::Dm003 | DmCode::Dm004 => {
                Some("copy/clone required data, release/drop the guard, then mutate".to_string())
            }
            DmCode::Dm005 => Some("a DashMap guard must not survive across .await".to_string()),
            _ => None,
        };
        let mut d = Diagnostic::new(code, self.file(), line, column, message);
        let guard_name = guard.name.clone();
        d.guard = Some(guard_name);
        d.acquired = Some(self.loc(self.cur_file, guard.acquired_line, 1));
        d.acquired_src = Some(self.line_text(self.cur_file, guard.acquired_line));
        if let Some(c) = &conflict {
            d.conflict = Some(c.clone());
            d.conflict_src = Some(self.line_text(self.find_file(c), c.line));
        }
        d.effect_path = effect_path;
        if let Some(h) = hint {
            d.hint = Some(h);
        }
        self.diags.push(d);
    }

    fn find_file(&self, target: &Loc) -> usize {
        self.file_paths
            .iter()
            .position(|p| p == &target.file)
            .unwrap_or(self.cur_file)
    }

    fn loc(&self, file: usize, line: usize, column: usize) -> Loc {
        Loc {
            file: self.file_paths[file].to_string(),
            line,
            column,
        }
    }

    fn walk_plain_args(&mut self, args: &syn::punctuated::Punctuated<Expr, syn::token::Comma>) {
        for a in args {
            let _ = self.walk_expr(a);
        }
    }

    fn walk_each_arg_with_guard(
        &mut self,
        args: &syn::punctuated::Punctuated<Expr, syn::token::Comma>,
        guard: LiveGuard,
    ) {
        for a in args {
            self.guards.push(guard.clone());
            let _ = self.walk_expr(a);
            self.guards.pop();
        }
    }

    fn walk_arg_with_guard(
        &mut self,
        args: &syn::punctuated::Punctuated<Expr, syn::token::Comma>,
        index: usize,
        guard: LiveGuard,
    ) {
        for (i, a) in args.iter().enumerate() {
            if i == index {
                self.guards.push(guard.clone());
                let _ = self.walk_expr(a);
                self.guards.pop();
            } else {
                let _ = self.walk_expr(a);
            }
        }
    }
}

/// The bare identifier at the base of a method-call/reference chain, if any.
pub fn innermost_base_ident(expr: &Expr) -> Option<String> {
    match expr {
        Expr::MethodCall(m) => innermost_base_ident(&m.receiver),
        Expr::Reference(r) => innermost_base_ident(&r.expr),
        Expr::Paren(p) => innermost_base_ident(&p.expr),
        Expr::Unary(u) if matches!(u.op, syn::UnOp::Deref(_)) => innermost_base_ident(&u.expr),
        Expr::Path(p) if p.path.segments.len() == 1 => Some(p.path.segments[0].ident.to_string()),
        _ => None,
    }
}

/// All bare identifiers that are *moved* into a by-value adapter chain:
/// the base receiver ident plus every bare-ident argument (`it.chain(other)`,
/// `it.zip(other)` move `it` and `other`).
pub fn collect_moved_idents(expr: &Expr) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(e: &Expr, out: &mut Vec<String>) {
        match e {
            Expr::MethodCall(m) => {
                walk(&m.receiver, out);
                for a in &m.args {
                    push_ident(a, out);
                }
            }
            Expr::Reference(r) => walk(&r.expr, out),
            Expr::Paren(p) => walk(&p.expr, out),
            Expr::Unary(u) if matches!(u.op, syn::UnOp::Deref(_)) => walk(&u.expr, out),
            Expr::Path(p) if p.path.segments.len() == 1 => {
                push_ident(e, out);
            }
            _ => {}
        }
    }
    fn push_ident(e: &Expr, out: &mut Vec<String>) {
        if let Expr::Path(p) = e {
            if p.path.segments.len() == 1 {
                out.push(p.path.segments[0].ident.to_string());
            }
        }
    }
    walk(expr, &mut out);
    out
}

fn is_direct_iter_let(expr: &Expr, gk: GuardKind) -> bool {
    if !matches!(gk, GuardKind::IterRead | GuardKind::IterWrite) {
        return true;
    }
    matches!(
        expr,
        Expr::MethodCall(mc) if matches!(mc.method.to_string().as_str(), "iter" | "iter_mut")
    )
}

fn walk_root_of(map: &MapId) -> String {
    map.concrete.split('.').next().unwrap_or("").to_string()
}

fn effect_desc(e: EffectKind) -> &'static str {
    match e {
        EffectKind::ReadLock => "reading",
        EffectKind::WriteLock => "borrowing mutably",
        EffectKind::ExclusiveMutation => "mutating",
        EffectKind::Remove => "removing from",
        EffectKind::ClearRetain => "retaining/clearing",
        EffectKind::IterRead => "iterating",
        EffectKind::IterWrite => "mutably iterating",
    }
}

fn effect_op(e: EffectKind) -> &'static str {
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

fn guard_kind_desc(g: GuardKind) -> &'static str {
    match g {
        GuardKind::Read => "shared (get)",
        GuardKind::Write => "exclusive (get_mut/entry)",
        GuardKind::IterRead => "iterator shared (iter)",
        GuardKind::IterWrite => "iterator exclusive (iter_mut)",
    }
}
