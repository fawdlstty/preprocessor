//! Dynamic execution engine powered by cargo subprocess.
//!
//! Creates a temporary cargo project, injects the code, compiles and runs it,
//! then captures the output. This allows arbitrary code with external crates (like chrono)
//! to be evaluated at compile time.
//!
//! ## Architecture
//!
//! 1. **Dependency Detection**: Scans code for external crate paths
//! 2. **Dynamic Injection**: Auto-injects dependencies into Cargo.toml
//! 3. **Execution**: Creates temp project, runs `cargo run`, captures stdout
//! 4. **Result Parsing**: Parses text output back into typed `Value`s

use std::path::PathBuf;
use std::process::Command;
use syn::Expr;

use crate::evaluator::{EvalResult, Value};

/// Standard library crates that don't need dependency injection.
const STD_CRATES: &[&str] = &["std", "core", "alloc", "proc_macro"];

/// Common module/type names that should not be treated as crate names.
const COMMON_NAMES: &[&str] = &[
    // chrono types
    "Local",
    "Utc",
    "DateTime",
    "NaiveDateTime",
    "Duration",
    "TimeZone",
    // Result/Option variants
    "Some",
    "None",
    "Ok",
    "Err",
    // Common types
    "Vec",
    "String",
    "Box",
    "Option",
    "Result",
    // Collection types
    "HashMap",
    "HashSet",
    "BTreeMap",
    "BTreeSet",
    "LinkedList",
    "VecDeque",
    "BinaryHeap",
    // Hash types
    "DefaultHasher",
    "RandomState",
    // Path types
    "PathBuf",
    "Path",
    // Network types
    "Ipv4Addr",
    "Ipv6Addr",
    "SocketAddr",
    // Process types
    "Command",
    "Child",
    "Output",
    // Filesystem types
    "File",
    "OpenOptions",
    "Metadata",
    "DirBuilder",
    "ReadDir",
    "DirEntry",
    "FileType",
    "Permissions",
    // Time/Thread types
    "SystemTime",
    "Instant",
    "Thread",
    "JoinHandle",
    "Builder",
    "LocalKey",
    // std module names
    "mem",
    "size_of",
    "size_of_val",
    "align_of",
    "env",
    "net",
    "hash",
    "collections",
    "process",
    "path",
    "time",
    "fs",
    "io",
    "str",
    "slice",
    "array",
    "cmp",
    "fmt",
    "default",
    "clone",
    "iter",
    "ops",
];

/// Extracts external crate names from code by scanning:
/// 1. `use crate_name::...` statements
/// 2. `crate_name::...` path expressions
fn detect_external_crates(code: &str) -> Vec<String> {
    let mut crates = Vec::new();

    // Pattern 1: `use crate_name::` or `use ::crate_name::`
    let use_re = regex::Regex::new(r"use\s+(?:::)?([a-zA-Z_][a-zA-Z0-9_]*)::").unwrap();
    for cap in use_re.captures_iter(code) {
        if let Some(m) = cap.get(1) {
            let name = m.as_str();
            if !STD_CRATES.contains(&name) && !crates.contains(&name.to_string()) {
                crates.push(name.to_string());
            }
        }
    }

    // Pattern 2: `crate_name::` in paths (with optional spaces around ::)
    let path_re = regex::Regex::new(r"([a-zA-Z_][a-zA-Z0-9_]*)\s*::").unwrap();
    for cap in path_re.captures_iter(code) {
        if let Some(m) = cap.get(1) {
            let name = m.as_str();
            // 检查匹配前是否有 std::, core::, alloc:: 等前缀
            let start = m.start();
            if start > 0 {
                let before = &code[..start];
                if before.ends_with("std::")
                    || before.ends_with("core::")
                    || before.ends_with("alloc::")
                {
                    continue;
                }
            }
            if !STD_CRATES.contains(&name)
                && !COMMON_NAMES.contains(&name)
                && !crates.contains(&name.to_string())
            {
                crates.push(name.to_string());
            }
        }
    }

    crates
}

