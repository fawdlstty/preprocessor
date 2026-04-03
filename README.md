# preprocessor

编译期计算宏库 — 分析代码中的可计算子表达式，将可在编译期执行的部分直接求值，使最终二进制只包含结果。

## 安装

```toml
[dependencies]
preprocessor = { path = "path/to/preprocessor/preprocessor" }
```

## 用法

### `op!` — 表达式级宏

对单个表达式进行编译期求值，可求值的子表达式替换为字面量，含自由变量的部分透传。

```rust
use preprocessor::op;

fn main() {
    // 纯字面量算术 — 编译期直接求值
    let result = op!(1 + 2 * 3);
    assert_eq!(result, 7);

    // 嵌套括号
    let x = op!((10 + 5) / 3);
    assert_eq!(x, 5);

    // 布尔逻辑
    let flag = op!(true && false || true);
    assert_eq!(flag, true);

    // 比较运算
    let cmp = op!(42 > 10 && 1 <= 1);
    assert_eq!(cmp, true);

    // 含自由变量 — 透传不优化
    let a = 5;
    let y = op!(a + 1);  // → a + 1（保持原样）

    // 元组/数组字面量
    let t = op!((1, 2, 3));
    let arr = op!([10, 20, 30]);

    // 字符串/字符/字节
    let s = op!("hello");
    let c = op!('x');
    let b = op!(b'A');

    // 位运算
    let bits = op!(0xFF & 0x0F);
    assert_eq!(bits, 0x0F);

    // 类型转换
    let n = op!(42 as i64);
}
```

### `#[optimize]` — 函数级属性宏

递归遍历函数体内所有语句和表达式，识别可提升到编译期的子表达式并重写为预计算字面量。

```rust
use preprocessor::optimize;

#[optimize]
fn compute() -> i32 {
    let a = 1 + 2;     // → let a = 3;
    let b = 4 * 5;     // → let b = 20;
    let c = 100 / 10;  // → let c = 10;
    a + b + c          // 含变量，保留
}

fn main() {
    let result = compute();
    assert_eq!(result, 33);
}
```

## 支持的表达式（v1）

| 类别 | 示例 |
|---|---|
| 字面量 | `1`, `"hello"`, `1.0f64`, `b'x'`, `true` |
| 算术运算 | `a + b`, `a - b`, `a * b`, `a / b`, `a % b` |
| 位运算 | `a & b`, `a \| b`, `a ^ b`, `a << b`, `a >> b` |
| 比较运算 | `a == b`, `a != b`, `a < b`, `a <= b`, `a > b`, `a >= b` |
| 逻辑运算 | `a && b`, `a \|\| b`, `!a` |
| 一元取反 | `-a`, `!a` |
| 括号分组 | `(a + b) * c` |
| 类型转换 | `a as i32` |
| 元组/数组 | `(1, 2)`, `[1, 2, 3]` |
| 块表达式 | `{ let x = 1; x + 1 }` |

## 语义策略

| 场景 | 行为 |
|---|---|
| 整数溢出 | 以编译期为准（checked 运算，溢出时报错） |
| 浮点数 NaN/inf | 正常输出 `f64::NAN`、`f64::INFINITY` 等 |
| 除零 | 发出 `compile_error!`，中止编译 |
| 含自由变量 | 透传原表达式，不报错 |
| 闭包/非 `'static` 捕获 | 直接透传 |
| 递归/无限循环 | 解释器加步数限制（1,000,000 步），超限跳过优化 |
| 不支持的表达式 | 直接透传，不报错 |

## Features

| Feature | 说明 |
|---|---|
| `disabled` | 禁用所有编译期优化，宏变为透明透传。用于调试或对比性能。 |

```toml
[dependencies]
preprocessor = { path = "...", features = ["disabled"] }
```

启用 `disabled` 后：
- `op!(expr)` → 直接展开为 `expr`
- `#[optimize]` → 函数体保持不变

## 项目结构

```
preprocessor/
├── Cargo.toml              # Workspace 根配置
├── preprocessor/           # 运行时 crate — 宏重导出
│   ├── Cargo.toml
│   └── src/lib.rs
└── preprocessor-derive/    # Proc-macro crate — 实现求值引擎和宏
    ├── Cargo.toml
    └── src/
        ├── lib.rs          # op! 和 #[optimize] 宏入口
        └── evaluator.rs    # AST 遍历求值引擎
```

## 技术栈

- **AST 解析**: `syn` 2.x
- **代码生成**: `quote!`
- **求值引擎**: 纯自研解释器（AST 遍历求值，不调用 rustc 内部 API）
- **构建**: Cargo workspace

## 与 LLVM 优化器的关系

本宏的价值在于**保证**编译期求值。对于 `DateTime` 等接口调用，LLVM 无法优化，本宏填补此空白。
