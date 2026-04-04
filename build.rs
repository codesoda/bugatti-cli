fn main() {
    let target = std::env::var("TARGET").expect("TARGET not set");
    println!("cargo:rustc-env=TARGET={target}");
}
