//! File discovery, parsing, and the intra-crate function/static index.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use proc_macro2::Span;
use syn::spanned::Spanned;
use syn::{Expr, Item};

use crate::identity::{Binding, Bindings, StaticMaps};

#[derive(Debug, Clone)]
pub struct FnMetadata {
    pub id: usize,
    pub name: String,
    pub display_name: String,
    pub file_idx: usize,
    pub line: usize,
    pub is_method: bool,
    pub param_names: Vec<String>,
    pub has_self: bool,
    pub param_is_map: Vec<bool>,
    /// Declared type name for each parameter (peeled of references/wrappers).
    pub param_types: Vec<Option<String>>,
    /// Owning impl type for methods, e.g. `ConsoleHandler`.
    pub impl_owner: Option<String>,
}

#[derive(Debug, Default)]
pub struct FileInfo {
    pub path: String,
    pub has_parse_error: bool,
    pub parse_error: Option<String>,
    pub parse_line: Option<usize>,
    pub parse_column: Option<usize>,
    pub fns: Vec<usize>,
    pub statics: Vec<String>,
    pub uses: Vec<(String, String)>,
}

pub struct CrateIndex {
    pub files: Vec<FileInfo>,
    pub fns: Vec<FnMetadata>,
    pub fn_bodies: Vec<Option<syn::Block>>,
    pub by_name: HashMap<String, Vec<usize>>,
    /// `TypeName::method` -> fn id.
    pub by_qualified: HashMap<String, usize>,
    pub statics: StaticMaps,
    pub map_fields: BTreeSet<String>,
    pub file_fns: HashMap<usize, Vec<usize>>,
    /// `type Alias = Target` (target is a peeled type name).
    pub type_aliases: HashMap<String, String>,
    /// struct/enum name -> field name -> peeled field type name.
    pub struct_fields: HashMap<String, BTreeMap<String, String>>,
    /// Fully-qualified local type names, used to distinguish same-named types
    /// declared in different modules.
    pub local_types: BTreeSet<String>,
}

impl Default for CrateIndex {
    fn default() -> CrateIndex {
        CrateIndex::new()
    }
}

impl CrateIndex {
    pub fn new() -> CrateIndex {
        CrateIndex {
            files: Vec::new(),
            fns: Vec::new(),
            fn_bodies: Vec::new(),
            by_name: HashMap::new(),
            by_qualified: HashMap::new(),
            statics: BTreeSet::new(),
            map_fields: BTreeSet::new(),
            file_fns: HashMap::new(),
            type_aliases: HashMap::new(),
            struct_fields: HashMap::new(),
            local_types: BTreeSet::new(),
        }
    }

    pub fn add_file(&mut self, path: &str, _root: &Path, src: &str) -> Vec<usize> {
        let file_idx = self.files.len();
        let mut info = FileInfo {
            path: path.to_string(),
            ..Default::default()
        };
        let parsed = syn::parse_file(src);
        let syntax = match parsed {
            Ok(f) => f,
            Err(e) => {
                info.has_parse_error = true;
                info.parse_error = Some(e.to_string());
                if let Some(start) = e.span().start().line.checked_sub(0) {
                    info.parse_line = Some(start);
                    info.parse_column = Some(e.span().start().column + 1);
                }
                self.files.push(info);
                return Vec::new();
            }
        };

        let mut new_ids = Vec::new();
        self.collect_items(file_idx, &syntax.items, &mut info, &mut new_ids, &[]);
        self.files.push(info);
        new_ids
    }

