use rustdoc_types::{Crate, Item, ItemEnum, Visibility};

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
        format!(
            "{}{}fn {}() {{\n{}}}\n",
            indent_str,
            visibility,
            item.name.as_deref().unwrap_or("?"),
            indent_str
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustdoc_types::{Abi, FnDecl, Function, Generics, Header, Module};

    use rustdoc_types::Id;
    use std::collections::HashMap;

    fn create_function_item(name: &str, visibility: Visibility) -> Item {
        Item {
            id: Id(name.to_string()),
            crate_id: 0,
            name: Some(name.to_string()),
            span: None,
            visibility,
            docs: None,
            links: HashMap::new(),
            attrs: vec![],
            deprecation: None,
            inner: ItemEnum::Function(Function {
                decl: FnDecl {
                    inputs: vec![],
                    output: None,
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

    #[test]
    fn test_render_public_function() {
        let function = create_function_item("test_function", Visibility::Public);
        let output = Renderer::render_function(&function, 0);
        assert_eq!(output, "pub fn test_function() {\n}\n");
    }

    #[test]
    fn test_render_private_function() {
        let function = create_function_item("private_function", Visibility::Default);
        let output = Renderer::render_function(&function, 0);
        assert_eq!(output, "fn private_function() {\n}\n");
    }

    #[test]
    fn test_render_module() {
        let function_id = Id("function".to_string());
        let module = Item {
            id: Id("module".to_string()),
            crate_id: 0,
            name: Some("test_module".to_string()),
            span: None,
            visibility: Visibility::Public,
            docs: None,
            links: HashMap::new(),
            attrs: vec![],
            deprecation: None,
            inner: ItemEnum::Module(Module {
                is_crate: false,
                items: vec![function_id.clone()],
                is_stripped: false,
            }),
        };

        let mut index = HashMap::new();
        index.insert(
            function_id.clone(),
            create_function_item("test_function", Visibility::Public),
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
}