/// 根据代码中使用的类型/模块，自动生成 std use 语句。
/// 这确保动态引擎生成的 main.rs 中包含必要的 std 导入。
fn generate_std_imports(code: &str) -> String {
    let mut imports = Vec::new();

    // 集合类型
    if code.contains("HashMap::") || code.contains("HashMap<") {
        imports.push("use std::collections::HashMap;".to_string());
    }
    if code.contains("HashSet::") || code.contains("HashSet<") {
        imports.push("use std::collections::HashSet;".to_string());
    }
    if code.contains("BTreeMap::") || code.contains("BTreeMap<") {
        imports.push("use std::collections::BTreeMap;".to_string());
    }
    if code.contains("BTreeSet::") || code.contains("BTreeSet<") {
        imports.push("use std::collections::BTreeSet;".to_string());
    }
    if code.contains("VecDeque::") || code.contains("VecDeque<") {
        imports.push("use std::collections::VecDeque;".to_string());
    }
    if code.contains("BinaryHeap::") || code.contains("BinaryHeap<") {
        imports.push("use std::collections::BinaryHeap;".to_string());
    }
    if code.contains("LinkedList::") || code.contains("LinkedList<") {
        imports.push("use std::collections::LinkedList;".to_string());
    }

    // Hash 相关
    if code.contains("DefaultHasher::") || code.contains("hash::") {
        imports.push("use std::hash::{Hash, Hasher, DefaultHasher};".to_string());
    }
    if code.contains("RandomState") {
        imports.push("use std::collections::hash_map::RandomState;".to_string());
    }

    // 路径相关
    if code.contains("PathBuf::") || code.contains("PathBuf") {
        imports.push("use std::path::PathBuf;".to_string());
    }
    if code.contains("Path::") || (code.contains("Path") && !code.contains("PathBuf")) {
        imports.push("use std::path::Path;".to_string());
    }

    // 网络相关
    if code.contains("Ipv4Addr::") || code.contains("Ipv4Addr") {
        imports.push("use std::net::Ipv4Addr;".to_string());
    }
    if code.contains("Ipv6Addr::") || code.contains("Ipv6Addr") {
        imports.push("use std::net::Ipv6Addr;".to_string());
    }
    if code.contains("SocketAddr::") || code.contains("SocketAddr") {
        imports.push("use std::net::SocketAddr;".to_string());
    }

    // 进程相关
    if code.contains("Command::") && code.contains("new()") {
        imports.push("use std::process::Command;".to_string());
    }
    if code.contains("Child") {
        imports.push("use std::process::Child;".to_string());
    }
    if code.contains("Output") {
        imports.push("use std::process::Output;".to_string());
    }

    // 文件系统相关
    if code.contains("File::") || (code.contains("File") && code.contains("open")) {
        imports.push("use std::fs::File;".to_string());
    }
    if code.contains("OpenOptions::") {
        imports.push("use std::fs::OpenOptions;".to_string());
    }
    if code.contains("Metadata") && code.contains("metadata") {
        imports.push("use std::fs::Metadata;".to_string());
    }
    if code.contains("DirBuilder::") {
        imports.push("use std::fs::DirBuilder;".to_string());
    }
    if code.contains("read_dir") || code.contains("ReadDir") {
        imports.push("use std::fs::{read_dir, ReadDir};".to_string());
    }
    if code.contains("DirEntry") {
        imports.push("use std::fs::DirEntry;".to_string());
    }
    if code.contains("FileType") {
        imports.push("use std::fs::FileType;".to_string());
    }
    if code.contains("Permissions") {
        imports.push("use std::fs::Permissions;".to_string());
    }

    // 时间/线程相关
    if code.contains("SystemTime::") || code.contains("SystemTime") {
        imports.push("use std::time::SystemTime;".to_string());
    }
    if code.contains("Instant::") || code.contains("Instant") {
        imports.push("use std::time::Instant;".to_string());
    }
    if code.contains("Duration::") || code.contains("Duration") {
        imports.push("use std::time::Duration;".to_string());
    }
    if code.contains("thread::") || code.contains("Thread") {
        imports.push("use std::thread;".to_string());
    }
    if code.contains("JoinHandle") {
        imports.push("use std::thread::JoinHandle;".to_string());
    }

    // mem 相关
    if code.contains("size_of::") || code.contains("size_of::<") {
        imports.push("use std::mem::size_of;".to_string());
    }
    if code.contains("size_of_val::") {
        imports.push("use std::mem::size_of_val;".to_string());
    }
    if code.contains("align_of::") {
        imports.push("use std::mem::align_of;".to_string());
    }

    // env 相关
    if code.contains("env::") {
        imports.push("use std::env;".to_string());
    }

    // cmp 相关
    if code.contains("cmp::") || code.contains("Ordering::") {
        imports.push("use std::cmp::Ordering;".to_string());
    }

    // default 相关
    if code.contains("Default::default()") && !code.contains("use std::default::Default;") {
        imports.push("use std::default::Default;".to_string());
    }

    // clone 相关
    if code.contains(".clone()") && !code.contains("use std::clone::Clone;") {
        imports.push("use std::clone::Clone;".to_string());
    }

    // iter 相关
    if code.contains(".iter()") || code.contains(".into_iter()") {
        imports.push("use std::iter::Iterator;".to_string());
    }

    // ops 相关
    if code.contains("ops::") || code.contains("Range::") {
        imports.push("use std::ops;".to_string());
    }

    // io 相关
    if code.contains("io::") || code.contains("Read") || code.contains("Write") {
        imports.push("use std::io;".to_string());
    }

    imports.join("\n")
}

