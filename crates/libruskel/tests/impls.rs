mod utils;
use libruskel::Renderer;
use utils::*;

gen_tests! {
    impl_tests, {
        idemp {
            basic: r#"
                struct BasicStruct;
                
                impl BasicStruct {
                    pub fn new() -> Self {}
                    
                    pub fn public_method(&self) {}
                    
                    fn private_method(&self) {}
                }
            "#
        }
        idemp {
            trait_impl: r#"
                trait SomeTrait {
                    fn trait_method(&self);
                }
                
                struct TraitStruct;
                
                impl SomeTrait for TraitStruct {
                    fn trait_method(&self) {}
                }
            "#
        }
        idemp {
            generic_impl: r#"
                struct GenericStruct<T>(T);
                
                impl<T> GenericStruct<T> {
                    pub fn new(value: T) -> Self {}
                }
            "#
        }
        idemp {
            impl_with_where_clause: r#"
                struct WhereStruct<T>(T);
                
                impl<T> WhereStruct<T>
                where
                    T: Clone,
                {
                    pub fn cloned(&self) -> Self {}
                }
            "#
        }
        idemp {
            impl_for_generic_trait: r#"
                pub trait GenericTrait<T> {
                    fn generic_method(&self, value: T);
                }
                
                struct GenericTraitStruct;
                
                impl<U> GenericTrait<U> for GenericTraitStruct {
                    fn generic_method(&self, value: U) {}
                }
            "#
        }
        idemp {
            associated_types_impl: r#"
                struct AssocTypeStruct;
                
                impl TraitWithAssocType for AssocTypeStruct {
                    type Item = i32;
                    fn get_item(&self) -> Self::Item {
                    }
                }

                trait TraitWithAssocType {
                    type Item;
                    fn get_item(&self) -> Self::Item;
                }
            "#
        }
        idemp {
            assoicated_type_bounds: r#"
                struct BoundedAssocTypeStruct;
                
                impl BoundedAssocType for BoundedAssocTypeStruct {
                    type Item = i32;
                    fn get_item(&self) -> Self::Item {
                    }
                }

                trait BoundedAssocType {
                    type Item: Clone + 'static;
                    fn get_item(&self) -> Self::Item;
                }
            "#
        }
        idemp {
            default_impl: r#"
                trait DefaultTrait {
                    fn default_method(&self) { }
                }
                
                struct DefaultImpl;
                
                impl DefaultTrait for DefaultImpl {}
            "#
        }
        idemp {
            impl_with_const_fn: r#"
                struct ConstStruct;
                
                impl ConstStruct {
                    pub const fn const_method(&self) -> i32 { }
                }
            "#
        }
        idemp {
            impl_with_async_fn: r#"
                struct AsyncStruct;
                
                impl AsyncStruct {
                    pub async fn async_method(&self) {}
                }
            "#
        }
        idemp {
            deserialize: r#"
            pub trait Deserialize<'de>: Sized {
                fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
                where
                    D: Deserializer<'de>;
            }

            pub trait Deserializer<'de>: Sized {
                type Error;
            }

            pub struct Message;

            impl<'de> Deserialize<'de> for Message {
                fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
                where
                    D: Deserializer<'de>
                {
                }
            }
            "#
        }
        // FIXME: This appears to be a bug in rustdoc - unsafe is not set on the unsafe impl block.
        rt {
            unsafe_impl: {
                input: r#"
                    pub unsafe trait UnsafeTrait {
                        unsafe fn unsafe_method(&self);
                    }

                    pub struct UnsafeStruct;

                    unsafe impl UnsafeTrait for UnsafeStruct {
                        unsafe fn unsafe_method(&self) {}
                    }
                "#,
                output: r#"
                    pub unsafe trait UnsafeTrait {
                        unsafe fn unsafe_method(&self);
                    }

                    pub struct UnsafeStruct;

                    impl UnsafeTrait for UnsafeStruct {
                        unsafe fn unsafe_method(&self) {}
                    }
                "#
            }
        }
        rt {
            private_impl: {
                input: r#"
                    pub struct PublicStruct;
                    
                    impl PublicStruct {
                        pub fn public_method(&self) {}
                        fn private_method(&self) {}
                    }
                "#,
                output: r#"
                    pub struct PublicStruct;
                    
                    impl PublicStruct {
                        pub fn public_method(&self) {}
                    }
                "#
            }
        }
        rt {
            private_trait_impl: {
                input: r#"
                    trait PrivateTrait {
                        fn trait_method(&self);
                    }
                    
                    pub struct PublicStruct;
                    
                    impl PrivateTrait for PublicStruct {
                        fn trait_method(&self) {}
                    }
                "#,
                output: r#"
                    pub struct PublicStruct;
                "#
            }
        }
        rt {
            blanket_impl_disabled: {
                input: r#"
                    pub trait SomeTrait {
                        fn trait_method(&self);
                    }
                    
                    impl<T: Clone> SomeTrait for T {
                        fn trait_method(&self) {}
                    }
                "#,
                output: r#"
                    pub trait SomeTrait {
                        fn trait_method(&self);
                    }
                "#
            }
        }
        rt_custom {
            blanket_impl_enabled: {
                renderer: Renderer::default().with_blanket_impls(true),
                input: r#"
                    pub trait MyTrait {
                        fn trait_method(&self);
                    }

                    impl<T: Clone> MyTrait for T {
                        fn trait_method(&self) {}
                    }

                    pub struct MyStruct;

                    impl Clone for MyStruct {
                        fn clone(&self) -> Self {
                            MyStruct
                        }
                    }
                "#,
                output: r#"
                    pub trait MyTrait {
                        fn trait_method(&self);
                    }

                    pub struct MyStruct;

                    impl<T> MyTrait for MyStruct
                    where
                        T: Clone,
                    {
                        fn trait_method(&self) {}
                    }

                    impl Clone for MyStruct {
                        fn clone(&self) -> Self {}
                    }
                "#
            }
        }
    }
}
