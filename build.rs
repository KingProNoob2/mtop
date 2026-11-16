fn main() {
    println!("cargo:rustc-link-lib=IOReport");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
    println!("cargo:rustc-link-lib=framework=IOKit");
}
