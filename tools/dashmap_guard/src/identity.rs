//! Map identity and alias canonicalization.
//!
//! The analyzer tracks DashMap access through canonical identities such as
//! `world.tiles` / `self.tiles` rather than whichever expression spelled them
//! at any one site. Simple aliases (`let tiles = &world.tiles; let map = tiles;`)
//! are folded onto the same identity via a scoped binding table.

use std::collections::HashMap;

use syn::Expr;

/// A canonical DashMap identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MapId {
    /// Resolved dotted path, e.g. `world.tiles`, `self.world.enemies`,
    /// `connections`. Used for exact same-object matching.
    pub concrete: String,
    /// The last path segment (the field name), e.g. `tiles`. Used as a
    /// fallback identity across functions where the root object spelling
    /// differs (e.g. `world.tiles` caller vs `w.world.tiles` helper).
    pub field: String,
}

impl MapId {
    pub fn new(concrete: impl Into<String>, field: impl Into<String>) -> MapId {
        MapId {
            concrete: concrete.into(),
            field: field.into(),
        }
    }

    pub fn single(name: impl Into<String>) -> MapId {
        let n = name.into();
        let field = n.rsplit('.').next().unwrap_or(&n).to_string();
        MapId { concrete: n, field }
    }

    /// Exact same-map match: identical concrete identity (e.g. both `world.tiles`).
    /// Blocking diagnostics (DM001-DM005) require this.
    pub fn exact(&self, other: &MapId) -> bool {
        self.concrete == other.concrete
    }

    /// Same DashMap field name on (possibly) different objects. This is NOT
    /// sufficient for a blocking diagnostic — it turns into a DM900.
    pub fn same_field(&self, other: &MapId) -> bool {
        self.field == other.field && !self.exact(other)
    }

    pub fn is_root_map(&self) -> bool {
        !self.concrete.contains('.')
    }
}

/// Rewrite a callee-side map identity into the caller's namespace.
///
/// `subst` maps a callee leading identifier (`self`, a parameter name) to the
/// caller's resolved prefix (`self.world`, `world`, `view`). This is how
/// `fn helper(w: &World) { w.tiles.insert(..); }` called as `helper(world)`
/// is proven to re-enter `world.tiles` rather than being demoted to DM900.
pub fn apply_subst(map: &MapId, subst: &[(String, String)]) -> MapId {
    for (from, to) in subst {
        if from.is_empty() || to.is_empty() {
            continue;
        }
        if map.concrete == *from {
            let field = to.rsplit('.').next().unwrap_or(to).to_string();
            return MapId::new(to.clone(), field);
        }
        let prefix = format!("{from}.");
        if let Some(rest) = map.concrete.strip_prefix(&prefix) {
            let remapped = format!("{to}.{rest}");
            return MapId::new(remapped, map.field.clone());
        }
    }
    map.clone()
}

/// What a local identifier currently refers to.
#[derive(Debug, Clone)]
pub enum Binding {
    /// A map (an alias of a map, or a freshly constructed DashMap).
    Map(MapId),
    /// A plain object that can be the base of a `.field` path (function
    /// parameters, `self`, and let-bound values).
    Root,
    /// A value with a statically-known peeled type name.
    Typed(String),
    /// A DashMap guard (Ref/RefMut/Entry/Iterator) or unknown value.
    Other,
}

/// A stack of scopes mapping local identifiers to bindings (shadowing-safe).
#[derive(Debug, Clone, Default)]
pub struct Bindings {
    scopes: Vec<HashMap<String, Binding>>,
}

impl Bindings {
    pub fn new() -> Bindings {
        Bindings::default()
    }

    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    pub fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    pub fn bind(&mut self, name: &str, binding: Binding) {
        if let Some(top) = self.scopes.last_mut() {
            top.insert(name.to_string(), binding);
        } else {
            let mut map = HashMap::new();
            map.insert(name.to_string(), binding);
            self.scopes.push(map);
        }
    }

    pub fn lookup(&self, name: &str) -> Option<&Binding> {
        for scope in self.scopes.iter().rev() {
            if let Some(b) = scope.get(name) {
                return Some(b);
            }
        }
        None
    }

    pub fn is_bound(&self, name: &str) -> bool {
        self.lookup(name).is_some()
    }
}

/// Module-level identifiers declared as DashMap statics/consts (by leaf name).
pub type StaticMaps = std::collections::BTreeSet<String>;

/// Resolve `expr` as the base of a field path to a root-object path text.
/// Returns `Some("world")`, `Some("self")`, `Some("w.world")`, ... when the
/// expression is, or chains to, a known root object. Returns `None` when the
/// expression cannot be proven to be a root object.
pub fn resolve_root(expr: &Expr, bindings: &Bindings, statics: &StaticMaps) -> Option<String> {
    use syn::Expr::*;
    match expr {
        Path(p) => {
            // Fully qualified/normal paths: use the whole path text as a root
            // only when it is a known static (module-level DashMap).
            let text = path_text(&p.path);
            if statics.contains(
                &p.path
                    .segments
                    .last()
                    .map(|s| s.ident.to_string())
                    .unwrap_or_default(),
            ) {
                Some(text)
            } else if p.path.segments.len() == 1 {
                let name = p.path.segments[0].ident.to_string();
                match bindings.lookup(&name) {
                    Some(Binding::Map(_)) => None,
                    Some(Binding::Root) | Some(Binding::Typed(_)) => Some(name),
                    Some(Binding::Other) => None,
                    None => None,
                }
            } else {
                None
            }
        }
        Field(f) => {
            // e.g. base.world.tiles -> resolve base root then append fields,
            // but stop at the first recognized DashMap field (a DashMap has no
            // further fields we care about).
            let base_text = resolve_root(&f.base, bindings, statics)?;
            let member = match &f.member {
                syn::Member::Named(ident) => ident.to_string(),
                syn::Member::Unnamed(index) => index.index.to_string(),
            };
            Some(format!("{base_text}.{member}"))
        }
        Reference(r) => resolve_root(&r.expr, bindings, statics),
        Paren(p) => resolve_root(&p.expr, bindings, statics),
        Unary(u) => match &u.op {
            syn::UnOp::Deref(_) => resolve_root(&u.expr, bindings, statics),
            _ => None,
        },
        _ => None,
    }
}

