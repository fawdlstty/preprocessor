//! AST traversal evaluator — pure Rust interpreter for compile-time expression evaluation.
//!
//! Traverses `syn::Expr` AST, evaluates using Rust primitive types,
//! and outputs minimized AST (literal tokens).

use proc_macro2::{Span, TokenStream};
use quote::quote_spanned;
use syn::parse::Parser;
use syn::spanned::Spanned;
use syn::{
    BinOp, Expr, ExprArray, ExprBinary, ExprBlock, ExprCast, ExprGroup, ExprLit, ExprMacro,
    ExprParen, ExprTuple, ExprUnary, Lit, LitBool, LitByte, LitChar, LitFloat, LitInt, LitStr,
    Stmt, Token, UnOp,
};

/// Maximum evaluation steps before aborting (prevents infinite loops).
const MAX_EVAL_STEPS: u64 = 1_000_000;

/// Evaluation result.
#[derive(Debug, Clone)]
pub enum EvalResult {
    /// Successfully evaluated to a value.
    Value(Value),
    /// Cannot fully evaluate — contains free variables or unsupported constructs.
    /// The original expression should be passed through.
    PassThrough,
    /// Evaluation error (e.g., division by zero, overflow).
    Error(String),
}

/// Runtime value produced by the evaluator.
#[derive(Debug, Clone)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Char(char),
    Str(String),
    Byte(u8),
    Tuple(Vec<Value>),
    Array(Vec<Value>),
    /// Unit type `()` — result of statements with side effects.
    Unit,
}

