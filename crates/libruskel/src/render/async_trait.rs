//! Restore source-level async signatures from complete `async_trait`
//! expansions.

use rustdoc_types::{
    AssocItemConstraintKind, Crate, Function, GenericArg, GenericArgs, GenericBound,
    GenericParamDef, GenericParamDefKind, Id, Item, ItemEnum, Path, PolyTrait, PreciseCapturingArg,
    Term, Type, WherePredicate,
};

/// Send policy shared by all expanded methods in one container.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AsyncTraitPolicy {
    /// Futures require `Send`.
    Send,
    /// Futures stay on the local thread.
    Local,
}

impl AsyncTraitPolicy {
    /// Return the source-level attribute for the matching policy.
    pub(super) fn attribute(self) -> &'static str {
        match self {
            Self::Send => "#[async_trait]\n",
            Self::Local => "#[async_trait(?Send)]\n",
        }
    }
}

/// Classify every method before a container changes. Ordinary methods do not
/// participate, but an incomplete expansion blocks the whole container.
pub(super) fn classify(crate_data: &Crate, item_ids: &[Id]) -> Option<AsyncTraitPolicy> {
    let mut policy = None;
    for id in item_ids {
        let item = crate_data.index.get(id)?;
        let ItemEnum::Function(function) = &item.inner else {
            continue;
        };
        if !has_async_trait_marker(function) {
            continue;
        }
        let current = match_expansion(crate_data, function)?;
        if policy.is_some_and(|prior| prior != current) {
            return None;
        }
        policy = Some(current);
    }
    policy
}

/// Rewrite one previously classified method. A caller must classify the whole
/// trait or impl before calling this function.
pub(super) fn rewrite(crate_data: &Crate, item: &Item, policy: AsyncTraitPolicy) -> Item {
    let mut rewritten = item.clone();
    let ItemEnum::Function(function) = &mut rewritten.inner else {
        return rewritten;
    };
    if match_expansion(crate_data, function) != Some(policy) {
        return rewritten;
    }

    let Type::ResolvedPath(pin) = function.sig.output.as_ref().expect("classified output") else {
        unreachable!("classified output is Pin")
    };
    let GenericArgs::AngleBracketed { args, .. } = pin.args.as_deref().expect("Pin arguments")
    else {
        unreachable!("classified Pin arguments are angle bracketed")
    };
    let GenericArg::Type(Type::ResolvedPath(boxed)) = &args[0] else {
        unreachable!("classified Pin contains Box")
    };
    let GenericArgs::AngleBracketed { args, .. } = boxed.args.as_deref().expect("Box arguments")
    else {
        unreachable!("classified Box arguments are angle bracketed")
    };
    let GenericArg::Type(Type::DynTrait(dynamic)) = &args[0] else {
        unreachable!("classified Box contains a trait object")
    };
    let GenericArgs::AngleBracketed { constraints, .. } = dynamic.traits[0]
        .trait_
        .args
        .as_deref()
        .expect("Future arguments")
    else {
        unreachable!("classified Future arguments are angle bracketed")
    };
    let AssocItemConstraintKind::Equality(Term::Type(output)) = &constraints[0].binding else {
        unreachable!("classified Future has an Output type")
    };
    let mut output = output.clone();
    rewrite_type(&mut output);

    function.header.is_async = true;
    function.sig.output = (output != Type::Tuple(Vec::new())).then_some(output);
    function
        .generics
        .params
        .retain(|param| !generated_lifetime(&param.name));
    function
        .generics
        .where_predicates
        .retain(|predicate| !mentions_async_trait(predicate));
    for param in &mut function.generics.params {
        rewrite_param(param);
    }
    for predicate in &mut function.generics.where_predicates {
        rewrite_predicate(predicate);
    }
    for (_, ty) in &mut function.sig.inputs {
        rewrite_type(ty);
    }
    rewritten
}

/// Test for the lifetime parameter that the macro inserts on each method.
fn has_async_trait_parameter(function: &Function) -> bool {
    function.generics.params.iter().any(|param| {
        param.name == "'async_trait" && matches!(param.kind, GenericParamDefKind::Lifetime { .. })
    })
}

/// Find possible expansions, including incomplete ones that block a rewrite.
fn has_async_trait_marker(function: &Function) -> bool {
    has_async_trait_parameter(function)
        || function
            .sig
            .output
            .as_ref()
            .is_some_and(type_mentions_async_trait)
        || function
            .generics
            .where_predicates
            .iter()
            .any(mentions_async_trait)
}

