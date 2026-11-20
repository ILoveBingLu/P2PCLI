//! 读几次鼠标指针（位置、样子），验证能不能单独读指针：cargo run -p p2pcli-screen --example cursor_probe
fn main() {
    let backend = p2pcli_screen::backend().expect("这个系统不支持");
    let display = backend.displays().unwrap().into_iter().find(|d| d.primary).unwrap();
    for _ in 0..5 {
        match backend.cursor(&display.id) {
            Some(c) => match &c.shape {
                Some(s) => println!("位置 ({:.3}, {:.3}) 可见={} 样子 {}x{} 热点 ({:.1}, {:.1}) 缩放 {} 编号 {:x}", c.x, c.y, c.visible, s.width, s.height, s.hot_x, s.hot_y, s.scale, s.id),
                None => println!("位置 ({:.3}, {:.3}) 可见={} 样子读不到", c.x, c.y, c.visible),
            },
            None => println!("这个系统不能单独读指针"),
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}
