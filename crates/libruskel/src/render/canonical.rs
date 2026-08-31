//! Canonical policies for snapshot format 1.

use std::collections::HashSet;

use rustdoc_types::{
    AssocItemConstraintKind, Attribute, AttributeRepr, Crate, GenericArg, GenericArgs,
    GenericBound, GenericParamDefKind, Generics, Id, Item, ItemEnum, ReprKind, Type, Visibility,
};
use syn::parse::Parser;

use crate::{
    crateutils::{render_generic_bound, render_name, render_poly_trait, render_where_predicate},
    error::{Result, RuskelError},
    search::SearchItemKind,
    signature,
};

/// Stable sort key for one independently ordered declaration.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct CanonicalItemKey {
    /// Fixed declaration category.
    category: u8,
    /// Name visible at this occurrence.
    name: String,
    /// Normalized public signature.
    signature: String,
    /// Full source fragment used as the final deterministic tie breaker.
    fragment: String,
}

impl CanonicalItemKey {
    /// Build a key for a rendered item occurrence.
    pub(super) fn new(crate_data: &Crate, item: &Item, fragment: String) -> Self {
        let (key_item, exported_name) = if let ItemEnum::Use(import) = &item.inner {
            import
                .id
                .and_then(|id| crate_data.index.get(&id))
                .map_or((item, import.name.clone()), |target| {
                    (target, import.name.clone())
                })
        } else {
            (item, render_name(item))
        };
        let (category, kind) = item_category_and_kind(key_item);
        let signature = kind
            .and_then(|kind| signature::item_signature(crate_data, key_item, kind))
            .unwrap_or_else(|| fragment.trim().to_string());
        Self {
            category,
            name: exported_name,
            signature,
            fragment,
        }
    }

    /// Return the rendered fragment owned by this key.
    pub(super) fn into_fragment(self) -> String {
        self.fragment
    }
}

/// Render retained public API attributes in their rustdoc order.
pub(super) fn retained_attributes(item: &Item) -> Result<String> {
    let mut output = String::new();
    for attribute in &item.attrs {
        if let Some(rendered) = render_attribute(attribute)? {
            output.push_str(&rendered);
            output.push('\n');
        }
    }
    if let Some(deprecation) = &item.deprecation {
        let mut fields = Vec::new();
        if let Some(since) = &deprecation.since {
            fields.push(format!("since = {}", rust_string(since)));
        }
        if let Some(note) = &deprecation.note {
            fields.push(format!("note = {}", rust_string(note)));
        }
        if fields.is_empty() {
            output.push_str("#[deprecated]\n");
        } else {
            output.push_str(&format!("#[deprecated({})]\n", fields.join(", ")));
        }
    }
    Ok(output)
}

/// Validate all public occurrences reachable from the crate root.
pub(super) fn validate_reachable(crate_data: &Crate) -> Result<()> {
    let mut active = HashSet::new();
    validate_item(crate_data, crate_data.root, true, &mut active)
}

/// Clone rustdoc data and canonicalize only set-like semantic sequences.
pub(super) fn canonicalized(crate_data: &Crate) -> Crate {
    let mut normalized = crate_data.clone();
    for item in normalized.index.values_mut() {
        canonicalize_item(item);
    }
    normalized
}

/// Canonicalize set-like fields without changing ordered declarations.
fn canonicalize_item(item: &mut Item) {
    match &mut item.inner {
        ItemEnum::Union(union_) => canonicalize_generics(&mut union_.generics),
        ItemEnum::Struct(struct_) => canonicalize_generics(&mut struct_.generics),
        ItemEnum::Enum(enum_) => canonicalize_generics(&mut enum_.generics),
        ItemEnum::Function(function) => {
            canonicalize_generics(&mut function.generics);
            for (_, type_) in &mut function.sig.inputs {
                canonicalize_type(type_);
            }
            if let Some(output) = &mut function.sig.output {
                canonicalize_type(output);
            }
        }
        ItemEnum::Trait(trait_) => {
            canonicalize_generics(&mut trait_.generics);
            canonicalize_bounds(&mut trait_.bounds);
        }
        ItemEnum::TraitAlias(alias) => {
            canonicalize_generics(&mut alias.generics);
            canonicalize_bounds(&mut alias.params);
        }
        ItemEnum::Impl(impl_) => {
            canonicalize_generics(&mut impl_.generics);
            canonicalize_type(&mut impl_.for_);
        }
        ItemEnum::TypeAlias(alias) => {
            canonicalize_generics(&mut alias.generics);
            canonicalize_type(&mut alias.type_);
        }
        ItemEnum::Constant { type_, .. }
        | ItemEnum::StructField(type_)
        | ItemEnum::AssocConst { type_, .. } => canonicalize_type(type_),
        ItemEnum::Static(static_) => canonicalize_type(&mut static_.type_),
        ItemEnum::AssocType {
            generics,
            bounds,
            type_,
            ..
        } => {
            canonicalize_generics(generics);
            canonicalize_bounds(bounds);
            if let Some(type_) = type_ {
                canonicalize_type(type_);
            }
        }
        _ => {}
    }
}

