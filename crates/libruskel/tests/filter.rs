mod utils;
use libruskel::Renderer;
use utils::*;

gen_tests! {
    filtering, {
        rt_custom {
            filter_module: {
                // Test filtering a specific module
                // Module docs should be rendered for the filtered module
                renderer: Renderer::default().with_filter("my_module"),
                input: r#"
                    pub mod my_module {
                        //! My module docs
                        pub fn public_function() {}
                        fn private_function() {}
                    }
                    
                    pub mod other_module {
                        //! Other module docs
                        pub fn other_function() {}
                    }
                "#,
                output: r#"
                    pub mod my_module {
                        //! My module docs
                        pub fn public_function() {}
                    }
                "#
            }
        }
        rt_custom {
            filter_nested_module: {
                // Test filtering a nested module
                // Module docs should not be rendered for parent modules
                renderer: Renderer::default().with_filter("outer::inner"),
                input: r#"
                    pub mod outer {
                        //! Outer module docs
                        pub mod inner {
                            //! Inner module docs
                            pub fn inner_function() {}
                        }
                        pub fn outer_function() {}
                    }
                "#,
                output: r#"
                    pub mod outer {
                        pub mod inner {
                            //! Inner module docs
                            pub fn inner_function() {}
                        }
                    }
                "#
            }
        }
        rt_custom {
            filter_specific_item: {
                // Test filtering a specific item within a module
                // Module docs should not be rendered when filtering a specific item
                renderer: Renderer::default().with_filter("my_module::MyStruct"),
                input: r#"
                    pub mod my_module {
                        //! My module docs
                        /// MyStruct docs
                        pub struct MyStruct {
                            pub field: i32,
                        }
                        
                        pub fn other_function() {}
                    }
                "#,
                output: r#"
                    pub mod my_module {
                        /// MyStruct docs
                        pub struct MyStruct {
                            pub field: i32,
                        }
                    }
                "#
            }
        }
        rt_custom {
            filter_trait: {
                // Test filtering a trait
                // Module docs should not be rendered when filtering a trait
                renderer: Renderer::default().with_filter("my_module::MyTrait"),
                input: r#"
                    pub mod my_module {
                        //! My module docs
                        /// MyTrait docs
                        pub trait MyTrait {
                            fn trait_method(&self);
                        }
                        
                        pub struct MyStruct;
                        
                        impl MyTrait for MyStruct {
                            fn trait_method(&self) {}
                        }
                    }
                "#,
                output: r#"
                    pub mod my_module {
                        /// MyTrait docs
                        pub trait MyTrait {
                            fn trait_method(&self);
                        }
                    }
                "#
            }
        }
        rt_custom {
            filter_enum: {
                // Test filtering an enum
                // Module docs should not be rendered when filtering an enum
                renderer: Renderer::default().with_filter("my_module::MyEnum"),
                input: r#"
                    pub mod my_module {
                        //! My module docs
                        /// MyEnum docs
                        pub enum MyEnum {
                            Variant1,
                            Variant2(i32),
                            Variant3 { field: bool },
                        }

                        pub struct MyStruct;
                    }
                "#,
                output: r#"
                    pub mod my_module {
                        /// MyEnum docs
                        pub enum MyEnum {
                            Variant1,
                            Variant2(i32),
                            Variant3 { field: bool },
                        }
                    }
                "#
            }
        }
        rt_custom {
            filter_with_impl: {
                // Test filtering a struct with its impl
                // Module docs should not be rendered when filtering a struct
                renderer: Renderer::default().with_filter("my_module::MyStruct"),
                input: r#"
                    pub mod my_module {
                        //! My module docs
                        /// MyStruct docs
                        pub struct MyStruct;
                        
                        impl MyStruct {
                            pub fn new() -> Self {
                                MyStruct
                            }
                        }

                        pub fn other_function() {}
                    }
                "#,
                output: r#"
                    pub mod my_module {
                        /// MyStruct docs
                        pub struct MyStruct;
                        
                        impl MyStruct {
                            pub fn new() -> Self {}
                        }
                    }
                "#
            }
        }
        rt_custom {
            filter_impl_fn: {
                // Test filtering a struct with its impl
                // Module docs should not be rendered when filtering a struct
                renderer: Renderer::default().with_filter("my_module::MyStruct::new"),
                input: r#"
                    pub mod my_module {
                        //! My module docs
                        /// MyStruct docs
                        pub struct MyStruct;

                        impl MyStruct {
                            pub fn new() -> Self {
                                MyStruct
                            }

                            pub fn excluded() -> Self {
                                MyStruct
                            }
                        }
                    }
                "#,
                output: r#"
                    pub mod my_module {
                        /// MyStruct docs
                        pub struct MyStruct;

                        impl MyStruct {
                            pub fn new() -> Self {}
                        }
                    }
                "#
            }
        }
        rt_custom {
            filter_function: {
                // Test filtering a function
                // Module docs should not be rendered when filtering a function
                renderer: Renderer::default().with_filter("my_module::my_function"),
                input: r#"
                    pub mod my_module {
                        //! My module docs
                        /// my_function docs
                        pub fn my_function(x: i32) -> i32 {
                            x * 2
                        }
                        
                        pub fn other_function() {}
                        
                        pub struct MyStruct;
                    }
                "#,
                output: r#"
                    pub mod my_module {
                        /// my_function docs
                        pub fn my_function(x: i32) -> i32 {}
                    }
                "#
            }
        }
        rt_custom {
            filter_with_private_items: {
                // Test filtering with private items included
                // Module docs should be rendered for the filtered module
                renderer: Renderer::default().with_filter("my_module").with_private_items(true),
                input: r#"
                    pub mod my_module {
                        //! My module docs
                        pub fn public_function() {}
                        fn private_function() {}
                    }
                    
                    pub mod other_module {
                        //! Other module docs
                        pub fn other_function() {}
                    }
                "#,
                output: r#"
                    pub mod my_module {
                        //! My module docs
                        pub fn public_function() {}
                        fn private_function() {}
                    }
                "#
            }
        }
        rt_custom {
            no_filter: {
                // Test with no filter
                // All module docs should be rendered
                renderer: Renderer::default(),
                input: r#"
                    //! Root module docs
                    
                    pub mod a {
                        //! Module A docs
                        pub fn function_in_a() {}
                    }

                    pub mod b {
                        //! Module B docs
                        pub struct StructInB;
                    }
                "#,
                output: r#"
                    //! Root module docs
                    
                    pub mod a {
                        //! Module A docs
                        pub fn function_in_a() {}
                    }

                    pub mod b {
                        //! Module B docs
                        pub struct StructInB;
                    }
                "#
            }
        }
    }
}