    /// Resolve declarations after all files have been indexed. This makes
    /// aliases and cross-file fields independent of filesystem traversal
    /// order.
    pub fn finalize(&mut self) {
        let aliases = self.type_aliases.clone();
        for (name, target) in aliases {
            let resolved = resolve_alias_name_with(&name, &self.type_aliases, &mut BTreeSet::new());
            self.type_aliases.insert(name, resolved);
            let _ = target;
        }
        for fields in self.struct_fields.values_mut() {
            for ty in fields.values_mut() {
                *ty = resolve_alias_name_with(ty, &self.type_aliases, &mut BTreeSet::new());
            }
        }
        for meta in &mut self.fns {
            if let Some(owner) = &meta.impl_owner {
                meta.impl_owner =
                    Some(resolve_alias_name_with(owner, &self.type_aliases, &mut BTreeSet::new()));
            }
            for ty in meta.param_types.iter_mut().flatten() {
                *ty = resolve_alias_name_with(ty, &self.type_aliases, &mut BTreeSet::new());
            }
            for (i, ty) in meta.param_types.iter().enumerate() {
                if let Some(ty) = ty {
                    if i < meta.param_is_map.len() {
                        meta.param_is_map[i] = is_dashmap_resolved(ty, &self.type_aliases);
                    }
                }
            }
        }
        self.map_fields.clear();
        for fields in self.struct_fields.values() {
            for (field, ty) in fields {
                if is_dashmap_resolved(ty, &self.type_aliases) {
                    self.map_fields.insert(field.clone());
                }
            }
        }
    }