/// Sort bounds and predicates while preserving generic parameter order.
fn canonicalize_generics(generics: &mut Generics) {
    for parameter in &mut generics.params {
        match &mut parameter.kind {
            GenericParamDefKind::Lifetime { outlives } => outlives.sort(),
            GenericParamDefKind::Type {
                bounds, default, ..
            } => {
                canonicalize_bounds(bounds);
                if let Some(default) = default {
                    canonicalize_type(default);
                }
            }
            GenericParamDefKind::Const { type_, .. } => canonicalize_type(type_),
        }
    }
    for predicate in &mut generics.where_predicates {
        match predicate {
            rustdoc_types::WherePredicate::BoundPredicate {
                type_,
                bounds,
                generic_params,
            } => {
                canonicalize_type(type_);
                canonicalize_bounds(bounds);
                for parameter in generic_params {
                    if let GenericParamDefKind::Type { bounds, .. } = &mut parameter.kind {
                        canonicalize_bounds(bounds);
                    }
                }
            }
            rustdoc_types::WherePredicate::LifetimePredicate { outlives, .. } => outlives.sort(),
            rustdoc_types::WherePredicate::EqPredicate { lhs, rhs } => {
                canonicalize_type(lhs);
                if let rustdoc_types::Term::Type(type_) = rhs {
                    canonicalize_type(type_);
                }
            }
        }
    }
    generics
        .where_predicates
        .sort_by_key(|predicate| render_where_predicate(predicate).unwrap_or_default());
}

/// Sort one set of generic bounds after normalizing nested arguments.
fn canonicalize_bounds(bounds: &mut [GenericBound]) {
    for bound in bounds.iter_mut() {
        if let GenericBound::TraitBound {
            trait_,
            generic_params,
            ..
        } = bound
        {
            if let Some(arguments) = &mut trait_.args {
                canonicalize_args(arguments);
            }
            for parameter in generic_params {
                if let GenericParamDefKind::Type { bounds, .. } = &mut parameter.kind {
                    canonicalize_bounds(bounds);
                }
            }
        }
    }
    bounds.sort_by_key(render_generic_bound);
}

/// Canonicalize set-like values nested inside a type.
fn canonicalize_type(type_: &mut Type) {
    match type_ {
        Type::ResolvedPath(path) => {
            if let Some(arguments) = &mut path.args {
                canonicalize_args(arguments);
            }
        }
        Type::DynTrait(dyn_trait) => {
            for trait_ in &mut dyn_trait.traits {
                if let Some(arguments) = &mut trait_.trait_.args {
                    canonicalize_args(arguments);
                }
            }
            dyn_trait.traits.sort_by_key(render_poly_trait);
        }
        Type::FunctionPointer(function) => {
            for parameter in &mut function.generic_params {
                if let GenericParamDefKind::Type { bounds, .. } = &mut parameter.kind {
                    canonicalize_bounds(bounds);
                }
            }
            for (_, input) in &mut function.sig.inputs {
                canonicalize_type(input);
            }
            if let Some(output) = &mut function.sig.output {
                canonicalize_type(output);
            }
        }
        Type::Tuple(types) => {
            for type_ in types {
                canonicalize_type(type_);
            }
        }
        Type::Slice(type_)
        | Type::Array { type_, .. }
        | Type::RawPointer { type_, .. }
        | Type::BorrowedRef { type_, .. }
        | Type::Pat { type_, .. } => canonicalize_type(type_),
        Type::ImplTrait(bounds) => canonicalize_bounds(bounds),
        Type::QualifiedPath {
            args,
            self_type,
            trait_,
            ..
        } => {
            canonicalize_type(self_type);
            if let Some(arguments) = args {
                canonicalize_args(arguments);
            }
            if let Some(trait_) = trait_
                && let Some(arguments) = &mut trait_.args
            {
                canonicalize_args(arguments);
            }
        }
        Type::Generic(_) | Type::Primitive(_) | Type::Infer => {}
    }
}

