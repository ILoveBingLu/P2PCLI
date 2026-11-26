//! macOS 剪贴板（NSPasteboard）：远程控制时和对方互通。
//!
//! 读：文件（public.file-url）、两边通用的文字、HTML、RTF、PNG（TIFF 换成 PNG），再加上每种类型的原始数据（mac:类型），
//! 对方也是 Mac 时原样放回去，程序自己的格式也不丢。
//! 写：小的直接放；大的和文件挂一个数据提供者，有程序粘贴时 AppKit 在主线程上来要，这时才从对方传过来。

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use anyhow::{Result, anyhow, bail};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2::{AllocAnyThread, DefinedClass, MainThreadMarker, define_class, msg_send};
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard, NSPasteboardItem, NSPasteboardItemDataProvider, NSPasteboardType, NSPasteboardWriting,
};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSObject, NSObjectProtocol, NSString, NSURL};
use p2pcli_core::screen_clip::{ClipContent, ClipPlatform, ClipSource};
use tracing::warn;

const FILE_URL: &str = "public.file-url";
const TEXT: &str = "public.utf8-plain-text";
const HTML: &str = "public.html";
const RTF: &str = "public.rtf";
const PNG: &str = "public.png";
const TIFF: &str = "public.tiff";
/// 告诉剪贴板管理软件别记这一份：记的话会马上把要现传的大内容、文件全要一遍
const TRANSIENT: &str = "org.nspasteboard.TransientType";

/// 自己放进剪贴板以后的 changeCount
static OURS: AtomicI64 = AtomicI64::new(-1);

pub struct MacClip;

fn general() -> Retained<NSPasteboard> {
    NSPasteboard::generalPasteboard()
}

fn bytes(data: &NSData) -> Vec<u8> {
    data.to_vec()
}

/// TIFF 换成 PNG（截图、预览里复制的图片常常只有 TIFF）
fn tiff_to_png(tiff: &NSData) -> Option<Vec<u8>> {
    let rep = NSBitmapImageRep::imageRepWithData(tiff)?;
    let png = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }?;
    Some(bytes(&png))
}

fn file_paths(items: &NSArray<NSPasteboardItem>) -> Vec<PathBuf> {
    let ty = NSString::from_str(FILE_URL);
    items
        .iter()
        .filter_map(|item| item.stringForType(&ty))
        .filter_map(|s| NSURL::URLWithString(&s))
        .filter_map(|u| u.path())
        .map(|p| PathBuf::from(p.to_string()))
        .collect()
}

/// 两边通用的格式：从第一项里按 key 取
fn portable(item: &NSPasteboardItem, key: &str) -> Option<Vec<u8>> {
    let get = |ty: &str| item.dataForType(&NSString::from_str(ty)).map(|d| bytes(&d));
    match key {
        "text" => item.stringForType(&NSString::from_str(TEXT)).map(|s| s.to_string().into_bytes()),
        "html" => get(HTML),
        "rtf" => get(RTF),
        "png" => get(PNG).or_else(|| item.dataForType(&NSString::from_str(TIFF)).and_then(|d| tiff_to_png(&d))),
        _ => None,
    }
}

/// 读起来便宜的类型（文字类）：复制时就读，不用等粘贴
fn cheap(ty: &str) -> bool {
    ty.starts_with("public.") && (ty.contains("text") || ty == HTML || ty == RTF || ty == "public.url" || ty == "public.url-name")
}

impl ClipPlatform for MacClip {
    fn change_count(&self) -> u64 {
        general().changeCount() as u64
    }

    fn is_ours(&self) -> bool {
        general().changeCount() as i64 == OURS.load(Ordering::SeqCst)
    }

