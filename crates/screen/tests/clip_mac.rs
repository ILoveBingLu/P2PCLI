//! 真的系统剪贴板：放进对方的内容，让别的程序（pbpaste、osascript）来粘贴；再反过来读别的程序复制的。
//! 会改掉这台电脑的剪贴板，平时不跑：cargo test -p p2pcli-screen --test clip_mac -- --ignored
//! 剪贴板的数据提供者在主线程上被调，所以自己写 main：主线程跑 run loop，测试在别的线程上
#[cfg(target_os = "macos")]
mod mac {
    use std::io::{Cursor, Read};
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use anyhow::{Result, anyhow};
    use p2pcli_core::screen_clip::{ClipEntry, ClipFile, ClipFormat, ClipOffer, ClipSource};

    struct Fake {
        offer: ClipOffer,
        big: Vec<(String, Vec<u8>)>,
        asked: AtomicUsize,
        staged: Mutex<Option<Vec<PathBuf>>>,
    }

    impl ClipSource for Fake {
        fn offer(&self) -> &ClipOffer {
            &self.offer
        }
        fn format(&self, key: &str) -> Result<Vec<u8>> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            if let Some(d) = self.offer.format(key).and_then(|f| f.data.clone()) {
                return Ok(d);
            }
            self.big.iter().find(|(k, _)| k == key).map(|(_, d)| d.clone()).ok_or_else(|| anyhow!("没有 {key}"))
        }
        fn file_list(&self) -> Result<Arc<Vec<ClipEntry>>> {
            unreachable!()
        }
        fn open_file(&self, _: usize) -> Result<Box<dyn Read + Send>> {
            Ok(Box::new(Cursor::new(vec![])))
        }
        fn download(&self) -> Result<Vec<PathBuf>> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            let mut staged = self.staged.lock().unwrap();
            if let Some(p) = staged.as_ref() {
                return Ok(p.clone());
            }
            let dir = std::env::temp_dir().join(format!("p2pcli-clip-mac-{}", std::process::id()));
            std::fs::create_dir_all(dir.join("folder")).unwrap();
            std::fs::write(dir.join("folder/a.txt"), b"hello").unwrap();
            std::fs::write(dir.join("b.txt"), b"bee").unwrap();
            let paths = vec![dir.join("folder"), dir.join("b.txt")];
            *staged = Some(paths.clone());
            Ok(paths)
        }
    }

    fn fake(formats: Vec<ClipFormat>, big: Vec<(String, Vec<u8>)>, files: Vec<ClipFile>) -> Arc<Fake> {
        Arc::new(Fake { offer: ClipOffer { id: 1, formats, files }, big, asked: AtomicUsize::new(0), staged: Mutex::default() })
    }

    fn sh(cmd: &str, args: &[&str]) -> String {
        let out = Command::new(cmd).args(args).output().expect(cmd);
        assert!(out.status.success(), "{cmd} 失败：{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim_end().to_string()
    }

    fn jxa(script: &str) -> String {
        sh("osascript", &["-l", "JavaScript", "-e", &format!("ObjC.import('AppKit'); var pb=$.NSPasteboard.generalPasteboard; {script}")])
    }

    pub fn run() {
        let clip = p2pcli_screen::clipboard().unwrap();

        // 文字现要，HTML 直接带着
        let text = "你好 clipboard\nsecond line".to_string();
        let src = fake(
            vec![
                ClipFormat { key: "text".into(), size: text.len() as u64, data: None },
                ClipFormat { key: "html".into(), size: 16, data: Some(b"<b>bold</b> html".to_vec()) },
            ],
            vec![("text".into(), text.clone().into_bytes())],
            vec![],
        );
        clip.write(src.clone()).unwrap();
        assert!(clip.is_ours());
        assert_eq!(src.asked.load(Ordering::SeqCst), 0, "放进去时不该要内容");
        assert_eq!(sh("pbpaste", &[]), text);
        assert_eq!(src.asked.load(Ordering::SeqCst), 1);
        let html = jxa("ObjC.unwrap(pb.pasteboardItems.objectAtIndex(0).stringForType('public.html'))");
        assert_eq!(html, "<b>bold</b> html");
        println!("文字、HTML：ok");

        // 对方也是 Mac：原样的类型
        let src = fake(vec![ClipFormat { key: "mac:com.example.custom".into(), size: 3, data: None }], vec![("mac:com.example.custom".into(), b"xyz".to_vec())], vec![]);
        clip.write(src).unwrap();
        let custom = jxa("ObjC.unwrap($.NSString.alloc.initWithDataEncoding(pb.pasteboardItems.objectAtIndex(0).dataForType('com.example.custom'), 4))");
        assert_eq!(custom, "xyz");
        println!("原样的类型：ok");

        // 文件：粘贴时才传，传完给出路径
        let src = fake(vec![], vec![], vec![ClipFile { name: "folder".into(), dir: true }, ClipFile { name: "b.txt".into(), dir: false }]);
        clip.write(src.clone()).unwrap();
        assert_eq!(src.asked.load(Ordering::SeqCst), 0);
        let urls = jxa("var items=pb.pasteboardItems; var out=[]; for (var i=0;i<items.count;i++) out.push(ObjC.unwrap(items.objectAtIndex(i).stringForType('public.file-url'))); out.join('\\n')");
        let paths: Vec<PathBuf> = urls.lines().map(|u| PathBuf::from(u.trim_start_matches("file://").trim_end_matches('/'))).collect();
        assert_eq!(paths.len(), 2, "{urls}");
        assert_eq!(std::fs::read(paths[0].join("a.txt")).unwrap(), b"hello");
        assert_eq!(std::fs::read(&paths[1]).unwrap(), b"bee");
        assert_eq!(src.asked.load(Ordering::SeqCst), 2, "两项各要一次，只传一份");
        println!("文件：ok");

        // 读别的程序复制的
        let before = clip.change_count();
        let mut child = Command::new("pbcopy").stdin(std::process::Stdio::piped()).spawn().unwrap();
        std::io::Write::write_all(child.stdin.as_mut().unwrap(), "from pbcopy".as_bytes()).unwrap();
        drop(child.stdin.take());
        child.wait().unwrap();
        assert_ne!(clip.change_count(), before);
        assert!(!clip.is_ours());
        let c = clip.read().unwrap();
        let text = c.formats.iter().find(|(k, _)| k == "text").and_then(|(_, d)| d.as_ref()).map(|d| String::from_utf8_lossy(d).into_owned());
        assert_eq!(text.as_deref(), Some("from pbcopy"));
        assert!(c.formats.iter().any(|(k, _)| k == "mac:public.utf8-plain-text"));
        println!("读文字：ok");

        sh("osascript", &["-e", "set the clipboard to POSIX file \"/etc/hosts\""]);
        let c = clip.read().unwrap();
        assert_eq!(c.files.len(), 1, "{:?}", c.files);
        assert!(c.files[0].ends_with("hosts"), "{:?}", c.files);
        println!("读文件：ok");

        // TIFF 换成 PNG
        jxa("var img=$.NSImage.alloc.initWithSize($.NSMakeSize(4,3)); img.lockFocus; $.NSColor.redColor.set; $.NSRectFill($.NSMakeRect(0,0,4,3)); img.unlockFocus; pb.clearContents; pb.setDataForType(img.TIFFRepresentation, 'public.tiff'); 'ok'");
        let c = clip.read().unwrap();
        assert!(c.formats.iter().any(|(k, d)| k == "png" && d.is_none()), "图片只记名字");
        let png = clip.read_format("png").expect("要能读出图片");
        assert_eq!(&png[1..4], b"PNG");
        println!("读图片：ok");
    }
}

#[cfg(target_os = "macos")]
fn main() {
    use objc2_foundation::{NSDate, NSRunLoop};
    if !std::env::args().any(|a| a == "--ignored") {
        println!("会改掉这台电脑的剪贴板，要加 --ignored 才跑");
        return;
    }
    let t = std::thread::spawn(mac::run);
    while !t.is_finished() {
        NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.05));
    }
    if t.join().is_err() {
        std::process::exit(1);
    }
    println!("全部通过");
}

#[cfg(not(target_os = "macos"))]
fn main() {}