/// Extracts free variable names from an expression AST.
fn extract_free_variables(expr: &Expr) -> Vec<String> {
    let mut vars = Vec::new();
    collect_vars(expr, &mut vars);
    vars.sort();
    vars.dedup();
    vars
}

fn collect_vars(expr: &Expr, vars: &mut Vec<String>) {
    match expr {
        Expr::Path(path_expr) => {
            let path = &path_expr.path;
            if path.segments.len() == 1 && path.leading_colon.is_none() {
                let ident = &path.segments[0].ident;
                let name = ident.to_string();
                if !is_keyword(&name) && !vars.contains(&name) {
                    vars.push(name);
                }
            }
        }
        Expr::Binary(bin) => {
            collect_vars(&bin.left, vars);
            collect_vars(&bin.right, vars);
        }
        Expr::Unary(unary) => {
            collect_vars(&unary.expr, vars);
        }
        Expr::Paren(paren) => {
            collect_vars(&paren.expr, vars);
        }
        Expr::Group(group) => {
            collect_vars(&group.expr, vars);
        }
        Expr::Call(call) => {
            collect_vars(&call.func, vars);
            for arg in &call.args {
                collect_vars(arg, vars);
            }
        }
        Expr::MethodCall(method) => {
            collect_vars(&method.receiver, vars);
            for arg in &method.args {
                collect_vars(arg, vars);
            }
        }
        Expr::Block(block) => {
            for stmt in &block.block.stmts {
                match stmt {
                    syn::Stmt::Local(local) => {
                        if let Some(init) = &local.init {
                            collect_vars(&init.expr, vars);
                        }
                    }
                    syn::Stmt::Expr(e, _) => {
                        collect_vars(e, vars);
                    }
                    _ => {}
                }
            }
        }
        Expr::If(if_expr) => {
            collect_vars(&if_expr.cond, vars);
            collect_vars_block(&if_expr.then_branch, vars);
            if let Some((_, else_branch)) = &if_expr.else_branch {
                collect_vars(else_branch, vars);
            }
        }
        Expr::Match(match_expr) => {
            collect_vars(&match_expr.expr, vars);
            for arm in &match_expr.arms {
                collect_vars(&arm.body, vars);
                if let Some((_, guard)) = &arm.guard {
                    collect_vars(guard, vars);
                }
            }
        }
        Expr::Loop(loop_expr) => {
            collect_vars_block(&loop_expr.body, vars);
        }
        Expr::While(while_expr) => {
            collect_vars(&while_expr.cond, vars);
            collect_vars_block(&while_expr.body, vars);
        }
        Expr::ForLoop(for_expr) => {
            collect_vars(&for_expr.expr, vars);
            collect_vars_block(&for_expr.body, vars);
        }
        Expr::Closure(closure) => {
            collect_vars(&closure.body, vars);
        }
        Expr::Reference(ref_expr) => {
            collect_vars(&ref_expr.expr, vars);
        }
        Expr::Cast(cast) => {
            collect_vars(&cast.expr, vars);
        }
        Expr::Tuple(tuple) => {
            for elem in &tuple.elems {
                collect_vars(elem, vars);
            }
        }
        Expr::Array(array) => {
            for elem in &array.elems {
                collect_vars(elem, vars);
            }
        }
        Expr::Range(range) => {
            if let Some(start) = &range.start {
                collect_vars(start, vars);
            }
            if let Some(end) = &range.end {
                collect_vars(end, vars);
            }
        }
        Expr::Index(index) => {
            collect_vars(&index.expr, vars);
            collect_vars(&index.index, vars);
        }
        Expr::Field(field) => {
            collect_vars(&field.base, vars);
        }
        _ => {}
    }
}

