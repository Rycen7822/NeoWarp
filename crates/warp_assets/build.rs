fn main() {
    println!("cargo:rerun-if-changed=../../app/assets/bundled");
    println!("cargo:rerun-if-changed=../../app/assets/async");
}
