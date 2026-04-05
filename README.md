# preprocessor

![version](https://img.shields.io/badge/dynamic/toml?url=https%3A%2F%2Fraw.githubusercontent.com%2Ffawdlstty%2Fpreprocessor%2Fmain%2F/preprocessor/Cargo.toml&query=package.version&label=version)
![status](https://img.shields.io/github/actions/workflow/status/fawdlstty/preprocessor/rust.yml)

English | [简体中文](README.zh.md)

Compile-time computation macro library — analyzes computable sub-expressions in code and evaluates parts that can be executed at compile time.

## Installation

```shell
cargo add preprocessor
```

## Usage

### `#[optimize]` — Function-level attribute macro

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

### `op!` — Expression-level macro

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

### Async Support

The `op!` macro fully supports async/await and the `?` operator, enabling compile-time evaluation of asynchronous code:

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

**Key Features:**
- ✅ Full async/await support
- ✅ `?` error propagation operator
- ✅ Compile-time evaluation of async operations