fn collect_vars_block(block: &syn::Block, vars: &mut Vec<String>) {
    for stmt in &block.stmts {
        match stmt {
            syn::Stmt::Local(local) => {
                if let Some(init) = &local.init {
                    collect_vars(&init.expr, vars);
                }
            }
            syn::Stmt::Expr(e, _) => {
                collect_vars(e, vars);
            }
            _ => {}
        }
    }
}

fn is_keyword(name: &str) -> bool {
    matches!(
        name,
        "true"
            | "false"
            | "self"
            | "Self"
            | "super"
            | "crate"
            | "Some"
            | "None"
            | "Ok"
            | "Err"
            | "Vec"
            | "String"
            | "Box"
            | "Option"
            | "Result"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "f32"
            | "f64"
            | "bool"
            | "char"
            | "str"
            | "if"
            | "else"
            | "match"
            | "for"
            | "while"
            | "loop"
            | "fn"
            | "let"
            | "mut"
            | "pub"
            | "use"
            | "mod"
            | "struct"
            | "enum"
            | "impl"
            | "trait"
            | "type"
            | "return"
            | "break"
            | "continue"
            | "move"
            | "ref"
            | "async"
            | "await"
            | "where"
            | "const"
            | "static"
            | "in"
            | "as"
            | "dyn"
            | "unsafe"
            | "extern"
    )
}

/// Dynamic execution engine powered by cargo subprocess.
pub struct DynamicEngine {}

impl DynamicEngine {
    pub fn new() -> Result<Self, String> {
        // Verify that cargo is available
        let output = Command::new("cargo")
            .arg("--version")
            .output()
            .map_err(|e| format!("cargo not available: {}", e))?;
        if !output.status.success() {
            return Err("cargo --version failed".to_string());
        }
        Ok(Self {})
    }

