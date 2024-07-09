mod utils;
use libruskel::Renderer;
use utils::*;

gen_tests! {
    enums, {
        idemp {
            basic: r#"
                pub enum BasicEnum {
                    Variant1,
                    Variant2,
                    Variant3,
                }
            "#
        }
        idemp {
            with_tuple_variants: r#"
                pub enum TupleEnum {
                    Variant1(i32, String),
                    Variant2(bool),
                }
            "#
        }
        idemp {
            with_struct_variants: r#"
                pub enum StructEnum {
                    Variant1 {
                        field1: i32,
                        field2: String,
                    },
                    Variant2 {
                        field: bool,
                    },
                }
            "#
        }
        idemp {
            mixed_variants: r#"
                pub enum MixedEnum {
                    Variant1,
                    Variant2(i32, String),
                    Variant3 {
                        field: bool,
                    },
                }
            "#
        }
        idemp {
            with_discriminants: r#"
                pub enum DiscriminantEnum {
                    Variant1 = 1,
                    Variant2 = 2,
                    Variant3 = 4,
                }
            "#
        }
        idemp {
            generic: r#"
                pub enum GenericEnum<T, U> {
                    Variant1(T),
                    Variant2(U),
                    Variant3(T, U),
                }
            "#
        }
        idemp {
            with_lifetime: r#"
                pub enum LifetimeEnum<'a> {
                    Variant1(&'a str),
                    Variant2(String),
                }
            "#
        }
        idemp {
            with_where_clause: r#"
                pub enum WhereEnum<T, U>
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
            "#
        }
        rt {
            private_enum: {
                input: r#"
                    enum PrivateEnum {
                        Variant1,
                        Variant2(i32),
                    }
                "#,
                output: r#"
                "#
            }
        }
        rt {
            private_variants: {
                input: r#"
                    pub enum PrivateVariantsEnum {
                        Variant1,
                        #[doc(hidden)]
                        Variant2,
                    }
                "#,
                output: r#"
                    pub enum PrivateVariantsEnum {
                        Variant1,
                    }
                "#
            }
        }
        rt_custom {
            pub_enum_with_private_rendering: {
                renderer: Renderer::default().with_private_items(false),
                input: r#"
                    pub enum PubEnumWithPrivateFields {
                        Variant1,
                        Variant2(i32),
                        Variant3 {
                            field1: String,
                            field2: bool,
                        }
                    }

                    enum PrivateEnum {
                        PrivateVariant1,
                        PrivateVariant2,
                    }
                "#,
                output: r#"
                    pub enum PubEnumWithPrivateFields {
                        Variant1,
                        Variant2(i32),
                        Variant3 {
                            field1: String,
                            field2: bool,
                        }
                    }
                "#
            }
        }
    }


}
