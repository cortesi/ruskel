mod utils;
use utils::*;

#[test]
fn test_render_constant() {
    rt(
        r#"
            /// This is a documented constant.
            pub const CONSTANT: u32 = 42;
            const PRIVATE_CONSTANT: &str = "Hello, world!";
        "#,
        r#"
            /// This is a documented constant.
            pub const CONSTANT: u32 = 42;
        "#,
    );
    rt_priv_idemp(
        r#"
            /// This is a documented constant.
            pub const CONSTANT: u32 = 42;
            const PRIVATE_CONSTANT: &str = "Hello, world!";
        "#,
    );
}

#[test]
fn test_render_imports() {
    rt(
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
    let input = r#"
            mod private {
                pub struct PrivateStruct;
            }

            pub use private::PrivateStruct;
        "#;

    rt(
        input,
        r#"
            pub struct PrivateStruct;
        "#,
    );
    rt_private(
        input,
        r#"
            mod private {
                pub struct PrivateStruct;
            }

            pub struct PrivateStruct;
        "#,
    );
}

#[test]
fn test_render_type_alias_with_bounds() {
    rt_idemp(
        r#"
        pub trait Trait<T> {
            fn as_ref(&self) -> &T;
        }

        pub type Alias<T> = dyn Trait<T> + Send + 'static;

        pub fn use_alias<T: 'static>(value: Box<Alias<T>>) -> &'static T { }
        "#,
    );
}

#[test]
fn test_render_type_alias() {
    rt_idemp(
        r#"
            /// A simple type alias
            pub type SimpleAlias = Vec<String>;

            /// A type alias with generics
            pub type GenericAlias<T> = Result<T, std::io::Error>;

            /// A type alias with generics and where clause
            pub type ComplexAlias<T, U> where T: Clone, U: Default = Result<Vec<(T, U)>, Box<dyn std::error::Error>>;
        "#,
    );
}

#[test]
fn test_reserved_word() {
    rt_idemp(
        r#"
            pub fn r#try() { }
        "#,
    );
}

#[test]
fn test_render_macro() {
    let source = r#"
        /// A simple macro for creating a vector
        #[macro_export]
        macro_rules! myvec {
            ( $( $x:expr ),* ) => {
                {
                    let mut temp_vec = Vec::new();
                    $(
                        temp_vec.push($x);
                    )*
                    temp_vec
                }
            };
        }

        // A private macro
        macro_rules! private_macro {
            ($x:expr) => {
                $x + 1
            };
        }
    "#;

    let expected_output = r#"
        /// A simple macro for creating a vector
        #[macro_export]
        macro_rules! myvec {
            ( $( $x:expr ),* ) => { ... };
        }
    "#;

    rt(source, expected_output);
}

#[test]
fn test_render_macro_in_module() {
    let source = r#"
        pub mod macros {
            /// A public macro in a module
            #[macro_export]
            macro_rules! public_macro {
                ($x:expr) => {
                    $x * 2
                };
            }

            // A private macro in a module
            macro_rules! private_macro {
                ($x:expr) => {
                    $x + 1
                };
            }
        }
    "#;

    // #[macro_export] pulls the macro to the top of the crate
    let expected_output = r#"
        pub mod macros {
        }
        /// A public macro in a module
        #[macro_export]
        macro_rules! public_macro {
            ($x:expr) => { ... };
        }
    "#;

    rt(source, expected_output);
}

#[test]
fn test_render_proc_macro() {
    let source = r#"
        extern crate proc_macro;

        use proc_macro::TokenStream;

        /// Expands to the function `answer` that returns `42`.
        #[proc_macro]
        pub fn make_answer(_input: TokenStream) -> TokenStream {
            "fn answer() -> u32 { 42 }".parse().unwrap()
        }

        /// Derives the HelloMacro trait for the input type.
        #[proc_macro_derive(HelloMacro)]
        pub fn hello_macro_derive(input: TokenStream) -> TokenStream {
            // Implementation here
            input
        }

        /// Attribute macro for routing.
        #[proc_macro_attribute]
        pub fn route(attr: TokenStream, item: TokenStream) -> TokenStream {
            // Implementation here
            item
        }
    "#;

    let expected_output = r#"
        /// Expands to the function `answer` that returns `42`.
        #[proc_macro]
        pub fn make_answer(input: proc_macro::TokenStream) -> proc_macro::TokenStream {}

        /// Derives the HelloMacro trait for the input type.
        #[proc_macro_derive(HelloMacro)]
        pub fn HelloMacro(input: proc_macro::TokenStream) -> proc_macro::TokenStream {}

        /// Attribute macro for routing.
        #[proc_macro_attribute]
        pub fn route(attr: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {}
    "#;

    rt_procmacro(source, expected_output);
}

#[test]
fn test_render_proc_macro_with_attributes() {
    let source = r#"
        extern crate proc_macro;
        use proc_macro::TokenStream;

        /// A derive macro for generating Debug implementations.
        #[proc_macro_derive(MyDebug, attributes(debug_format))]
        pub fn my_debug(input: TokenStream) -> TokenStream {}

        /// An attribute macro for timing function execution.
        #[proc_macro_attribute]
        pub fn debug_format(attr: TokenStream, item: TokenStream) -> TokenStream {}
    "#;

    let expected_output = r#"
        /// A derive macro for generating Debug implementations.
        #[proc_macro_derive(MyDebug, attributes(debug_format))]
        pub fn MyDebug(input: proc_macro::TokenStream) -> proc_macro::TokenStream {}

        /// An attribute macro for timing function execution.
        #[proc_macro_attribute]
        pub fn debug_format(attr: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {}
    "#;

    rt_procmacro(source, expected_output);
}
