# preprocessor

![version](https://img.shields.io/badge/dynamic/toml?url=https%3A%2F%2Fraw.githubusercontent.com%2Ffawdlstty%2Fpreprocessor%2Fmain%2F/preprocessor/Cargo.toml&query=package.version&label=version)
![status](https://img.shields.io/github/actions/workflow/status/fawdlstty/preprocessor/rust.yml)

[English](README.md) | 简体中文

编译期计算宏库 — 分析代码中的可计算子表达式，将可在编译期执行的部分直接求值。

## 安装

```shell
cargo add preprocessor
```

## 用法

### `#[optimize]` — 函数级属性宏

```shell
cargo add chrono
```

```rust
#[preprocessor::optimize]
fn compute() -> String {
    chrono::Local::now()
        .naive_local()
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

fn main() {
    let time = compute();
    println!("build_time: {time}");
}
```

### `op!` — 表达式级宏

```shell
cargo add chrono
```

```rust
fn main() {
    let time = preprocessor::op!(
        chrono::Local::now()
            .naive_local()
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()
    );
    println!("build_time: {time}");
}
```

### 异步支持

`op!` 宏完整支持 async/await 和 `?` 运算符，可以对异步代码进行编译期求值：

```shell
cargo add tokio reqwest
```

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let text = preprocessor::op!({
        let response = reqwest::get("https://www.fawdlstty.com").await?;
        response.text().await?
    });
    println!("{}", text);
    Ok(())
}
```

**核心特性：**
- ✅ 完整支持 async/await
- ✅ `?` 错误传播运算符
- ✅ 异步操作的编译期求值
