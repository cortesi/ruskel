use rust_format::{Formatter, RustFmt};
use rustdoc_types::{
    Crate, DynTrait, FnDecl, FunctionPointer, GenericArg, GenericArgs, GenericBound,
    GenericParamDef, GenericParamDefKind, Generics, Id, Impl, Item, ItemEnum, Path, PolyTrait,
    StructKind, Term, TraitBoundModifier, Type, TypeBinding, TypeBindingKind, VariantKind,
    Visibility, WherePredicate,
};

use crate::error::Result;

pub struct Renderer {
    formatter: RustFmt,
    render_auto_impls: bool,
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            formatter: RustFmt::default(),
            render_auto_impls: false,
        }
    }

    pub fn with_auto_impls(mut self, render_auto_impls: bool) -> Self {
        self.render_auto_impls = render_auto_impls;
        self
    }

    fn should_render_impl(&self, impl_: &Impl) -> bool {
        if self.render_auto_impls {
            true // Render all impls if render_auto_impls is true
        } else {
            if impl_.synthetic {
                return false; // Never render synthetic impls when render_auto_impls is false
            }

            // List of standard traits that we don't want to render by default
            const STANDARD_TRAITS: &[&str] = &[
                "Any",
                "Send",
                "Sync",
                "Unpin",
                "UnwindSafe",
                "RefUnwindSafe",
                "Borrow",
                "BorrowMut",
                "From",
                "Into",
                "TryFrom",
                "TryInto",
                "AsRef",
                "AsMut",
                "Default",
                "Clone",
                "Copy",
                "Debug",
                "PartialEq",
                "Eq",
                "PartialOrd",
                "Ord",
                "Hash",
                "Deref",
                "DerefMut",
                "Drop",
                "IntoIterator",
            ];

            // Check if the impl is for a standard trait
            if let Some(trait_path) = &impl_.trait_ {
                let trait_name = trait_path
                    .name
                    .split("::")
                    .last()
                    .unwrap_or(&trait_path.name);
                !STANDARD_TRAITS.contains(&trait_name)
            } else {
                true // Always render inherent impls (impls without a trait)
            }
        }
    }

    pub fn render(&self, crate_data: &Crate) -> Result<String> {
        if let Some(root_item) = crate_data.index.get(&crate_data.root) {
            let unformatted = self.render_item(root_item, crate_data);
            Ok(self.formatter.format_str(&unformatted)?)
        } else {
            Ok(String::new())
        }
    }

    fn render_item(&self, item: &Item, crate_data: &Crate) -> String {
        match &item.inner {
            ItemEnum::Module(_) => self.render_module(item, crate_data),
            ItemEnum::Function(_) => Self::render_function(item, false),
            ItemEnum::Constant { .. } => Self::render_constant(item),
            ItemEnum::Struct(_) => self.render_struct(item, crate_data),
            ItemEnum::Enum(_) => Self::render_enum(item, crate_data),
            ItemEnum::Trait(_) => Self::render_trait(item, crate_data),
            ItemEnum::Import(_) => self.render_import(item, crate_data),
            // Add other item types as needed
            _ => String::new(),
        }
    }

    fn render_import(&self, item: &Item, crate_data: &Crate) -> String {
        if let ItemEnum::Import(import) = &item.inner {
            // Check if the imported item is present in the crate's index
            if let Some(imported_item) = import.id.as_ref().and_then(|id| crate_data.index.get(id))
            {
                println!("Importing item: {:#?}", imported_item);
                // If the item is not public, render it directly without the import declaration
                if !matches!(imported_item.visibility, Visibility::Public) {
                    return self.render_item(imported_item, crate_data);
                }
            }

            // For public items, render the import declaration as before
            let mut output = String::new();

            // Add doc comment if present
            if let Some(docs) = &item.docs {
                for line in docs.lines() {
                    output.push_str(&format!("/// {}\n", line));
                }
            }

            output.push_str("pub use ");

            if import.glob {
                output.push_str(&format!("{}::*;\n", import.source));
            } else if import.name != import.source.split("::").last().unwrap_or(&import.source) {
                output.push_str(&format!("{} as {};\n", import.source, import.name));
            } else {
                output.push_str(&format!("{};\n", import.source));
            }

            output
        } else {
            String::new()
        }
    }

    fn render_impl(&self, item: &Item, crate_data: &Crate) -> String {
        let mut output = String::new();

        if let ItemEnum::Impl(impl_) = &item.inner {
            let generics = Self::render_generics(&impl_.generics);
            let where_clause = Self::render_where_clause(&impl_.generics);
            let unsafe_prefix = if impl_.is_unsafe { "unsafe " } else { "" };

            let trait_part = if let Some(trait_) = &impl_.trait_ {
                format!("{} for ", Self::render_path(trait_))
            } else {
                String::new()
            };

            output.push_str(&format!(
                "{}impl{} {}{}{}{}",
                unsafe_prefix,
                generics,
                trait_part,
                Self::render_type(&impl_.for_),
                where_clause,
                " {\n"
            ));

            for item_id in &impl_.items {
                if let Some(item) = crate_data.index.get(item_id) {
                    output.push_str(&Self::render_impl_item(item));
                }
            }

            output.push_str("}\n");
        }

        output
    }

    fn render_impl_item(item: &Item) -> String {
        match &item.inner {
            ItemEnum::Function(_) => Self::render_function(item, false),
            ItemEnum::Constant { .. } => Self::render_constant(item),
            ItemEnum::AssocType { .. } => Self::render_associated_type(item),
            _ => String::new(),
        }
    }

    fn render_associated_type(item: &Item) -> String {
        if let ItemEnum::AssocType {
            bounds, default, ..
        } = &item.inner
        {
            let bounds_str = if !bounds.is_empty() {
                format!(": {}", Self::render_generic_bounds(bounds))
            } else {
                String::new()
            };
            let default_str = default
                .as_ref()
                .map(|d| format!(" = {}", Self::render_type(d)))
                .unwrap_or_default();
            format!(
                "type {}{}{};\n",
                item.name.as_deref().unwrap_or("?"),
                bounds_str,
                default_str
            )
        } else {
            String::new()
        }
    }

    fn render_enum(item: &Item, crate_data: &Crate) -> String {
        let visibility = match &item.visibility {
            Visibility::Public => "pub ",
            _ => "",
        };

        let mut output = String::new();

        // Add doc comment if present
        if let Some(docs) = &item.docs {
            for line in docs.lines() {
                output.push_str(&format!("/// {}\n", line));
            }
        }

        if let ItemEnum::Enum(enum_) = &item.inner {
            let generics = Self::render_generics(&enum_.generics);
            let where_clause = Self::render_where_clause(&enum_.generics);

            output.push_str(&format!(
                "{}enum {}{}{} {{\n",
                visibility,
                item.name.as_deref().unwrap_or("?"),
                generics,
                where_clause
            ));

            for variant_id in &enum_.variants {
                if let Some(variant_item) = crate_data.index.get(variant_id) {
                    output.push_str(&Self::render_enum_variant(variant_item, crate_data));
                }
            }

            output.push_str("}\n");
        }

        output
    }

    fn render_enum_variant(item: &Item, crate_data: &Crate) -> String {
        let mut output = String::new();

        // Add doc comment if present
        if let Some(docs) = &item.docs {
            for line in docs.lines() {
                output.push_str(&format!("    /// {}\n", line));
            }
        }

        if let ItemEnum::Variant(variant) = &item.inner {
            output.push_str(&format!("    {}", item.name.as_deref().unwrap_or("?")));

            match &variant.kind {
                VariantKind::Plain => {}
                VariantKind::Tuple(fields) => {
                    let fields_str = fields
                        .iter()
                        .filter_map(|field| {
                            field.as_ref().map(|id| {
                                if let Some(field_item) = crate_data.index.get(id) {
                                    if let ItemEnum::StructField(ty) = &field_item.inner {
                                        Self::render_type(ty)
                                    } else {
                                        "".to_string()
                                    }
                                } else {
                                    "".to_string()
                                }
                            })
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    output.push_str(&format!("({})", fields_str));
                }
                VariantKind::Struct { fields, .. } => {
                    output.push_str(" {\n");
                    for field in fields {
                        if let Some(_field_item) = crate_data.index.get(field) {
                            output.push_str(&format!(
                                "        {}\n",
                                Self::render_struct_field(crate_data, field)
                            ));
                        }
                    }
                    output.push_str("    }");
                }
            }

            if let Some(discriminant) = &variant.discriminant {
                output.push_str(&format!(" = {}", discriminant.expr));
            }

            output.push_str(",\n");
        }

        output
    }

    fn render_trait(item: &Item, crate_data: &Crate) -> String {
        let visibility = match &item.visibility {
            Visibility::Public => "pub ",
            _ => "",
        };

        let mut output = String::new();

        // Add doc comment if present
        if let Some(docs) = &item.docs {
            for line in docs.lines() {
                output.push_str(&format!("/// {}\n", line));
            }
        }

        if let ItemEnum::Trait(trait_) = &item.inner {
            let generics = Self::render_generics(&trait_.generics);
            let where_clause = Self::render_where_clause(&trait_.generics);

            let bounds = if !trait_.bounds.is_empty() {
                format!(": {}", Self::render_generic_bounds(&trait_.bounds))
            } else {
                String::new()
            };

            let unsafe_prefix = if trait_.is_unsafe { "unsafe " } else { "" };

            output.push_str(&format!(
                "{}{}trait {}{}{}{} {{\n",
                visibility,
                unsafe_prefix,
                item.name.as_deref().unwrap_or("?"),
                generics,
                bounds,
                where_clause
            ));

            for item_id in &trait_.items {
                if let Some(item) = crate_data.index.get(item_id) {
                    output.push_str(&Self::render_trait_item(item));
                }
            }

            output.push_str("}\n");
        }

        output
    }

    fn render_trait_item(item: &Item) -> String {
        match &item.inner {
            ItemEnum::Function(_) => Self::render_function(item, true),
            ItemEnum::AssocConst { type_, default } => {
                let default_str = default
                    .as_ref()
                    .map(|d| format!(" = {}", d))
                    .unwrap_or_default();
                format!(
                    "const {}: {}{};\n",
                    item.name.as_deref().unwrap_or("?"),
                    Self::render_type(type_),
                    default_str
                )
            }
            ItemEnum::AssocType {
                bounds,
                generics,
                default,
            } => {
                let bounds_str = if !bounds.is_empty() {
                    format!(": {}", Self::render_generic_bounds(bounds))
                } else {
                    String::new()
                };
                let generics_str = Self::render_generics(generics);
                let default_str = default
                    .as_ref()
                    .map(|d| format!(" = {}", Self::render_type(d)))
                    .unwrap_or_default();
                format!(
                    "type {}{}{}{};\n",
                    item.name.as_deref().unwrap_or("?"),
                    generics_str,
                    bounds_str,
                    default_str
                )
            }
            _ => String::new(),
        }
    }

    fn render_generic_bounds(bounds: &[GenericBound]) -> String {
        bounds
            .iter()
            .map(Self::render_generic_bound)
            .collect::<Vec<_>>()
            .join(" + ")
    }

    fn render_struct(&self, item: &Item, crate_data: &Crate) -> String {
        let visibility = match &item.visibility {
            Visibility::Public => "pub ",
            _ => "",
        };

        let mut output = String::new();

        // Add doc comment if present
        if let Some(docs) = &item.docs {
            for line in docs.lines() {
                output.push_str(&format!("/// {}\n", line));
            }
        }

        if let ItemEnum::Struct(struct_) = &item.inner {
            let generics = Self::render_generics(&struct_.generics);
            let where_clause = Self::render_where_clause(&struct_.generics);

            match &struct_.kind {
                StructKind::Unit => {
                    output.push_str(&format!(
                        "{}struct {}{}{};\n",
                        visibility,
                        item.name.as_deref().unwrap_or("?"),
                        generics,
                        where_clause
                    ));
                }
                StructKind::Tuple(fields) => {
                    let fields_str = fields
                        .iter()
                        .filter_map(|field| {
                            field.as_ref().map(|id| {
                                if let Some(field_item) = crate_data.index.get(id) {
                                    if let ItemEnum::StructField(ty) = &field_item.inner {
                                        let visibility = match &field_item.visibility {
                                            Visibility::Public => "pub ",
                                            _ => "",
                                        };
                                        format!("{}{}", visibility, Self::render_type(ty))
                                    } else {
                                        "".to_string()
                                    }
                                } else {
                                    "".to_string()
                                }
                            })
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    output.push_str(&format!(
                        "{}struct {}{}({}){};\n",
                        visibility,
                        item.name.as_deref().unwrap_or("?"),
                        generics,
                        fields_str,
                        where_clause
                    ));
                }
                StructKind::Plain { fields, .. } => {
                    output.push_str(&format!(
                        "{}struct {}{}{} {{\n",
                        visibility,
                        item.name.as_deref().unwrap_or("?"),
                        generics,
                        where_clause
                    ));
                    for field in fields {
                        output.push_str(&Self::render_struct_field(crate_data, field));
                    }
                    output.push_str("}\n");
                }
            }

            // Render impl blocks
            for impl_id in &struct_.impls {
                if let Some(impl_item) = crate_data.index.get(impl_id) {
                    if let ItemEnum::Impl(impl_) = &impl_item.inner {
                        if self.should_render_impl(impl_) {
                            output.push_str(&self.render_impl(impl_item, crate_data));
                        }
                    }
                }
            }
        }

        output
    }

    fn render_struct_field(crate_data: &Crate, field_id: &Id) -> String {
        if let Some(field_item) = crate_data.index.get(field_id) {
            let visibility = match &field_item.visibility {
                Visibility::Public => "pub ",
                _ => "",
            };
            if let ItemEnum::StructField(ty) = &field_item.inner {
                format!(
                    "{}{}: {},\n",
                    visibility,
                    field_item.name.as_deref().unwrap_or("?"),
                    Self::render_type(ty)
                )
            } else {
                "// Unknown field type\n".to_string()
            }
        } else {
            "// Field not found\n".to_string()
        }
    }

    fn render_constant(item: &Item) -> String {
        let visibility = match &item.visibility {
            Visibility::Public => "pub ",
            _ => "",
        };

        let mut output = String::new();

        // Add doc comment if present
        if let Some(docs) = &item.docs {
            for line in docs.lines() {
                output.push_str(&format!("/// {}\n", line));
            }
        }

        if let ItemEnum::Constant { type_, const_ } = &item.inner {
            output.push_str(&format!(
                "{}const {}: {} = {};\n",
                visibility,
                item.name.as_deref().unwrap_or("?"),
                Self::render_type(type_),
                const_.expr
            ));
        }

        output
    }

    fn render_module(&self, item: &Item, crate_data: &Crate) -> String {
        let mut output = format!("mod {} {{\n", item.name.as_deref().unwrap_or("?"));

        if let ItemEnum::Module(module) = &item.inner {
            for item_id in &module.items {
                if let Some(item) = crate_data.index.get(item_id) {
                    output.push_str(&self.render_item(item, crate_data));
                }
            }
        }

        output.push_str("}\n");
        output
    }

    fn render_function(item: &Item, is_trait_method: bool) -> String {
        let visibility = match &item.visibility {
            Visibility::Public => "pub ",
            _ => "",
        };

        let mut output = String::new();

        // Add doc comment if present
        if let Some(docs) = &item.docs {
            for line in docs.lines() {
                output.push_str(&format!("/// {}\n", line));
            }
        }

        if let ItemEnum::Function(function) = &item.inner {
            let generics = Self::render_generics(&function.generics);
            let args = Self::render_function_args(&function.decl);
            let return_type = Self::render_return_type(&function.decl);
            let where_clause = Self::render_where_clause(&function.generics);

            // Handle unsafe, const, and async keywords
            let mut prefixes = Vec::new();
            if function.header.const_ {
                prefixes.push("const");
            }
            if function.header.unsafe_ {
                prefixes.push("unsafe");
            }
            if function.header.async_ {
                prefixes.push("async");
            }
            let prefix = if !prefixes.is_empty() {
                format!("{} ", prefixes.join(" "))
            } else {
                String::new()
            };

            output.push_str(&format!(
                "{}{}fn {}{}({}){}{}",
                visibility,
                prefix,
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

            if is_trait_method {
                if function.has_body {
                    output.push_str(" {\n}\n");
                } else {
                    output.push_str(";\n");
                }
            } else {
                output.push_str(" {\n}\n");
            }
        }

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
            .map(|(name, ty)| {
                if name == "self" {
                    match ty {
                        Type::BorrowedRef { mutable, .. } => {
                            if *mutable {
                                "&mut self".to_string()
                            } else {
                                "&self".to_string()
                            }
                        }
                        Type::ResolvedPath(path) => {
                            if path.name == "Self" && path.args.is_none() {
                                "self".to_string()
                            } else {
                                format!("self: {}", Self::render_type(ty))
                            }
                        }
                        Type::Generic(name) => {
                            if name == "Self" {
                                "self".to_string()
                            } else {
                                format!("self: {}", Self::render_type(ty))
                            }
                        }
                        _ => format!("self: {}", Self::render_type(ty)),
                    }
                } else {
                    format!("{}: {}", name, Self::render_type(ty))
                }
            })
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
                args,
                self_type,
                trait_,
            } => {
                match &**self_type {
                    Type::Generic(s) if s == "Self" => {
                        // This is the Self::AssocType case
                        let args_str = Self::render_generic_args(args);
                        format!("Self::{}{}", name, args_str)
                    }
                    _ => {
                        // This is a <Type as Trait>::AssocType case
                        let trait_part = trait_
                            .as_ref()
                            .map(|t| format!(" as {}", Self::render_path(t)))
                            .unwrap_or_default();
                        let args_str = Self::render_generic_args(args);
                        format!(
                            "<{}{}>::{}{}",
                            Self::render_type(self_type),
                            trait_part,
                            name,
                            args_str
                        )
                    }
                }
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
                if args.is_empty() && bindings.is_empty() {
                    String::new()
                } else {
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
    use crate::Ruskel;
    use pretty_assertions::assert_eq;
    use std::fs;
    use tempfile::TempDir;

    fn normalize_whitespace(s: &str) -> String {
        let lines: Vec<&str> = s
            .lines()
            .map(|line| line.trim_end()) // Remove trailing whitespace
            .filter(|line| !line.is_empty()) // Remove blank lines
            .collect();

        if lines.is_empty() {
            return String::new();
        }

        // Find the minimum indentation
        let min_indent = lines
            .iter()
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.len() - line.trim_start().len())
            .min()
            .unwrap_or(0);

        // Dedent all lines by the minimum indentation
        lines
            .into_iter()
            .map(|line| {
                if line.len() > min_indent {
                    &line[min_indent..]
                } else {
                    line.trim_start()
                }
            })
            .collect::<Vec<&str>>()
            .join("\n")
    }

    fn strip_module_declaration(s: &str) -> String {
        let lines: Vec<&str> = s
            .lines()
            .map(|line| line.trim_end())
            .filter(|line| !line.is_empty())
            .collect();

        if lines.len() <= 2 {
            return String::new();
        }

        lines[1..lines.len() - 1].join("\n")
    }

    fn render_roundtrip(source: &str, expected_output: &str) {
        // Create a temporary directory for our dummy crate
        let temp_dir = TempDir::new().unwrap();
        let crate_path = temp_dir.path().join("src");
        fs::create_dir(&crate_path).unwrap();

        // Write the source code to a file
        let lib_rs_path = crate_path.join("lib.rs");
        fs::write(&lib_rs_path, source).unwrap();

        // Create a dummy Cargo.toml
        let cargo_toml_content = r#"
        [package]
        name = "dummy_crate"
        version = "0.1.0"
        edition = "2021"
        "#;
        fs::write(temp_dir.path().join("Cargo.toml"), cargo_toml_content).unwrap();

        // Parse the crate using Ruskel
        let ruskel = Ruskel::new(lib_rs_path.to_str().unwrap()).unwrap();
        let crate_data = ruskel.json().unwrap();

        // Render the crate data
        let renderer = Renderer::new();
        let rendered = renderer.render(&crate_data).unwrap();

        // Strip the module declaration, normalize whitespace, and compare
        let normalized_rendered = normalize_whitespace(&strip_module_declaration(&rendered));

        let formatter = RustFmt::default();
        let normalized_expected =
            normalize_whitespace(&formatter.format_str(expected_output).unwrap());

        assert_eq!(normalized_rendered, normalized_expected);
    }

    #[test]
    fn test_render_public_function() {
        render_roundtrip(
            r#"
                /// This is a documented function.
                pub fn test_function() {
                    // Function body
                }
            "#,
            r#"
                /// This is a documented function.
                pub fn test_function() {}
            "#,
        );
    }

    #[test]
    fn test_render_private_function() {
        render_roundtrip(
            r#"
            fn private_function() {
                // Function body
            }
            "#,
            r#"
            fn private_function() {}
            "#,
        );
    }

    #[test]
    fn test_render_function_with_args_and_return() {
        render_roundtrip(
            r#"
            pub fn complex_function(arg1: i32, arg2: String) -> bool {
                // Function body
            }
            "#,
            r#"
            pub fn complex_function(arg1: i32, arg2: String) -> bool {}
            "#,
        );
    }

    #[test]
    fn test_render_function_with_docs() {
        render_roundtrip(
            r#"
            /// This is a documented function.
            /// It has multiple lines of documentation.
            pub fn documented_function() {
                // Function body
            }
        "#,
            r#"
            /// This is a documented function.
            /// It has multiple lines of documentation.
            pub fn documented_function() {
            }
        "#,
        );
    }

    #[test]
    fn test_render_module() {
        render_roundtrip(
            r#"
                mod test_module {
                    pub fn test_function() {
                        // Function body
                    }
                }
            "#,
            r#"
                mod test_module {
                    pub fn test_function() {}
                }
            "#,
        );
    }

    #[test]
    fn test_render_complex_type() {
        render_roundtrip(
            r#"
                pub fn complex_type_function<'a>(arg: &'a mut [u8]) {
                    // Function body
                }
            "#,
            r#"
                pub fn complex_type_function<'a>(arg: &'a mut [u8]) {
                }
            "#,
        );
    }

    #[test]
    fn test_render_function_pointer() {
        render_roundtrip(
            r#"
                pub fn function_with_fn_pointer(f: fn(arg1: i32, arg2: String) -> bool) {
                    // Function body
                }
            "#,
            r#"
                pub fn function_with_fn_pointer(f: fn(arg1: i32, arg2: String) -> bool) {
                }
            "#,
        );
    }

    #[test]
    fn test_render_function_with_generics() {
        render_roundtrip(
            r#"
                pub fn generic_function<T, U>(t: T, u: U) -> T {
                    // Function body
                }
            "#,
            r#"
                pub fn generic_function<T, U>(t: T, u: U) -> T {
                }
            "#,
        );
    }

    #[test]
    fn test_render_function_with_lifetimes() {
        render_roundtrip(
            r#"
                pub fn lifetime_function<'a>(x: &'a str) -> &'a str {
                    // Function body
                }
            "#,
            r#"
                pub fn lifetime_function<'a>(x: &'a str) -> &'a str {}
            "#,
        );
    }

    #[test]
    fn test_render_function_with_where_clause() {
        render_roundtrip(
            r#"
                pub fn where_function<T>(t: T) -> T
                where
                    T: Clone,
                {
                    // Function body
                }
            "#,
            r#"
                pub fn where_function<T>(t: T) -> T
                where
                    T: Clone,
                {
                }
            "#,
        );
    }

    #[test]
    fn test_render_function_with_complex_generics_and_where_clause() {
        render_roundtrip(
            r#"
                pub fn complex_function<T, U, R>(t: T, u: U) -> R
                where
                    T: Clone,
                    U: std::fmt::Debug,
                    R: From<T>,
                {
                    // Function body
                }
            "#,
            r#"
                pub fn complex_function<T, U, R>(t: T, u: U) -> R
                where
                    T: Clone,
                    U: std::fmt::Debug,
                    R: From<T>,
                {
                }
            "#,
        );
    }

    #[test]
    fn test_render_function_with_hrtb() {
        render_roundtrip(
            r#"
                pub fn hrtb_function<F>(f: F)
                where
                    for<'a> F: Fn(&'a str) -> bool,
                {
                    // Function body
                }
            "#,
            r#"
                pub fn hrtb_function<F>(f: F) 
                where
                    for<'a> F: Fn(&'a str) -> bool,
                {
                }
            "#,
        );
    }

    #[test]
    fn test_render_constant() {
        render_roundtrip(
            r#"
                /// This is a documented constant.
                pub const CONSTANT: u32 = 42;
            "#,
            r#"
                /// This is a documented constant.
                pub const CONSTANT: u32 = 42;
            "#,
        );
    }

    #[test]
    fn test_render_private_constant() {
        render_roundtrip(
            r#"
                const PRIVATE_CONSTANT: &str = "Hello, world!";
            "#,
            r#"
                const PRIVATE_CONSTANT: &str = "Hello, world!";
            "#,
        );
    }

    #[test]
    fn test_render_unit_struct() {
        render_roundtrip(
            r#"
                /// A unit struct
                pub struct UnitStruct;
            "#,
            r#"
                /// A unit struct
                pub struct UnitStruct;
            "#,
        );
    }

    #[test]
    fn test_render_tuple_struct() {
        render_roundtrip(
            r#"
                /// A tuple struct
                pub struct TupleStruct(pub i32, String);
            "#,
            r#"
                /// A tuple struct
                pub struct TupleStruct(pub i32, String);
            "#,
        );
    }

    #[test]
    fn test_render_plain_struct() {
        render_roundtrip(
            r#"
                /// A plain struct
                pub struct PlainStruct {
                    pub field1: i32,
                    field2: String,
                }
            "#,
            r#"
                /// A plain struct
                pub struct PlainStruct {
                    pub field1: i32,
                    field2: String,
                }
            "#,
        );
    }

    #[test]
    fn test_render_generic_struct() {
        render_roundtrip(
            r#"
                /// A generic struct
                pub struct GenericStruct<T, U>
                where
                    T: Clone,
                    U: Default,
                {
                    field1: T,
                    field2: U,
                }
            "#,
            r#"
                /// A generic struct
                pub struct GenericStruct<T, U>
                where
                    T: Clone,
                    U: Default,
                {
                    field1: T,
                    field2: U,
                }
            "#,
        );
    }

    #[test]
    fn test_render_struct_with_lifetime() {
        render_roundtrip(
            r#"
                /// A struct with a lifetime
                pub struct LifetimeStruct<'a> {
                    field: &'a str,
                }
            "#,
            r#"
                /// A struct with a lifetime
                pub struct LifetimeStruct<'a> {
                    field: &'a str,
                }
            "#,
        );
    }

    #[test]
    fn test_render_struct_with_generic() {
        render_roundtrip(
            r#"
                /// A struct with a generic type
                pub struct GenericStruct<T> {
                    field: T,
                }
            "#,
            r#"
                /// A struct with a generic type
                pub struct GenericStruct<T> {
                    field: T,
                }
            "#,
        );
    }

    #[test]
    fn test_render_struct_with_multiple_generics_and_where_clause() {
        render_roundtrip(
            r#"
                /// A struct with multiple generic types and a where clause
                pub struct ComplexStruct<T, U>
                where
                    T: Clone,
                    U: Default,
                {
                    field1: T,
                    field2: U,
                }
            "#,
            r#"
                /// A struct with multiple generic types and a where clause
                pub struct ComplexStruct<T, U>
                where
                    T: Clone,
                    U: Default,
                {
                    field1: T,
                    field2: U,
                }
            "#,
        );
    }

    #[test]
    fn test_render_tuple_struct_with_generics() {
        render_roundtrip(
            r#"
                /// A tuple struct with generic types
                pub struct TupleStruct<T, U>(T, U);
            "#,
            r#"
                /// A tuple struct with generic types
                pub struct TupleStruct<T, U>(T, U);
            "#,
        );
    }

    #[test]
    fn test_render_struct_with_lifetime_and_generic() {
        render_roundtrip(
            r#"
                /// A struct with both lifetime and generic type
                pub struct MixedStruct<'a, T> {
                    reference: &'a str,
                    value: T,
                }
            "#,
            r#"
                /// A struct with both lifetime and generic type
                pub struct MixedStruct<'a, T> {
                    reference: &'a str,
                    value: T,
                }
            "#,
        );
    }

    #[test]
    fn test_render_simple_trait() {
        render_roundtrip(
            r#"
                /// A simple trait
                pub trait SimpleTrait {
                    fn method(&self);
                }
            "#,
            r#"
                /// A simple trait
                pub trait SimpleTrait {
                    fn method(&self);
                }
            "#,
        );
    }

    #[test]
    fn test_render_trait_with_generics() {
        render_roundtrip(
            r#"
                /// A trait with generics
                pub trait GenericTrait<T> {
                    fn method(&self, value: T);
                }
            "#,
            r#"
                /// A trait with generics
                pub trait GenericTrait<T> {
                    fn method(&self, value: T);
                }
            "#,
        );
    }

    #[test]
    fn test_render_trait_with_default_methods() {
        render_roundtrip(
            r#"
                /// A trait with default methods
                pub trait TraitWithDefault {
                    fn method_with_default(&self) {
                        // Default implementation
                    }
                    fn method_without_default(&self);
                }
            "#,
            r#"
                /// A trait with default methods
                pub trait TraitWithDefault {
                    fn method_with_default(&self) {}
                    fn method_without_default(&self);
                }
            "#,
        );
    }

    #[test]
    fn test_render_unsafe_trait() {
        render_roundtrip(
            r#"
                /// An unsafe trait
                pub unsafe trait UnsafeTrait {
                    unsafe fn unsafe_method(&self);
                }
            "#,
            r#"
                /// An unsafe trait
                pub unsafe trait UnsafeTrait {
                    unsafe fn unsafe_method(&self);
                }
            "#,
        );
    }

    #[test]
    fn test_render_trait_with_supertraits() {
        render_roundtrip(
            r#"
                /// A trait with supertraits
                pub trait SuperTrait: std::fmt::Debug + Clone {
                    fn super_method(&self);
                }
            "#,
            r#"
                /// A trait with supertraits
                pub trait SuperTrait: std::fmt::Debug + Clone {
                    fn super_method(&self);
                }
            "#,
        );
    }

    #[test]
    fn test_render_trait_with_self_methods() {
        render_roundtrip(
            r#"
                pub trait TraitWithSelfMethods {
                    fn method1(self);
                    fn method2(&self);
                    fn method3(&mut self);
                    fn method4(self: Box<Self>);
                }
            "#,
            r#"
                pub trait TraitWithSelfMethods {
                    fn method1(self);
                    fn method2(&self);
                    fn method3(&mut self);
                    fn method4(self: Box<Self>);
                }
            "#,
        );
    }

    #[test]
    fn test_render_trait_with_associated_types() {
        render_roundtrip(
            r#"
            /// A trait with associated types
            pub trait TraitWithAssocTypes {
                type Item;
                type Container<T>;
                type WithBounds: Clone + 'static;
                fn get_item(&self) -> Self::Item;
            }
        "#,
            r#"
            /// A trait with associated types
            pub trait TraitWithAssocTypes {
                type Item;
                type Container<T>;
                type WithBounds: Clone + 'static;
                fn get_item(&self) -> Self::Item;
            }
        "#,
        );
    }

    #[test]
    fn test_render_simple_enum() {
        render_roundtrip(
            r#"
                /// A simple enum
                pub enum SimpleEnum {
                    Variant1,
                    Variant2,
                    Variant3,
                }
            "#,
            r#"
                /// A simple enum
                pub enum SimpleEnum {
                    Variant1,
                    Variant2,
                    Variant3,
                }
            "#,
        );
    }

    #[test]
    fn test_render_enum_with_tuple_variants() {
        render_roundtrip(
            r#"
                /// An enum with tuple variants
                pub enum TupleEnum {
                    Variant1(i32, String),
                    Variant2(bool),
                }
            "#,
            r#"
                /// An enum with tuple variants
                pub enum TupleEnum {
                    Variant1(i32, String),
                    Variant2(bool),
                }
            "#,
        );
    }

    #[test]
    fn test_render_enum_with_struct_variants() {
        render_roundtrip(
            r#"
                /// An enum with struct variants
                pub enum StructEnum {
                    Variant1 {
                        field1: i32,
                        field2: String,
                    },
                    Variant2 {
                        field: bool,
                    },
                }
            "#,
            r#"
                /// An enum with struct variants
                pub enum StructEnum {
                    Variant1 {
                        field1: i32,
                        field2: String,
                    },
                    Variant2 {
                        field: bool,
                    },
                }
            "#,
        );
    }

    #[test]
    fn test_render_enum_with_mixed_variants() {
        render_roundtrip(
            r#"
                /// An enum with mixed variant types
                pub enum MixedEnum {
                    Variant1,
                    Variant2(i32, String),
                    Variant3 {
                        field: bool,
                    },
                }
            "#,
            r#"
                /// An enum with mixed variant types
                pub enum MixedEnum {
                    Variant1,
                    Variant2(i32, String),
                    Variant3 {
                        field: bool,
                    },
                }
            "#,
        );
    }

    #[test]
    fn test_render_enum_with_discriminants() {
        render_roundtrip(
            r#"
                /// An enum with discriminants
                pub enum DiscriminantEnum {
                    Variant1 = 1,
                    Variant2 = 2,
                    Variant3 = 4,
                }
            "#,
            r#"
                /// An enum with discriminants
                pub enum DiscriminantEnum {
                    Variant1 = 1,
                    Variant2 = 2,
                    Variant3 = 4,
                }
            "#,
        );
    }

    #[test]
    fn test_render_enum_with_generics() {
        render_roundtrip(
            r#"
            /// An enum with generic types
            pub enum GenericEnum<T, U> {
                Variant1(T),
                Variant2(U),
                Variant3(T, U),
            }
        "#,
            r#"
            /// An enum with generic types
            pub enum GenericEnum<T, U> {
                Variant1(T),
                Variant2(U),
                Variant3(T, U),
            }
        "#,
        );
    }

    #[test]
    fn test_render_enum_with_lifetimes() {
        render_roundtrip(
            r#"
            /// An enum with lifetimes
            pub enum LifetimeEnum<'a, 'b> {
                Variant1(&'a str),
                Variant2(&'b str),
                Variant3(&'a str, &'b str),
            }
        "#,
            r#"
            /// An enum with lifetimes
            pub enum LifetimeEnum<'a, 'b> {
                Variant1(&'a str),
                Variant2(&'b str),
                Variant3(&'a str, &'b str),
            }
        "#,
        );
    }

    #[test]
    fn test_render_enum_with_generics_and_where_clause() {
        render_roundtrip(
            r#"
            /// An enum with generics and a where clause
            pub enum ComplexEnum<T, U>
            where
                T: Clone,
                U: Default,
            {
                Variant1(T),
                Variant2(U),
                Variant3 {
                    field1: T,
                    field2: U,
                },
            }
        "#,
            r#"
            /// An enum with generics and a where clause
            pub enum ComplexEnum<T, U>
            where
                T: Clone,
                U: Default,
            {
                Variant1(T),
                Variant2(U),
                Variant3 {
                    field1: T,
                    field2: U,
                },
            }
        "#,
        );
    }

    #[test]
    fn test_render_enum_with_lifetimes_and_generics() {
        render_roundtrip(
            r#"
            /// An enum with lifetimes and generics
            pub enum MixedEnum<'a, T: 'a> {
                Variant1(&'a T),
                Variant2(T),
                Variant3(&'a [T]),
            }
        "#,
            r#"
            /// An enum with lifetimes and generics
            pub enum MixedEnum<'a, T: 'a> {
                Variant1(&'a T),
                Variant2(T),
                Variant3(&'a [T]),
            }
        "#,
        );
    }

    #[test]
    fn test_render_simple_impl() {
        render_roundtrip(
            r#"
                struct MyStruct;

                impl MyStruct {
                    fn new() -> Self {
                        MyStruct
                    }

                    fn mymethod(&self) {
                        // Method body
                    }
                }
            "#,
            r#"
                struct MyStruct;

                impl MyStruct {
                    fn new() -> Self {}

                    fn mymethod(&self) {}
                }
            "#,
        );
    }

    #[test]
    fn test_render_impl_with_trait() {
        render_roundtrip(
            r#"
                trait MyTrait {
                    fn trait_method(&self);
                }

                struct MyStruct;

                impl MyTrait for MyStruct {
                    fn trait_method(&self) {
                        // Method body
                    }
                }
            "#,
            r#"
                trait MyTrait {
                    fn trait_method(&self);
                }

                struct MyStruct;

                impl MyTrait for MyStruct {
                    fn trait_method(&self) {}
                }
            "#,
        );
    }

    #[test]
    fn test_render_impl_with_generics() {
        render_roundtrip(
            r#"
                struct GenericStruct<T>(T);

                impl<T: Clone> GenericStruct<T> {
                    fn get_value(&self) -> T {
                        self.0.clone()
                    }
                }
            "#,
            r#"
                struct GenericStruct<T>(T);

                impl<T: Clone> GenericStruct<T> {
                    fn get_value(&self) -> T { }
                }
            "#,
        );
    }

    #[test]
    fn test_render_impl_with_associated_types() {
        render_roundtrip(
            r#"
                struct MyIterator<T>(Vec<T>);

                impl<T> Iterator for MyIterator<T> {
                    type Item = T;

                    fn next(&mut self) -> Option<Self::Item> {
                        self.0.pop()
                    }
                }
            "#,
            r#"
                struct MyIterator<T>(Vec<T>);

                impl<T> Iterator for MyIterator<T> {
                    type Item = T;

                    fn next(&mut self) -> Option<Self::Item> { }
                }
            "#,
        );
    }

    #[test]
    fn test_render_unsafe_impl() {
        // FIXME: This appears to be a bug in rustdoc - unsafe is not set on the unsafe impl block.
        render_roundtrip(
            r#"
                unsafe trait Foo {}

                struct UnsafeStruct;

                unsafe impl Foo for UnsafeStruct {}
            "#,
            r#"
                unsafe trait Foo {}

                struct UnsafeStruct;

                impl Foo for UnsafeStruct {}
            "#,
        );
    }

    #[test]
    fn test_render_imports() {
        render_roundtrip(
            r#"
                use std::collections::HashMap;
                pub use std::rc::Rc;
                pub use std::sync::{Arc, Mutex};
            "#,
            r#"
                pub use std::rc::Rc;
                pub use std::sync::Arc;
                pub use std::sync::Mutex;
            "#,
        );
    }

    #[test]
    fn test_render_imports_inline() {
        render_roundtrip(
            r#"
                mod private {
                    pub struct PrivateStruct;
                }

                pub use private::PrivateStruct;
            "#,
            r#"
                struct PrivateStruct;
            "#,
        );
    }
}