    fn read(&self) -> Result<ClipContent> {
        autoreleasepool(|_| {
            let pb = general();
            let Some(items) = pb.pasteboardItems() else { return Ok(ClipContent::default()) };
            let files = file_paths(&items);
            let Some(first) = items.firstObject() else { return Ok(ClipContent::default()) };
            let mut formats: Vec<(String, Option<Vec<u8>>)> = Vec::new();
            if files.is_empty() {
                let types: Vec<String> = first.types().iter().map(|t| t.to_string()).collect();
                let has = |t: &str| types.iter().any(|x| x == t);
                // 两边通用的：文字类的现在就读，图片只记名字，粘贴时再读（还要转格式）
                for key in ["text", "html", "rtf"] {
                    if let Some(data) = portable(&first, key) {
                        formats.push((key.to_string(), Some(data)));
                    }
                }
                if has(PNG) || has(TIFF) {
                    formats.push(("png".into(), None));
                }
                // 原样的类型：对方也是 Mac 时原样放回去。程序自己的格式只记名字
                for name in types.iter().filter(|n| *n != TRANSIENT) {
                    let data = cheap(name).then(|| first.dataForType(&NSString::from_str(name)).map(|d| bytes(&d))).flatten();
                    formats.push((format!("mac:{name}"), data));
                }
            } else if let Some(text) = portable(&first, "text") {
                // 复制文件时文字是文件名，粘贴到文本框里用
                formats.push(("text".into(), Some(text)));
            }
            Ok(ClipContent { formats, files })
        })
    }

    fn read_format(&self, key: &str) -> Result<Vec<u8>> {
        autoreleasepool(|_| {
            let items = general().pasteboardItems().ok_or_else(|| anyhow!("剪贴板是空的"))?;
            let first = items.firstObject().ok_or_else(|| anyhow!("剪贴板是空的"))?;
            if let Some(ty) = key.strip_prefix("mac:") {
                return first.dataForType(&NSString::from_str(ty)).map(|d| bytes(&d)).ok_or_else(|| anyhow!("剪贴板里没有 {ty}"));
            }
            portable(&first, key).ok_or_else(|| anyhow!("剪贴板里没有 {key}"))
        })
    }

    fn write(&self, source: Arc<dyn ClipSource>) -> Result<()> {
        // NSPasteboardItem 的数据提供者要在主线程上挂，AppKit 也在主线程上来要
        if let Some(mtm) = MainThreadMarker::new() {
            return write_main(mtm, source);
        }
        let mut result = Err(anyhow!("主线程没有响应"));
        dispatch2::DispatchQueue::main().exec_sync(|| {
            result = write_main(MainThreadMarker::new().expect("主线程"), source);
        });
        result
    }
}

thread_local! {
    /// 挂在剪贴板上的数据提供者：留着，直到下一次放新的
    static PROVIDERS: RefCell<Vec<Retained<Provider>>> = const { RefCell::new(Vec::new()) };
}

