use rustdoc_types::{
    Abi, AssocItemConstraint, AssocItemConstraintKind, FunctionHeader, FunctionPointer,
    FunctionSignature, GenericArg, GenericArgs, GenericBound, GenericParamDef, GenericParamDefKind,
    Generics, Item, ItemEnum, Path, PolyTrait, Term, TraitBoundModifier, Type, Visibility,
    WherePredicate,
};

use crate::keywords::is_reserved_word;

/// Convenience macro to destructure `rustdoc_types::Item` variants during
/// rendering.
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

/// Fallible variant of `extract_item!` that returns a `RuskelError` instead of
/// panicking.
macro_rules! try_extract_item {
    ($item:expr, $variant:path) => {
        match &$item.inner {
            $variant(inner) => Ok(inner),
            _ => Err(RuskelError::Generate(format!(
                "Expected {}, found {:?}",
                stringify!($variant),
                $item.inner
            ))),
        }
    };
    ($item:expr, $variant:path { $($field:ident),+ }) => {
        match &$item.inner {
            $variant { $($field,)+ .. } => Ok(($($field,)+)),
            _ => Err(RuskelError::Generate(format!(
                "Expected {}, found {:?}",
                stringify!($variant),
                $item.inner
            ))),
        }
    };
}

pub(crate) use try_extract_item;

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

/// Render an identifier, escaping Rust keywords when necessary.
pub fn render_identifier(ident: &str) -> String {
    if is_reserved_word(ident) {
        format!("r#{ident}")
    } else {
        ident.to_string()
    }
}

/// Render an item name, escaping Rust keywords when necessary.
pub fn render_name(item: &Item) -> String {
    item.name
        .as_deref()
        .map_or_else(|| "?".to_string(), render_identifier)
}

/// Render a rustdoc-provided expression into valid Rust source.
///
/// rustdoc sometimes emits associated-item paths with generic arguments as
/// `Type<T>::ASSOC`, which is not valid in expression position. This helper
/// normalizes those paths to `Type::<T>::ASSOC`.
pub fn render_expression(expr: &str) -> String {
    fix_missing_turbofish(expr)
}

