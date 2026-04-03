fn main() {
    let n = preprocessor::op!({
        println!("hello");
        1 + 2
    });
    println!("{n}");
}