    /// Executes code through a temporary cargo project and returns the raw text output.
    fn execute_code(&mut self, code: &str) -> Result<String, String> {
        // Detect and collect dependencies
        let external_crates = detect_external_crates(code);

        // Create a unique temp directory based on code hash to enable caching
        let code_hash = format!("{:x}", md5::compute(code));
        let temp_dir = std::env::temp_dir().join(format!("preprocessor_cargo_{}", code_hash));

        // Always recreate the project to ensure dependencies are correct
        if temp_dir.exists() {
            std::fs::remove_dir_all(&temp_dir)
                .map_err(|e| format!("failed to remove old temp dir: {}", e))?;
        }

        let src_dir = temp_dir.join("src");
        std::fs::create_dir_all(&src_dir)
            .map_err(|e| format!("failed to create temp dir: {}", e))?;

        // Write Cargo.toml with detected dependencies
        write_cargo_toml(&temp_dir, &external_crates, code)?;

        // Write src/main.rs with the code
        write_main_rs(&temp_dir, code)?;

        // Run `cargo run` and capture output
        let output = Command::new("cargo")
            .arg("run")
            .arg("--quiet")
            .current_dir(&temp_dir)
            .output()
            .map_err(|e| format!("failed to run cargo: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !output.status.success() {
            return Err(format!("compilation failed:\n{}", stderr));
        }

        Ok(stdout)
    }

    /// Evaluates a `syn::Expr` through dynamic cargo execution.
    pub fn evaluate(&mut self, expr: &Expr) -> EvalResult {
        // Check for free variables first
        let free_vars = extract_free_variables(expr);
        if !free_vars.is_empty() {
            return EvalResult::PassThrough;
        }

        // Convert expression to executable code
        let code = expr_to_code(expr);

        // Execute through cargo
        match self.execute_code(&code) {
            Ok(output) => match parse_output(&output) {
                Some(value) => EvalResult::Value(value),
                None => {
                    let trimmed = output.trim();
                    if trimmed == "()" || trimmed.is_empty() {
                        EvalResult::Value(Value::Unit)
                    } else {
                        EvalResult::PassThrough
                    }
                }
            },
            Err(e) => {
                eprintln!("[preprocessor:dynamic] Execution failed: {}", e);
                EvalResult::PassThrough
            }
        }
    }

    /// Evaluates a block of statements through dynamic cargo execution.
    #[allow(dead_code)]
    pub fn evaluate_block(&mut self, block: &syn::Block) -> EvalResult {
        let code = block_to_code(block);

        match self.execute_code(&code) {
            Ok(output) => match parse_output(&output) {
                Some(value) => EvalResult::Value(value),
                None => EvalResult::PassThrough,
            },
            Err(e) => {
                eprintln!("[preprocessor:dynamic] Block execution failed: {}", e);
                EvalResult::PassThrough
            }
        }
    }
}

impl Default for DynamicEngine {
    fn default() -> Self {
        Self::new().expect("Failed to initialize dynamic engine")
    }
}

// ============================================================================
// Temp project helpers
// ============================================================================

fn write_cargo_toml(dir: &PathBuf, external_crates: &[String], code: &str) -> Result<(), String> {
    let mut toml = String::from(
        r#"[package]
name = "preprocessor_eval"
version = "0.1.0"
edition = "2021"

[dependencies]
"#,
    );

    for crate_name in external_crates {
        toml.push_str(&format!("{} = \"*\"\n", crate_name));
    }

    // 坡底逻辑：如果代码中明显使用了 chrono 但未被检测到，则强制添加
    if (code.contains("Local") || code.contains("chrono::"))
        && !external_crates.iter().any(|c| c == "chrono")
    {
        toml.push_str("chrono = \"*\"\n");
    }

    // 检测异步代码并自动添加 tokio 依赖
    let is_async = code.contains(".await") || code.contains("async ");
    if is_async && !external_crates.iter().any(|c| c == "tokio") {
        toml.push_str("tokio = { version = \"*\", features = [\"full\"] }\n");
    }

    // 检测是否使用了 reqwest，确保添加完整特性
    if code.contains("reqwest::") && !external_crates.iter().any(|c| c == "reqwest") {
        toml.push_str("reqwest = { version = \"*\", features = [\"blocking\"] }\n");
    }

    let cargo_toml_path = dir.join("Cargo.toml");
    std::fs::write(&cargo_toml_path, toml)
        .map_err(|e| format!("failed to write Cargo.toml: {}", e))?;
    Ok(())
}

fn write_main_rs(dir: &PathBuf, code: &str) -> Result<(), String> {
    let main_rs_path = dir.join("src").join("main.rs");

    // 尝试从原始代码中提取 use 语句
    let mut use_statements = String::new();
    let mut body_code = String::new();

    for line in code.lines() {
        if line.trim().starts_with("use ") {
            use_statements.push_str(line);
            use_statements.push('\n');
        } else {
            body_code.push_str(line);
            body_code.push('\n');
        }
    }

    // 自动生成 std 库 use 语句
    let std_imports = generate_std_imports(&body_code);
    if !std_imports.is_empty() {
        use_statements.insert_str(0, &std_imports);
        use_statements.push('\n');
    }

    // 检测是否使用了 chrono 相关的类型，如果有则自动注入依赖
    let needs_chrono =
        body_code.contains("chrono::") || body_code.contains("Local") || body_code.contains("Utc");

    if needs_chrono && !use_statements.contains("chrono") {
        use_statements.insert_str(0, "use chrono::{Local, Utc, TimeZone, NaiveDateTime};\n");
    }

    // 检测是否是异步代码
    let is_async = body_code.contains(".await") || body_code.contains("async ");
    
    // 检测是否使用了 ? 运算符
    let uses_try = body_code.contains('?');
    
    let content = if is_async {
        // 异步代码处理
        if uses_try {
            // 使用 ? 运算符时，需要返回 Result 类型
            format!(
                r#"{}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {{
    let result = (async {{
        let _result = {{
            {}
        }};
        Ok::<_, Box<dyn std::error::Error>>(_result)
    }}).await?;
    println!("{{:?}}", result);
    Ok(())
}}
"#,
                use_statements, body_code
            )
        } else {
            // 不使用 ? 运算符的异步代码
            format!(
                r#"{}
#[tokio::main]
async fn main() {{
    let result = async {{
        {}
    }}.await;
    println!("{{:?}}", result);
}}
"#,
                use_statements, body_code
            )
        }
    } else {
        // 同步代码处理
        format!(
            r#"{}
fn main() {{
    let result = {{
        {}
    }};
    println!("{{:?}}", result);
}}
"#,
            use_statements, body_code
        )
    };
    
    std::fs::write(&main_rs_path, content)
        .map_err(|e| format!("failed to write main.rs: {}", e))?;
    Ok(())
}