/// Check the exact boxed-future shape and return its Send policy.
fn match_expansion(crate_data: &Crate, function: &Function) -> Option<AsyncTraitPolicy> {
    if function.header.is_async || !has_async_trait_parameter(function) {
        return None;
    }
    let Type::ResolvedPath(pin) = function.sig.output.as_ref()? else {
        return None;
    };
    let [GenericArg::Type(Type::ResolvedPath(boxed))] =
        type_arguments(crate_data, pin, &["core::pin::Pin"])?
    else {
        return None;
    };
    let [GenericArg::Type(Type::DynTrait(dynamic))] =
        type_arguments(crate_data, boxed, &["alloc::boxed::Box"])?
    else {
        return None;
    };
    if dynamic.lifetime.as_deref() != Some("'async_trait") {
        return None;
    }
    let [future, rest @ ..] = dynamic.traits.as_slice() else {
        return None;
    };
    if !future.generic_params.is_empty()
        || !matches_path(
            crate_data,
            &future.trait_,
            &["core::future::future::Future"],
        )
    {
        return None;
    }
    let Some(GenericArgs::AngleBracketed { args, constraints }) = future.trait_.args.as_deref()
    else {
        return None;
    };
    if !args.is_empty()
        || constraints.len() != 1
        || constraints[0].name != "Output"
        || constraints[0].args.is_some()
    {
        return None;
    }
    if !matches!(
        constraints[0].binding,
        AssocItemConstraintKind::Equality(Term::Type(_))
    ) {
        return None;
    }
    match rest {
        [] => Some(AsyncTraitPolicy::Local),
        [send]
            if send.generic_params.is_empty()
                && send.trait_.args.is_none()
                && matches_path(crate_data, &send.trait_, &["core::marker::Send"]) =>
        {
            Some(AsyncTraitPolicy::Send)
        }
        _ => None,
    }
}

/// Return positional type arguments only for a recognized path.
fn type_arguments<'a>(
    crate_data: &Crate,
    path: &'a Path,
    names: &[&str],
) -> Option<&'a [GenericArg]> {
    if !matches_path(crate_data, path, names) {
        return None;
    }
    let GenericArgs::AngleBracketed { args, constraints } = path.args.as_deref()? else {
        return None;
    };
    constraints.is_empty().then_some(args.as_slice())
}

/// Match a macro expansion path by its definition ID.
fn matches_path(crate_data: &Crate, path: &Path, names: &[&str]) -> bool {
    crate_data
        .paths
        .get(&path.id)
        .is_some_and(|summary| names.iter().any(|name| summary.path.join("::") == *name))
}

/// Identify the lifetime names reserved by the expansion.
fn generated_lifetime(name: &str) -> bool {
    name == "'async_trait"
        || name.strip_prefix("'life").is_some_and(|digits| {
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
        })
}

/// Remove generated lifetimes from one generic parameter.
fn rewrite_param(param: &mut GenericParamDef) {
    match &mut param.kind {
        GenericParamDefKind::Lifetime { outlives } => {
            outlives.retain(|lifetime| !generated_lifetime(lifetime));
        }
        GenericParamDefKind::Type {
            bounds, default, ..
        } => {
            bounds.retain(|bound| !bound_mentions_async_trait(bound));
            for bound in bounds {
                rewrite_bound(bound);
            }
            if let Some(default) = default {
                rewrite_type(default);
            }
        }
        GenericParamDefKind::Const { type_, .. } => rewrite_type(type_),
    }
}

/// Replace generated lifetimes in a retained where predicate.
fn rewrite_predicate(predicate: &mut WherePredicate) {
    match predicate {
        WherePredicate::BoundPredicate {
            type_,
            bounds,
            generic_params,
        } => {
            rewrite_type(type_);
            for bound in bounds {
                rewrite_bound(bound);
            }
            for param in generic_params {
                rewrite_param(param);
            }
        }
        WherePredicate::LifetimePredicate { lifetime, outlives } => {
            if generated_lifetime(lifetime) {
                *lifetime = "'_".to_string();
            }
            for other in outlives {
                if generated_lifetime(other) {
                    *other = "'_".to_string();
                }
            }
        }
        WherePredicate::EqPredicate { lhs, rhs } => {
            rewrite_type(lhs);
            if let Term::Type(ty) = rhs {
                rewrite_type(ty);
            }
        }
    }
}

