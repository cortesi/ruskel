use rustdoc_types::{
    Crate, DynTrait, FnDecl, FunctionPointer, GenericArg, GenericArgs, GenericBound,
    GenericParamDef, GenericParamDefKind, Generics, Item, ItemEnum, Path, PolyTrait, Term,
    TraitBoundModifier, Type, TypeBinding, TypeBindingKind, Visibility, WherePredicate,
};

pub struct Renderer;

impl Renderer {
    pub fn render(crate_data: &Crate) -> String {
        if let Some(root_item) = crate_data.index.get(&crate_data.root) {
            Self::render_item(root_item, crate_data, 0)
        } else {
            String::new()
        }
    }

    fn render_item(item: &Item, crate_data: &Crate, indent: usize) -> String {
        match &item.inner {
            ItemEnum::Module(_) => Self::render_module(item, crate_data, indent),
            ItemEnum::Function(_) => Self::render_function(item, indent),
            // Add other item types as needed
            _ => String::new(),
        }
    }

    fn render_module(item: &Item, crate_data: &Crate, indent: usize) -> String {
        let indent_str = "    ".repeat(indent);
        let mut output = format!(
            "{}mod {} {{\n",
            indent_str,
            item.name.as_deref().unwrap_or("?")
        );

        if let ItemEnum::Module(module) = &item.inner {
            for item_id in &module.items {
                if let Some(item) = crate_data.index.get(item_id) {
                    output.push_str(&Self::render_item(item, crate_data, indent + 1));
                }
            }
        }

        output.push_str(&format!("{}}}\n", indent_str));
        output
    }

    fn render_function(item: &Item, indent: usize) -> String {
        let indent_str = "    ".repeat(indent);
        let visibility = match &item.visibility {
            Visibility::Public => "pub ",
            _ => "",
        };

        let mut output = String::new();

        // Add doc comment if present
        if let Some(docs) = &item.docs {
            for line in docs.lines() {
                output.push_str(&format!("{}/// {}\n", indent_str, line));
            }
        }

        if let ItemEnum::Function(function) = &item.inner {
            let generics = Self::render_generics(&function.generics);
            let args = Self::render_function_args(&function.decl);
            let return_type = Self::render_return_type(&function.decl);
            let where_clause = Self::render_where_clause(&function.generics);

            output.push_str(&format!(
                "{}{}fn {}{}({}){}{} {{",
                indent_str,
                visibility,
                item.name.as_deref().unwrap_or("?"),
                generics,
                args,
                if return_type.is_empty() {
                    String::new()
                } else {
                    format!(" -> {}", return_type)
                },
                where_clause
            ));
        }

        output.push_str(&format!("\n{}}}\n", indent_str));
        output
    }

    fn render_generics(generics: &Generics) -> String {
        if generics.params.is_empty() {
            String::new()
        } else {
            let params: Vec<String> = generics
                .params
                .iter()
                .map(Self::render_generic_param_def)
                .collect();
            format!("<{}>", params.join(", "))
        }
    }

    fn render_where_clause(generics: &Generics) -> String {
        if generics.where_predicates.is_empty() {
            String::new()
        } else {
            let predicates: Vec<String> = generics
                .where_predicates
                .iter()
                .map(Self::render_where_predicate)
                .collect();
            format!(" where {}", predicates.join(", "))
        }
    }

    fn render_where_predicate(pred: &WherePredicate) -> String {
        match pred {
            WherePredicate::BoundPredicate {
                type_,
                bounds,
                generic_params,
            } => {
                let hrtb = if !generic_params.is_empty() {
                    let params = generic_params
                        .iter()
                        .map(Self::render_generic_param_def)
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("for<{}> ", params)
                } else {
                    String::new()
                };

                let bounds_str = bounds
                    .iter()
                    .map(Self::render_generic_bound)
                    .collect::<Vec<_>>()
                    .join(" + ");

                format!("{}{}: {}", hrtb, Self::render_type(type_), bounds_str)
            }
            WherePredicate::LifetimePredicate { lifetime, outlives } => {
                if outlives.is_empty() {
                    lifetime.clone()
                } else {
                    format!("{}: {}", lifetime, outlives.join(" + "))
                }
            }
            WherePredicate::EqPredicate { lhs, rhs } => {
                format!("{} = {}", Self::render_type(lhs), Self::render_term(rhs))
            }
        }
    }

