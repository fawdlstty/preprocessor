use proc_macro::TokenStream;
use quote::quote;
use syn::{Expr, File, ItemFn, parse_macro_input};

mod evaluator;
mod evcxr_engine;
mod prelude;

/// `preprocessor::op!(...)` — Expression-level macro for compile-time evaluation.
///
/// Parses the input expression, evaluates it at compile time,
/// and replaces it with the literal result.
///
/// **Important**: If the expression cannot be fully evaluated at compile time
/// (e.g., contains free variables, unknown functions, etc.), the macro will
/// generate a compile-time error instead of passing through unchanged.
///
/// # Example
/// ```ignore
/// let result = preprocessor::op!(1 + 2 * 3);  // → let result = 7;
/// let x = preprocessor::op!(a + 1);           // → COMPILE ERROR: cannot evaluate at compile time
/// ```
#[proc_macro]
pub fn op(input: TokenStream) -> TokenStream {
    let expr = parse_macro_input!(input as Expr);

    let (transformed, changed) = evaluator::transform_expr(&expr);

    if changed {
        quote!(#transformed).into()
    } else {
        quote!(#expr).into()
    }
}

/// `#[preprocessor::optimize]` — Function-level attribute macro for compile-time optimization.
///
/// Recursively traverses all statements and expressions in the function body,
/// identifies sub-expressions that can be evaluated at compile time,
/// and rewrites them as pre-computed literals.
///
/// # Example
/// ```ignore
/// #[preprocessor::optimize]
/// fn compute() -> i32 {
///     let x = 1 + 2;  // → let x = 3;
///     let y = x * 10; // kept as-is (depends on local variable)
///     y
/// }
/// ```
#[proc_macro_attribute]
pub fn optimize(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input_fn = parse_macro_input!(item as ItemFn);

    // Transform the function body
    let transformed_block = evaluator::transform_block(&input_fn.block);

    let attrs = &input_fn.attrs;
    let vis = &input_fn.vis;
    let sig = &input_fn.sig;

    let output = quote! {
        #(#attrs)*
        #vis #sig {
            #transformed_block
        }
    };

    output.into()
}

/// `#[preprocessor::prelude]` — Crate-level attribute macro for automatic dependency resolution.
///
/// Scans the crate for `op!` macro usages, identifies unqualified paths (like `Local`),
/// resolves them to their full crate paths (like `chrono::Local`) based on `use` statements,
/// and rewrites the `op!` calls to use these full paths. This allows `op!` to work
/// without requiring fully qualified paths inside the macro invocation.
///
/// # Example
/// ```ignore
/// #![preprocessor::prelude]
///
/// fn main() {
///     // Works even without `use chrono::Local;` if `Local` is imported elsewhere
///     let time = preprocessor::op!(Local::now().to_string());
/// }
/// ```
#[proc_macro_attribute]
pub fn prelude(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let file = parse_macro_input!(item as File);
    let output = prelude::process_file(file);

    quote!(#output).into()
}