    fn collect_items(
        &mut self,
        file_idx: usize,
        items: &[Item],
        info: &mut FileInfo,
        new_ids: &mut Vec<usize>,
        module_path: &[String],
    ) {
        for item in items {
            match item {
                Item::Fn(f) => {
                    let id = self.fns.len();
                    let name = f.sig.ident.to_string();
                    let (line, _) = span_of(&f.sig);
                    let (params, param_is_map, param_types) = sig_params(&f.sig, self, module_path);
                    let has_self = f.sig.receiver().is_some();
                    self.fns.push(FnMetadata {
                        id,
                        name: name.clone(),
                        display_name: name.clone(),
                        file_idx,
                        line,
                        is_method: false,
                        param_names: params,
                        has_self,
                        param_is_map,
                        param_types,
                        impl_owner: None,
                    });
                    self.fn_bodies.push(Some((*f.block).clone()));
                    info.fns.push(id);
                    self.by_name.entry(name).or_default().push(id);
                    self.file_fns.entry(file_idx).or_default().push(id);
                    new_ids.push(id);
                }
                Item::Impl(imp) => {
                    let ty = type_name_peel_in(&imp.self_ty, self, module_path);
                    for inner in &imp.items {
                        if let syn::ImplItem::Fn(m) = inner {
                            let id = self.fns.len();
                            let name = m.sig.ident.to_string();
                            let (line, _) = span_of(&m.sig);
                            let (params, param_is_map, param_types) =
                                sig_params(&m.sig, self, module_path);
                            let has_self = m.sig.receiver().is_some();
                            let display_name = if ty.is_empty() {
                                name.clone()
                            } else {
                                format!("{ty}::{name}")
                            };
                            self.fns.push(FnMetadata {
                                id,
                                name: name.clone(),
                                display_name: display_name.clone(),
                                file_idx,
                                line,
                                is_method: true,
                                param_names: params,
                                has_self,
                                param_is_map,
                                param_types,
                                impl_owner: if ty.is_empty() { None } else { Some(ty.clone()) },
                            });
                            self.fn_bodies.push(Some(m.block.clone()));
                            info.fns.push(id);
                            self.by_name.entry(name).or_default().push(id);
                            if !ty.is_empty() {
                                self.by_qualified.insert(display_name, id);
                            }
                            self.file_fns.entry(file_idx).or_default().push(id);
                            new_ids.push(id);
                        }
                    }
                }
                Item::Mod(m) => {
                    if let Some((_, inner_items)) = &m.content {
                        let mut nested = module_path.to_vec();
                        nested.push(m.ident.to_string());
                        self.collect_items(file_idx, inner_items, info, new_ids, &nested);
                    }
                }
                Item::Static(s) => {
                    let name = s.ident.to_string();
                    if type_resolves_to_dashmap_in(&s.ty, self, module_path) {
                        info.statics.push(name.clone());
                        self.statics.insert(name);
                    }
                }
                Item::Const(c) => {
                    let name = c.ident.to_string();
                    if type_resolves_to_dashmap_in(&c.ty, self, module_path) {
                        info.statics.push(name.clone());
                        self.statics.insert(name);
                    }
                }
                Item::Use(u) => {
                    if let syn::UseTree::Name(n) = &u.tree {
                        let short = n.ident.to_string();
                        info.uses.push((short.clone(), short));
                    }
                }
                Item::Type(t) => {
                    let name = t.ident.to_string();
                    let qualified = qualify_name(module_path, &name);
                    let target = type_name_peel_in(&t.ty, self, module_path);
                    self.type_aliases.insert(qualified, target);
                }
                Item::Struct(st) => {
                    let struct_name = qualify_name(module_path, &st.ident.to_string());
                    self.local_types.insert(struct_name.clone());
                    collect_struct_fields(&struct_name, &st.fields, self, module_path);
                }
                Item::Enum(en) => {
                    let enum_name = qualify_name(module_path, &en.ident.to_string());
                    self.local_types.insert(enum_name.clone());
                    for v in &en.variants {
                        collect_struct_fields(&enum_name, &v.fields, self, module_path);
                    }
                }
                Item::Trait(t) => {
                    for inner in &t.items {
                        if let syn::TraitItem::Fn(m) = inner {
                            let _ = m;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    pub fn resolve_type_alias(&self, name: &str) -> String {
        resolve_alias_name(name, self, &mut BTreeSet::new())
    }

    pub fn type_is_dashmap_name(&self, name: &str) -> bool {
        let resolved = self.resolve_type_alias(name);
        resolved == "DashMap"
            || resolved == "DashSet"
            || resolved.ends_with("::DashMap")
            || resolved.ends_with("::DashSet")
    }

    /// Resolve a method on a statically-known receiver type.
    pub fn resolve_method_for_type(&self, method: &str, owner: &str) -> Option<usize> {
        let owner = self.resolve_type_alias(owner);
        let key = format!("{owner}::{method}");
        if let Some(&id) = self.by_qualified.get(&key) {
            return Some(id);
        }
        None
    }

    pub fn resolve_call(
        &self,
        func_expr: &Expr,
        caller_file: usize,
        caller_impl_owner: Option<&str>,
    ) -> Option<usize> {
        match func_expr {
            Expr::Path(p) => {
                let segs = &p.path.segments;
                let last = segs.last()?;
                let name = last.ident.to_string();
                if segs.len() >= 2 {
                    let first = segs.first()?.ident.to_string();
                    if first == "Self" || first == "self" {
                        if let Some(owner) = caller_impl_owner {
                            return self.resolve_method_for_type(&name, owner);
                        }
                        return None;
                    }
                }
                let candidates = self.by_name.get(&name).cloned().unwrap_or_default();
                let same_file: Vec<usize> = candidates
                    .iter()
                    .copied()
                    .filter(|id| {
                        self.fns[*id].file_idx == caller_file && !self.fns[*id].is_method
                    })
                    .collect();
                if same_file.len() == 1 {
                    return Some(same_file[0]);
                }
                match candidates.len() {
                    1 => Some(candidates[0]),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Resolve receiver expression to a peeled type name when statically known.
    pub fn resolve_receiver_type(
        &self,
        expr: &Expr,
        bindings: &Bindings,
        caller_impl_owner: Option<&str>,
    ) -> Option<String> {
        match expr {
            Expr::Path(p) if p.path.segments.len() == 1 => {
                let name = p.path.segments[0].ident.to_string();
                if name == "self" {
                    return caller_impl_owner.map(|o| self.resolve_type_alias(o));
                }
                if let Some(Binding::Typed(ty)) = bindings.lookup(&name) {
                    return Some(self.resolve_type_alias(ty));
                }
                None
            }
            Expr::Field(f) => {
                let base_ty = self.resolve_receiver_type(&f.base, bindings, caller_impl_owner)?;
                let member = match &f.member {
                    syn::Member::Named(ident) => ident.to_string(),
                    syn::Member::Unnamed(index) => index.index.to_string(),
                };
                self.struct_fields
                    .get(&base_ty)
                    .and_then(|fields| fields.get(&member))
                    .map(|t| self.resolve_type_alias(t))
            }
            Expr::Reference(r) => {
                self.resolve_receiver_type(&r.expr, bindings, caller_impl_owner)
            }
            Expr::Paren(p) => self.resolve_receiver_type(&p.expr, bindings, caller_impl_owner),
            Expr::Unary(u) if matches!(u.op, syn::UnOp::Deref(_)) => {
                self.resolve_receiver_type(&u.expr, bindings, caller_impl_owner)
            }
            _ => None,
        }
    }

    pub fn field_type_of(&self, owner: &str, field: &str) -> Option<String> {
        let owner = self.resolve_type_alias(owner);
        self.struct_fields
            .get(&owner)
            .and_then(|m| m.get(field))
            .map(|t| self.resolve_type_alias(t))
    }
}

fn qualify_name(module_path: &[String], name: &str) -> String {
    if module_path.is_empty() {
        name.to_string()
    } else {
        format!("{}::{name}", module_path.join("::"))
    }
}

fn resolve_alias_name_with(
    name: &str,
    aliases: &HashMap<String, String>,
    seen: &mut BTreeSet<String>,
) -> String {
    if !seen.insert(name.to_string()) {
        return name.to_string();
    }
    if let Some(target) = aliases.get(name) {
        resolve_alias_name_with(target, aliases, seen)
    } else {
        name.to_string()
    }
}

fn resolve_alias_name(name: &str, index: &CrateIndex, seen: &mut BTreeSet<String>) -> String {
    if !seen.insert(name.to_string()) {
        return name.to_string();
    }
    if let Some(target) = index.type_aliases.get(name) {
        resolve_alias_name(target, index, seen)
    } else {
        name.to_string()
    }
}

pub fn type_name_peel(ty: &syn::Type, index: &CrateIndex) -> String {
    type_name_peel_in(ty, index, &[])
}

fn type_name_peel_in(ty: &syn::Type, index: &CrateIndex, module_path: &[String]) -> String {
    match ty {
        syn::Type::Path(t) => {
            let path = t
                .path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect::<Vec<_>>();
            let leaf = path.last().cloned().unwrap_or_default();
            if let Some(last) = t.path.segments.last() {
                if let syn::PathArguments::AngleBracketed(args) = &last.arguments {
                    if matches!(leaf.as_str(), "Arc" | "Box" | "Rc" | "Option" | "Result") {
                        if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                            return type_name_peel_in(inner, index, module_path);
                        }
                    }
                }
            }
            if path.len() > 1 {
                path.join("::")
            } else if matches!(
                leaf.as_str(),
                "DashMap"
                    | "DashSet"
                    | "String"
                    | "Path"
                    | "PathBuf"
                    | "Vec"
                    | "HashMap"
                    | "HashSet"
                    | "BTreeMap"
                    | "BTreeSet"
                    | "u8"
                    | "u16"
                    | "u32"
                    | "u64"
                    | "i8"
                    | "i16"
                    | "i32"
                    | "i64"
                    | "usize"
                    | "isize"
                    | "f32"
                    | "f64"
                    | "bool"
            ) {
                leaf
            } else {
                qualify_name(module_path, &leaf)
            }
        }
        syn::Type::Reference(r) => type_name_peel_in(&r.elem, index, module_path),
        syn::Type::Group(g) => type_name_peel_in(&g.elem, index, module_path),
        syn::Type::Paren(p) => type_name_peel_in(&p.elem, index, module_path),
        syn::Type::Ptr(p) => type_name_peel_in(&p.elem, index, module_path),
        syn::Type::Slice(s) => type_name_peel_in(&s.elem, index, module_path),
        syn::Type::Array(a) => type_name_peel_in(&a.elem, index, module_path),
        _ => String::new(),
    }
}

fn collect_struct_fields(
    struct_name: &str,
    fields: &syn::Fields,
    index: &mut CrateIndex,
    module_path: &[String],
) {
    let mut collected: Vec<(String, String, bool)> = Vec::new();
    match fields {
        syn::Fields::Named(named) => {
            for f in &named.named {
                if let Some(ident) = &f.ident {
                    let field_name = ident.to_string();
                    let field_ty = type_name_peel_in(&f.ty, index, module_path);
                    let is_map = type_name_is_dashmap(&field_ty, index);
                    collected.push((field_name, field_ty, is_map));
                }
            }
        }
        syn::Fields::Unnamed(unnamed) => {
            for (i, f) in unnamed.unnamed.iter().enumerate() {
                let field_name = format!("{i}");
                let field_ty = type_name_peel_in(&f.ty, index, module_path);
                let is_map = type_name_is_dashmap(&field_ty, index);
                collected.push((field_name, field_ty, is_map));
            }
        }
        syn::Fields::Unit => {}
    }
    let entry = index.struct_fields.entry(struct_name.to_string()).or_default();
    for (field_name, field_ty, is_map) in collected {
        entry.insert(field_name.clone(), field_ty);
        if is_map {
            index.map_fields.insert(field_name);
        }
    }
}

fn type_name_is_dashmap(name: &str, index: &CrateIndex) -> bool {
    let resolved = index.resolve_type_alias(name);
    resolved == "DashMap"
        || resolved == "DashSet"
        || resolved.ends_with("::DashMap")
        || resolved.ends_with("::DashSet")
}

fn type_resolves_to_dashmap_in(
    ty: &syn::Type,
    index: &CrateIndex,
    module_path: &[String],
) -> bool {
    let name = type_name_peel_in(ty, index, module_path);
    type_name_is_dashmap(&name, index)
}

fn is_dashmap_resolved(name: &str, aliases: &HashMap<String, String>) -> bool {
    let resolved = resolve_alias_name_with(name, aliases, &mut BTreeSet::new());
    resolved == "DashMap"
        || resolved == "DashSet"
        || resolved.ends_with("::DashMap")
        || resolved.ends_with("::DashSet")
}

fn sig_params(
    sig: &syn::Signature,
    index: &CrateIndex,
    module_path: &[String],
) -> (Vec<String>, Vec<bool>, Vec<Option<String>>) {
    let mut names = Vec::new();
    let mut is_map = Vec::new();
    let mut types = Vec::new();
    for input in &sig.inputs {
        match input {
            syn::FnArg::Receiver(_) => {}
            syn::FnArg::Typed(pt) => {
                if let syn::Pat::Ident(pid) = &*pt.pat {
                    names.push(pid.ident.to_string());
                    let peeled = type_name_peel_in(&pt.ty, index, module_path);
                    is_map.push(type_name_is_dashmap(&peeled, index));
                    types.push(if peeled.is_empty() { None } else { Some(peeled) });
                }
            }
        }
    }
    (names, is_map, types)
}

pub fn span_of<T: syn::spanned::Spanned>(node: &T) -> (usize, usize) {
    let sp: Span = node.span();
    let loc = sp.start();
    (loc.line, loc.column + 1)
}

pub fn span_of_expr(e: &Expr) -> (usize, usize) {
    let sp = e.span();
    let loc = sp.start();
    (loc.line, loc.column + 1)
}
