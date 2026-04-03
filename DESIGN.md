# Preprocessor — 编译期计算宏库

## 项目概述

通过两个宏实现编译期优化：分析代码中的可计算子表达式，将可在编译期执行的部分直接求值，使最终二进制只包含结果。

| 宏 | 场景 | 示例 |
|---|---|---|
| `#[preprocessor::optimize]` | 函数级属性宏 | `#[preprocessor::optimize] fn f(a: i32) -> i64 { ... }` |
| `preprocessor::op!(...)` | 表达式级宏 | `let r = preprocessor::op!(1 + 1);` |

---

## 架构

```
用户代码
    ├─► preprocessor-derive（proc-macro crate）
    │       ├─► #[preprocessor::optimize] — 输入 FunctionItem，输出转换后的 FunctionItem
    │       └─► preprocessor::op! — 输入 Expr，输出求值结果或原表达式透传
    └─► preprocessor（运行时 crate）
            ├─► 公开导出 derive 宏
            └─► 可选运行时工具函数
```

### 技术选型

| 层次 | 选型 | 说明 |
|---|---|---|
| AST 解析 | `syn` 2.x | `syn::Expr`、`syn::Item`、`syn::Stmt` |
| 代码生成 | `quote!` | TokenStream 转换 |
| 求值引擎 | **纯自研解释器** | AST 遍历求值，输出最简化 AST |
| 构建配置 | Cargo workspace | `preprocessor/` + `preprocessor-derive/` |

---

## 仓库现状 & 阻塞问题

- [ ] 无 Cargo workspace — 缺少根 `Cargo.toml`
- [ ] `preprocessor-derive/Cargo.toml` 缺少 `proc-macro = true`
- [ ] 无依赖配置 — `syn`、`quote`、`proc-macro2` 均缺失
- [ ] 两 crate 内容完全一致 — 模板残留代码
- [ ] `preprocessor` 未配置对 `preprocessor-derive` 的依赖及宏重导出

---

## 求值引擎规范

### 核心设计

纯自研解释器：在 proc-macro 中遍历 `syn::Expr` AST，用 Rust 原始类型直接求值，输出最简化 AST。

**不调用 rustc 内部 API**（Clippy 已验证 `tcx.const_eval_*` 不可作为 proc-macro 稳定 API）。

### CTFE 约束（指导解释器设计）

- const 上下文仅允许受限表达式子集；只有 `const fn` 可在 const 表达式中调用
- const 求值中的 panic 视为编译错误
- 堆分配有未解决 soundness 问题；内联 asm、raw pointer 与 int 互转、线程本地访问被禁止
- CTFE 使用 `AllocId`+offset 虚拟内存（符号化地址，非具体数值）
- MIR 解释器与 Miri 共享；`trivial_const` 快速路径用于不需完整解释器的常量
- const trait 方法调用仍重度门控（tracking issue #143874）

### 支持表达式（v1）

| 类别 | 示例 |
|---|---|
| 字面量 | `1`, `"hello"`, `1.0f64`, `b'x'` |
| 算术/位/比较/逻辑运算 | `a + b`, `a & b`, `a == b`, `a && b`, `!a` |
| 括号分组 | `(a + b) * c` |
| 基本类型转换 | `a as i32` |
| 元组/数组/块表达式 | `(1, 2)`, `[1, 2, 3]`, `{ let x = 1; x + 1 }` |
| `const` 静态项 / `const fn` 调用 | `const X: i32 = 1 + 2;` |

### 语义细节

| 问题 | 策略 |
|---|---|
| 整数溢出 | 以编译期为准 |
| 浮点数 NaN/inf/subnormal | 以编译期为准，不考虑与运行时差异 |
| Panic（如除零） | 发出警告，中止编译 |
| 副作用（如 `println!`） | 在编译期执行，并发出警告 |
| 泛型函数 | 检查可求值部分并求值，其余保留给运行时 |
| 自由变量 | 宽松策略：能求什么求什么，其余透传 |
| 闭包/非 `'static` 捕获 | 直接透传 |
| `dyn Trait` 方法 | 能确定具体方法则尝试计算，否则透传 |
| 堆分配操作（如 `vec!`） | 模拟原始类型操作方式计算，不依赖 const 求值器 |
| 递归/无限循环 | 解释器加超时/步数限制，超限则跳过优化并报警告 |
| Edition 2024 兼容性 | 忽略，遇到则编译报错 |
| 编译时间 vs 二进制体积 | 通过 `disabled` feature 开关控制 |
| 错误报告 | 不支持求值的直接透传，不报错 |

### 与 LLVM 优化器的交互

宏的价值在于**保证**编译期求值——对于 `DateTime` 等接口调用，LLVM 无法优化，宏填补此空白。

---

## `op!` 行为

1. `syn::Expr` 解析输入表达式
2. 前序/后序遍历 AST
3. 对不含自由运行时变量的可求值子表达式：编译期求值 → 替换为字面量 token
4. 返回转换后的表达式

## `#[preprocessor::optimize]` 行为

1. 接收完整函数体，递归遍历所有语句和表达式
2. 识别可提升到编译期的子表达式
3. 重写函数体：编译期部分提前计算，运行时部分保留原样
4. `let x = <编译期可求值>;` → `let x = <已计算字面量>;`

---

## 先例与相关工作

> 穷尽搜索 GitHub（100+ ⭐，2026-04-03）