/// Canonicalize set-like constraints inside generic arguments.
fn canonicalize_args(arguments: &mut GenericArgs) {
    match arguments {
        GenericArgs::AngleBracketed { args, constraints } => {
            for argument in args {
                if let GenericArg::Type(type_) = argument {
                    canonicalize_type(type_);
                }
            }
            for constraint in constraints.iter_mut() {
                match &mut constraint.binding {
                    AssocItemConstraintKind::Equality(rustdoc_types::Term::Type(type_)) => {
                        canonicalize_type(type_);
                    }
                    AssocItemConstraintKind::Constraint(bounds) => canonicalize_bounds(bounds),
                    AssocItemConstraintKind::Equality(rustdoc_types::Term::Constant(_)) => {}
                }
            }
            constraints.sort_by(|left, right| left.name.cmp(&right.name));
        }
        GenericArgs::Parenthesized { inputs, output } => {
            for input in inputs {
                canonicalize_type(input);
            }
            if let Some(output) = output {
                canonicalize_type(output);
            }
        }
        GenericArgs::ReturnTypeNotation => {}
    }
}

/// Recursively validate one public occurrence.
fn validate_item(
    crate_data: &Crate,
    id: Id,
    forced_by_public_reexport: bool,
    active: &mut HashSet<Id>,
) -> Result<()> {
    if !active.insert(id) {
        return Ok(());
    }
    let item = crate_data
        .index
        .get(&id)
        .ok_or_else(|| RuskelError::ItemNotFound(format!("{id:?}")))?;
    let is_root = id == crate_data.root;
    let public =
        is_root || forced_by_public_reexport || matches!(item.visibility, Visibility::Public);
    if !public {
        active.remove(&id);
        return Ok(());
    }

    match &item.inner {
        ItemEnum::Module(module) => {
            for child in &module.items {
                validate_item(crate_data, *child, false, active)?;
            }
        }
        ItemEnum::Use(import) => {
            if import.id.is_none() {
                return Err(RuskelError::Generate(format!(
                    "snapshot format 1 cannot resolve public export '{}'",
                    import.source
                )));
            }
            if let Some(target) = import.id {
                validate_item(crate_data, target, true, active)?;
            }
        }
        ItemEnum::Struct(struct_) => {
            for child in struct_fields(struct_) {
                validate_item(crate_data, child, true, active)?;
            }
            validate_impls(crate_data, &struct_.impls, active)?;
        }
        ItemEnum::Union(union_) => {
            for child in &union_.fields {
                validate_item(crate_data, *child, true, active)?;
            }
            validate_impls(crate_data, &union_.impls, active)?;
        }
        ItemEnum::Enum(enum_) => {
            for child in &enum_.variants {
                validate_item(crate_data, *child, true, active)?;
            }
            validate_impls(crate_data, &enum_.impls, active)?;
        }
        ItemEnum::Variant(variant) => {
            for child in variant_fields(variant) {
                validate_item(crate_data, child, true, active)?;
            }
        }
        ItemEnum::Trait(trait_) => {
            for child in &trait_.items {
                validate_item(crate_data, *child, true, active)?;
            }
        }
        ItemEnum::Impl(impl_) => {
            if !impl_.is_synthetic && impl_.blanket_impl.is_none() {
                for child in &impl_.items {
                    validate_item(crate_data, *child, true, active)?;
                }
            }
        }
        ItemEnum::Function(_)
        | ItemEnum::TraitAlias(_)
        | ItemEnum::TypeAlias(_)
        | ItemEnum::Constant { .. }
        | ItemEnum::Static(_)
        | ItemEnum::Macro(_)
        | ItemEnum::ProcMacro(_)
        | ItemEnum::StructField(_)
        | ItemEnum::AssocConst { .. }
        | ItemEnum::AssocType { .. } => {}
        unsupported => {
            return Err(RuskelError::Generate(format!(
                "snapshot format 1 does not support reachable {:?} item '{}'",
                unsupported.item_kind(),
                render_name(item)
            )));
        }
    }
    active.remove(&id);
    Ok(())
}