gen_tests! {
    filter_error, {
        rt_custom {
            filter_matched: {
                renderer: Renderer::default().with_filter("my_module"),
                input: r#"
                    pub mod my_module {
                        pub fn my_function() {}
                    }
                "#,
                output: r#"
                    pub mod my_module {
                        pub fn my_function() {}
                    }
                "#
            }
        }
        rt_custom {
            no_filter: {
                renderer: Renderer::default(),
                input: r#"
                    pub mod my_module {
                        pub fn my_function() {}
                    }
                    pub mod other_module {
                        pub fn other_function() {}
                    }
                "#,
                output: r#"
                    pub mod my_module {
                        pub fn my_function() {}
                    }
                    pub mod other_module {
                        pub fn other_function() {}
                    }
                "#
            }
        }
        rt_custom {
            partial_match: {
                renderer: Renderer::default().with_filter("my_module"),
                input: r#"
                    pub mod my_module {
                        pub mod nested_module {
                            pub fn nested_function() {}
                        }
                    }
                "#,
                output: r#"
                    pub mod my_module {
                        pub mod nested_module {
                            pub fn nested_function() {}
                        }
                    }
                "#
            }
        }
        rt_err {
            filter_not_matched: {
                renderer: Renderer::default().with_filter("non_existent_module"),
                input: r#"
                    pub mod my_module {
                        pub fn my_function() {}
                    }
                "#,
                error: "Filter 'non_existent_module' did not match any items"
            }
        }

    }
}