fn write_main(_mtm: MainThreadMarker, source: Arc<dyn ClipSource>) -> Result<()> {
    autoreleasepool(|_| {
        let offer = source.offer();
        // 对方也是 Mac：原样放回去；不然放两边通用的
        let native = offer.has_prefix("mac:");
        let mut types: Vec<(String, String)> = Vec::new();
        for f in &offer.formats {
            let ty = if native {
                match f.key.strip_prefix("mac:") {
                    Some(t) => t.to_string(),
                    None => continue,
                }
            } else {
                match f.key.as_str() {
                    "text" => TEXT.into(),
                    "html" => HTML.into(),
                    "rtf" => RTF.into(),
                    "png" => PNG.into(),
                    _ => continue,
                }
            };
            if ty == TRANSIENT || types.iter().any(|(t, _)| *t == ty) {
                continue;
            }
            types.push((ty, f.key.clone()));
        }

        let mut items: Vec<Retained<NSPasteboardItem>> = Vec::new();
        let mut providers: Vec<Retained<Provider>> = Vec::new();
        if offer.files.is_empty() {
            let item = NSPasteboardItem::new();
            let mut lazy: HashMap<String, String> = HashMap::new();
            for (ty, key) in &types {
                match offer.format(key).and_then(|f| f.data.as_ref()) {
                    Some(data) => {
                        item.setData_forType(&NSData::with_bytes(data), &NSString::from_str(ty));
                    }
                    None => {
                        lazy.insert(ty.clone(), key.clone());
                    }
                }
            }
            if !lazy.is_empty() {
                let names: Vec<Retained<NSString>> = lazy.keys().map(|t| NSString::from_str(t)).collect();
                let provider = Provider::new(Ivars { source: source.clone(), file: None, keys: lazy });
                item.setDataProvider_forTypes(ProtocolObject::from_ref(&*provider), &NSArray::from_retained_slice(&names));
                providers.push(provider);
            }
            items.push(item);
        } else {
            // 文件：每项一个 file-url，粘贴时才传
            let text = NSString::from_str(TEXT);
            let url = NSArray::from_retained_slice(&[NSString::from_str(FILE_URL)]);
            for (i, f) in offer.files.iter().enumerate() {
                let item = NSPasteboardItem::new();
                let provider = Provider::new(Ivars { source: source.clone(), file: Some(i), keys: HashMap::new() });
                item.setDataProvider_forTypes(ProtocolObject::from_ref(&*provider), &url);
                item.setString_forType(&NSString::from_str(&f.name), &text);
                providers.push(provider);
                items.push(item);
            }
        }
        if offer.has_lazy()
            && let Some(first) = items.first()
        {
            first.setData_forType(&NSData::new(), &NSString::from_str(TRANSIENT));
        }

        let pb = general();
        pb.clearContents();
        let objects: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> =
            items.into_iter().map(ProtocolObject::from_retained).collect();
        if !pb.writeObjects(&NSArray::from_retained_slice(&objects)) {
            bail!("放不进剪贴板");
        }
        OURS.store(pb.changeCount() as i64, Ordering::SeqCst);
        PROVIDERS.with(|p| *p.borrow_mut() = providers);
        Ok(())
    })
}

struct Ivars {
    source: Arc<dyn ClipSource>,
    /// 文件项：第几个
    file: Option<usize>,
    /// 类型 → 对方的格式
    keys: HashMap<String, String>,
}

define_class!(
    // SAFETY: 只继承 NSObject，没有重写 dealloc
    #[unsafe(super(NSObject))]
    #[name = "P2PCLIClipProvider"]
    #[ivars = Ivars]
    struct Provider;

    unsafe impl NSObjectProtocol for Provider {}

    unsafe impl NSPasteboardItemDataProvider for Provider {
        #[unsafe(method(pasteboard:item:provideDataForType:))]
        fn provide(&self, _pasteboard: Option<&NSPasteboard>, item: &NSPasteboardItem, ty: &NSPasteboardType) {
            let iv = self.ivars();
            let name = ty.to_string();
            // 有程序在粘贴：现在从对方传过来，传完才返回（粘贴的程序等着）
            if let Some(i) = iv.file {
                match iv.source.download() {
                    Ok(paths) => {
                        if let Some(p) = paths.get(i)
                            && let Some(s) = NSURL::fileURLWithPath(&NSString::from_str(&p.to_string_lossy())).absoluteString()
                        {
                            item.setString_forType(&s, ty);
                        }
                    }
                    Err(e) => warn!("粘贴对方剪贴板里的文件失败：{e:#}"),
                }
                return;
            }
            if let Some(key) = iv.keys.get(&name) {
                match iv.source.format(key) {
                    Ok(data) => {
                        item.setData_forType(&NSData::with_bytes(&data), ty);
                    }
                    Err(e) => warn!("粘贴对方剪贴板里的内容失败：{e:#}"),
                }
            }
        }
    }
);

impl Provider {
    fn new(ivars: Ivars) -> Retained<Self> {
        let this = Self::alloc().set_ivars(ivars);
        unsafe { msg_send![super(this), init] }
    }
}
