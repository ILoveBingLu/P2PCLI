//! 生成 Kotlin、Swift 绑定：
//! cargo run -p p2pcli-mobile-ffi --features bindgen --bin uniffi-bindgen -- generate --library <编好的库> --language kotlin --out-dir <目录>
fn main() {
    uniffi::uniffi_bindgen_main()
}
