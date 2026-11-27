//! 真的截一张 Windows 屏幕：只看 PNG 能不能解开、大小对不对，不看内容。
//! cargo test -p p2pcli-screen --test shot_windows -- --ignored
#![cfg(windows)]

#[test]
#[ignore]
fn captures_primary_display() {
    let backend = p2pcli_screen::backend().expect("这台 Windows 支持远程画面");
    let displays = backend.displays().unwrap();
    let primary = displays.iter().find(|d| d.primary).unwrap();
    let shot = backend.screenshot(&primary.id, 640).unwrap();
    assert!(shot.width <= 640 && shot.width > 0 && shot.height > 0);
    let dec = png::Decoder::new(std::io::Cursor::new(&shot.png)).read_info().unwrap();
    assert_eq!((dec.info().width, dec.info().height), (shot.width, shot.height));
    println!("主屏 {}×{} 截成 {}×{}，PNG {} 字节", primary.width, primary.height, shot.width, shot.height, shot.png.len());
}
