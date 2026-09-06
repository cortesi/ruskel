//! Integration tests validating function signature rendering.
mod utils;
use libruskel::Renderer;
use utils::*;

gen_tests! {
    functions, {
        idemp {
            basic: r#"
                pub fn basic_function() {}
            "#
        }
        idemp {
            with_args: r#"
                pub fn function_with_args(x: i32, y: String) {}
            "#
        }
        idemp {
            with_return: r#"
                pub fn function_with_return() -> i32 {
                }
            "#
        }
        idemp {
            generic: r#"
                pub fn generic_function<T>(value: T) -> T {
                }
            "#
        }
        idemp {
            with_lifetime: r#"
                pub fn lifetime_function<'a>(x: &'a str) -> &'a str {
                }
            "#
        }
        idemp {
            with_where_clause: r#"
                pub fn where_function<T>(value: T) -> T
                where
                    T: Clone,
                {
                }
            "#
        }
        idemp {
            async_function: r#"
                pub async fn async_function() {}
            "#
        }
        idemp {
            const_function: r#"
                pub const fn const_function() -> i32 {
                }
            "#
        }
        idemp {
            unsafe_function: r#"
                pub unsafe fn unsafe_function() {}
            "#
        }
        idemp {
            unsafe_c_function: r#"
                pub unsafe extern "C" fn unsafe_c_function(value: i32) -> i32 {}
            "#
        }
        idemp {
            c_unwind_function: r#"
                pub extern "C-unwind" fn c_unwind_function(value: i32) -> i32 {}
            "#
        }
        idemp {
            complex: r#"
                pub async unsafe fn complex_function<'a, T, U>(x: &'a T, y: U) -> Result<T, U>
                where
                    T: Clone + Send + 'a,
                    U: std::fmt::Debug,
                {
                }
            "#
        }
        idemp {
            function_pointer: r#"
                pub fn function_with_fn_pointer(f: fn(arg1: i32, arg2: String) -> bool) {
                }
            "#
        }
        idemp {
            unsafe_c_function_pointer: r#"
                pub fn function_with_unsafe_c_pointer(
                    callback: unsafe extern "C" fn(value: i32) -> i32,
                ) {
                }
            "#
        }
        idemp {
            hrtb_function_pointer: r#"
                pub fn function_with_hrtb_pointer(
                    callback: for<'a> fn(value: &'a i32) -> &'a i32,
                ) {
                }
            "#
        }
        idemp {
            singleton_tuple_types: r#"
                pub fn singleton_tuple(value: (u32,)) -> (u32,) {}
            "#
        }
        idemp {
            hrtb: r#"
                pub fn hrtb_function<F>(f: F)
                where
                    for<'a> F: Fn(&'a str) -> bool,
                {
                }
            "#
        }
        idemp {
            dyn_trait_arg: r#"
                pub fn function_with_dyn_trait(arg: &dyn std::fmt::Debug) {}
            "#
        }
        idemp {
            multiple_dyn_trait_args: r#"
                pub fn function_with_multiple_dyn_traits(
                    arg1: &dyn std::fmt::Debug,
                    arg2: Box<dyn std::fmt::Display>,
                ) {}
            "#
        }
        idemp {
            dyn_trait_with_lifetime: r#"
                pub fn function_with_dyn_trait_lifetime<'a>(arg: &'a dyn std::fmt::Debug) {}
            "#
        }
        idemp {
            dyn_trait_return: r#"
                pub fn function_returning_dyn_trait() -> Box<dyn std::fmt::Debug> { }
            "#
        }
        idemp {
            dyn_trait_parens: r#"
                pub fn myfn() -> &'static (dyn std::any::Any + 'static) { }
            "#
        }
        idemp {
            dyn_trait_with_associated_type: r#"
                pub trait Iterator {
                    type Item;
                    fn next(&mut self) -> Option<Self::Item>;
                }
                pub fn function_with_dyn_iterator(iter: &mut dyn Iterator<Item = i32>) {}
            "#
        }
        idemp {
            dyn_fn_sync: r#"
                pub fn function_with_dyn_fn_sync(f: &(dyn Fn() + Sync)) {}
            "#
        }
        idemp {
            dyn_multiple_bounds: r#"
                pub fn function_with_dyn_multiple_bounds(arg: &(dyn std::fmt::Debug + Send)) {}
            "#
        }
        idemp {
            box_dyn_fn_send_sync: r#"
                pub fn function_with_box_dyn_fn(f: Box<dyn Fn() + Send + Sync>) {}
            "#
        }
        idemp {
            impl_trait_with_multiple_bounds: r#"
                pub fn request_value<'a, T>(err: &'a (impl std::error::Error + ?Sized)) -> Option<T> 
                where 
                    T: 'static 
                {
                }
            "#
        }
        idemp {
            impl_trait_single_bound: r#"
                pub fn takes_impl_error(err: &impl std::error::Error) {}
            "#
        }
        idemp {
            impl_trait_sized_bound_only: r#"
                pub fn takes_impl_sized(val: &impl ?Sized) {}
            "#
        }
        idemp {
            impl_trait_complex_bounds: r#"
                pub fn complex_impl<T>(val: &(impl Iterator<Item = T> + Send)) 
                where 
                    T: Clone 
                {
                }
            "#
        }
        rt {
            private_function: {
                input: r#"
                    fn private_function() {}
                "#,
                output: r#"
                "#
            }
        }
        rt {
            with_doc_comments: {
                input: r#"
                    /// This is a documented function.
                    /// It has multiple lines of documentation.
                    pub fn documented_function() {}
                "#,
                output: r#"
                    /// This is a documented function.
                    /// It has multiple lines of documentation.
                    pub fn documented_function() {}
                "#
            }
        }
        rt {
           with_attributes: {
                input: r#"
                    #[inline]
                    #[cold]
                    pub fn function_with_attributes() {}
                "#,
                output: r#"
                    pub fn function_with_attributes() {}
                "#
            }
        }
        rt_custom {
            render_private: {
                renderer: Renderer::default().with_private_items(true),
                input: r#"
                    fn private_function() {}
                    pub fn public_function() {}
                "#,
                output: r#"
                    fn private_function() {}
                    pub fn public_function() {}
                "#
            }
        }
    }

}