/// Replace generated lifetimes throughout a type tree.
fn rewrite_type(ty: &mut Type) {
    match ty {
        Type::ResolvedPath(path) => rewrite_path(path),
        Type::DynTrait(dynamic) => {
            for trait_ in &mut dynamic.traits {
                rewrite_poly_trait(trait_);
            }
            if dynamic.lifetime.as_deref().is_some_and(generated_lifetime) {
                dynamic.lifetime = Some("'_".to_string());
            }
        }
        Type::FunctionPointer(pointer) => {
            for (_, input) in &mut pointer.sig.inputs {
                rewrite_type(input);
            }
            if let Some(output) = &mut pointer.sig.output {
                rewrite_type(output);
            }
            for param in &mut pointer.generic_params {
                rewrite_param(param);
            }
        }
        Type::Tuple(items) => items.iter_mut().for_each(rewrite_type),
        Type::Slice(inner)
        | Type::Array { type_: inner, .. }
        | Type::Pat { type_: inner, .. }
        | Type::RawPointer { type_: inner, .. } => rewrite_type(inner),
        Type::ImplTrait(bounds) => bounds.iter_mut().for_each(rewrite_bound),
        Type::BorrowedRef {
            lifetime, type_, ..
        } => {
            if lifetime.as_deref().is_some_and(generated_lifetime) {
                *lifetime = None;
            }
            rewrite_type(type_);
        }
        Type::QualifiedPath {
            args,
            self_type,
            trait_,
            ..
        } => {
            if let Some(args) = args {
                rewrite_args(args);
            }
            rewrite_type(self_type);
            if let Some(trait_) = trait_ {
                rewrite_path(trait_);
            }
        }
        Type::Generic(_) | Type::Primitive(_) | Type::Infer => {}
    }
}

/// Replace generated lifetimes in path arguments.
fn rewrite_path(path: &mut Path) {
    if let Some(args) = &mut path.args {
        rewrite_args(args);
    }
}

/// Replace generated lifetimes in generic arguments and constraints.
fn rewrite_args(args: &mut GenericArgs) {
    match args {
        GenericArgs::AngleBracketed { args, constraints } => {
            for arg in args {
                match arg {
                    GenericArg::Lifetime(lifetime) if generated_lifetime(lifetime) => {
                        *lifetime = "'_".to_string();
                    }
                    GenericArg::Type(ty) => rewrite_type(ty),
                    _ => {}
                }
            }
            for constraint in constraints {
                if let Some(args) = &mut constraint.args {
                    rewrite_args(args);
                }
                match &mut constraint.binding {
                    AssocItemConstraintKind::Equality(Term::Type(ty)) => rewrite_type(ty),
                    AssocItemConstraintKind::Constraint(bounds) => {
                        bounds.iter_mut().for_each(rewrite_bound)
                    }
                    _ => {}
                }
            }
        }
        GenericArgs::Parenthesized { inputs, output } => {
            inputs.iter_mut().for_each(rewrite_type);
            if let Some(output) = output {
                rewrite_type(output);
            }
        }
        GenericArgs::ReturnTypeNotation => {}
    }
}

/// Replace generated lifetimes in a trait object bound.
fn rewrite_poly_trait(poly: &mut PolyTrait) {
    rewrite_path(&mut poly.trait_);
    for param in &mut poly.generic_params {
        rewrite_param(param);
    }
}

/// Replace generated lifetimes in a generic bound.
fn rewrite_bound(bound: &mut GenericBound) {
    match bound {
        GenericBound::TraitBound {
            trait_,
            generic_params,
            ..
        } => {
            rewrite_path(trait_);
            for param in generic_params {
                rewrite_param(param);
            }
        }
        GenericBound::Outlives(lifetime) if generated_lifetime(lifetime) => {
            *lifetime = "'_".to_string();
        }
        GenericBound::Use(args) => {
            for arg in args {
                if let PreciseCapturingArg::Lifetime(lifetime) = arg
                    && generated_lifetime(lifetime)
                {
                    *lifetime = "'_".to_string();
                }
            }
        }
        _ => {}
    }
}

/// Detect the macro lifetime anywhere in a where predicate.
fn mentions_async_trait(predicate: &WherePredicate) -> bool {
    match predicate {
        WherePredicate::BoundPredicate {
            type_,
            bounds,
            generic_params,
        } => {
            type_mentions_async_trait(type_)
                || bounds.iter().any(bound_mentions_async_trait)
                || generic_params.iter().any(param_mentions_async_trait)
        }
        WherePredicate::LifetimePredicate { lifetime, outlives } => {
            lifetime == "'async_trait" || outlives.iter().any(|name| name == "'async_trait")
        }
        WherePredicate::EqPredicate { lhs, rhs } => {
            type_mentions_async_trait(lhs)
                || matches!(rhs, Term::Type(ty) if type_mentions_async_trait(ty))
        }
    }
}

