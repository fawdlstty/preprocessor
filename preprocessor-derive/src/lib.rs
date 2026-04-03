use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Expr, ItemFn};

mod evaluator;

/// `preprocessor::op!(...)` — Expression-level macro for compile-time evaluation.
///
/// Parses the input expression, evaluates all evaluable sub-expressions at compile time,
/// and replaces them with literal results. Non-evaluable parts (free variables, etc.)
/// are passed through unchanged.
///
/// # Example
/// ```ignore
/// let result = preprocessor::op!(1 + 2 * 3);  // → let result = 7;
/// let x = preprocessor::op!(a + 1);           // → let x = a + 1; (passthrough)
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