| 项目 | ⭐ | 启示 |
|---|---|---|
| [rust-phf](https://github.com/rust-phf/rust-phf) | 2120 | 经典"宏时预计算"模式 |
| [MIRAI](https://github.com/facebookexperimental/MIRAI) | 1010 | MIR 抽象解释器，与"编译期推理"相邻 |
| [static-assertions-rs](https://github.com/nvzqz/static-assertions-rs) | 653 | 编译期布尔求值参考 |
| [typenum](https://github.com/paholg/typenum) | 582 | 类型级计算极限 |
| [const_format](https://github.com/rodrimati1992/const_format_crates) | 270 | const-context 严格限制 |
| [konst](https://github.com/rodrimati1992/konst) | 121 | 填补 const 语言特性缺失 |
| [const-str](https://github.com/Nugine/const-str) | 118 | "真正 const 求值" vs "const-fn 可求值"边界 |
| [seq-macro](https://github.com/dtolnay/seq-macro) | 165 | token 级编程模式 |
| [const-eval](https://github.com/rust-lang/const-eval) | 114 | MIR 解释器核心作用 |

**关键发现**：无广泛使用的 crate 做通用函数体优化。现有项目局限在字符串、断言、查找表或类型级算术。

**Clippy 混合方案**（`src/tools/clippy/clippy_utils/src/consts.rs`）：使用 `tcx.const_eval_resolve` + 自定义求值器。注释明确说明 *"This cannot use rustc's const eval ... arbitrary HIR expressions cannot be lowered"*。

---

## Proc-Macro 实现模式参考

> 从 tracing、tokio、async-trait、Rocket、Clippy 提取

### 函数体遍历与重写

**提取函数最后一个表达式**（tracing-attributes `expand.rs`）：
```rust
let (last_expr_stmt, last_expr) = block.stmts.iter().rev().find_map(|stmt| {
    if let Stmt::Expr(expr, _semi) = stmt { Some((stmt, expr)) } else { None }
})?;
```

**重写函数体**（async-trait `expand.rs`）：
```rust
block.stmts = parse_quote!(#box_pin);
```

**自定义 `ItemFn` 解析器**（tokio-macros `entry.rs`）：
```rust
impl Parse for ItemFn {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> { ... }
}
```

### 表达式级解析（`op!` 模板）

**Rocket `ArgExpr` 解析**（`core/codegen/src/bang/uri_parsing.rs`）：
```rust
impl Parse for ArgExpr {
    fn parse(input: ParseStream<'_>) -> parse::Result<Self> {
        if input.peek(Token![_]) { ... }
        input.parse::<Expr>().map(ArgExpr::Expr)
    }
}
```

**表达式提取与安全转换**（`core/codegen/src/bang/uri.rs`）：
```rust
let let_stmt = quote_spanned!(span => let #tmp_ident = #expr);
to.push(quote_spanned!(mixed(span) =>
    #[allow(non_snake_case)] #let_stmt;
    let #ident = <#ty as ...>::from_uri_param(#tmp_ident);
));
```

### Span 与诊断

**优雅降级**（tracing-attributes）：
```rust
instrument_precise(args.clone(), item.clone())
    .unwrap_or_else(|_err| instrument_speculative(args, item))
```

**`quote_spanned!` 保留 span 信息**（tokio-macros）：
```rust
let use_builder = quote_spanned! {Span::call_site().located_at(last_stmt_start_span)=>
    use #crate_path::runtime::Builder;
};
```

---

## 威胁模型

| 威胁 | 可能性 | 缓解 |
|---|---|---|
| 编译期无限循环（DoS） | 中 | v1 限制非递归；解释器加步数限制 |
| 优化失败的二进制膨胀 | 低 | 测试回归检测 |
| 与 unsafe 代码交互 | 中 | v1 范围排除 unsafe |
| 与 `asm!`、`std::hint` 等交互 | 低 | v1 范围排除 |

---

## 实现 TODO

### 基础设施（阻塞项）

- [ ] 创建根 `Cargo.toml` workspace（`members = ["preprocessor", "preprocessor-derive"]`, `resolver = "2"`）
- [ ] `preprocessor-derive/Cargo.toml` 添加 `proc-macro = true`
- [ ] `preprocessor-derive` 添加 `syn = "2"`, `quote = "2"`, `proc-macro2 = "2"` 依赖
- [ ] `preprocessor` 添加对 `preprocessor-derive` 的依赖并重导出宏
- [ ] 清理子 crate 中的嵌套 `.git/` 目录

### 核心实现

- [ ] 构建 AST 遍历求值器（支持 v1 表达式子集）
- [ ] 实现 `preprocessor::op!` 表达式宏
- [ ] 实现 `#[preprocessor::optimize]` 属性宏
- [ ] 解释器步数限制/超时机制
- [ ] 编译期副作用执行 + 警告机制
- [ ] `disabled` feature 开关

### 规范化

- [ ] 编写支持表达式的形式化文法（EBNF）
- [ ] 定义可优化表达式的三个层次：
  - **可直接求值**（字面量、基本算术、无自由变量）
  - **条件可求值**（含 const 变量、`const fn`）
  - **无法求值**（含泛型、dyn、副作用、堆分配）→ 透传
- [ ] 规范错误信息格式与 span 标注方式

### 测试与验证

- [ ] `trybuild` / `compiletest_rs` 编译失败测试
- [ ] 基准测试：真实场景下的编译时间开销
- [ ] 测试与 `rustc` 优化器的交互（确保无回归）
- [ ] 参考 tokio-macros 优雅降级模式，确保 IDE 可用性