    fn render_function_args(decl: &FnDecl) -> String {
        decl.inputs
            .iter()
            .map(|(name, ty)| format!("{}: {}", name, Self::render_type(ty)))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn render_return_type(decl: &FnDecl) -> String {
        match &decl.output {
            Some(ty) => Self::render_type(ty),
            None => String::new(),
        }
    }

    fn render_type(ty: &Type) -> String {
        match ty {
            Type::ResolvedPath(path) => Self::render_path(path),
            Type::DynTrait(dyn_trait) => Self::render_dyn_trait(dyn_trait),
            Type::Generic(s) => s.clone(),
            Type::Primitive(s) => s.clone(),
            Type::FunctionPointer(f) => Self::render_function_pointer(f),
            Type::Tuple(types) => {
                let inner = types
                    .iter()
                    .map(Self::render_type)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("({})", inner)
            }
            Type::Slice(ty) => format!("[{}]", Self::render_type(ty)),
            Type::Array { type_, len } => format!("[{}; {}]", Self::render_type(type_), len),
            Type::ImplTrait(bounds) => {
                let bounds = bounds
                    .iter()
                    .map(Self::render_generic_bound)
                    .collect::<Vec<_>>()
                    .join(" + ");
                format!("impl {}", bounds)
            }
            Type::Infer => "_".to_string(),
            Type::RawPointer { mutable, type_ } => {
                let mutability = if *mutable { "mut" } else { "const" };
                format!("*{} {}", mutability, Self::render_type(type_))
            }
            Type::BorrowedRef {
                lifetime,
                mutable,
                type_,
            } => {
                let lifetime = lifetime
                    .as_ref()
                    .map(|lt| format!("{} ", lt))
                    .unwrap_or_default();
                let mutability = if *mutable { "mut " } else { "" };
                format!("&{}{}{}", lifetime, mutability, Self::render_type(type_))
            }
            Type::QualifiedPath {
                name,
                args: _,
                self_type,
                trait_,
            } => {
                let trait_part = trait_
                    .as_ref()
                    .map(|t| format!(" as {}", Self::render_path(t)))
                    .unwrap_or_default();
                format!("<{}{}>::{}", Self::render_type(self_type), trait_part, name)
            }
            Type::Pat { .. } => "/* pattern */".to_string(), // This is a special case, might need more specific handling
        }
    }

    fn render_path(path: &Path) -> String {
        let args = path
            .args
            .as_ref()
            .map(|args| Self::render_generic_args(args))
            .unwrap_or_default();
        format!("{}{}", path.name, args)
    }

    fn render_dyn_trait(dyn_trait: &DynTrait) -> String {
        let traits = dyn_trait
            .traits
            .iter()
            .map(Self::render_poly_trait)
            .collect::<Vec<_>>()
            .join(" + ");
        let lifetime = dyn_trait
            .lifetime
            .as_ref()
            .map(|lt| format!(" + {}", lt))
            .unwrap_or_default();
        format!("dyn {}{}", traits, lifetime)
    }

    fn render_function_pointer(f: &FunctionPointer) -> String {
        let args = Self::render_function_args(&f.decl);
        let return_type = Self::render_return_type(&f.decl);
        if return_type.is_empty() {
            format!("fn({})", args)
        } else {
            format!("fn({}) -> {}", args, return_type)
        }
    }

    fn render_generic_args(args: &GenericArgs) -> String {
        match args {
            GenericArgs::AngleBracketed { args, bindings } => {
                let args = args
                    .iter()
                    .map(Self::render_generic_arg)
                    .collect::<Vec<_>>()
                    .join(", ");
                let bindings = bindings
                    .iter()
                    .map(Self::render_type_binding)
                    .collect::<Vec<_>>()
                    .join(", ");
                let all = if bindings.is_empty() {
                    args
                } else {
                    format!("{}, {}", args, bindings)
                };
                format!("<{}>", all)
            }
            GenericArgs::Parenthesized { inputs, output } => {
                let inputs = inputs
                    .iter()
                    .map(Self::render_type)
                    .collect::<Vec<_>>()
                    .join(", ");
                let output = output
                    .as_ref()
                    .map(|ty| format!(" -> {}", Self::render_type(ty)))
                    .unwrap_or_default();
                format!("({}){}", inputs, output)
            }
        }
    }

    fn render_generic_arg(arg: &GenericArg) -> String {
        match arg {
            GenericArg::Lifetime(lt) => lt.clone(),
            GenericArg::Type(ty) => Self::render_type(ty),
            GenericArg::Const(c) => c.expr.clone(),
            GenericArg::Infer => "_".to_string(),
        }
    }

    fn render_type_binding(binding: &TypeBinding) -> String {
        let binding_kind = match &binding.binding {
            TypeBindingKind::Equality(term) => format!(" = {}", Self::render_term(term)),
            TypeBindingKind::Constraint(bounds) => {
                let bounds = bounds
                    .iter()
                    .map(Self::render_generic_bound)
                    .collect::<Vec<_>>()
                    .join(" + ");
                format!(": {}", bounds)
            }
        };
        format!("{}{}", binding.name, binding_kind)
    }

    fn render_term(term: &Term) -> String {
        match term {
            Term::Type(ty) => Self::render_type(ty),
            Term::Constant(c) => c.expr.clone(),
        }
    }

    fn render_generic_bound(bound: &GenericBound) -> String {
        match bound {
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
                let generic_params = if generic_params.is_empty() {
                    String::new()
                } else {
                    let params = generic_params
                        .iter()
                        .map(Self::render_generic_param_def)
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("for<{}> ", params)
                };
                format!(
                    "{}{}{}",
                    modifier,
                    generic_params,
                    Self::render_path(trait_)
                )
            }
            GenericBound::Outlives(lifetime) => lifetime.clone(),
        }
    }

