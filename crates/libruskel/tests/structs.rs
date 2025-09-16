//! Integration tests covering struct rendering scenarios.
mod utils;
use utils::*;

gen_tests! {
    tuple_struct, {
        idemp {
            basic: r#"
                pub struct BasicTuple(i32, String);
            "#
        }
        idemp {
            with_pub_fields: r#"
                pub struct PubFieldsTuple(pub i32, pub String);
            "#
        }
        idemp {
            mixed_visibility: r#"
                pub struct MixedVisibilityTuple(pub i32, String, pub bool);
            "#
        }
        idemp {
            generic: r#"
                pub struct GenericTuple<T, U>(T, U);
            "#
        }
        idemp {
            with_lifetime: r#"
                pub struct LifetimeTuple<'a>(&'a str, String);
            "#
        }
        idemp {
            with_lifetime_and_generic: r#"
                pub struct MixedTuple<'a, T>(&'a str, T);
            "#
        }
        idemp {
            with_where_clause: r#"
                pub struct WhereTuple<T, U>(T, U)
                where
                    T: Clone,
                    U: Default;
            "#
        }
        idemp {
            complex: r#"
                pub struct ComplexTuple<'a, T, U>(&'a str, T, U, i32)
                where
                    T: Clone,
                    U: Default + 'a;
            "#
        }
        rt {
            with_private_fields: {
                input: r#"
                    pub struct PrivateFieldsTuple(pub i32, String, pub bool);
                "#,
                output: r#"
                    pub struct PrivateFieldsTuple(pub i32, _, pub bool);
                "#
            }
        }
        rt {
            generic_with_private_fields: {
                input: r#"
                    pub struct GenericPrivateTuple<T, U>(pub T, U);
                "#,
                output: r#"
                    pub struct GenericPrivateTuple<T, U>(pub T, _);
                "#
            }
        }
        rt {
            only_private_fields: {
                input: r#"
                    pub struct OnlyPrivateTuple(String, i32);
                "#,
                output: r#"
                    pub struct OnlyPrivateTuple(_, _);
                "#
            }
        }
        rt {
            private_struct: {
                input: r#"
                    struct PrivateTuple(i32, String);
                "#,
                output: r#"
                "#
            }
        }
    }
}

gen_tests! {
    unit_struct, {
        idemp {
            basic: r#"
                pub struct UnitStruct;
            "#
        }
        rt {
            private: {
                input: r#"
                    struct PrivateUnitStruct;
                "#,
                output: r#""#
            }
        }
    }
}

gen_tests! {
    plain_struct, {
        idemp {
            empty: r#"
                pub struct EmptyStruct {}
            "#
        }
        idemp {
            basic: r#"
                pub struct BasicStruct {
                    pub field1: i32,
                    field2: String,
                }
            "#
        }
        idemp {
            generic: r#"
                pub struct GenericStruct<T, U> {
                    pub field1: T,
                    field2: U,
                }
            "#
        }
        idemp {
            with_lifetime: r#"
                pub struct LifetimeStruct<'a> {
                    field: &'a str,
                }
            "#
        }
        idemp {
            with_lifetime_and_generic: r#"
                pub struct MixedStruct<'a, T> {
                    reference: &'a str,
                    value: T,
                }
            "#
        }
        idemp {
            with_where_clause: r#"
                pub struct WhereStruct<T, U>
                where
                    T: Clone,
                    U: Default,
                {
                    pub field1: T,
                    field2: U,
                }
            "#
        }
        rt {
            with_private_fields: {
                input: r#"
                    pub struct PrivateFieldStruct {
                        pub field1: i32,
                        field2: String,
                    }
                "#,
                output: r#"
                    pub struct PrivateFieldStruct {
                        pub field1: i32,
                    }
                "#
            }
        }
        rt {
            generic_with_private_fields: {
                input: r#"
                    pub struct GenericPrivateFieldStruct<T, U> {
                        pub field1: T,
                        field2: U,
                    }
                "#,
                output: r#"
                    pub struct GenericPrivateFieldStruct<T, U> {
                        pub field1: T,
                    }
                "#
            }
        }
        rt {
            where_clause_with_private_fields: {
                input: r#"
                    pub struct WherePrivateFieldStruct<T, U>
                    where
                        T: Clone,
                        U: Default,
                    {
                        pub field1: T,
                        field2: U,
                    }
                "#,
                output: r#"
                    pub struct WherePrivateFieldStruct<T, U>
                    where
                        T: Clone,
                        U: Default,
                    {
                        pub field1: T,
                    }
                "#
            }
        }
        rt {
            only_private_fields: {
                input: r#"
                    pub struct OnlyPrivateFieldStruct {
                        field: String,
                    }
                "#,
                output: r#"
                    pub struct OnlyPrivateFieldStruct {}
                "#
            }
        }
    }
}
