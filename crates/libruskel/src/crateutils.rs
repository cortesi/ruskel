use rustdoc_types::{
    AssocItemConstraint, AssocItemConstraintKind, FunctionPointer, FunctionSignature, GenericArg,
    GenericArgs, GenericBound, GenericParamDef, GenericParamDefKind, Generics, Item, ItemEnum,
    Path, PolyTrait, Term, TraitBoundModifier, Type, Visibility, WherePredicate,
};

use crate::keywords::is_reserved_word;

/// Convenience macro to destructure `rustdoc_types::Item` variants during rendering.
macro_rules! extract_item {
    ($item:expr, $variant:path) => {
        match &$item.inner {
            $variant(inner) => inner,
            _ => panic!("Expected {}, found {:?}", stringify!($variant), $item.inner),
        }
    };
    ($item:expr, $variant:path { $($field:ident),+ }) => {
        match &$item.inner {
            $variant { $($field,)+ .. } => ($($field,)+),
            _ => panic!("Expected {}, found {:?}", stringify!($variant), $item.inner),
        }
    };
}

pub(crate) use extract_item;

/// Format documentation comments as triple-slash lines.
pub fn docs(item: &Item) -> String {
    let mut output = String::new();
    if let Some(docs) = &item.docs {
        for line in docs.lines() {
            output.push_str(&format!("/// {line}\n"));
        }
    }
    output
}

/// Render the visibility modifier for an item if it is public.
pub fn render_vis(item: &Item) -> String {
    match &item.visibility {
        Visibility::Public => "pub ".to_string(),
        _ => String::new(),
    }
}

/// Render an item name, escaping Rust keywords when necessary.
pub fn render_name(item: &Item) -> String {
    item.name.as_deref().map_or_else(
        || "?".to_string(),
        |n| {
            if is_reserved_word(n) {
                format!("r#{n}")
            } else {
                n.to_string()
            }
        },
    )
}

/// Render the generic parameter list for an item.
pub fn render_generics(generics: &Generics) -> String {
    let params: Vec<String> = generics
        .params
        .iter()
        .filter_map(render_generic_param_def)
        .collect();

    if params.is_empty() {
        String::new()
    } else {
        format!("<{}>", params.join(", "))
    }
}

/// Render an individual generic parameter definition.
pub fn render_generic_param_def(param: &GenericParamDef) -> Option<String> {
    match &param.kind {
        GenericParamDefKind::Lifetime { outlives } => {
            let outlives = if outlives.is_empty() {
                String::new()
            } else {
                format!(": {}", outlives.join(" + "))
            };
            Some(format!("{}{outlives}", param.name))
        }
        GenericParamDefKind::Type {
            bounds,
            default,
            is_synthetic,
        } => {
            if *is_synthetic {
                None
            } else {
                let bounds = if bounds.is_empty() {
                    String::new()
                } else {
                    let b = render_generic_bounds(bounds);
                    if b.is_empty() {
                        String::new()
                    } else {
                        format!(": {b}")
                    }
                };
                let default = default
                    .as_ref()
                    .map(|ty| format!(" = {}", render_type(ty)))
                    .unwrap_or_default();
                Some(format!("{}{bounds}{default}", param.name))
            }
        }
        GenericParamDefKind::Const { type_, default } => {
            let default = default
                .as_ref()
                .map(|expr| format!(" = {expr}"))
                .unwrap_or_default();
            Some(format!(
                "const {}: {}{default}",
                param.name,
                render_type(type_)
            ))
        }
    }
}

/// Render a generic bound expression into Rust syntax.
pub fn render_generic_bound(bound: &GenericBound) -> String {
    match bound {
        GenericBound::Use(_) => {
            // Omit unstable precise-capturing bounds to keep output valid
            String::new()
        }
        GenericBound::TraitBound {
            trait_,
            generic_params,
            modifier,
        } => {
            let modifier = match modifier {
                TraitBoundModifier::None => "",
                TraitBoundModifier::Maybe => "?",
                TraitBoundModifier::MaybeConst => "~const",
            };
            let poly_trait = PolyTrait {
                trait_: trait_.clone(),
                generic_params: generic_params.clone(),
            };
            match modifier {
                "" => render_poly_trait(&poly_trait),
                "~const" => format!("{modifier} {}", render_poly_trait(&poly_trait)),
                _ => format!("{modifier}{}", render_poly_trait(&poly_trait)),
            }
        }
        GenericBound::Outlives(lifetime) => lifetime.clone(),
    }
}