/// Validate non-synthetic and non-blanket implementations.
fn validate_impls(crate_data: &Crate, impls: &[Id], active: &mut HashSet<Id>) -> Result<()> {
    for id in impls {
        let item = crate_data
            .index
            .get(id)
            .ok_or_else(|| RuskelError::ItemNotFound(format!("{id:?}")))?;
        if let ItemEnum::Impl(impl_) = &item.inner
            && !impl_.is_synthetic
            && impl_.blanket_impl.is_none()
        {
            validate_item(crate_data, *id, true, active)?;
        }
    }
    Ok(())
}

/// Return struct field IDs without changing their semantic order.
fn struct_fields(struct_: &rustdoc_types::Struct) -> Vec<Id> {
    match &struct_.kind {
        rustdoc_types::StructKind::Unit => Vec::new(),
        rustdoc_types::StructKind::Tuple(fields) => fields.iter().flatten().copied().collect(),
        rustdoc_types::StructKind::Plain { fields, .. } => fields.clone(),
    }
}

/// Return variant field IDs without changing their semantic order.
fn variant_fields(variant: &rustdoc_types::Variant) -> Vec<Id> {
    match &variant.kind {
        rustdoc_types::VariantKind::Plain => Vec::new(),
        rustdoc_types::VariantKind::Tuple(fields) => fields.iter().flatten().copied().collect(),
        rustdoc_types::VariantKind::Struct { fields, .. } => fields.clone(),
    }
}

/// Map an item to the fixed format 1 category and signature kind.
fn item_category_and_kind(item: &Item) -> (u8, Option<SearchItemKind>) {
    match &item.inner {
        ItemEnum::Module(_) => (0, Some(SearchItemKind::Module)),
        ItemEnum::Macro(_) | ItemEnum::ProcMacro(_) => (1, Some(SearchItemKind::Macro)),
        ItemEnum::Union(_) => (2, Some(SearchItemKind::Union)),
        ItemEnum::Struct(_) => (2, Some(SearchItemKind::Struct)),
        ItemEnum::Enum(_) => (2, Some(SearchItemKind::Enum)),
        ItemEnum::TypeAlias(_) => (2, Some(SearchItemKind::TypeAlias)),
        ItemEnum::Trait(_) => (3, Some(SearchItemKind::Trait)),
        ItemEnum::TraitAlias(_) => (3, Some(SearchItemKind::TraitAlias)),
        ItemEnum::Constant { .. } => (4, Some(SearchItemKind::Constant)),
        ItemEnum::Static(_) => (4, Some(SearchItemKind::Static)),
        ItemEnum::Function(_) => (5, Some(SearchItemKind::Function)),
        ItemEnum::Impl(_) => (6, None),
        ItemEnum::Use(_) => (2, Some(SearchItemKind::Use)),
        _ => (7, None),
    }
}

