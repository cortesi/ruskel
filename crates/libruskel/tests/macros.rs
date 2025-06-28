mod utils;
use utils::*;

#[test]
fn test_macro_expansion_with_dollar_signs() {
    // This test reproduces the issue with $ signs in macro expansions
    let source = r#"
        #[macro_export]
        macro_rules! define_simd_type {
            ($name:ident, $size:expr, $elems:expr) => {
                type Bytes = Simd<u8, { $size * $elems }>;
            };
        }

        #[macro_export]
        macro_rules! define_simd_alias {
            ($name:ident, $num_elements:expr) => {
                pub type $name = simd::Simd<f32, $num_elements>;
            };
        }

        // Usage
        define_simd_type!(MyType, 4, 8);
        define_simd_alias!(f32x1, 1);
    "#;

    // The expected output should show the macro definitions
    // but not the unexpanded macro invocations
    let expected_output = r#"
        #[macro_export]
        macro_rules! define_simd_type {
            ($name:ident, $size:expr, $elems:expr) => { ... };
        }

        #[macro_export]
        macro_rules! define_simd_alias {
            ($name:ident, $num_elements:expr) => { ... };
        }
    "#;

    rt(source, expected_output);
}

#[test]
fn test_macro_expansion_in_type_alias() {
    // Test case for macro expansions that generate type aliases
    // The macro invocation itself should not appear in the output
    let source = r#"
        use std::simd::Simd;

        #[macro_export]
        macro_rules! simd_bytes_type {
            ($size:expr, $elems:expr) => {
                type Bytes = Simd<u8, { $size * $elems }>;
            };
        }

        simd_bytes_type!(4, 8);
    "#;

    // Only the macro definition should appear, not the expansion
    let expected_output = r#"
        #[macro_export]
        macro_rules! simd_bytes_type {
            ($size:expr, $elems:expr) => { ... };
        }
    "#;

    rt(source, expected_output);
}