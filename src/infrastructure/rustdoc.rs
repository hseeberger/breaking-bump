use crate::domain::{Undecided, reference::Reference};
use anyhow::{Context, bail};
use rustdoc_types::{
    Crate, GenericArg, GenericArgs, Id, ItemEnum, Path as RustdocPath, StructKind, Type,
    VariantKind,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::Path,
};

pub(crate) const FORMAT_VERSION: u32 = rustdoc_types::FORMAT_VERSION;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForeignReference {
    pub(crate) holder: Id,
    pub(crate) position: String,
    pub(crate) crate_id: u32,
    pub(crate) path: String,
    pub(crate) kind: String,
}

pub(crate) fn read(path: &Path) -> anyhow::Result<Crate> {
    let (json, format_version) = json_with_format_version(path)?;
    if format_version != FORMAT_VERSION {
        bail!(
            "{} has rustdoc JSON format {format_version}, but breaking-bump supports format \
             {FORMAT_VERSION}",
            path.display()
        );
    }
    serde_json::from_str(&json)
        .with_context(|| format!("cannot parse rustdoc JSON {}", path.display()))
}

pub(crate) fn format_version(path: &Path) -> anyhow::Result<u32> {
    let (_, format_version) = json_with_format_version(path)?;
    Ok(format_version)
}

pub(crate) fn crate_grouped_references(
    doc: &Crate,
) -> anyhow::Result<BTreeMap<u32, BTreeSet<Reference>>> {
    let sites = local_sites(doc);
    let mut crate_ids = HashMap::<Reference, u32>::new();
    let mut references = BTreeMap::<u32, BTreeSet<Reference>>::new();
    for foreign in foreign_references(doc)? {
        let reference = Reference {
            site: sites[&foreign.holder].clone(),
            position: foreign.position,
            path: foreign.path,
            kind: foreign.kind,
        };
        let crate_id = *crate_ids
            .entry(reference.clone())
            .or_insert(foreign.crate_id);
        if crate_id != foreign.crate_id {
            return Err(Undecided(format!(
                "items of the public API named `{}` refer to `{}` from different crates",
                reference.site, reference.path
            ))
            .into());
        }
        references
            .entry(foreign.crate_id)
            .or_default()
            .insert(reference);
    }
    Ok(references)
}

pub(crate) fn foreign_references(doc: &Crate) -> anyhow::Result<Vec<ForeignReference>> {
    let mut references = Vec::new();
    for (id, item) in sorted(&doc.index) {
        if item.crate_id != 0 {
            continue;
        }
        if let ItemEnum::Impl(implementation) = &item.inner
            && (implementation.blanket_impl.is_some() || implementation.is_synthetic)
        {
            continue;
        }
        for (position, referenced) in referenced_positions(&serde_json::to_value(&item.inner)?) {
            let referenced = Id(referenced);
            let (crate_id, path, kind) = if let Some(summary) = doc.paths.get(&referenced) {
                (
                    summary.crate_id,
                    summary.path.join("::"),
                    variant_name(&serde_json::to_value(summary.kind)?),
                )
            } else if let Some(foreign) = doc.index.get(&referenced) {
                (
                    foreign.crate_id,
                    foreign.name.clone().unwrap_or_default(),
                    variant_name(&serde_json::to_value(&foreign.inner)?),
                )
            } else {
                continue;
            };
            if crate_id != 0 {
                references.push(ForeignReference {
                    holder: *id,
                    position,
                    crate_id,
                    path,
                    kind,
                });
            }
        }
    }
    Ok(references)
}

fn json_with_format_version(path: &Path) -> anyhow::Result<(String, u32)> {
    let json = fs::read_to_string(path)
        .with_context(|| format!("cannot read rustdoc JSON {}", path.display()))?;
    let format = serde_json::from_str::<FormatVersion>(&json)
        .with_context(|| format!("{} is no rustdoc JSON", path.display()))?;
    Ok((json, format.format_version))
}

#[derive(Deserialize)]
struct FormatVersion {
    format_version: u32,
}

fn sorted<V>(map: &HashMap<Id, V>) -> Vec<(&Id, &V)> {
    let mut entries = map.iter().collect::<Vec<_>>();
    entries.sort_by_key(|(id, _)| **id);
    entries
}

