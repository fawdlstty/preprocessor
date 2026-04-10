//! AST traversal evaluator — evcxr-powered dynamic execution engine.
//!
//! Converts `syn::Expr` AST to executable Rust code, evaluates through evcxr,
//! and outputs minimized AST (literal tokens). Supports arbitrary expressions
//! including function calls, external crate usage, and complex control flow.
//!
//! ## Architecture
//!
//! 1. **Fast Path**: Pure literals evaluated without evcxr
//! 2. **Dynamic Path**: evcxr compiles and executes arbitrary code
//! 3. **Graceful Fallback**: If evcxr fails to initialize, falls back to built-in interpreter
//! 4. **Dependency Detection**: Auto-detects and loads external crate dependencies

use proc_macro2::{Span, TokenStream};
use quote::quote_spanned;
use syn::parse::Parser;
use syn::spanned::Spanned;
use syn::{
    BinOp, Expr, ExprBinary, ExprCast, ExprGroup, ExprLit, ExprParen,
    ExprUnary, Lit, LitBool, LitByte, LitChar, LitFloat, LitInt, LitStr, Stmt, Token,
    UnOp,
};

use crate::evcxr_engine::DynamicEngine;

/// Evaluation result.
#[derive(Debug, Clone)]
pub enum EvalResult {
    Value(Value),
    PassThrough,
    Error(String),
}

/// Runtime value produced by the evaluator.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Char(char),
    Str(String),
    Byte(u8),
    Tuple(Vec<Value>),
    Array(Vec<Value>),
    Unit,
}