fn path_text(p: &syn::Path) -> String {
    p.segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

/// Canonicalize a DashMap receiver expression to a map identity.
///
/// Accepts:
///   - field paths on root objects (`world.tiles`, `self.world.tiles`),
///   - bare identifiers through the binding table (map aliases, statics),
///   - references/derefs/parens transparently.
pub fn canonicalize(expr: &Expr, bindings: &Bindings, statics: &StaticMaps) -> Option<MapId> {
    use syn::Expr::*;
    match expr {
        Field(f) => {
            let member = match &f.member {
                syn::Member::Named(ident) => ident.to_string(),
                syn::Member::Unnamed(_) => return None,
            };
            let root = resolve_root(&f.base, bindings, statics)?;
            Some(MapId::new(format!("{root}.{member}"), member))
        }
        Path(p) if p.path.segments.len() == 1 => {
            let name = p.path.segments[0].ident.to_string();
            match bindings.lookup(&name) {
                Some(Binding::Map(id)) => Some(id.clone()),
                Some(Binding::Root) | Some(Binding::Typed(_)) | Some(Binding::Other) | None => {
                    if statics.contains(&name) {
                        Some(MapId::single(name))
                    } else {
                        None
                    }
                }
            }
        }
        Path(p) => {
            // Full path: only a known static module-level DashMap is a map.
            let leaf = p
                .path
                .segments
                .last()
                .map(|s| s.ident.to_string())
                .unwrap_or_default();
            if statics.contains(&leaf) {
                Some(MapId::single(path_text(&p.path)))
            } else {
                None
            }
        }
        Reference(r) => canonicalize(&r.expr, bindings, statics),
        Paren(p) => canonicalize(&p.expr, bindings, statics),
        Unary(u) => match &u.op {
            syn::UnOp::Deref(_) => canonicalize(&u.expr, bindings, statics),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn field_on_root_is_map() {
        let expr: Expr = parse_quote!(world.tiles);
        let mut b = Bindings::new();
        b.push_scope();
        b.bind("world", Binding::Root);
        let statics = StaticMaps::new();
        let id = canonicalize(&expr, &b, &statics).unwrap();
        assert_eq!(id.concrete, "world.tiles");
        assert_eq!(id.field, "tiles");
    }

    #[test]
    fn alias_chain_canonicalizes() {
        let alias: Expr = parse_quote!(world.tiles);
        let mut b = Bindings::new();
        b.push_scope();
        b.bind("world", Binding::Root);
        let id = canonicalize(&alias, &b, &StaticMaps::new()).unwrap();
        b.bind("tiles", Binding::Map(id));
        // let map = tiles;
        let id2 = canonicalize(&parse_quote!(tiles), &b, &StaticMaps::new()).unwrap();
        b.bind("map", Binding::Map(id2));
        let use_it: Expr = parse_quote!(map);
        assert_eq!(
            canonicalize(&use_it, &b, &StaticMaps::new())
                .unwrap()
                .concrete,
            "world.tiles"
        );
    }

    #[test]
    fn field_matching_does_not_imply_exact() {
        let a = MapId::new("world.tiles", "tiles");
        let b = MapId::new("w.world.tiles", "tiles");
        assert!(!a.exact(&b));
        assert!(a.same_field(&b));
        assert!(a.exact(&a));
        assert!(!a.same_field(&a));
    }

    #[test]
    fn subst_renames_param_root() {
        let callee = MapId::new("w.tiles", "tiles");
        let remapped = apply_subst(&callee, &[("w".into(), "world".into())]);
        assert_eq!(remapped.concrete, "world.tiles");
        assert_eq!(remapped.field, "tiles");
        assert!(remapped.exact(&MapId::new("world.tiles", "tiles")));
    }

    #[test]
    fn subst_self_to_receiver() {
        let callee = MapId::new("self.world.tiles", "tiles");
        let remapped = apply_subst(&callee, &[("self".into(), "view".into())]);
        assert_eq!(remapped.concrete, "view.world.tiles");
    }

    #[test]
    fn subst_dashmap_param_to_field() {
        let callee = MapId::single("tiles");
        let remapped = apply_subst(&callee, &[("tiles".into(), "world.tiles".into())]);
        assert_eq!(remapped.concrete, "world.tiles");
        assert_eq!(remapped.field, "tiles");
    }

    #[test]
    fn substitution_preserves_legitimate_repeated_paths() {
        let map = MapId::new("root.x.x.map", "map");
        let remapped = apply_subst(&map, &[("root".into(), "a".into())]);
        assert_eq!(remapped.concrete, "a.x.x.map");

        let map = MapId::new("a.b.a.b.map", "map");
        let remapped = apply_subst(&map, &[]);
        assert_eq!(remapped.concrete, "a.b.a.b.map");
    }
}