fn local_sites(doc: &Crate) -> HashMap<Id, String> {
    let mut parents = HashMap::new();
    for (id, item) in sorted(&doc.index) {
        if item.crate_id == 0 {
            for child in child_ids(&item.inner) {
                parents.entry(child).or_insert(*id);
            }
        }
    }
    let mut sites = HashMap::new();
    for (id, item) in sorted(&doc.index) {
        if item.crate_id == 0 {
            site(doc, &parents, &mut sites, *id);
        }
    }
    sites
}

fn site(doc: &Crate, parents: &HashMap<Id, Id>, sites: &mut HashMap<Id, String>, id: Id) -> String {
    if let Some(site) = sites.get(&id) {
        return site.clone();
    }
    let name = match doc.index.get(&id) {
        None => "?".to_owned(),
        Some(item) => match (&item.inner, doc.paths.get(&id)) {
            (ItemEnum::Impl(implementation), _) => {
                let implemented = type_text(&implementation.for_, doc);
                match &implementation.trait_ {
                    Some(trait_) => format!("impl {} for {implemented}", path_text(trait_, doc)),
                    None => format!("impl {implemented}"),
                }
            }
            (_, Some(summary)) if summary.crate_id == 0 => summary.path.join("::"),
            _ => {
                let prefix = match parents.get(&id) {
                    Some(parent) => site(doc, parents, sites, *parent),
                    None => "?".to_owned(),
                };
                let name = match &item.name {
                    Some(name) => name.clone(),
                    None => serde_json::to_value(&item.inner)
                        .map(|inner| variant_name(&inner))
                        .unwrap_or_default(),
                };
                format!("{prefix}::{name}")
            }
        },
    };
    sites.insert(id, name.clone());
    name
}

fn child_ids(inner: &ItemEnum) -> Vec<Id> {
    match inner {
        ItemEnum::Module(module) => module.items.clone(),
        ItemEnum::Struct(structure) => match &structure.kind {
            StructKind::Unit => vec![],
            StructKind::Tuple(fields) => fields.iter().flatten().copied().collect(),
            StructKind::Plain { fields, .. } => fields.clone(),
        },
        ItemEnum::Union(union) => union.fields.clone(),
        ItemEnum::Enum(enumeration) => enumeration.variants.clone(),
        ItemEnum::Variant(variant) => match &variant.kind {
            VariantKind::Plain => vec![],
            VariantKind::Tuple(fields) => fields.iter().flatten().copied().collect(),
            VariantKind::Struct { fields, .. } => fields.clone(),
        },
        ItemEnum::Trait(trait_) => trait_.items.clone(),
        ItemEnum::Impl(implementation) => implementation.items.clone(),
        _ => vec![],
    }
}

fn type_text(ty: &Type, doc: &Crate) -> String {
    match ty {
        Type::ResolvedPath(path) => path_text(path, doc),
        Type::Generic(name) | Type::Primitive(name) => name.clone(),
        Type::BorrowedRef {
            is_mutable, type_, ..
        } => {
            let mutability = if *is_mutable { "mut " } else { "" };
            format!("&{mutability}{}", type_text(type_, doc))
        }
        Type::Tuple(types) => {
            let types = types
                .iter()
                .map(|ty| type_text(ty, doc))
                .collect::<Vec<_>>();
            format!("({})", types.join(", "))
        }
        Type::Slice(ty) => format!("[{}]", type_text(ty, doc)),
        Type::Array { type_, len } => format!("[{}; {len}]", type_text(type_, doc)),
        other => json_text(other),
    }
}

fn path_text(path: &RustdocPath, doc: &Crate) -> String {
    let name = match doc.paths.get(&path.id) {
        Some(summary) => summary.path.join("::"),
        None => format!("?{}", path.path),
    };
    format!("{name}{}", args_text(path.args.as_deref(), doc))
}