    fn render_generic_param_def(param: &GenericParamDef) -> String {
        match &param.kind {
            GenericParamDefKind::Lifetime { outlives } => {
                let outlives = if outlives.is_empty() {
                    String::new()
                } else {
                    format!(": {}", outlives.join(" + "))
                };
                format!("{}{}", param.name, outlives)
            }
            GenericParamDefKind::Type {
                bounds, default, ..
            } => {
                let bounds = if bounds.is_empty() {
                    String::new()
                } else {
                    format!(
                        ": {}",
                        bounds
                            .iter()
                            .map(Self::render_generic_bound)
                            .collect::<Vec<_>>()
                            .join(" + ")
                    )
                };
                let default = default
                    .as_ref()
                    .map(|ty| format!(" = {}", Self::render_type(ty)))
                    .unwrap_or_default();
                format!("{}{}{}", param.name, bounds, default)
            }
            GenericParamDefKind::Const { type_, default } => {
                let default = default
                    .as_ref()
                    .map(|expr| format!(" = {}", expr))
                    .unwrap_or_default();
                format!(
                    "const {}: {}{}",
                    param.name,
                    Self::render_type(type_),
                    default
                )
            }
        }
    }

    fn render_poly_trait(poly_trait: &PolyTrait) -> String {
        let generic_params = if poly_trait.generic_params.is_empty() {
            String::new()
        } else {
            let params = poly_trait
                .generic_params
                .iter()
                .map(Self::render_generic_param_def)
                .collect::<Vec<_>>()
                .join(", ");
            format!("for<{}> ", params)
        };
        format!(
            "{}{}",
            generic_params,
            Self::render_path(&poly_trait.trait_)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustdoc_types::{Abi, FnDecl, Function, Generics, Header, Id, Module, Type as RustDocType};
    use std::collections::HashMap;

    fn create_function(
        id: &str,
        name: &str,
        visibility: Visibility,
        inputs: Vec<(String, RustDocType)>,
        output: Option<RustDocType>,
        docs: Option<String>,
    ) -> Item {
        Item {
            id: Id(id.to_string()),
            crate_id: 0,
            name: Some(name.to_string()),
            span: None,
            visibility,
            docs,
            links: HashMap::new(),
            attrs: vec![],
            deprecation: None,
            inner: ItemEnum::Function(Function {
                decl: FnDecl {
                    inputs,
                    output,
                    c_variadic: false,
                },
                generics: Generics {
                    params: vec![],
                    where_predicates: vec![],
                },
                header: Header {
                    const_: false,
                    unsafe_: false,
                    async_: false,
                    abi: Abi::Rust,
                },
                has_body: true,
            }),
        }
    }

    fn create_module(id: &str, name: &str, visibility: Visibility, items: Vec<Id>) -> Item {
        Item {
            id: Id(id.to_string()),
            crate_id: 0,
            name: Some(name.to_string()),
            span: None,
            visibility,
            docs: None,
            links: HashMap::new(),
            attrs: vec![],
            deprecation: None,
            inner: ItemEnum::Module(Module {
                is_crate: false,
                items,
                is_stripped: false,
            }),
        }
    }

    #[test]
    fn test_render_public_function() {
        let function = create_function(
            "test_function",
            "test_function",
            Visibility::Public,
            vec![],
            None,
            None,
        );
        let output = Renderer::render_function(&function, 0);
        assert_eq!(output, "pub fn test_function() {\n}\n");
    }

    #[test]
    fn test_render_private_function() {
        let function = create_function(
            "private_function",
            "private_function",
            Visibility::Default,
            vec![],
            None,
            None,
        );
        let output = Renderer::render_function(&function, 0);
        assert_eq!(output, "fn private_function() {\n}\n");
    }

    #[test]
    fn test_render_function_with_args_and_return() {
        let function = create_function(
            "complex_function",
            "complex_function",
            Visibility::Public,
            vec![
                (
                    "arg1".to_string(),
                    RustDocType::Primitive("i32".to_string()),
                ),
                (
                    "arg2".to_string(),
                    RustDocType::Primitive("String".to_string()),
                ),
            ],
            Some(RustDocType::Primitive("bool".to_string())),
            None,
        );
        let output = Renderer::render_function(&function, 0);
        assert_eq!(
            output,
            "pub fn complex_function(arg1: i32, arg2: String) -> bool {\n}\n"
        );
    }

    #[test]
    fn test_render_function_with_docs() {
        let function = create_function(
            "documented_function",
            "documented_function",
            Visibility::Public,
            vec![],
            None,
            Some("This is a documented function.".to_string()),
        );
        let output = Renderer::render_function(&function, 0);
        assert_eq!(
            output,
            "/// This is a documented function.\npub fn documented_function() {\n}\n"
        );
    }

    #[test]
    fn test_render_module() {
        let function_id = Id("function".to_string());
        let module = create_module(
            "test_module",
            "test_module",
            Visibility::Public,
            vec![function_id.clone()],
        );

        let mut index = HashMap::new();
        index.insert(
            function_id.clone(),
            create_function(
                "test_function",
                "test_function",
                Visibility::Public,
                vec![],
                None,
                None,
            ),
        );

        let crate_data = Crate {
            root: Id("root".to_string()),
            crate_version: None,
            includes_private: false,
            index,
            paths: HashMap::new(),
            external_crates: HashMap::new(),
            format_version: 0,
        };

        let output = Renderer::render_module(&module, &crate_data, 0);
        let expected = "mod test_module {\n    pub fn test_function() {\n    }\n}\n";
        assert_eq!(output, expected);
    }

    #[test]
    fn test_render_complex_type() {
        let complex_type = RustDocType::BorrowedRef {
            lifetime: Some("'a".to_string()),
            mutable: true,
            type_: Box::new(RustDocType::Slice(Box::new(RustDocType::Primitive(
                "u8".to_string(),
            )))),
        };
        let rendered = Renderer::render_type(&complex_type);
        assert_eq!(rendered, "&'a mut [u8]");
    }

    #[test]
    fn test_render_function_pointer() {
        let fn_pointer = RustDocType::FunctionPointer(Box::new(FunctionPointer {
            decl: FnDecl {
                inputs: vec![
                    (
                        "arg1".to_string(),
                        RustDocType::Primitive("i32".to_string()),
                    ),
                    (
                        "arg2".to_string(),
                        RustDocType::Primitive("String".to_string()),
                    ),
                ],
                output: Some(RustDocType::Primitive("bool".to_string())),
                c_variadic: false,
            },
            generic_params: vec![],
            header: Header {
                const_: false,
                unsafe_: false,
                async_: false,
                abi: Abi::Rust,
            },
        }));
        let rendered = Renderer::render_type(&fn_pointer);
        assert_eq!(rendered, "fn(arg1: i32, arg2: String) -> bool");
    }

    #[test]
    fn test_render_function_with_generics() {
        let mut function = create_function(
            "generic_function",
            "generic_function",
            Visibility::Public,
            vec![
                ("t".to_string(), RustDocType::Generic("T".to_string())),
                ("u".to_string(), RustDocType::Generic("U".to_string())),
            ],
            Some(RustDocType::Generic("T".to_string())),
            None,
        );
        if let ItemEnum::Function(ref mut f) = function.inner {
            f.generics.params = vec![
                GenericParamDef {
                    name: "T".to_string(),
                    kind: GenericParamDefKind::Type {
                        bounds: vec![],
                        default: None,
                        synthetic: false,
                    },
                },
                GenericParamDef {
                    name: "U".to_string(),
                    kind: GenericParamDefKind::Type {
                        bounds: vec![],
                        default: None,
                        synthetic: false,
                    },
                },
            ];
        }
        let output = Renderer::render_function(&function, 0);
        assert_eq!(
            output,
            "pub fn generic_function<T, U>(t: T, u: U) -> T {\n}\n"
        );
    }

    #[test]
    fn test_render_function_with_lifetimes() {
        let mut function = create_function(
            "lifetime_function",
            "lifetime_function",
            Visibility::Public,
            vec![(
                "x".to_string(),
                RustDocType::BorrowedRef {
                    lifetime: Some("'a".to_string()),
                    mutable: false,
                    type_: Box::new(RustDocType::Primitive("str".to_string())),
                },
            )],
            Some(RustDocType::BorrowedRef {
                lifetime: Some("'a".to_string()),
                mutable: false,
                type_: Box::new(RustDocType::Primitive("str".to_string())),
            }),
            None,
        );
        if let ItemEnum::Function(ref mut f) = function.inner {
            f.generics.params = vec![GenericParamDef {
                name: "'a".to_string(),
                kind: GenericParamDefKind::Lifetime { outlives: vec![] },
            }];
        }
        let output = Renderer::render_function(&function, 0);
        assert_eq!(
            output,
            "pub fn lifetime_function<'a>(x: &'a str) -> &'a str {\n}\n"
        );
    }

    #[test]
    fn test_render_function_with_where_clause() {
        let mut function = create_function(
            "where_function",
            "where_function",
            Visibility::Public,
            vec![("t".to_string(), RustDocType::Generic("T".to_string()))],
            Some(RustDocType::Generic("T".to_string())),
            None,
        );
        if let ItemEnum::Function(ref mut f) = function.inner {
            f.generics.params = vec![GenericParamDef {
                name: "T".to_string(),
                kind: GenericParamDefKind::Type {
                    bounds: vec![],
                    default: None,
                    synthetic: false,
                },
            }];
            f.generics.where_predicates = vec![WherePredicate::BoundPredicate {
                type_: RustDocType::Generic("T".to_string()),
                bounds: vec![GenericBound::TraitBound {
                    trait_: Path {
                        name: "Clone".to_string(),
                        id: Id("0".to_string()),
                        args: None,
                    },
                    generic_params: vec![],
                    modifier: TraitBoundModifier::None,
                }],
                generic_params: vec![],
            }];
        }
        let output = Renderer::render_function(&function, 0);
        assert_eq!(
            output,
            "pub fn where_function<T>(t: T) -> T where T: Clone {\n}\n"
        );
    }

    #[test]
    fn test_render_function_with_complex_generics_and_where_clause() {
        let mut function = create_function(
            "complex_function",
            "complex_function",
            Visibility::Public,
            vec![
                ("t".to_string(), RustDocType::Generic("T".to_string())),
                ("u".to_string(), RustDocType::Generic("U".to_string())),
            ],
            Some(RustDocType::Generic("R".to_string())),
            None,
        );
        if let ItemEnum::Function(ref mut f) = function.inner {
            f.generics.params = vec![
                GenericParamDef {
                    name: "T".to_string(),
                    kind: GenericParamDefKind::Type {
                        bounds: vec![],
                        default: None,
                        synthetic: false,
                    },
                },
                GenericParamDef {
                    name: "U".to_string(),
                    kind: GenericParamDefKind::Type {
                        bounds: vec![],
                        default: None,
                        synthetic: false,
                    },
                },
                GenericParamDef {
                    name: "R".to_string(),
                    kind: GenericParamDefKind::Type {
                        bounds: vec![],
                        default: None,
                        synthetic: false,
                    },
                },
            ];
            f.generics.where_predicates = vec![
                WherePredicate::BoundPredicate {
                    type_: RustDocType::Generic("T".to_string()),
                    bounds: vec![GenericBound::TraitBound {
                        trait_: Path {
                            name: "Clone".to_string(),
                            id: Id("0".to_string()),
                            args: None,
                        },
                        generic_params: vec![],
                        modifier: TraitBoundModifier::None,
                    }],
                    generic_params: vec![],
                },
                WherePredicate::BoundPredicate {
                    type_: RustDocType::Generic("U".to_string()),
                    bounds: vec![GenericBound::TraitBound {
                        trait_: Path {
                            name: "Debug".to_string(),
                            id: Id("1".to_string()),
                            args: None,
                        },
                        generic_params: vec![],
                        modifier: TraitBoundModifier::None,
                    }],
                    generic_params: vec![],
                },
                WherePredicate::BoundPredicate {
                    type_: RustDocType::Generic("R".to_string()),
                    bounds: vec![GenericBound::TraitBound {
                        trait_: Path {
                            name: "From".to_string(),
                            id: Id("2".to_string()),
                            args: Some(Box::new(GenericArgs::AngleBracketed {
                                args: vec![GenericArg::Type(RustDocType::Generic("T".to_string()))],
                                bindings: vec![],
                            })),
                        },
                        generic_params: vec![],
                        modifier: TraitBoundModifier::None,
                    }],
                    generic_params: vec![],
                },
            ];
        }
        let output = Renderer::render_function(&function, 0);
        assert_eq!(output, "pub fn complex_function<T, U, R>(t: T, u: U) -> R where T: Clone, U: Debug, R: From<T> {\n}\n");
    }

    #[test]
    fn test_render_function_with_hrtb() {
        let mut function = create_function(
            "hrtb_function",
            "hrtb_function",
            Visibility::Public,
            vec![("f".to_string(), RustDocType::Generic("F".to_string()))],
            Some(RustDocType::Tuple(vec![])),
            None,
        );
        if let ItemEnum::Function(ref mut f) = function.inner {
            f.generics.params = vec![GenericParamDef {
                name: "F".to_string(),
                kind: GenericParamDefKind::Type {
                    bounds: vec![],
                    default: None,
                    synthetic: false,
                },
            }];
            f.generics.where_predicates = vec![WherePredicate::BoundPredicate {
                type_: RustDocType::Generic("F".to_string()),
                bounds: vec![GenericBound::TraitBound {
                    trait_: Path {
                        name: "Fn".to_string(),
                        id: Id("0".to_string()),
                        args: Some(Box::new(GenericArgs::Parenthesized {
                            inputs: vec![RustDocType::BorrowedRef {
                                lifetime: Some("'a".to_string()),
                                mutable: false,
                                type_: Box::new(RustDocType::Primitive("str".to_string())),
                            }],
                            output: Some(RustDocType::Primitive("bool".to_string())),
                        })),
                    },
                    generic_params: vec![],
                    modifier: TraitBoundModifier::None,
                }],
                generic_params: vec![GenericParamDef {
                    name: "'a".to_string(),
                    kind: GenericParamDefKind::Lifetime { outlives: vec![] },
                }],
            }];
        }
        let output = Renderer::render_function(&function, 0);
        assert_eq!(
            output,
            "pub fn hrtb_function<F>(f: F) -> () where for<'a> F: Fn(&'a str) -> bool {\n}\n"
        );
    }
}