impl Value {
    /// Convert a `Value` back to a `TokenStream` representing the literal.
    pub fn to_token(&self, span: Span) -> TokenStream {
        match self {
            Value::Int(n) => {
                let lit = LitInt::new(&n.to_string(), span);
                quote_spanned!(span => #lit)
            }
            Value::Float(f) => {
                let s = if f.is_infinite() {
                    if *f > 0.0 {
                        "f64::INFINITY"
                    } else {
                        "f64::NEG_INFINITY"
                    }
                } else if f.is_nan() {
                    "f64::NAN"
                } else {
                    let lit = LitFloat::new(&f.to_string(), span);
                    return quote_spanned!(span => #lit);
                };
                let lit = LitStr::new(s, span);
                quote_spanned!(span => #lit.parse::<f64>().unwrap())
            }
            Value::Bool(b) => {
                let lit = LitBool::new(*b, span);
                quote_spanned!(span => #lit)
            }
            Value::Char(c) => {
                let lit = LitChar::new(*c, span);
                quote_spanned!(span => #lit)
            }
            Value::Str(s) => {
                let lit = LitStr::new(s, span);
                quote_spanned!(span => #lit)
            }
            Value::Byte(b) => {
                let lit = LitByte::new(*b, span);
                quote_spanned!(span => #lit)
            }
            Value::Tuple(vals) => {
                let elems = vals.iter().map(|v| v.to_token(span));
                quote_spanned!(span => (#(#elems),*))
            }
            Value::Array(vals) => {
                let elems = vals.iter().map(|v| v.to_token(span));
                quote_spanned!(span => [#(#elems),*])
            }
            Value::Unit => {
                quote_spanned!(span => ())
            }
        }
    }
}

/// Evaluator state with step counter for infinite-loop protection.
pub struct Evaluator {
    steps: u64,
}

impl Evaluator {
    pub fn new() -> Self {
        Self { steps: 0 }
    }

    fn step(&mut self) -> Result<(), String> {
        self.steps += 1;
        if self.steps > MAX_EVAL_STEPS {
            return Err(format!(
                "preprocessor: evaluation exceeded maximum steps ({}). \
                 Possible infinite loop — skipping optimization.",
                MAX_EVAL_STEPS
            ));
        }
        Ok(())
    }

    /// Evaluate an expression, returning EvalResult.
    pub fn eval(&mut self, expr: &Expr) -> EvalResult {
        if self.step().is_err() {
            return EvalResult::PassThrough;
        }

        match expr {
            // === Literals ===
            Expr::Lit(ExprLit { lit, .. }) => self.eval_lit(lit),

            // === Parenthesized expressions ===
            Expr::Paren(ExprParen { expr, .. }) => self.eval(expr),

            // === Grouped expressions ===
            Expr::Group(ExprGroup { expr, .. }) => self.eval(expr),

            // === Unary operations ===
            Expr::Unary(ExprUnary { op, expr, .. }) => self.eval_unary(op, expr),

            // === Binary operations ===
            Expr::Binary(ExprBinary {
                left, op, right, ..
            }) => self.eval_binary(left, op, right),

            // === Cast expressions (limited: numeric as numeric) ===
            Expr::Cast(ExprCast { expr, ty, .. }) => self.eval_cast(expr, ty),

            // === Tuple expressions ===
            Expr::Tuple(ExprTuple { elems, .. }) => self.eval_tuple(elems),

            // === Array expressions ===
            Expr::Array(ExprArray { elems, .. }) => self.eval_array(elems),

            // === Block expressions ===
            Expr::Block(ExprBlock { block, .. }) => self.eval_block(block),

            // === Path expressions (constants like `true`, `false`, or const items) ===
            Expr::Path(path) => self.eval_path(path),

            // === Macro expressions (println!, format!, etc.) ===
            Expr::Macro(ExprMacro { mac, .. }) => {
                try_compile_time_macro(mac).unwrap_or(EvalResult::PassThrough)
            }

            // === Range, loop, if, match, etc. — pass through for now ===
            _ => EvalResult::PassThrough,
        }
    }

    fn eval_lit(&self, lit: &Lit) -> EvalResult {
        match lit {
            Lit::Str(l) => EvalResult::Value(Value::Str(l.value())),
            Lit::ByteStr(_) => EvalResult::PassThrough,
            Lit::Byte(l) => EvalResult::Value(Value::Byte(l.value())),
            Lit::Char(l) => EvalResult::Value(Value::Char(l.value())),
            Lit::Int(l) => {
                let s = l.base10_digits();
                // Handle negative literals (parsed as unary negation in some cases)
                if let Ok(n) = s.parse::<i64>() {
                    EvalResult::Value(Value::Int(n))
                } else if let Ok(n) = s.parse::<u64>() {
                    EvalResult::Value(Value::Int(n as i64))
                } else {
                    // Try as float fallback
                    s.parse::<f64>()
                        .map(Value::Float)
                        .map(EvalResult::Value)
                        .unwrap_or(EvalResult::PassThrough)
                }
            }
            Lit::Float(l) => l
                .base10_parse::<f64>()
                .map(Value::Float)
                .map(EvalResult::Value)
                .unwrap_or(EvalResult::PassThrough),
            Lit::Bool(l) => EvalResult::Value(Value::Bool(l.value)),
            Lit::Verbatim(_) => EvalResult::PassThrough,
            _ => EvalResult::PassThrough,
        }
    }

    fn eval_unary(&mut self, op: &UnOp, expr: &Expr) -> EvalResult {
        let val = self.eval(expr);
        match (op, val) {
            (UnOp::Not(_), EvalResult::Value(Value::Bool(b))) => EvalResult::Value(Value::Bool(!b)),
            (UnOp::Not(_), EvalResult::Value(Value::Int(n))) => EvalResult::Value(Value::Int(!n)),
            (UnOp::Neg(_), EvalResult::Value(Value::Int(n))) => EvalResult::Value(Value::Int(-n)),
            (UnOp::Neg(_), EvalResult::Value(Value::Float(f))) => {
                EvalResult::Value(Value::Float(-f))
            }
            (_, EvalResult::Value(_)) => EvalResult::PassThrough,
            (_, other) => other,
        }
    }

    fn eval_binary(&mut self, left: &Expr, op: &BinOp, right: &Expr) -> EvalResult {
        let l = self.eval(left);
        let r = self.eval(right);

        let (l, r) = match (l, r) {
            (EvalResult::Value(lv), EvalResult::Value(rv)) => (lv, rv),
            _ => return EvalResult::PassThrough,
        };

        match op {
            // === Arithmetic ===
            BinOp::Add(_) => self.bin_arith(
                &l,
                &r,
                |a, b| EvalResult::Value(Value::Int(a + b)),
                |a, b| EvalResult::Value(Value::Float(a + b)),
            ),
            BinOp::Sub(_) => self.bin_arith(
                &l,
                &r,
                |a, b| EvalResult::Value(Value::Int(a - b)),
                |a, b| EvalResult::Value(Value::Float(a - b)),
            ),
            BinOp::Mul(_) => self.bin_arith(
                &l,
                &r,
                |a, b| EvalResult::Value(Value::Int(a * b)),
                |a, b| EvalResult::Value(Value::Float(a * b)),
            ),
            BinOp::Div(_) => self.bin_arith(&l, &r, checked_div_i64, |a, b| {
                if b == 0.0 {
                    EvalResult::Error("division by zero".into())
                } else {
                    EvalResult::Value(Value::Float(a / b))
                }
            }),
            BinOp::Rem(_) => self.bin_arith(&l, &r, checked_rem_i64, |a, b| {
                if b == 0.0 {
                    EvalResult::Error("remainder by zero".into())
                } else {
                    EvalResult::Value(Value::Float(a % b))
                }
            }),

            // === Bitwise ===
            BinOp::BitAnd(_) => self.bin_arith(
                &l,
                &r,
                |a, b| EvalResult::Value(Value::Int(a & b)),
                |_, _| EvalResult::PassThrough,
            ),
            BinOp::BitOr(_) => self.bin_arith(
                &l,
                &r,
                |a, b| EvalResult::Value(Value::Int(a | b)),
                |_, _| EvalResult::PassThrough,
            ),
            BinOp::BitXor(_) => self.bin_arith(
                &l,
                &r,
                |a, b| EvalResult::Value(Value::Int(a ^ b)),
                |_, _| EvalResult::PassThrough,
            ),
            BinOp::Shl(_) => self.bin_arith(
                &l,
                &r,
                |a, b| EvalResult::Value(Value::Int(a << b)),
                |_, _| EvalResult::PassThrough,
            ),
            BinOp::Shr(_) => self.bin_arith(
                &l,
                &r,
                |a, b| EvalResult::Value(Value::Int(a >> b)),
                |_, _| EvalResult::PassThrough,
            ),

            // === Comparison ===
            BinOp::Eq(_) => self.bin_cmp(&l, &r, |a, b| a == b),
            BinOp::Ne(_) => self.bin_cmp(&l, &r, |a, b| a != b),
            BinOp::Lt(_) => self.bin_cmp(&l, &r, |a, b| a < b),
            BinOp::Le(_) => self.bin_cmp(&l, &r, |a, b| a <= b),
            BinOp::Gt(_) => self.bin_cmp(&l, &r, |a, b| a > b),
            BinOp::Ge(_) => self.bin_cmp(&l, &r, |a, b| a >= b),

            // === Logical ===
            BinOp::And(_) => match (l, r) {
                (Value::Bool(a), Value::Bool(b)) => EvalResult::Value(Value::Bool(a && b)),
                _ => EvalResult::PassThrough,
            },
            BinOp::Or(_) => match (l, r) {
                (Value::Bool(a), Value::Bool(b)) => EvalResult::Value(Value::Bool(a || b)),
                _ => EvalResult::PassThrough,
            },

            _ => EvalResult::PassThrough,
        }
    }

    fn bin_arith<F, G>(&self, l: &Value, r: &Value, int_op: F, float_op: G) -> EvalResult
    where
        F: Fn(i64, i64) -> EvalResult,
        G: Fn(f64, f64) -> EvalResult,
    {
        match (l, r) {
            (Value::Int(a), Value::Int(b)) => int_op(*a, *b),
            (Value::Float(a), Value::Float(b)) => float_op(*a, *b),
            (Value::Int(a), Value::Float(b)) => float_op(*a as f64, *b),
            (Value::Float(a), Value::Int(b)) => float_op(*a, *b as f64),
            _ => EvalResult::PassThrough,
        }
    }

    fn bin_cmp<F>(&self, l: &Value, r: &Value, cmp: F) -> EvalResult
    where
        F: Fn(i64, i64) -> bool,
    {
        match (l, r) {
            (Value::Int(a), Value::Int(b)) => EvalResult::Value(Value::Bool(cmp(*a, *b))),
            (Value::Float(a), Value::Float(b)) => {
                EvalResult::Value(Value::Bool(cmp(*a as i64, *b as i64)))
            }
            (Value::Bool(a), Value::Bool(b)) => {
                EvalResult::Value(Value::Bool(cmp(*a as i64, *b as i64)))
            }
            _ => EvalResult::PassThrough,
        }
    }

    fn eval_cast(&mut self, expr: &Expr, _ty: &syn::Type) -> EvalResult {
        // Evaluate the expression; if it's a value, keep it as-is
        // (type information is preserved in the output AST)
        self.eval(expr)
    }

    fn eval_tuple(&mut self, elems: &syn::punctuated::Punctuated<Expr, Token![,]>) -> EvalResult {
        let mut values = Vec::with_capacity(elems.len());
        for elem in elems {
            match self.eval(elem) {
                EvalResult::Value(v) => values.push(v),
                _ => return EvalResult::PassThrough,
            }
        }
        EvalResult::Value(Value::Tuple(values))
    }

    fn eval_array(&mut self, elems: &syn::punctuated::Punctuated<Expr, Token![,]>) -> EvalResult {
        let mut values = Vec::with_capacity(elems.len());
        for elem in elems {
            match self.eval(elem) {
                EvalResult::Value(v) => values.push(v),
                _ => return EvalResult::PassThrough,
            }
        }
        EvalResult::Value(Value::Array(values))
    }

    fn eval_block(&mut self, block: &syn::Block) -> EvalResult {
        let mut last_result = EvalResult::PassThrough;
        for stmt in &block.stmts {
            match stmt {
                Stmt::Local(local) => {
                    if let Some(init) = &local.init {
                        let val = self.eval(&init.expr);
                        match val {
                            EvalResult::Value(_) => last_result = val,
                            _ => return EvalResult::PassThrough,
                        }
                    }
                }
                Stmt::Expr(expr, _) => {
                    last_result = self.eval(expr);
                }
                Stmt::Item(_) => return EvalResult::PassThrough,
                Stmt::Macro(stmt_mac) => {
                    // Execute macro at compile time (e.g., println!, print!, etc.)
                    let result = try_compile_time_macro(&stmt_mac.mac);
                    match result {
                        Some(EvalResult::Value(_)) => {
                            // Macro executed successfully — side effect already happened
                            last_result = EvalResult::Value(Value::Unit);
                        }
                        Some(EvalResult::Error(msg)) => {
                            eprintln!("[preprocessor] error in macro: {}", msg);
                            return EvalResult::PassThrough;
                        }
                        Some(EvalResult::PassThrough) | None => {
                            // Unknown macro or can't evaluate — pass through
                            return EvalResult::PassThrough;
                        }
                    }
                }
            }
        }
        last_result
    }

    fn eval_path(&self, path: &syn::ExprPath) -> EvalResult {
        let qself = &path.qself;
        if qself.is_some() {
            return EvalResult::PassThrough;
        }

        let path = &path.path;
        if path.leading_colon.is_some() {
            return EvalResult::PassThrough;
        }

        // Handle simple identifiers
        if path.segments.len() == 1 {
            let ident = &path.segments[0].ident;
            let name = ident.to_string();

            match name.as_str() {
                "true" => return EvalResult::Value(Value::Bool(true)),
                "false" => return EvalResult::Value(Value::Bool(false)),
                _ => {}
            }
        }

        // Cannot resolve — pass through
        EvalResult::PassThrough
    }
}

fn checked_div_i64(a: i64, b: i64) -> EvalResult {
    if b == 0 {
        EvalResult::Error("division by zero".into())
    } else {
        a.checked_div(b)
            .map(Value::Int)
            .map(EvalResult::Value)
            .unwrap_or_else(|| EvalResult::Error("integer overflow in division".into()))
    }
}

fn checked_rem_i64(a: i64, b: i64) -> EvalResult {
    if b == 0 {
        EvalResult::Error("remainder by zero".into())
    } else {
        a.checked_rem(b)
            .map(Value::Int)
            .map(EvalResult::Value)
            .unwrap_or_else(|| EvalResult::Error("integer overflow in remainder".into()))
    }
}

/// Convert a `Value` to its `Display` string representation, used for format args.
fn value_to_string(v: &Value) -> String {
    match v {
        Value::Int(n) => n.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Char(c) => c.to_string(),
        Value::Str(s) => s.clone(),
        Value::Byte(b) => b.to_string(),
        Value::Tuple(vals) => {
            let parts: Vec<_> = vals.iter().map(value_to_string).collect();
            format!("({})", parts.join(", "))
        }
        Value::Array(vals) => {
            let parts: Vec<_> = vals.iter().map(value_to_string).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Unit => "()".to_string(),
    }
}

/// Execute a format string with given arguments, mimicking `format!`/`println!` behavior.
/// Returns the formatted string.
fn format_with_args(format_str: &str, args: &[Value]) -> String {
    let mut result = String::new();
    let mut chars = format_str.chars().peekable();
    let mut arg_index = 0;

    while let Some(ch) = chars.next() {
        if ch == '{' {
            if chars.peek() == Some(&'{') {
                chars.next();
                result.push('{');
            } else {
                // Find closing brace
                let mut placeholder = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(c) => placeholder.push(c),
                        None => {
                            // Malformed, output as-is
                            result.push('{');
                            result.push_str(&placeholder);
                            break;
                        }
                    }
                }
                // Parse format spec if present (e.g., ":?", ":x")
                let (spec, _) = placeholder.split_once(':').unzip();
                let spec = spec.unwrap_or("");
                let arg_idx = placeholder
                    .split_once(':')
                    .map(|(a, _)| a)
                    .unwrap_or(&placeholder)
                    .parse::<usize>()
                    .unwrap_or(arg_index);

                if arg_idx < args.len() {
                    let val = &args[arg_idx];
                    match spec {
                        "?" | ":?" => result.push_str(&format!("{:#?}", val)),
                        "x" | ":x" => {
                            if let Value::Int(n) = val {
                                result.push_str(&format!("{:x}", n));
                            } else {
                                result.push_str(&value_to_string(val));
                            }
                        }
                        _ => result.push_str(&value_to_string(val)),
                    }
                    if placeholder.is_empty() || placeholder.parse::<usize>().is_ok() {
                        arg_index = arg_index.saturating_add(1);
                    }
                } else {
                    result.push('{');
                    result.push_str(&placeholder);
                    result.push('}');
                }
            }
        } else if ch == '}' {
            if chars.peek() == Some(&'}') {
                chars.next();
                result.push('}');
            } else {
                result.push('}');
            }
        } else {
            result.push(ch);
        }
    }
    result
}

/// Try to execute a macro call at compile time.
/// Returns `Some(EvalResult)` if the macro is handled, `None` if it should pass through.
fn try_compile_time_macro(mac: &syn::Macro) -> Option<EvalResult> {
    let mac_name = mac.path.segments.last()?.ident.to_string();

    match mac_name.as_str() {
        "format" | "format_args" => {
            // format!("...", args...) — return the formatted string as Value::Str
            let (format_str, args) = parse_format_macro_tokens(&mac.tokens)?;
            let result = format_with_args(&format_str, &args);
            Some(EvalResult::Value(Value::Str(result)))
        }
        "println" | "print" | "eprintln" | "eprint" => {
            // Execute at compile time: print to stderr with [preprocessor] prefix
            let (format_str, args) = parse_format_macro_tokens(&mac.tokens)?;
            let result = format_with_args(&format_str, &args);
            // Print to stderr during compilation with a clear prefix
            eprintln!("[preprocessor] {}", result);
            Some(EvalResult::Value(Value::Unit))
        }
        "stringify" => {
            // stringify!(...) → return the tokens as a string
            let s = mac.tokens.to_string();
            Some(EvalResult::Value(Value::Str(s)))
        }
        "concat" => {
            // concat!("a", "b") → return concatenated string
            let result = parse_concat_tokens(&mac.tokens)?;
            Some(EvalResult::Value(Value::Str(result)))
        }
        "env" => {
            // env!("VAR") → look up environment variable at compile time
            let var_name = parse_env_token(&mac.tokens)?;
            match std::env::var(&var_name) {
                Ok(val) => Some(EvalResult::Value(Value::Str(val))),
                Err(_) => {
                    eprintln!("[preprocessor] warning: env var '{}' not set", var_name);
                    Some(EvalResult::PassThrough)
                }
            }
        }
        "include_str" => {
            // include_str!("path") → read file at compile time
            let path = parse_string_token(&mac.tokens)?;
            // Resolve path relative to the crate's manifest directory
            let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
            let full_path = std::path::Path::new(&manifest_dir).join(&path);
            match std::fs::read_to_string(&full_path) {
                Ok(content) => Some(EvalResult::Value(Value::Str(content))),
                Err(e) => {
                    eprintln!(
                        "[preprocessor] warning: include_str! failed to read '{}': {}",
                        full_path.display(),
                        e
                    );
                    Some(EvalResult::PassThrough)
                }
            }
        }
        "include_bytes" => {
            // include_bytes!("path") → read file bytes at compile time
            let path = parse_string_token(&mac.tokens)?;
            let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
            let full_path = std::path::Path::new(&manifest_dir).join(&path);
            match std::fs::read(&full_path) {
                Ok(bytes) => {
                    // Return as array of bytes
                    let vals: Vec<Value> = bytes.into_iter().map(|b| Value::Byte(b)).collect();
                    Some(EvalResult::Value(Value::Array(vals)))
                }
                Err(e) => {
                    eprintln!(
                        "[preprocessor] warning: include_bytes! failed to read '{}': {}",
                        full_path.display(),
                        e
                    );
                    Some(EvalResult::PassThrough)
                }
            }
        }
        "line" | "column" | "file" => {
            // These are compile-time macros that return location info
            // We can't know the exact line/column from proc-macro context, so pass through
            Some(EvalResult::PassThrough)
        }
        _ => None, // Unknown macro — pass through
    }
}

/// Parse tokens from a format-style macro (format!, println!, etc.)
/// Returns (format_string, evaluated_args).
fn parse_format_macro_tokens(tokens: &TokenStream) -> Option<(String, Vec<Value>)> {
    // Parse as: "format string", arg1, arg2, ...
    let parsed =
        syn::punctuated::Punctuated::<Expr, Token![,]>::parse_terminated.parse2(tokens.clone());

    let exprs = match parsed {
        Ok(e) => e,
        Err(_) => return None,
    };

    let mut exprs_iter = exprs.into_iter();

    // First argument should be the format string
    let first = exprs_iter.next()?;
    let format_str = if let Expr::Lit(syn::ExprLit {
        lit: Lit::Str(lit_str),
        ..
    }) = first
    {
        lit_str.value()
    } else {
        return None;
    };

    // Remaining arguments are format args
    let mut args = Vec::new();
    for arg in exprs_iter {
        let mut evaluator = Evaluator::new();
        match evaluator.eval(&arg) {
            EvalResult::Value(v) => args.push(v),
            _ => return None,
        }
    }

    Some((format_str, args))
}

/// Parse concat!("a", "b", ...) tokens and return concatenated string.
fn parse_concat_tokens(tokens: &TokenStream) -> Option<String> {
    let parsed =
        syn::punctuated::Punctuated::<Expr, Token![,]>::parse_terminated.parse2(tokens.clone());

    let exprs = parsed.ok()?;
    let mut result = String::new();

    for expr in exprs {
        match expr {
            Expr::Lit(syn::ExprLit {
                lit: Lit::Str(lit_str),
                ..
            }) => result.push_str(&lit_str.value()),
            Expr::Lit(syn::ExprLit {
                lit: Lit::Char(lit_char),
                ..
            }) => result.push(lit_char.value()),
            _ => return None,
        }
    }

    Some(result)
}

/// Parse a single string token from env!("VAR") or include_str!("path").
fn parse_env_token(tokens: &TokenStream) -> Option<String> {
    let parsed =
        syn::punctuated::Punctuated::<Expr, Token![,]>::parse_terminated.parse2(tokens.clone());

    let exprs = parsed.ok()?;
    let first = exprs.into_iter().next()?;

    if let Expr::Lit(syn::ExprLit {
        lit: Lit::Str(lit_str),
        ..
    }) = first
    {
        Some(lit_str.value())
    } else {
        None
    }
}

/// Parse a single string token.
fn parse_string_token(tokens: &TokenStream) -> Option<String> {
    parse_env_token(tokens)
}

/// Recursively transform an expression: evaluate what can be evaluated,
/// replace evaluable sub-expressions with literals, pass through the rest.
pub fn transform_expr(expr: &Expr) -> (Expr, bool) {
    let mut evaluator = Evaluator::new();
    let result = evaluator.eval(expr);

    match result {
        EvalResult::Value(val) => {
            // Entire expression is evaluable — replace with literal
            let span = expr.span();
            let tokens = val.to_token(span);
            // Parse the tokens back into an Expr
            if let Ok(new_expr) = syn::parse2(tokens) {
                (new_expr, true)
            } else {
                (expr.clone(), false)
            }
        }
        EvalResult::Error(msg) => {
            // Emit a compile_error! instead
            let span = expr.span();
            let error_token = quote_spanned!(span => compile_error!(#msg));
            if let Ok(new_expr) = syn::parse2(error_token) {
                (new_expr, true)
            } else {
                (expr.clone(), false)
            }
        }
        EvalResult::PassThrough => {
            // Try to transform sub-expressions recursively
            transform_expr_recursive(expr)
        }
    }
}

/// Recursively traverse and transform sub-expressions within a non-evaluable expression.
fn transform_expr_recursive(expr: &Expr) -> (Expr, bool) {
    let mut changed = false;

    let new_expr = match expr {
        Expr::Paren(paren) => {
            let (inner, inner_changed) = transform_expr(&paren.expr);
            if inner_changed {
                changed = true;
                Expr::Paren(ExprParen {
                    expr: Box::new(inner),
                    ..paren.clone()
                })
            } else {
                return (expr.clone(), false);
            }
        }
        Expr::Binary(binary) => {
            let (l, l_changed) = transform_expr(&binary.left);
            let (r, r_changed) = transform_expr(&binary.right);
            if l_changed || r_changed {
                changed = true;
                Expr::Binary(ExprBinary {
                    left: Box::new(l),
                    right: Box::new(r),
                    ..binary.clone()
                })
            } else {
                return (expr.clone(), false);
            }
        }
        Expr::Unary(unary) => {
            let (inner, inner_changed) = transform_expr(&unary.expr);
            if inner_changed {
                changed = true;
                Expr::Unary(ExprUnary {
                    expr: Box::new(inner),
                    ..unary.clone()
                })
            } else {
                return (expr.clone(), false);
            }
        }
        Expr::Tuple(tuple) => {
            let mut new_elems = syn::punctuated::Punctuated::new();
            for pair in tuple.elems.pairs() {
                let (new_elem, elem_changed) = transform_expr(pair.value());
                if elem_changed {
                    changed = true;
                }
                new_elems.push_value(new_elem);
                if let Some(punct) = pair.punct() {
                    new_elems.push_punct((**punct).clone());
                }
            }
            if changed {
                Expr::Tuple(ExprTuple {
                    elems: new_elems,
                    ..tuple.clone()
                })
            } else {
                return (expr.clone(), false);
            }
        }
        Expr::Array(array) => {
            let mut new_elems = syn::punctuated::Punctuated::new();
            for pair in array.elems.pairs() {
                let (new_elem, elem_changed) = transform_expr(pair.value());
                if elem_changed {
                    changed = true;
                }
                new_elems.push_value(new_elem);
                if let Some(punct) = pair.punct() {
                    new_elems.push_punct((**punct).clone());
                }
            }
            if changed {
                Expr::Array(ExprArray {
                    elems: new_elems,
                    ..array.clone()
                })
            } else {
                return (expr.clone(), false);
            }
        }
        _ => return (expr.clone(), false),
    };

    (new_expr, changed)
}

/// Transform all statements in a block, replacing evaluable expressions with literals.
pub fn transform_block(block: &syn::Block) -> syn::Block {
    let mut new_stmts = Vec::new();

    for stmt in &block.stmts {
        let new_stmt = match stmt {
            Stmt::Local(local) => {
                if let Some(init) = &local.init {
                    let (new_expr, _changed) = transform_expr(&init.expr);
                    Stmt::Local(syn::Local {
                        init: Some(syn::LocalInit {
                            expr: Box::new(new_expr),
                            ..init.clone()
                        }),
                        ..local.clone()
                    })
                } else {
                    stmt.clone()
                }
            }
            Stmt::Expr(expr, semi) => {
                let (new_expr, _changed) = transform_expr(expr);
                Stmt::Expr(new_expr, *semi)
            }
            Stmt::Item(_) => stmt.clone(),
            Stmt::Macro(stmt_mac) => {
                // Try to execute the macro at compile time
                match try_compile_time_macro(&stmt_mac.mac) {
                    Some(EvalResult::Value(_)) => {
                        // Macro executed — side effect already happened at compile time.
                        // Remove this statement from the output (don't emit it at runtime).
                        continue;
                    }
                    Some(EvalResult::Error(msg)) => {
                        eprintln!("[preprocessor] error in macro: {}", msg);
                        stmt.clone()
                    }
                    Some(EvalResult::PassThrough) | None => {
                        // Unknown macro or can't evaluate — keep at runtime
                        stmt.clone()
                    }
                }
            }
        };
        new_stmts.push(new_stmt);
    }

    syn::Block {
        stmts: new_stmts,
        brace_token: block.brace_token,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn test_eval_simple_arithmetic() {
        let expr: Expr = parse_quote!(1 + 2);
        let mut eval = Evaluator::new();
        match eval.eval(&expr) {
            EvalResult::Value(Value::Int(3)) => {}
            other => panic!("expected Value::Int(3), got {:?}", other),
        }
    }

    #[test]
    fn test_eval_nested_arithmetic() {
        let expr: Expr = parse_quote!((1 + 2) * 3);
        let mut eval = Evaluator::new();
        match eval.eval(&expr) {
            EvalResult::Value(Value::Int(9)) => {}
            other => panic!("expected Value::Int(9), got {:?}", other),
        }
    }

    #[test]
    fn test_eval_bool_logic() {
        let expr: Expr = parse_quote!(true && false);
        let mut eval = Evaluator::new();
        match eval.eval(&expr) {
            EvalResult::Value(Value::Bool(false)) => {}
            other => panic!("expected Value::Bool(false), got {:?}", other),
        }
    }

    #[test]
    fn test_eval_passthrough() {
        let expr: Expr = parse_quote!(x + 1);
        let mut eval = Evaluator::new();
        match eval.eval(&expr) {
            EvalResult::PassThrough => {}
            other => panic!("expected PassThrough, got {:?}", other),
        }
    }

    #[test]
    fn test_eval_division_by_zero() {
        let expr: Expr = parse_quote!(1 / 0);
        let mut eval = Evaluator::new();
        match eval.eval(&expr) {
            EvalResult::Error(_) => {}
            other => panic!("expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_eval_tuple() {
        let expr: Expr = parse_quote!((1, 2, 3));
        let mut eval = Evaluator::new();
        match eval.eval(&expr) {
            EvalResult::Value(Value::Tuple(vals)) => {
                assert_eq!(vals.len(), 3);
            }
            other => panic!("expected Value::Tuple, got {:?}", other),
        }
    }

    #[test]
    fn test_eval_array() {
        let expr: Expr = parse_quote!([1, 2, 3]);
        let mut eval = Evaluator::new();
        match eval.eval(&expr) {
            EvalResult::Value(Value::Array(vals)) => {
                assert_eq!(vals.len(), 3);
            }
            other => panic!("expected Value::Array, got {:?}", other),
        }
    }

    #[test]
    fn test_transform_expr_replaces_literal() {
        let expr: Expr = parse_quote!(1 + 2 * 3);
        let (new_expr, changed) = transform_expr(&expr);
        assert!(changed);
        // The result should be 7
        if let Expr::Lit(ref lit) = new_expr {
            if let Lit::Int(i) = &lit.lit {
                assert_eq!(i.base10_parse::<i64>().unwrap(), 7);
                return;
            }
        }
        panic!("expected literal 7, got {:?}", new_expr);
    }
}