fn args_text(args: Option<&GenericArgs>, doc: &Crate) -> String {
    match args {
        None => String::new(),
        Some(GenericArgs::AngleBracketed { args, constraints }) => {
            let mut parts = args
                .iter()
                .map(|arg| match arg {
                    GenericArg::Type(ty) => type_text(ty, doc),
                    GenericArg::Lifetime(lifetime) => lifetime.clone(),
                    other => json_text(other),
                })
                .collect::<Vec<_>>();
            if !constraints.is_empty() {
                parts.push(json_text(constraints));
            }
            if parts.is_empty() {
                String::new()
            } else {
                format!("<{}>", parts.join(", "))
            }
        }
        Some(other) => json_text(other),
    }
}

fn json_text<T>(value: &T) -> String
where
    T: Serialize,
{
    serde_json::to_value(value)
        .map(|value| without_ids(&value).to_string())
        .unwrap_or_default()
}

fn without_ids(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| *key != "id")
                .map(|(key, value)| (key.clone(), without_ids(value)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(without_ids).collect()),
        other => other.clone(),
    }
}

fn referenced_positions(value: &Value) -> BTreeSet<(String, u32)> {
    let mut found = BTreeSet::new();
    let mut stack = vec![(value, String::new())];
    while let Some((current, position)) = stack.pop() {
        match current {
            Value::Object(map) => {
                for (key, child) in map {
                    match child {
                        Value::Number(number) if key == "id" => {
                            if let Some(id) = number.as_u64().and_then(|id| u32::try_from(id).ok())
                            {
                                found.insert((position.clone(), id));
                            }
                        }
                        Value::Object(_) | Value::Array(_) => {
                            stack.push((child, format!("{position}/{key}")))
                        }
                        _ => {}
                    }
                }
            }
            Value::Array(values) => {
                for (index, child) in values.iter().enumerate() {
                    if matches!(child, Value::Object(_) | Value::Array(_)) {
                        stack.push((child, format!("{position}/{index}")));
                    }
                }
            }
            _ => {}
        }
    }
    found
}

fn variant_name(value: &Value) -> String {
    match value {
        Value::String(name) => name.clone(),
        Value::Object(map) => map.keys().next().cloned().unwrap_or_default(),
        _ => String::new(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::{
        domain::Undecided,
        infrastructure::rustdoc::{FORMAT_VERSION, crate_grouped_references},
    };
    use rustdoc_types::Crate;
    use serde_json::{Value, json};

    #[test]
    fn colliding_site_names_are_undecided() {
        let error = crate_grouped_references(&doc(9, 10)).unwrap_err();
        assert!(error.is::<Undecided>());
    }

    #[test]
    fn colliding_site_names_with_the_same_crate_are_merged() {
        let references = crate_grouped_references(&doc(9, 9)).unwrap();
        assert_eq!(references.keys().copied().collect::<Vec<_>>(), [1]);
        let reference = references[&1].first().unwrap();
        assert_eq!(reference.site, "?::get");
        assert_eq!(reference.position, "/struct_field/resolved_path");
        assert_eq!(reference.path, "c::T");
        assert_eq!(reference.kind, "struct");
    }

    pub(crate) fn doc(first: u32, second: u32) -> Crate {
        serde_json::from_value(json!({
            "root": 0,
            "crate_version": null,
            "includes_private": false,
            "index": {
                "1": field(1, first),
                "2": field(2, second),
            },
            "paths": {
                "9": { "crate_id": 1, "path": ["c", "T"], "kind": "struct" },
                "10": { "crate_id": 2, "path": ["c", "T"], "kind": "struct" },
            },
            "external_crates": {
                "1": { "name": "c", "html_root_url": null, "path": "/nowhere/libc-1.rlib" },
                "2": { "name": "c", "html_root_url": null, "path": "/nowhere/libc-2.rlib" },
            },
            "target": { "triple": "x86_64-unknown-linux-gnu", "target_features": [] },
            "format_version": FORMAT_VERSION,
        }))
        .unwrap()
    }

    fn field(id: u32, referenced: u32) -> Value {
        json!({
            "id": id,
            "crate_id": 0,
            "name": "get",
            "span": null,
            "visibility": "public",
            "docs": null,
            "links": {},
            "attrs": [],
            "deprecation": null,
            "inner": {
                "struct_field": {
                    "resolved_path": { "path": "T", "id": referenced, "args": null }
                }
            },
        })
    }
}