/// Convert one rustdoc attribute to source or omit non-surface metadata.
fn render_attribute(attribute: &Attribute) -> Result<Option<String>> {
    let rendered = match attribute {
        Attribute::NonExhaustive => "#[non_exhaustive]".to_string(),
        Attribute::MustUse { reason: None } => "#[must_use]".to_string(),
        Attribute::MustUse {
            reason: Some(reason),
        } => format!("#[must_use = {}]", rust_string(reason)),
        Attribute::ExportName(name) => {
            format!("#[unsafe(export_name = {})]", rust_string(name))
        }
        Attribute::LinkSection(name) => {
            format!("#[unsafe(link_section = {})]", rust_string(name))
        }
        Attribute::Repr(repr) => render_repr(repr),
        Attribute::NoMangle => "#[unsafe(no_mangle)]".to_string(),
        Attribute::TargetFeature { enable } => format!(
            "#[target_feature({})]",
            enable
                .iter()
                .map(|feature| format!("enable = {}", rust_string(feature)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Attribute::MacroExport => "#[macro_export]".to_string(),
        Attribute::AutomaticallyDerived => return Ok(None),
        Attribute::Other(source) => {
            if omit_other_attribute(source) {
                return Ok(None);
            }
            let parser = syn::Attribute::parse_outer;
            let parsed = parser.parse_str(source).map_err(|error| {
                RuskelError::Generate(format!(
                    "snapshot format 1 cannot parse retained attribute '{source}': {error}"
                ))
            })?;
            if parsed.len() != 1 {
                return Err(RuskelError::Generate(format!(
                    "snapshot format 1 expected one retained attribute in '{source}'"
                )));
            }
            source.clone()
        }
    };
    Ok(Some(rendered))
}

/// Omit attributes that do not define supported public API shape.
fn omit_other_attribute(source: &str) -> bool {
    let trimmed = source.trim();
    let compact = trimmed
        .strip_prefix("#[")
        .or_else(|| trimmed.strip_prefix("# ["))
        .unwrap_or(trimmed)
        .trim_start();
    [
        "allow",
        "warn",
        "deny",
        "forbid",
        "cfg_attr(test",
        "test",
        "bench",
        "derive",
        "automatically_derived",
        "attr = ",
        "rustc_",
        "stable",
        "unstable",
    ]
    .iter()
    .any(|prefix| compact.starts_with(prefix))
}

/// Render a structured representation attribute.
fn render_repr(repr: &AttributeRepr) -> String {
    let mut values = vec![
        match repr.kind {
            ReprKind::Rust => "Rust",
            ReprKind::C => "C",
            ReprKind::Transparent => "transparent",
            ReprKind::Simd => "simd",
        }
        .to_string(),
    ];
    if let Some(align) = repr.align {
        values.push(format!("align({align})"));
    }
    if let Some(packed) = repr.packed {
        values.push(format!("packed({packed})"));
    }
    if let Some(int) = &repr.int {
        values.push(int.clone());
    }
    format!("#[repr({})]", values.join(", "))
}

/// Quote a string for a Rust attribute.
fn rust_string(value: &str) -> String {
    serde_json::to_string(value).expect("string serialization cannot fail")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use rustdoc_types::{Attribute, Id, Item, ItemEnum, Visibility};

    use super::retained_attributes;
    use crate::error::Result;

    /// Create an item for attribute policy tests.
    fn item_with_attributes(attrs: Vec<Attribute>) -> Item {
        Item {
            id: Id(1),
            crate_id: 0,
            name: Some("exported".to_string()),
            span: None,
            visibility: Visibility::Public,
            docs: None,
            links: HashMap::new(),
            attrs,
            deprecation: None,
            stability: None,
            const_stability: None,
            inner: ItemEnum::ExternType,
        }
    }

    #[test]
    fn attribute_policy_keeps_public_metadata_and_omits_build_metadata() -> Result<()> {
        let item = item_with_attributes(vec![
            Attribute::Other("#[derive(Clone)]".to_string()),
            Attribute::Other("#[allow(dead_code)]".to_string()),
            Attribute::Other("#[doc(hidden)]".to_string()),
            Attribute::MacroExport,
            Attribute::NoMangle,
            Attribute::ExportName("external_name".to_string()),
            Attribute::LinkSection("api".to_string()),
        ]);
        assert_eq!(
            retained_attributes(&item)?,
            "#[doc(hidden)]\n#[macro_export]\n#[unsafe(no_mangle)]\n#[unsafe(export_name = \"external_name\")]\n#[unsafe(link_section = \"api\")]\n"
        );
        Ok(())
    }

    #[test]
    fn attribute_policy_rejects_unparseable_retained_metadata() {
        let item = item_with_attributes(vec![Attribute::Other("#[broken(]".to_string())]);
        let error = retained_attributes(&item).expect_err("invalid attribute must fail");
        assert!(
            error
                .to_string()
                .contains("cannot parse retained attribute")
        );
    }
}