// ============================================================================
// Output parsing
// ============================================================================

fn parse_output(output: &str) -> Option<Value> {
    let text = output.trim();
    if text.is_empty() {
        return Some(Value::Unit);
    }

    parse_value(text)
}

fn parse_value(text: &str) -> Option<Value> {
    if text.is_empty() {
        return Some(Value::Unit);
    }

    if text == "()" {
        return Some(Value::Unit);
    }

    if text == "true" {
        return Some(Value::Bool(true));
    }
    if text == "false" {
        return Some(Value::Bool(false));
    }

    // Char: 'x'
    if text.starts_with('\'') && text.ends_with('\'') && text.len() >= 3 {
        if let Ok(c) = text[1..text.len() - 1].parse::<char>() {
            return Some(Value::Char(c));
        }
    }

    // String: "hello"
    if text.starts_with('"') && text.ends_with('"') && text.len() >= 2 {
        let inner = &text[1..text.len() - 1];
        let unescaped = inner
            .replace("\\n", "\n")
            .replace("\\t", "\t")
            .replace("\\\"", "\"")
            .replace("\\\\", "\\");
        return Some(Value::Str(unescaped));
    }

    // Float special cases
    match text {
        "inf" | "infinity" | "f64::INFINITY" => return Some(Value::Float(f64::INFINITY)),
        "-inf" | "-infinity" | "f64::NEG_INFINITY" => return Some(Value::Float(f64::NEG_INFINITY)),
        "NaN" | "f64::NAN" | "nan" => return Some(Value::Float(f64::NAN)),
        _ => {}
    }

    // Float
    if text.contains('.') || text.contains('e') || text.contains('E') {
        if let Ok(f) = text.parse::<f64>() {
            return Some(Value::Float(f));
        }
    }

    // Integer
    if let Ok(n) = text.parse::<i64>() {
        return Some(Value::Int(n));
    }

    if let Ok(n) = text.parse::<u64>() {
        if n <= i64::MAX as u64 {
            return Some(Value::Int(n as i64));
        }
    }

    // Tuple
    if text.starts_with('(') && text.ends_with(')') {
        if let Some(vals) = parse_tuple(text) {
            return Some(Value::Tuple(vals));
        }
    }

    // Array/Vec
    if text.starts_with('[') && text.ends_with(']') {
        if let Some(vals) = parse_array(text) {
            return Some(Value::Array(vals));
        }
    }

    None
}

fn parse_tuple(text: &str) -> Option<Vec<Value>> {
    let inner = &text[1..text.len() - 1];
    if inner.trim().is_empty() {
        return Some(vec![]);
    }

    let elements = split_top_level(inner, ',');
    let mut values = Vec::new();
    for elem in elements {
        values.push(parse_value(elem.trim())?);
    }
    Some(values)
}