impl Value {
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
                quote_spanned!(span => ::std::string::String::from(#lit))
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

/// Evaluator with dynamic execution engine + built-in fallback.
///
/// Lazily initializes the dynamic engine on first use. If initialization fails,
/// all subsequent evaluations fall back to the built-in interpreter.
pub struct Evaluator {
    engine: Option<DynamicEngine>,
    fallback_available: bool,
}

impl Evaluator {
    pub fn new() -> Self {
        // Try to initialize dynamic engine, but don't panic if it fails
        let engine = match DynamicEngine::new() {
            Ok(e) => Some(e),
            Err(_) => None,
        };

        Self {
            engine,
            fallback_available: true,
        }
    }

    /// Evaluate an expression.
    /// Tries evcxr first; falls back to built-in interpreter if unavailable.
    pub fn eval(&mut self, expr: &Expr) -> EvalResult {
        // Fast path: pure literals
        if let Expr::Lit(ExprLit { lit, .. }) = expr {
            return eval_lit(lit);
        }

        // Fast path: parenthesized/grouped
        if let Expr::Paren(ExprParen { expr: inner, .. }) = expr {
            return self.eval(inner);
        }
        if let Expr::Group(ExprGroup { expr: inner, .. }) = expr {
            return self.eval(inner);
        }

        // Try evcxr if available
        if let Some(ref mut engine) = self.engine {
            let result = engine.evaluate(expr);
            // If evcxr returns PassThrough, try the built-in fallback
            if let EvalResult::PassThrough = result {
                if self.fallback_available {
                    return eval_builtin(expr);
                }
            }
            return result;
        }

        // Fallback to built-in interpreter
        eval_builtin(expr)
    }

    /// Evaluate a block of statements.
    #[allow(dead_code)]
    pub fn eval_block(&mut self, block: &syn::Block) -> EvalResult {
        if let Some(ref mut engine) = self.engine {
            let result = engine.evaluate_block(block);
            if let EvalResult::PassThrough = result {
                if self.fallback_available {
                    return eval_builtin_block(block);
                }
            }
            return result;
        }

        eval_builtin_block(block)
    }
}

impl Default for Evaluator {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Built-in fallback interpreter (for when evcxr is unavailable or returns PassThrough)
// ============================================================================

fn eval_builtin(expr: &Expr) -> EvalResult {
    match expr {
        Expr::Lit(ExprLit { lit, .. }) => eval_lit(lit),
        Expr::Paren(ExprParen { expr: inner, .. }) => eval_builtin(inner),
        Expr::Group(ExprGroup { expr: inner, .. }) => eval_builtin(inner),
        Expr::Unary(ExprUnary { op, expr, .. }) => eval_builtin_unary(op, expr),
        Expr::Binary(ExprBinary {
            left, op, right, ..
        }) => eval_builtin_binary(left, op, right),
        Expr::Cast(ExprCast { expr, .. }) => eval_builtin(expr),
        Expr::Tuple(tuple) => eval_builtin_tuple(&tuple.elems),
        Expr::Array(array) => eval_builtin_array(&array.elems),
        Expr::Block(block) => eval_builtin_block(&block.block),
        Expr::Path(path) => eval_builtin_path(path),
        Expr::Macro(mac) => try_compile_time_macro(&mac.mac).unwrap_or(EvalResult::PassThrough),
        _ => EvalResult::PassThrough,
    }
}

fn eval_builtin_block(block: &syn::Block) -> EvalResult {
    let mut last_result = EvalResult::PassThrough;
    for stmt in &block.stmts {
        match stmt {
            Stmt::Local(local) => {
                if let Some(init) = &local.init {
                    let val = eval_builtin(&init.expr);
                    match val {
                        EvalResult::Value(_) => last_result = val,
                        _ => return EvalResult::PassThrough,
                    }
                }
            }
            Stmt::Expr(expr, _) => {
                last_result = eval_builtin(expr);
            }
            Stmt::Item(_) => return EvalResult::PassThrough,
            Stmt::Macro(stmt_mac) => match try_compile_time_macro(&stmt_mac.mac) {
                Some(EvalResult::Value(_)) => last_result = EvalResult::Value(Value::Unit),
                Some(EvalResult::Error(msg)) => {
                    eprintln!("[preprocessor] error in macro: {}", msg);
                    return EvalResult::PassThrough;
                }
                Some(EvalResult::PassThrough) | None => return EvalResult::PassThrough,
            },
        }
    }
    last_result
}

fn eval_lit(lit: &Lit) -> EvalResult {
    match lit {
        Lit::Str(l) => EvalResult::Value(Value::Str(l.value())),
        Lit::ByteStr(_) => EvalResult::PassThrough,
        Lit::Byte(l) => EvalResult::Value(Value::Byte(l.value())),
        Lit::Char(l) => EvalResult::Value(Value::Char(l.value())),
        Lit::Int(l) => {
            let s = l.base10_digits();
            if let Ok(n) = s.parse::<i64>() {
                EvalResult::Value(Value::Int(n))
            } else if let Ok(n) = s.parse::<u64>() {
                EvalResult::Value(Value::Int(n as i64))
            } else {
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

fn eval_builtin_unary(op: &UnOp, expr: &Expr) -> EvalResult {
    let val = eval_builtin(expr);
    match (op, val) {
        (UnOp::Not(_), EvalResult::Value(Value::Bool(b))) => EvalResult::Value(Value::Bool(!b)),
        (UnOp::Not(_), EvalResult::Value(Value::Int(n))) => EvalResult::Value(Value::Int(!n)),
        (UnOp::Neg(_), EvalResult::Value(Value::Int(n))) => EvalResult::Value(Value::Int(-n)),
        (UnOp::Neg(_), EvalResult::Value(Value::Float(f))) => EvalResult::Value(Value::Float(-f)),
        (_, EvalResult::Value(_)) => EvalResult::PassThrough,
        (_, other) => other,
    }
}

fn eval_builtin_binary(left: &Expr, op: &BinOp, right: &Expr) -> EvalResult {
    let l = eval_builtin(left);
    let r = eval_builtin(right);

    let (l, r) = match (l, r) {
        (EvalResult::Value(lv), EvalResult::Value(rv)) => (lv, rv),
        _ => return EvalResult::PassThrough,
    };

    match op {
        BinOp::Add(_) => bin_arith(
            &l,
            &r,
            |a, b| EvalResult::Value(Value::Int(a + b)),
            |a, b| EvalResult::Value(Value::Float(a + b)),
        ),
        BinOp::Sub(_) => bin_arith(
            &l,
            &r,
            |a, b| EvalResult::Value(Value::Int(a - b)),
            |a, b| EvalResult::Value(Value::Float(a - b)),
        ),
        BinOp::Mul(_) => bin_arith(
            &l,
            &r,
            |a, b| EvalResult::Value(Value::Int(a * b)),
            |a, b| EvalResult::Value(Value::Float(a * b)),
        ),
        BinOp::Div(_) => bin_arith(&l, &r, checked_div_i64, |a, b| {
            if b == 0.0 {
                EvalResult::Error("division by zero".into())
            } else {
                EvalResult::Value(Value::Float(a / b))
            }
        }),
        BinOp::Rem(_) => bin_arith(&l, &r, checked_rem_i64, |a, b| {
            if b == 0.0 {
                EvalResult::Error("remainder by zero".into())
            } else {
                EvalResult::Value(Value::Float(a % b))
            }
        }),
        BinOp::BitAnd(_) => bin_arith(
            &l,
            &r,
            |a, b| EvalResult::Value(Value::Int(a & b)),
            |_, _| EvalResult::PassThrough,
        ),
        BinOp::BitOr(_) => bin_arith(
            &l,
            &r,
            |a, b| EvalResult::Value(Value::Int(a | b)),
            |_, _| EvalResult::PassThrough,
        ),
        BinOp::BitXor(_) => bin_arith(
            &l,
            &r,
            |a, b| EvalResult::Value(Value::Int(a ^ b)),
            |_, _| EvalResult::PassThrough,
        ),
        BinOp::Shl(_) => bin_arith(
            &l,
            &r,
            |a, b| EvalResult::Value(Value::Int(a << b)),
            |_, _| EvalResult::PassThrough,
        ),
        BinOp::Shr(_) => bin_arith(
            &l,
            &r,
            |a, b| EvalResult::Value(Value::Int(a >> b)),
            |_, _| EvalResult::PassThrough,
        ),
        BinOp::Eq(_) => bin_cmp(&l, &r, |a, b| a == b),
        BinOp::Ne(_) => bin_cmp(&l, &r, |a, b| a != b),
        BinOp::Lt(_) => bin_cmp(&l, &r, |a, b| a < b),
        BinOp::Le(_) => bin_cmp(&l, &r, |a, b| a <= b),
        BinOp::Gt(_) => bin_cmp(&l, &r, |a, b| a > b),
        BinOp::Ge(_) => bin_cmp(&l, &r, |a, b| a >= b),
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

fn bin_arith<F, G>(l: &Value, r: &Value, int_op: F, float_op: G) -> EvalResult
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

fn bin_cmp<F>(l: &Value, r: &Value, cmp: F) -> EvalResult
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

fn eval_builtin_tuple(elems: &syn::punctuated::Punctuated<Expr, Token![,]>) -> EvalResult {
    let mut values = Vec::with_capacity(elems.len());
    for elem in elems {
        match eval_builtin(elem) {
            EvalResult::Value(v) => values.push(v),
            _ => return EvalResult::PassThrough,
        }
    }
    EvalResult::Value(Value::Tuple(values))
}

fn eval_builtin_array(elems: &syn::punctuated::Punctuated<Expr, Token![,]>) -> EvalResult {
    let mut values = Vec::with_capacity(elems.len());
    for elem in elems {
        match eval_builtin(elem) {
            EvalResult::Value(v) => values.push(v),
            _ => return EvalResult::PassThrough,
        }
    }
    EvalResult::Value(Value::Array(values))
}

fn eval_builtin_path(path: &syn::ExprPath) -> EvalResult {
    if path.qself.is_some() || path.path.leading_colon.is_some() {
        return EvalResult::PassThrough;
    }
    if path.path.segments.len() == 1 {
        let name = path.path.segments[0].ident.to_string();
        match name.as_str() {
            "true" => return EvalResult::Value(Value::Bool(true)),
            "false" => return EvalResult::Value(Value::Bool(false)),
            _ => {}
        }
    }
    EvalResult::PassThrough
}

// ============================================================================
// Compile-time macro execution
// ============================================================================

fn try_compile_time_macro(mac: &syn::Macro) -> Option<EvalResult> {
    let mac_name = mac.path.segments.last()?.ident.to_string();

    match mac_name.as_str() {
        "format" | "format_args" => {
            let (format_str, args) = parse_format_macro_tokens(&mac.tokens)?;
            let result = format_with_args(&format_str, &args);
            Some(EvalResult::Value(Value::Str(result)))
        }
        "println" | "print" | "eprintln" | "eprint" => {
            let (format_str, args) = parse_format_macro_tokens(&mac.tokens)?;
            let result = format_with_args(&format_str, &args);
            eprintln!("[preprocessor] {}", result);
            Some(EvalResult::Value(Value::Unit))
        }
        "stringify" => Some(EvalResult::Value(Value::Str(mac.tokens.to_string()))),
        "concat" => parse_concat_tokens(&mac.tokens).map(|s| EvalResult::Value(Value::Str(s))),
        "env" => {
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
            let path = parse_string_token(&mac.tokens)?;
            let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
            let full_path = std::path::Path::new(&manifest_dir).join(&path);
            match std::fs::read_to_string(&full_path) {
                Ok(content) => Some(EvalResult::Value(Value::Str(content))),
                Err(e) => {
                    eprintln!("[preprocessor] warning: include_str! failed: {}", e);
                    Some(EvalResult::PassThrough)
                }
            }
        }
        "include_bytes" => {
            let path = parse_string_token(&mac.tokens)?;
            let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
            let full_path = std::path::Path::new(&manifest_dir).join(&path);
            match std::fs::read(&full_path) {
                Ok(bytes) => {
                    let vals: Vec<Value> = bytes.into_iter().map(|b| Value::Byte(b)).collect();
                    Some(EvalResult::Value(Value::Array(vals)))
                }
                Err(e) => {
                    eprintln!("[preprocessor] warning: include_bytes! failed: {}", e);
                    Some(EvalResult::PassThrough)
                }
            }
        }
        "line" | "column" | "file" => Some(EvalResult::PassThrough),
        _ => None,
    }
}

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
                let mut placeholder = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(c) => placeholder.push(c),
                        None => {
                            result.push('{');
                            result.push_str(&placeholder);
                            break;
                        }
                    }
                }
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

fn parse_format_macro_tokens(tokens: &TokenStream) -> Option<(String, Vec<Value>)> {
    let parsed = syn::punctuated::Punctuated::<Expr, Token![,]>::parse_terminated
        .parse2(tokens.clone())
        .ok()?;
    let mut exprs_iter = parsed.into_iter();
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

fn parse_concat_tokens(tokens: &TokenStream) -> Option<String> {
    let parsed = syn::punctuated::Punctuated::<Expr, Token![,]>::parse_terminated
        .parse2(tokens.clone())
        .ok()?;
    let mut result = String::new();
    for expr in parsed {
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

fn parse_env_token(tokens: &TokenStream) -> Option<String> {
    let parsed = syn::punctuated::Punctuated::<Expr, Token![,]>::parse_terminated
        .parse2(tokens.clone())
        .ok()?;
    let first = parsed.into_iter().next()?;
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

fn parse_string_token(tokens: &TokenStream) -> Option<String> {
    parse_env_token(tokens)
}

// ============================================================================
// Public transformation API
// ============================================================================

pub fn transform_expr(expr: &Expr) -> (Expr, bool) {
    let mut evaluator = Evaluator::new();
    let result = evaluator.eval(expr);

    match result {
        EvalResult::Value(val) => {
            let span = expr.span();
            let tokens = val.to_token(span);
            if let Ok(new_expr) = syn::parse2(tokens) {
                (new_expr, true)
            } else {
                (expr.clone(), false)
            }
        }
        EvalResult::Error(msg) => {
            let span = expr.span();
            let error_token = quote_spanned!(span => compile_error!(#msg));
            if let Ok(new_expr) = syn::parse2(error_token) {
                (new_expr, true)
            } else {
                (expr.clone(), false)
            }
        }
        EvalResult::PassThrough => {
            // PassThrough 意味着表达式无法在编译期求值，但可能是合法的运行时代码
            // 返回原始表达式，让 Rust 编译器在运行时处理
            (expr.clone(), false)
        }
    }
}

pub fn transform_block(block: &syn::Block) -> syn::Block {
    let mut new_stmts = Vec::new();

    for stmt in &block.stmts {
        let new_stmt = match stmt {
            Stmt::Local(local) => {
                if let Some(init) = &local.init {
                    let (new_expr, _) = transform_expr(&init.expr);
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
                let (new_expr, _) = transform_expr(expr);
                Stmt::Expr(new_expr, *semi)
            }
            Stmt::Item(_) => stmt.clone(),
            Stmt::Macro(stmt_mac) => match try_compile_time_macro(&stmt_mac.mac) {
                Some(EvalResult::Value(_)) => continue,
                Some(EvalResult::Error(msg)) => {
                    eprintln!("[preprocessor] error in macro: {}", msg);
                    stmt.clone()
                }
                Some(EvalResult::PassThrough) | None => stmt.clone(),
            },
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
        if let Expr::Lit(ref lit) = new_expr {
            if let Lit::Int(i) = &lit.lit {
                assert_eq!(i.base10_parse::<i64>().unwrap(), 7);
                return;
            }
        }
        panic!("expected literal 7, got {:?}", new_expr);
    }
}