/// Return whether `c` can appear in a Rust identifier continuation position.
fn is_ident_continue(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

/// Return the index of the closest non-whitespace character before `before`.
fn previous_non_whitespace(chars: &[char], before: usize) -> Option<usize> {
    if before == 0 {
        return None;
    }
    let mut i = before - 1;
    loop {
        if !chars[i].is_whitespace() {
            return Some(i);
        }
        if i == 0 {
            return None;
        }
        i -= 1;
    }
}

/// Return the index of the first non-whitespace character at or after `start`.
fn next_non_whitespace(chars: &[char], start: usize) -> Option<usize> {
    let mut i = start;
    while i < chars.len() {
        if !chars[i].is_whitespace() {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Find the matching `>` for the `<` located at `start`.
fn find_matching_angle(chars: &[char], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = start;
    while i < chars.len() {
        match chars[i] {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Determine whether a `<...>::` segment should be rewritten to turbofish form.
fn should_insert_turbofish(chars: &[char], angle_start: usize, angle_end: usize) -> bool {
    let Some(prev) = previous_non_whitespace(chars, angle_start) else {
        return false;
    };

    // We only normalize compact path segments like `Type<T>::ASSOC`,
    // not spaced comparisons like `a < b`.
    if prev + 1 != angle_start {
        return false;
    }

    let prev_char = chars[prev];
    // Already in turbofish form (`Type::<T>::ASSOC`) or malformed.
    if prev_char == ':' {
        return false;
    }
    if !(is_ident_continue(prev_char) || prev_char == '>') {
        return false;
    }

    let Some(next) = next_non_whitespace(chars, angle_end + 1) else {
        return false;
    };

    next + 1 < chars.len() && chars[next] == ':' && chars[next + 1] == ':'
}

/// Normalize invalid rustdoc expression paths such as `Type<T>::ASSOC`.
fn fix_missing_turbofish(expr: &str) -> String {
    if !expr.contains('<') || !expr.contains(">::") {
        return expr.to_string();
    }

    let chars: Vec<char> = expr.chars().collect();
    let mut out = String::with_capacity(expr.len());
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '<'
            && let Some(end) = find_matching_angle(&chars, i)
            && should_insert_turbofish(&chars, i, end)
        {
            out.push_str("::");
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Render the generic parameter list for an item.
pub fn render_generics(generics: &Generics) -> String {
    render_generic_params(&generics.params)
}

/// Render a generic parameter list.
pub fn render_generic_params(params: &[GenericParamDef]) -> String {
    let params: Vec<String> = params.iter().filter_map(render_generic_param_def).collect();

    if params.is_empty() {
        String::new()
    } else {
        format!("<{}>", params.join(", "))
    }
}

/// Render an ABI qualifier, omitting Rust's default ABI.
pub fn render_abi(abi: &Abi) -> String {
    match abi {
        Abi::Rust => String::new(),
        Abi::C { unwind } => format!("extern \"C{}\"", if *unwind { "-unwind" } else { "" }),
        Abi::Cdecl { unwind } => {
            format!("extern \"cdecl{}\"", if *unwind { "-unwind" } else { "" })
        }
        Abi::Stdcall { unwind } => {
            format!("extern \"stdcall{}\"", if *unwind { "-unwind" } else { "" })
        }
        Abi::Fastcall { unwind } => {
            format!(
                "extern \"fastcall{}\"",
                if *unwind { "-unwind" } else { "" }
            )
        }
        Abi::Aapcs { unwind } => {
            format!("extern \"aapcs{}\"", if *unwind { "-unwind" } else { "" })
        }
        Abi::Win64 { unwind } => {
            format!("extern \"win64{}\"", if *unwind { "-unwind" } else { "" })
        }
        Abi::SysV64 { unwind } => {
            format!("extern \"sysv64{}\"", if *unwind { "-unwind" } else { "" })
        }
        Abi::System { unwind } => {
            format!("extern \"system{}\"", if *unwind { "-unwind" } else { "" })
        }
        Abi::Other(name) => format!("extern {name:?}"),
    }
}

/// Render function qualifiers in Rust's source order.
pub fn render_function_qualifiers(header: &FunctionHeader) -> String {
    let mut qualifiers = Vec::new();
    if header.is_const {
        qualifiers.push("const".to_string());
    }
    if header.is_async {
        qualifiers.push("async".to_string());
    }
    if header.is_unsafe {
        qualifiers.push("unsafe".to_string());
    }
    let abi = render_abi(&header.abi);
    if !abi.is_empty() {
        qualifiers.push(abi);
    }
    qualifiers.join(" ")
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
                Some(format!(
                    "{}{bounds}{default}",
                    render_identifier(&param.name)
                ))
            }
        }
        GenericParamDefKind::Const { type_, default } => {
            let default = default
                .as_ref()
                .map(|expr| format!(" = {}", render_expression(expr)))
                .unwrap_or_default();
            Some(format!(
                "const {}: {}{default}",
                render_identifier(&param.name),
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
            if types.len() == 1 {
                format!("({inner},)")
            } else {
                format!("({inner})")
            }
        }
        Type::Slice(ty) => format!("[{}]", render_type_inner(ty, true)),
        Type::Array { type_, len } => {
            format!("[{}; {len}]", render_type_inner(type_, true))
        }
        Type::ImplTrait(bounds) => {
            let bounds_str = render_generic_bounds(bounds);
            // If we're nested (e.g., inside a reference or function parameter) and have
            // multiple bounds (indicated by presence of '+' in the bounds
            // string), we need parentheses to avoid ambiguity
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
    let mut signature = String::new();
    let generics = render_generic_params(&f.generic_params);
    if !generics.is_empty() {
        signature.push_str("for");
        signature.push_str(&generics);
        signature.push(' ');
    }
    let qualifiers = render_function_qualifiers(&f.header);
    if !qualifiers.is_empty() {
        signature.push_str(&qualifiers);
        signature.push(' ');
    }
    signature.push_str("fn(");
    signature.push_str(&render_function_args(&f.sig));
    signature.push(')');
    let return_type = render_return_type(&f.sig);
    if !return_type.is_empty() {
        signature.push(' ');
        signature.push_str(&return_type);
    }
    signature
}

/// Render a function's parameter list, including names and types.
pub fn render_function_args(decl: &FunctionSignature) -> String {
    let mut args = decl
        .inputs
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
                format!("{}: {}", render_identifier(name), render_type(ty))
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    if decl.is_c_variadic {
        if !args.is_empty() {
            args.push_str(", ");
        }
        args.push_str("...");
    }
    args
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
                render_expression(&c.expr)
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
    format!("{}{binding_kind}", render_identifier(&constraint.name))
}

/// Render a `Term` appearing in associated type constraints.
fn render_term(term: &Term) -> String {
    match term {
        Term::Type(ty) => render_type(ty),
        Term::Constant(c) => render_expression(&c.expr),
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

/// Render an associated type signature, including generics, defaults, and
/// bounds.
pub fn render_associated_type(item: &Item) -> String {
    let (generics, bounds, default) = extract_item!(
        item,
        ItemEnum::AssocType {
            generics,
            bounds,
            type_
        }
    );

    let mut signature = format!("type {}{}", render_name(item), render_generics(generics));
    let bounds_str = if !bounds.is_empty() {
        format!(": {}", render_generic_bounds(bounds))
    } else {
        String::new()
    };
    signature.push_str(&bounds_str);
    if let Some(default) = default {
        signature.push_str(" = ");
        signature.push_str(&render_type(default));
    }
    signature.push_str(&render_where_clause(generics));
    signature
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use rustdoc_types::{
        Abi, FunctionHeader, GenericBound, GenericParamDef, GenericParamDefKind, Generics, Id,
        Item, ItemEnum, Path, TraitBoundModifier, Type, Visibility, WherePredicate,
    };

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

    #[test]
    fn test_render_expression_inserts_turbofish_for_assoc_paths() {
        assert_eq!(
            render_expression("Date<Utc>::MAX_UTC"),
            "Date::<Utc>::MAX_UTC"
        );
        assert_eq!(
            render_expression("chrono::Date<crate::offset::Utc>::MAX_UTC"),
            "chrono::Date::<crate::offset::Utc>::MAX_UTC"
        );
    }

    #[test]
    fn test_render_expression_preserves_existing_turbofish() {
        assert_eq!(
            render_expression("Date::<Utc>::MAX_UTC"),
            "Date::<Utc>::MAX_UTC"
        );
    }

    #[test]
    fn render_abi_covers_known_and_custom_variants() {
        assert_eq!(render_abi(&Abi::Rust), "");
        assert_eq!(render_abi(&Abi::C { unwind: false }), "extern \"C\"");
        assert_eq!(render_abi(&Abi::C { unwind: true }), "extern \"C-unwind\"");
        assert_eq!(
            render_abi(&Abi::Cdecl { unwind: false }),
            "extern \"cdecl\""
        );
        assert_eq!(
            render_abi(&Abi::Cdecl { unwind: true }),
            "extern \"cdecl-unwind\""
        );
        assert_eq!(
            render_abi(&Abi::Stdcall { unwind: false }),
            "extern \"stdcall\""
        );
        assert_eq!(
            render_abi(&Abi::Stdcall { unwind: true }),
            "extern \"stdcall-unwind\""
        );
        assert_eq!(
            render_abi(&Abi::Fastcall { unwind: false }),
            "extern \"fastcall\""
        );
        assert_eq!(
            render_abi(&Abi::Fastcall { unwind: true }),
            "extern \"fastcall-unwind\""
        );
        assert_eq!(
            render_abi(&Abi::Aapcs { unwind: false }),
            "extern \"aapcs\""
        );
        assert_eq!(
            render_abi(&Abi::Aapcs { unwind: true }),
            "extern \"aapcs-unwind\""
        );
        assert_eq!(
            render_abi(&Abi::Win64 { unwind: false }),
            "extern \"win64\""
        );
        assert_eq!(
            render_abi(&Abi::Win64 { unwind: true }),
            "extern \"win64-unwind\""
        );
        assert_eq!(
            render_abi(&Abi::SysV64 { unwind: false }),
            "extern \"sysv64\""
        );
        assert_eq!(
            render_abi(&Abi::SysV64 { unwind: true }),
            "extern \"sysv64-unwind\""
        );
        assert_eq!(
            render_abi(&Abi::System { unwind: false }),
            "extern \"system\""
        );
        assert_eq!(
            render_abi(&Abi::System { unwind: true }),
            "extern \"system-unwind\""
        );
        assert_eq!(
            render_abi(&Abi::Other("custom\"abi".into())),
            r#"extern "custom\"abi""#
        );
    }

    #[test]
    fn render_function_qualifiers_use_rust_order() {
        let header = FunctionHeader {
            is_const: true,
            is_unsafe: true,
            is_async: true,
            abi: Abi::C { unwind: false },
        };
        assert_eq!(
            render_function_qualifiers(&header),
            "const async unsafe extern \"C\""
        );
    }

    #[test]
    fn render_function_pointer_preserves_qualifiers_generics_and_variadics() {
        let pointer = FunctionPointer {
            sig: FunctionSignature {
                inputs: vec![(
                    "value".into(),
                    Type::BorrowedRef {
                        lifetime: Some("'a".into()),
                        is_mutable: false,
                        type_: Box::new(Type::Primitive("i32".into())),
                    },
                )],
                output: Some(Type::Primitive("i32".into())),
                is_c_variadic: true,
            },
            generic_params: vec![GenericParamDef {
                name: "'a".into(),
                kind: GenericParamDefKind::Lifetime {
                    outlives: Vec::new(),
                },
            }],
            header: FunctionHeader {
                is_const: false,
                is_unsafe: true,
                is_async: false,
                abi: Abi::C { unwind: true },
            },
        };

        assert_eq!(
            render_type(&Type::FunctionPointer(Box::new(pointer))),
            "for<'a> unsafe extern \"C-unwind\" fn(value: &'a i32, ...) -> i32"
        );
        assert_eq!(
            render_function_args(&FunctionSignature {
                inputs: Vec::new(),
                output: None,
                is_c_variadic: true,
            }),
            "..."
        );
    }

    #[test]
    fn render_associated_type_preserves_generics_defaults_bounds_and_where_clause() {
        let clone_bound = GenericBound::TraitBound {
            trait_: Path {
                id: Id(2),
                path: "Clone".into(),
                args: None,
            },
            generic_params: Vec::new(),
            modifier: TraitBoundModifier::None,
        };
        let item = Item {
            id: Id(1),
            crate_id: 0,
            name: Some("Container".into()),
            span: None,
            visibility: Visibility::Default,
            docs: None,
            links: HashMap::new(),
            attrs: Vec::new(),
            deprecation: None,
            stability: None,
            const_stability: None,
            inner: ItemEnum::AssocType {
                generics: Generics {
                    params: vec![GenericParamDef {
                        name: "T".into(),
                        kind: GenericParamDefKind::Type {
                            bounds: Vec::new(),
                            default: None,
                            is_synthetic: false,
                        },
                    }],
                    where_predicates: vec![WherePredicate::BoundPredicate {
                        type_: Type::Generic("T".into()),
                        bounds: vec![clone_bound.clone()],
                        generic_params: Vec::new(),
                    }],
                },
                bounds: vec![clone_bound],
                type_: Some(Type::ResolvedPath(Path {
                    id: Id(3),
                    path: "Vec".into(),
                    args: None,
                })),
                default_unstable: None,
            },
        };

        assert_eq!(
            render_associated_type(&item),
            "type Container<T>: Clone = Vec where T: Clone"
        );
    }

    #[test]
    fn render_tuple_types_preserves_singleton_tuple_syntax() {
        assert_eq!(render_type(&Type::Tuple(Vec::new())), "()");
        assert_eq!(
            render_type(&Type::Tuple(vec![Type::Primitive("u32".into())])),
            "(u32,)"
        );
        assert_eq!(
            render_type(&Type::Tuple(vec![
                Type::Primitive("u32".into()),
                Type::Primitive("u64".into()),
            ])),
            "(u32, u64)"
        );
        let nested = Type::Tuple(vec![Type::Tuple(vec![Type::Primitive("u32".into())])]);
        assert_eq!(render_type(&nested), "((u32,),)");
    }
}