/// Render a type, tracking whether it is nested for parentheses handling.
pub fn render_type_inner(ty: &Type, nested: bool) -> String {
    match ty {
        Type::ResolvedPath(path) => {
            let args = path
                .args
                .as_ref()
                .map(|args| render_generic_args(args))
                .unwrap_or_default();
            format!("{}{}", path.path.replace("$crate::", ""), args)
        }
        Type::DynTrait(dyn_trait) => {
            let traits = dyn_trait
                .traits
                .iter()
                .map(render_poly_trait)
                .collect::<Vec<_>>()
                .join(" + ");
            let lifetime = dyn_trait
                .lifetime
                .as_ref()
                .map(|lt| format!(" + {lt}"))
                .unwrap_or_default();

            let inner = format!("dyn {traits}{lifetime}");
            if nested
                && (dyn_trait.lifetime.is_some()
                    || dyn_trait.traits.len() > 1
                    || traits.contains(" + "))
            {
                format!("({inner})")
            } else {
                inner
            }
        }
        Type::Generic(s) => s.clone(),
        Type::Primitive(s) => s.clone(),
        Type::FunctionPointer(f) => render_function_pointer(f),
        Type::Tuple(types) => {
            let inner = types
                .iter()
                .map(|ty| render_type_inner(ty, true))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({inner})")
        }
        Type::Slice(ty) => format!("[{}]", render_type_inner(ty, true)),
        Type::Array { type_, len } => {
            format!("[{}; {len}]", render_type_inner(type_, true))
        }
        Type::ImplTrait(bounds) => {
            let bounds_str = render_generic_bounds(bounds);
            // If we're nested (e.g., inside a reference or function parameter) and have multiple bounds
            // (indicated by presence of '+' in the bounds string), we need parentheses to avoid ambiguity
            if nested && bounds_str.contains(" + ") {
                format!("(impl {bounds_str})")
            } else {
                format!("impl {bounds_str}")
            }
        }
        Type::Infer => "_".to_string(),
        Type::RawPointer { is_mutable, type_ } => {
            let mutability = if *is_mutable { "mut" } else { "const" };
            format!("*{mutability} {}", render_type_inner(type_, true))
        }
        Type::BorrowedRef {
            lifetime,
            is_mutable,
            type_,
        } => {
            let lifetime = lifetime
                .as_ref()
                .map(|lt| format!("{lt} "))
                .unwrap_or_default();
            let mutability = if *is_mutable { "mut " } else { "" };
            format!("&{lifetime}{mutability}{}", render_type_inner(type_, true))
        }
        Type::QualifiedPath {
            name,
            args,
            self_type,
            trait_,
        } => {
            let self_type_str = render_type_inner(self_type, true);
            let args_str = args
                .as_ref()
                .map(|a| render_generic_args(a))
                .unwrap_or_default();

            if let Some(trait_) = trait_ {
                let trait_path = render_path(trait_);
                if !trait_path.is_empty() {
                    format!("<{self_type_str} as {trait_path}>::{name}{args_str}")
                } else {
                    format!("{self_type_str}::{name}{args_str}")
                }
            } else {
                format!("{self_type_str}::{name}{args_str}")
            }
        }
        Type::Pat { .. } => "/* pattern */".to_string(),
    }
}

/// Render a type without considering nesting.
pub fn render_type(ty: &Type) -> String {
    render_type_inner(ty, false)
}

/// Render a `PolyTrait` including any generic parameters.
pub fn render_poly_trait(poly_trait: &PolyTrait) -> String {
    let generic_params = if poly_trait.generic_params.is_empty() {
        String::new()
    } else {
        let params = poly_trait
            .generic_params
            .iter()
            .filter_map(render_generic_param_def)
            .collect::<Vec<_>>();

        if params.is_empty() {
            String::new()
        } else {
            format!("for<{}> ", params.join(", "))
        }
    };

    format!("{generic_params}{}", render_path(&poly_trait.trait_))
}

/// Render a type or module path into Rust source form.
pub fn render_path(path: &Path) -> String {
    let args = path
        .args
        .as_ref()
        .map(|args| render_generic_args(args))
        .unwrap_or_default();
    format!("{}{}", path.path.replace("$crate::", ""), args)
}

/// Render a function pointer signature.
fn render_function_pointer(f: &FunctionPointer) -> String {
    let args = render_function_args(&f.sig);
    format!("fn({}) {}", args, render_return_type(&f.sig))
}

