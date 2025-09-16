//! Integration tests ensuring trait rendering stays stable.
mod utils;
use utils::*;

gen_tests! {
    traits, {
        idemp {
            basic: r#"
                pub trait BasicTrait {
                    fn method(&self);
                    fn default_method(&self) {
                    }
                }
            "#
        }
        idemp {
            with_associated_types: r#"
                pub trait TraitWithAssocTypes {
                    type Item;
                    type Container<T>;
                    fn get_item(&self) -> Self::Item;
                }
            "#
        }
        idemp {
            with_associated_consts: r#"
                pub trait TraitWithAssocConsts {
                    const CONSTANT: i32;
                    const DEFAULT_CONSTANT: bool = true;
                }
            "#
        }
        idemp {
            generic: r#"
                pub trait GenericTrait<T, U> {
                    fn process(&self, t: T) -> U;
                }
            "#
        }
        idemp {
            with_lifetime: r#"
                pub trait LifetimeTrait<'a> {
                    fn process(&self, data: &'a str) -> &'a str;
                }
            "#
        }
        idemp {
            with_supertraits: r#"
                pub trait SuperTrait: std::fmt::Debug + Clone {
                    fn super_method(&self);
                }
            "#
        }
        idemp {
            with_where_clause: r#"
                pub trait WhereTraitMulti<T, U>
                where
                    T: Clone,
                    U: Default,
                {
                    fn process(&self, t: T, u: U);
                }
            "#
        }
        idemp {
            unsafe_trait: r#"
                pub unsafe trait UnsafeTrait {
                    unsafe fn unsafe_method(&self);
                }
            "#
        }
        idemp {
            with_associated_type_bounds: r#"
                pub trait BoundedAssocType {
                    type Item: Clone + 'static;
                    fn get_item(&self) -> Self::Item;
                }
            "#
        }
        idemp {
            with_self_type: r#"
                pub trait WithSelfType {
                    fn as_ref(&self) -> &Self;
                    fn into_owned(self) -> Self;
                }
            "#
        }
        rt {
            private_items: {
                input: r#"
                    pub trait TraitWithPrivateItems {
                        fn public_method(&self);
                        #[doc(hidden)]
                        fn private_method(&self);
                        type PublicType;
                        #[doc(hidden)]
                        type PrivateType;
                    }
                "#,
                output: r#"
                    pub trait TraitWithPrivateItems {
                        fn public_method(&self);
                        type PublicType;
                    }
                "#
            }
        }
        rt {
            private_trait: {
                input: r#"
                    trait PrivateTrait {
                        fn method(&self);
                    }
                "#,
                output: r#"
                "#
            }
        }
    }
}