/// Detect the macro lifetime anywhere in a generic parameter.
fn param_mentions_async_trait(param: &GenericParamDef) -> bool {
    param.name == "'async_trait"
        || match &param.kind {
            GenericParamDefKind::Lifetime { outlives } => {
                outlives.iter().any(|name| name == "'async_trait")
            }
            GenericParamDefKind::Type {
                bounds, default, ..
            } => {
                bounds.iter().any(bound_mentions_async_trait)
                    || default.as_ref().is_some_and(type_mentions_async_trait)
            }
            GenericParamDefKind::Const { type_, .. } => type_mentions_async_trait(type_),
        }
}

/// Detect the macro lifetime anywhere in a generic bound.
fn bound_mentions_async_trait(bound: &GenericBound) -> bool {
    match bound {
        GenericBound::TraitBound {
            trait_,
            generic_params,
            ..
        } => {
            path_mentions_async_trait(trait_)
                || generic_params.iter().any(param_mentions_async_trait)
        }
        GenericBound::Outlives(name) => name == "'async_trait",
        GenericBound::Use(args) => args.iter().any(
            |arg| matches!(arg, PreciseCapturingArg::Lifetime(name) if name == "'async_trait"),
        ),
    }
}

/// Detect the macro lifetime anywhere in a type tree.
fn type_mentions_async_trait(ty: &Type) -> bool {
    match ty {
        Type::ResolvedPath(path) => path_mentions_async_trait(path),
        Type::DynTrait(dynamic) => {
            dynamic.lifetime.as_deref() == Some("'async_trait")
                || dynamic.traits.iter().any(|poly| {
                    path_mentions_async_trait(&poly.trait_)
                        || poly.generic_params.iter().any(param_mentions_async_trait)
                })
        }
        Type::FunctionPointer(pointer) => {
            pointer
                .sig
                .inputs
                .iter()
                .any(|(_, ty)| type_mentions_async_trait(ty))
                || pointer
                    .sig
                    .output
                    .as_ref()
                    .is_some_and(type_mentions_async_trait)
                || pointer
                    .generic_params
                    .iter()
                    .any(param_mentions_async_trait)
        }
        Type::Tuple(items) => items.iter().any(type_mentions_async_trait),
        Type::Slice(inner)
        | Type::Array { type_: inner, .. }
        | Type::Pat { type_: inner, .. }
        | Type::RawPointer { type_: inner, .. } => type_mentions_async_trait(inner),
        Type::ImplTrait(bounds) => bounds.iter().any(bound_mentions_async_trait),
        Type::BorrowedRef {
            lifetime, type_, ..
        } => lifetime.as_deref() == Some("'async_trait") || type_mentions_async_trait(type_),
        Type::QualifiedPath {
            args,
            self_type,
            trait_,
            ..
        } => {
            args.as_ref()
                .is_some_and(|args| args_mention_async_trait(args))
                || type_mentions_async_trait(self_type)
                || trait_.as_ref().is_some_and(path_mentions_async_trait)
        }
        Type::Generic(_) | Type::Primitive(_) | Type::Infer => false,
    }
}

/// Detect the macro lifetime in a path's generic arguments.
fn path_mentions_async_trait(path: &Path) -> bool {
    path.args
        .as_ref()
        .is_some_and(|args| args_mention_async_trait(args))
}

/// Detect the macro lifetime in generic arguments or constraints.
fn args_mention_async_trait(args: &GenericArgs) -> bool {
    match args {
        GenericArgs::AngleBracketed { args, constraints } => {
            args.iter().any(|arg| match arg {
                GenericArg::Lifetime(name) => name == "'async_trait",
                GenericArg::Type(ty) => type_mentions_async_trait(ty),
                _ => false,
            }) || constraints.iter().any(|constraint| {
                constraint
                    .args
                    .as_ref()
                    .is_some_and(|args| args_mention_async_trait(args))
                    || match &constraint.binding {
                        AssocItemConstraintKind::Equality(Term::Type(ty)) => {
                            type_mentions_async_trait(ty)
                        }
                        AssocItemConstraintKind::Constraint(bounds) => {
                            bounds.iter().any(bound_mentions_async_trait)
                        }
                        _ => false,
                    }
            })
        }
        GenericArgs::Parenthesized { inputs, output } => {
            inputs.iter().any(type_mentions_async_trait)
                || output.as_ref().is_some_and(type_mentions_async_trait)
        }
        GenericArgs::ReturnTypeNotation => false,
    }
}