/// Render a function's parameter list, including names and types.
pub fn render_function_args(decl: &FunctionSignature) -> String {
    decl.inputs
        .iter()
        .map(|(name, ty)| {
            if name == "self" {
                match ty {
                    Type::BorrowedRef { is_mutable, .. } => {
                        if *is_mutable {
                            "&mut self".to_string()
                        } else {
                            "&self".to_string()
                        }
                    }
                    Type::ResolvedPath(path) => {
                        if path.path == "Self" && path.args.is_none() {
                            "self".to_string()
                        } else {
                            format!("self: {}", render_type(ty))
                        }
                    }
                    Type::Generic(name) => {
                        if name == "Self" {
                            "self".to_string()
                        } else {
                            format!("self: {}", render_type(ty))
                        }
                    }
                    _ => format!("self: {}", render_type(ty)),
                }
            } else {
                format!("{name}: {}", render_type(ty))
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Render a function's return type, including the `->` separator when needed.
pub fn render_return_type(decl: &FunctionSignature) -> String {
    match &decl.output {
        Some(ty) => format!("-> {}", render_type(ty)),
        None => String::new(),
    }
}

/// Render concrete generic arguments used in a path.
pub fn render_generic_args(args: &GenericArgs) -> String {
    match args {
        GenericArgs::AngleBracketed { args, constraints } => {
            if args.is_empty() && constraints.is_empty() {
                String::new()
            } else {
                let args = args
                    .iter()
                    .map(render_generic_arg)
                    .collect::<Vec<_>>()
                    .join(", ");
                let bindings = constraints
                    .iter()
                    .map(render_type_constraint)
                    .collect::<Vec<_>>()
                    .join(", ");
                let all = if args.is_empty() {
                    bindings
                } else if bindings.is_empty() {
                    args
                } else {
                    format!("{args}, {bindings}")
                };
                format!("<{all}>")
            }
        }
        GenericArgs::Parenthesized { inputs, output } => {
            let inputs = inputs
                .iter()
                .map(render_type)
                .collect::<Vec<_>>()
                .join(", ");
            let output = output
                .as_ref()
                .map(|ty| format!(" -> {}", render_type(ty)))
                .unwrap_or_default();
            format!("({inputs}){output}")
        }
        GenericArgs::ReturnTypeNotation => String::new(),
    }
}

/// Render an individual generic argument such as a lifetime or type.
fn render_generic_arg(arg: &GenericArg) -> String {
    match arg {
        GenericArg::Lifetime(lt) => lt.clone(),
        GenericArg::Type(ty) => render_type(ty),
        GenericArg::Const(c) => {
            // Check if the expression contains macro variables ($ signs)
            // These come from unexpanded macros and would create invalid syntax
            if c.expr.contains('$') {
                "/* macro expression */".to_string()
            } else {
                c.expr.clone()
            }
        }
        GenericArg::Infer => "_".to_string(),
    }
}

/// Render a comma-separated list of generic bounds.
pub fn render_generic_bounds(bounds: &[GenericBound]) -> String {
    let parts: Vec<String> = bounds
        .iter()
        .map(render_generic_bound)
        .filter(|s| !s.trim().is_empty())
        .collect();
    parts.join(" + ")
}

/// Render an associated type constraint with equality or bound semantics.
fn render_type_constraint(constraint: &AssocItemConstraint) -> String {
    let binding_kind = match &constraint.binding {
        AssocItemConstraintKind::Equality(term) => format!(" = {}", render_term(term)),
        AssocItemConstraintKind::Constraint(bounds) => {
            let b = render_generic_bounds(bounds);
            if b.is_empty() {
                String::new()
            } else {
                format!(": {b}")
            }
        }
    };
    format!("{}{binding_kind}", constraint.name)
}

/// Render a `Term` appearing in associated type constraints.
fn render_term(term: &Term) -> String {
    match term {
        Term::Type(ty) => render_type(ty),
        Term::Constant(c) => c.expr.clone(),
    }
}

/// Render a `where` clause for a generics block.
pub fn render_where_clause(generics: &Generics) -> String {
    let predicates: Vec<String> = generics
        .where_predicates
        .iter()
        .filter_map(render_where_predicate)
        .collect();
    if predicates.is_empty() {
        String::new()
    } else {
        format!(" where {}", predicates.join(", "))
    }
}

/// Render a single predicate within a `where` clause.
pub fn render_where_predicate(pred: &WherePredicate) -> Option<String> {
    match pred {
        WherePredicate::BoundPredicate {
            type_,
            bounds,
            generic_params,
        } => {
            // Check if this is a synthetic type
            if let Type::Generic(_name) = type_
                && generic_params.iter().any(|param| {
                    matches!(&param.kind, GenericParamDefKind::Type { is_synthetic, .. } if *is_synthetic)
                }) {
                    return None;
                }

            let hrtb = if !generic_params.is_empty() {
                let params = generic_params
                    .iter()
                    .filter_map(render_generic_param_def)
                    .collect::<Vec<_>>()
                    .join(", ");
                if params.is_empty() {
                    String::new()
                } else {
                    format!("for<{params}> ")
                }
            } else {
                String::new()
            };

            let bounds_str = render_generic_bounds(bounds);
            if bounds_str.is_empty() {
                None
            } else {
                Some(format!("{hrtb}{}: {bounds_str}", render_type(type_)))
            }
        }
        WherePredicate::LifetimePredicate { lifetime, outlives } => {
            if outlives.is_empty() {
                Some(lifetime.clone())
            } else {
                Some(format!("{lifetime}: {}", outlives.join(" + ")))
            }
        }
        WherePredicate::EqPredicate { lhs, rhs } => {
            Some(format!("{} = {}", render_type(lhs), render_term(rhs)))
        }
    }
}

/// Render an associated type definition, including defaults and bounds.
pub fn render_associated_type(item: &Item) -> String {
    let (bounds, default) = extract_item!(item, ItemEnum::AssocType { bounds, type_ });

    let bounds_str = if !bounds.is_empty() {
        format!(": {}", render_generic_bounds(bounds))
    } else {
        String::new()
    };
    let default_str = default
        .as_ref()
        .map(|d| format!(" = {}", render_type(d)))
        .unwrap_or_default();
    format!("type {}{bounds_str}{default_str};\n", render_name(item))
}

#[cfg(test)]
mod tests {
    use rustdoc_types::{GenericBound, Id, Path, TraitBoundModifier};

    use super::*;

    #[test]
    fn test_render_generic_bound_with_const_modifier() {
        // Test ~const modifier with a simple trait
        let trait_path = Path {
            id: Id(0),
            path: "MyTrait".to_string(),
            args: None,
        };
        let bound = GenericBound::TraitBound {
            trait_: trait_path,
            generic_params: vec![],
            modifier: TraitBoundModifier::MaybeConst,
        };

        let result = render_generic_bound(&bound);
        assert_eq!(result, "~const MyTrait");
    }

    #[test]
    fn test_render_generic_bound_with_const_modifier_and_path() {
        // Test ~const modifier with a trait path
        let trait_path = Path {
            id: Id(0),
            path: "fallback::DisjointBitOr".to_string(),
            args: None,
        };
        let bound = GenericBound::TraitBound {
            trait_: trait_path,
            generic_params: vec![],
            modifier: TraitBoundModifier::MaybeConst,
        };

        let result = render_generic_bound(&bound);
        assert_eq!(result, "~const fallback::DisjointBitOr");
    }

    #[test]
    fn test_render_generic_bound_with_maybe_modifier() {
        // Test ? modifier
        let trait_path = Path {
            id: Id(0),
            path: "Sized".to_string(),
            args: None,
        };
        let bound = GenericBound::TraitBound {
            trait_: trait_path,
            generic_params: vec![],
            modifier: TraitBoundModifier::Maybe,
        };

        let result = render_generic_bound(&bound);
        assert_eq!(result, "?Sized");
    }

    #[test]
    fn test_render_generic_bound_no_modifier() {
        // Test no modifier
        let trait_path = Path {
            id: Id(0),
            path: "Debug".to_string(),
            args: None,
        };
        let bound = GenericBound::TraitBound {
            trait_: trait_path,
            generic_params: vec![],
            modifier: TraitBoundModifier::None,
        };

        let result = render_generic_bound(&bound);
        assert_eq!(result, "Debug");
    }

    #[test]
    fn test_render_generic_bounds_omits_precise_capturing() {
        use rustdoc_types::{Id, Path, PreciseCapturingArg};

        // Prepare a normal trait bound
        let sized_path = Path {
            id: Id(0),
            path: "Sized".to_string(),
            args: None,
        };
        let trait_bound = GenericBound::TraitBound {
            trait_: sized_path,
            generic_params: vec![],
            modifier: TraitBoundModifier::None,
        };

        // And a precise-capturing `use<'a, T>` bound
        let use_bound = GenericBound::Use(vec![
            PreciseCapturingArg::Lifetime("'a".to_string()),
            PreciseCapturingArg::Param("T".to_string()),
        ]);

        // When combined, only the valid trait bound should render
        let rendered = render_generic_bounds(&[trait_bound, use_bound]);
        assert_eq!(rendered, "Sized");
    }

    #[test]
    fn test_render_generic_bounds_only_precise_capturing() {
        use rustdoc_types::PreciseCapturingArg;

        let use_only = GenericBound::Use(vec![
            PreciseCapturingArg::Lifetime("'a".to_string()),
            PreciseCapturingArg::Param("T".to_string()),
        ]);

        // If only `use<...>` is present, nothing should render
        let rendered = render_generic_bounds(&[use_only]);
        assert_eq!(rendered, "");
    }
}