fn parse_array(text: &str) -> Option<Vec<Value>> {
    let inner = &text[1..text.len() - 1];
    if inner.trim().is_empty() {
        return Some(vec![]);
    }

    let elements = split_top_level(inner, ',');
    let mut values = Vec::new();
    for elem in elements {
        values.push(parse_value(elem.trim())?);
    }
    Some(values)
}

fn split_top_level(text: &str, delimiter: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth_paren = 0;
    let mut depth_bracket = 0;
    let mut in_string = false;
    let mut in_char = false;
    let mut escape = false;
    let mut start = 0;

    let chars: Vec<char> = text.chars().collect();
    for i in 0..chars.len() {
        let c = chars[i];

        if escape {
            escape = false;
            continue;
        }

        match c {
            '\\' if in_string || in_char => {
                escape = true;
            }
            '"' if !in_char => {
                in_string = !in_string;
            }
            '\'' if !in_string => {
                in_char = !in_char;
            }
            '(' if !in_string && !in_char => {
                depth_paren += 1;
            }
            ')' if !in_string && !in_char => {
                depth_paren -= 1;
            }
            '[' if !in_string && !in_char => {
                depth_bracket += 1;
            }
            ']' if !in_string && !in_char => {
                depth_bracket -= 1;
            }
            c if c == delimiter
                && depth_paren == 0
                && depth_bracket == 0
                && !in_string
                && !in_char =>
            {
                parts.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }

    parts.push(&text[start..]);
    parts
}

fn expr_to_code(expr: &Expr) -> String {
    quote::quote!(#expr).to_string()
}

#[allow(dead_code)]
fn block_to_code(block: &syn::Block) -> String {
    quote::quote!(#block).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_external_crates() {
        let code = r#"use chrono::Utc;
use serde::Deserialize;
let now = Utc::now();"#;
        let crates = detect_external_crates(code);
        assert!(crates.contains(&"chrono".to_string()));
        assert!(crates.contains(&"serde".to_string()));
    }

    #[test]
    fn test_extract_free_variables_simple() {
        let expr: Expr = syn::parse_quote!(1 + 2);
        let vars = extract_free_variables(&expr);
        assert!(vars.is_empty());
    }

    #[test]
    fn test_extract_free_variables_with_vars() {
        let expr: Expr = syn::parse_quote!(x + 1);
        let vars = extract_free_variables(&expr);
        assert_eq!(vars, vec!["x"]);
    }

    #[test]
    fn test_parse_output_integer() {
        assert_eq!(parse_value("42"), Some(Value::Int(42)));
        assert_eq!(parse_value("-7"), Some(Value::Int(-7)));
    }

    #[test]
    fn test_parse_output_float() {
        assert_eq!(parse_value("3.14"), Some(Value::Float(3.14)));
        assert_eq!(parse_value("-2.5"), Some(Value::Float(-2.5)));
    }

    #[test]
    fn test_parse_output_bool() {
        assert_eq!(parse_value("true"), Some(Value::Bool(true)));
        assert_eq!(parse_value("false"), Some(Value::Bool(false)));
    }

    #[test]
    fn test_parse_output_string() {
        assert_eq!(
            parse_value("\"hello\""),
            Some(Value::Str("hello".to_string()))
        );
    }

    #[test]
    fn test_parse_output_tuple() {
        let result = parse_value("(1, 2, 3)");
        assert!(matches!(result, Some(Value::Tuple(_))));
        if let Some(Value::Tuple(vals)) = result {
            assert_eq!(vals.len(), 3);
        }
    }

    #[test]
    fn test_parse_output_array() {
        let result = parse_value("[1, 2, 3]");
        assert!(matches!(result, Some(Value::Array(_))));
        if let Some(Value::Array(vals)) = result {
            assert_eq!(vals.len(), 3);
        }
    }

    #[test]
    fn test_split_top_level() {
        let parts = split_top_level("1, (2, 3), 4", ',');
        assert_eq!(parts, vec!["1", " (2, 3)", " 4"]);
    }
}
