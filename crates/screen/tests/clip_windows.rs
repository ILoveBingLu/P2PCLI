//! 真的系统剪贴板：放进对方的内容，让别的程序（PowerShell、资源管理器）来粘贴；再反过来读别的程序复制的。
//! 会改掉这台电脑的剪贴板，平时不跑：cargo test -p p2pcli-screen --test clip_windows -- --ignored --test-threads=1
#![cfg(windows)]

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
    files: Vec<(ClipEntry, Vec<u8>)>,
    /// 来要了几次（粘贴时才传）
    asked: AtomicUsize,
    log: Mutex<Vec<String>>,
}

impl ClipSource for Fake {
    fn offer(&self) -> &ClipOffer {
        &self.offer
    }
    fn format(&self, key: &str) -> Result<Vec<u8>> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        self.log.lock().unwrap().push(format!("format:{key}"));
        if let Some(d) = self.offer.format(key).and_then(|f| f.data.clone()) {
            return Ok(d);
        }
        self.big.iter().find(|(k, _)| k == key).map(|(_, d)| d.clone()).ok_or_else(|| anyhow!("没有 {key}"))
    }
    fn file_list(&self) -> Result<Arc<Vec<ClipEntry>>> {
        self.log.lock().unwrap().push("file_list".into());
        Ok(Arc::new(self.files.iter().map(|(e, _)| e.clone()).collect()))
    }
    fn open_file(&self, index: usize) -> Result<Box<dyn Read + Send>> {
        self.log.lock().unwrap().push(format!("open:{index}"));
        Ok(Box::new(Cursor::new(self.files[index].1.clone())))
    }
    fn download(&self) -> Result<Vec<PathBuf>> {
        unreachable!("Windows 用虚拟文件，不先下载")
    }
}

fn fake(formats: Vec<ClipFormat>, big: Vec<(String, Vec<u8>)>, files: Vec<ClipFile>, entries: Vec<(ClipEntry, Vec<u8>)>) -> Arc<Fake> {
    Arc::new(Fake { offer: ClipOffer { id: 1, formats, files }, big, files: entries, asked: AtomicUsize::new(0), log: Mutex::default() })
}

fn ps(script: &str) -> String {
    let out = Command::new("powershell.exe").args(["-NoProfile", "-STA", "-Command", script]).output().expect("powershell");
    let text = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
    assert!(out.status.success(), "powershell 失败：{text}\n{}", String::from_utf8_lossy(&out.stderr));
    text
}

#[test]
#[ignore]
fn others_paste_what_we_put() {
    let clip = p2pcli_screen::clipboard().unwrap();

    // 文字、HTML：大的粘贴时才要
    let text = "你好 clipboard\nsecond line".to_string();
    let src = fake(
        vec![
            ClipFormat { key: "text".into(), size: text.len() as u64, data: None },
            ClipFormat { key: "html".into(), size: 20, data: Some(b"<b>bold</b> html".to_vec()) },
        ],
        vec![("text".into(), text.clone().into_bytes())],
        vec![],
        vec![],
    );
    clip.write(src.clone()).unwrap();
    assert!(clip.is_ours());
    assert_eq!(src.asked.load(Ordering::SeqCst), 0, "放进去时不该要内容");
    let got = ps("[Console]::OutputEncoding=[Text.Encoding]::UTF8; Get-Clipboard -Raw");
    assert_eq!(got, "你好 clipboard\r\nsecond line");
    let html = ps("[Console]::OutputEncoding=[Text.Encoding]::UTF8; Get-Clipboard -Format Text -TextFormatType Html");
    assert!(html.contains("<b>bold</b> html"), "{html}");

    // 图片：PNG 和 DIB 都能拿
    let mut png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png, 3, 2);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&[200u8; 3 * 2 * 4]).unwrap();
    }
    let src = fake(vec![ClipFormat { key: "png".into(), size: png.len() as u64, data: Some(png) }], vec![], vec![], vec![]);
    clip.write(src).unwrap();
    let size = ps("Add-Type -AssemblyName System.Windows.Forms; $i=[Windows.Forms.Clipboard]::GetImage(); \"$($i.Width)x$($i.Height)\"");
    assert_eq!(size, "3x2");

    // 文件夹：资源管理器粘贴，边读边写
    let big: Vec<u8> = (0..5_000_000u32).map(|i| (i % 253) as u8).collect();
    let entries = vec![
        (ClipEntry { path: "folder".into(), dir: true, size: 0 }, vec![]),
        (ClipEntry { path: "folder/a.txt".into(), dir: false, size: 5 }, b"hello".to_vec()),
        (ClipEntry { path: "folder/sub".into(), dir: true, size: 0 }, vec![]),
        (ClipEntry { path: "folder/sub/b.bin".into(), dir: false, size: big.len() as u64 }, big.clone()),
        (ClipEntry { path: "folder/empty".into(), dir: true, size: 0 }, vec![]),
    ];
    let src = fake(vec![], vec![], vec![ClipFile { name: "folder".into(), dir: true }], entries);
    clip.write(src.clone()).unwrap();
    let dest = std::env::temp_dir().join(format!("p2pcli-paste-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::create_dir_all(&dest).unwrap();
    ps(&format!(
        "$d='{}'; $f=\"$d\\folder\\sub\\b.bin\"; (New-Object -ComObject Shell.Application).Namespace($d).Self.InvokeVerb('Paste'); \
         for ($i=0; $i -lt 120; $i++) {{ if ((Test-Path $f) -and (Get-Item $f).Length -ge {}) {{ break }}; Start-Sleep -Milliseconds 250 }}; Start-Sleep -Seconds 1",
        dest.display(),
        big.len()
    ));
    println!("粘贴时要了：{:?}", src.log.lock().unwrap());
    assert_eq!(std::fs::read(dest.join("folder/a.txt")).unwrap(), b"hello");
    assert_eq!(std::fs::read(dest.join("folder/sub/b.bin")).unwrap(), big);
    assert!(dest.join("folder/empty").is_dir());
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
#[ignore]
fn read_what_others_copied() {
    let clip = p2pcli_screen::clipboard().unwrap();
    let before = clip.change_count();
    ps("Set-Clipboard -Value 'from powershell'");
    assert_ne!(clip.change_count(), before);
    assert!(!clip.is_ours());
    let c = clip.read().unwrap();
    let text = c.formats.iter().find(|(k, _)| k == "text").and_then(|(_, d)| d.as_ref()).map(|d| String::from_utf8_lossy(d).into_owned());
    assert_eq!(text.as_deref(), Some("from powershell"));
    assert!(c.formats.iter().any(|(k, _)| k == "win:#13"), "原样的 Unicode 文字也要带上：{:?}", c.formats.iter().map(|(k, _)| k).collect::<Vec<_>>());

    let win_ini = std::path::Path::new(&std::env::var("WINDIR").unwrap()).join("win.ini");
    ps(&format!("Set-Clipboard -Path '{}'", win_ini.display()));
    let c = clip.read().unwrap();
    assert_eq!(c.files, vec![win_ini]);

    ps("Add-Type -AssemblyName System.Windows.Forms; Add-Type -AssemblyName System.Drawing; $b=New-Object Drawing.Bitmap 4,3; [Windows.Forms.Clipboard]::SetImage($b)");
    let c = clip.read().unwrap();
    assert!(c.formats.iter().any(|(k, d)| k == "png" && d.is_none()), "图片只记名字");
    let png = clip.read_format("png").expect("要能读出图片");
    let dec = png::Decoder::new(Cursor::new(png)).read_info().unwrap();
    assert_eq!((dec.info().width, dec.info().height), (4, 3));
}