#[cfg(test)]
mod standalone {
    use libruskel::{CrateRequest, SearchDomain, SearchOptions};

    use super::*;

    #[test]
    fn search_signatures_preserve_abi_and_singleton_tuples() {
        let source = r#"
            pub unsafe extern "C" fn unsafe_c(value: i32) -> i32 { value }

            pub fn call_c(callback: unsafe extern "C" fn(value: i32) -> i32) {}

            pub fn singleton(value: (u32,)) -> (u32,) { value }
        "#;
        let (_workspace, target) = create_test_crate(source, false);
        let (_cache, ruskel) = isolated_ruskel();
        let request = CrateRequest {
            private_items: true,
            ..CrateRequest::default()
        };

        let options = SearchOptions::configured("unsafe_c", SearchDomain::NAMES, false, true);
        let response = ruskel.search(&target, &request, &options).unwrap();
        let unsafe_c = response
            .results
            .iter()
            .find(|result| result.path_string.ends_with("::unsafe_c"))
            .expect("unsafe C function search result");
        assert_eq!(
            unsafe_c.signature.as_deref(),
            Some("pub unsafe extern \"C\" fn unsafe_c(value: i32)-> i32")
        );

        let options = SearchOptions::configured("call_c", SearchDomain::NAMES, false, true);
        let response = ruskel.search(&target, &request, &options).unwrap();
        let call_c = response
            .results
            .iter()
            .find(|result| result.path_string.ends_with("::call_c"))
            .expect("function-pointer search result");
        assert_eq!(
            call_c.signature.as_deref(),
            Some("pub fn call_c(callback: unsafe extern \"C\" fn(value: i32) -> i32)")
        );

        let options = SearchOptions::configured("singleton", SearchDomain::NAMES, false, true);
        let response = ruskel.search(&target, &request, &options).unwrap();
        let singleton = response
            .results
            .iter()
            .find(|result| result.path_string.ends_with("::singleton"))
            .expect("singleton-tuple search result");
        assert_eq!(
            singleton.signature.as_deref(),
            Some("pub fn singleton(value: (u32,))-> (u32,)")
        );
    }
}
