mod utils;
use utils::*;

gen_tests! {
    modules, {
        idemp {
            basic: r#"
                pub mod basic_module {
                    pub fn public_function() {}
                    fn private_function() {}
                }
            "#
        }
        idemp {
            nested: r#"
                pub mod outer {
                    pub mod inner {
                        pub fn nested_function() {}
                    }
                    pub fn outer_function() {}
                }
            "#
        }
        idemp {
            with_structs_and_enums: r#"
                pub mod types {
                    pub struct PublicStruct {
                        pub field: i32,
                    }
                    
                    struct PrivateStruct {
                        field: i32,
                    }
                    
                    pub enum PublicEnum {
                        Variant1,
                        Variant2,
                    }
                    
                    enum PrivateEnum {
                        Variant1,
                        Variant2,
                    }
                }
            "#
        }
        idemp {
            with_traits: r#"
                pub mod traits {
                    pub trait PublicTrait {
                        fn public_method(&self);
                    }
                    
                    trait PrivateTrait {
                        fn private_method(&self);
                    }
                }
            "#
        }
        idemp {
            with_constants: r#"
                pub mod constants {
                    pub const PUBLIC_CONSTANT: i32 = 42;
                    const PRIVATE_CONSTANT: i32 = 7;
                }
            "#
        }
        idemp {
            with_type_aliases: r#"
                pub mod aliases {
                    pub type PublicAlias = Vec<String>;
                    type PrivateAlias = std::collections::HashMap<i32, String>;
                }
            "#
        }
        idemp {
            with_doc_comments_inner: r#"
                pub mod documented {
                    //! This is an inner module-level doc comment
                    
                    /// This is a documented function
                    pub fn documented_function() {}
                }
            "#
        }
        rt {
            module_with_inline_imports: {
                input: r#"
                    mod private_module {
                        pub struct PrivateStruct1;
                        pub struct PrivateStruct2;
                        struct NonPublicStruct;
                    }

                    pub mod public_module {
                        pub struct PublicStruct1;
                        pub struct PublicStruct2;
                        pub use super::private_module::PrivateStruct1;
                        pub use super::private_module::PrivateStruct2;
                    }

                    pub use self::public_module::PublicStruct1;
                    pub use self::public_module::PublicStruct2;
                    pub use self::public_module::PrivateStruct1;
                    pub use self::public_module::PrivateStruct2;
                "#,
                output: r#"
                    pub mod public_module {
                        pub struct PublicStruct1;
                        pub struct PublicStruct2;
                        pub struct PrivateStruct1;
                        pub struct PrivateStruct2;
                    }
                    pub struct PublicStruct1;
                    pub struct PublicStruct2;
                    pub struct PrivateStruct1;
                    pub struct PrivateStruct2;
                "#
            }
        }
        rt {
            module_with_glob_imports: {
                input: r#"
                    mod private_module {
                        pub struct PrivateStruct1;
                        pub struct PrivateStruct2;
                        struct NonPublicStruct;
                    }

                    pub mod public_module {
                        pub struct PublicStruct1;
                        pub struct PublicStruct2;
                        pub use super::private_module::*;
                    }

                    pub use self::public_module::*;
                "#,
                output: r#"
                    pub mod public_module {
                        pub struct PublicStruct1;
                        pub struct PublicStruct2;
                        pub struct PrivateStruct1;
                        pub struct PrivateStruct2;
                    }
                    pub struct PublicStruct1;
                    pub struct PublicStruct2;
                    pub struct PrivateStruct1;
                    pub struct PrivateStruct2;
                "#
            }
        }
        rt {
            with_doc_comments_outer: {
                input: r#"
                    /// This is a documented module, with outer comments
                    pub mod documented {
                        
                        /// This is a documented function
                        pub fn documented_function() {}
                    }
                "#,
                output: r#"
                    pub mod documented {
                        //! This is a documented module, with outer comments
                        
                        /// This is a documented function
                        pub fn documented_function() {}
                    }
                "#
            }
        }
        rt {
            with_multi_doc_comments: {
                input: r#"
                    /// This is a documented module, with duplicate comments
                    pub mod documented {
                        //! This is a module-level doc comment
                        
                        /// This is a documented function
                        pub fn documented_function() {}
                    }
                "#,
                output: r#"
                    pub mod documented {
                        //! This is a documented module, with duplicate comments
                        //! This is a module-level doc comment
                        
                        /// This is a documented function
                        pub fn documented_function() {}
                    }
                "#
            }
        }
        rt {
            with_use_statements: {
                input: r#"
                    pub mod use_module {
                        use std::collections::HashMap;
                        pub use std::vec::Vec;
                        
                        pub fn use_hash_map() -> HashMap<String, i32> {
                            HashMap::new()
                        }
                    }
                "#,
                output: r#"
                    pub mod use_module {
                        pub use std::vec::Vec;
                        
                        pub fn use_hash_map() -> std::collections::HashMap<String, i32> { }
                    }
                "#
            }
        }
        rt {
            private_module: {
                input: r#"
                    mod private_module {
                        pub fn function_in_private_module() {}
                    }
                "#,
                output: r#"
                "#
            }
        }
        rt {
            mixed_visibility: {
                input: r#"
                    pub mod mixed {
                        pub fn public_function() {}
                        fn private_function() {}
                        pub struct PublicStruct;
                        struct PrivateStruct;
                    }
                "#,
                output: r#"
                    pub mod mixed {
                        pub fn public_function() {}
                        pub struct PublicStruct;
                    }
                "#
            }
        }
        rt {
            re_exports: {
                input: r#"
                    mod private {
                        pub struct ReExported;
                    }
                    
                    pub mod public {
                        pub use super::private::ReExported;
                    }
                "#,
                output: r#"
                    pub mod public {
                        pub struct ReExported;
                    }
                "#
            }
        }
        rt {
            private_and_public_imports: {
                input: r#"
                    pub mod import_visibility {
                        use std::collections::HashMap;
                        pub use std::vec::Vec;
                        use std::fmt::Debug;
                        pub use std::fmt::Display;

                        pub fn public_vec() -> Vec<i32> {
                            Vec::new()
                        }

                        fn private_hash_map() -> HashMap<String, i32> {
                            HashMap::new()
                        }
                    }
                "#,
                output: r#"
                    pub mod import_visibility {
                        pub use std::vec::Vec;
                        pub use std::fmt::Display;

                        pub fn public_vec() -> Vec<i32> { }
                    }
                "#
            }
        }
        rt {
            re_exports_with_glob: {
                input: r#"
                    mod private {
                        pub struct ReExported1;
                        pub struct ReExported2;
                    }

                    pub mod public {
                        pub use super::private::*;
                        pub use std::collections::HashMap;
                    }
                "#,
                output: r#"
                    pub mod public {
                        pub use std::collections::HashMap;
                        pub struct ReExported1;
                        pub struct ReExported2;
                    }
                "#
            }
        }
        rt {
            nested_re_exports: {
                input: r#"
                    mod level1 {
                        pub mod level2 {
                            pub struct DeepStruct;
                        }
                    }

                    pub mod re_export {
                        pub use super::level1::level2::*;
                    }

                    pub use re_export::DeepStruct;
                "#,
                output: r#"
                    pub mod re_export {
                        pub struct DeepStruct;
                    }

                    pub struct DeepStruct;
                "#
            }
        }
    }
}
