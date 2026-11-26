//! Windows 剪贴板：远程控制时和对方互通。
//!
//! 读：文件（CF_HDROP）、两边通用的文字、HTML、RTF、PNG（只有 DIB 时换成 PNG），再加上每种格式的原始数据（win:格式名），
//! 对方也是 Windows 时原样放回去，程序自己的格式（Excel、Office 里复制的）也不丢。
//! 写：用 OLE 剪贴板挂一个自己的 IDataObject，有程序粘贴时才来要，这时才从对方传过来。文件用“虚拟文件”
//! （FileGroupDescriptorW + FileContents）：资源管理器边传边写，自己显示进度，多大都行；
//! 不走 WM_RENDERFORMAT 那条路，那条路系统只等 30 秒。

use std::collections::VecDeque;
use std::io::Read;
use std::mem::ManuallyDrop;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, mpsc};

use anyhow::{Context, Result, anyhow, bail};
use p2pcli_core::screen_clip::{ClipContent, ClipEntry, ClipPlatform, ClipSource};
use tracing::{debug, warn};
use windows::Win32::Foundation::{
    DATA_S_SAMEFORMATETC, DV_E_FORMATETC, E_FAIL, E_NOTIMPL, HANDLE, HGLOBAL, HWND, LPARAM, OLE_E_ADVISENOTSUPPORTED, S_FALSE, S_OK,
    STG_E_ACCESSDENIED, STG_E_READFAULT, WPARAM,
};
use windows::Win32::System::Com::{
    DATADIR_GET, DVASPECT_CONTENT, FORMATETC, IAdviseSink, IDataObject, IDataObject_Impl, IEnumFORMATETC, IEnumSTATDATA, ISequentialStream_Impl,
    IStream, IStream_Impl, LOCKTYPE, STATFLAG, STATSTG, STGC, STGMEDIUM, STGMEDIUM_0, STGTY_STREAM, STREAM_SEEK, STREAM_SEEK_CUR, STREAM_SEEK_SET,
    TYMED_HGLOBAL, TYMED_ISTREAM,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EnumClipboardFormats, GetClipboardData, GetClipboardFormatNameW, GetClipboardOwner, GetClipboardSequenceNumber, OpenClipboard,
    RegisterClipboardFormatW,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT, OleInitialize, OleSetClipboard, ReleaseStgMedium};
use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
use windows::Win32::UI::Shell::{DragQueryFileW, FD_ATTRIBUTES, FD_FILESIZE, FD_PROGRESSUI, FILEDESCRIPTORW, HDROP, SHCreateStdEnumFmtEtc};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, GetWindowThreadProcessId, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, TranslateMessage, WM_APP,
};
use windows::core::{BOOL, HRESULT, HSTRING, Ref, implement};

pub struct WinClip;

/// 这些格式不是一块内存（位图、图元文件这些 GDI 句柄），或者是系统从别的格式转出来的，不搬
const SKIP_IDS: &[u32] = &[1, 2, 3, 7, 9, 14, 16, 17, 0x80, 0x82, 0x83, 0x8E];
/// 这些登记的格式只在本机、本进程有意义，或者是给剪贴板历史看的标记，不搬
const SKIP_NAMES: &[&str] = &[
    "DataObject",
    "Ole Private Data",
    "Object Descriptor",
    "Link Source Descriptor",
    "Embed Source",
    "Link Source",
    "FileContents",
    "ExcludeClipboardContentFromMonitorProcessing",
    "CanIncludeInClipboardHistory",
    "CanUploadToCloudClipboard",
    "Clipboard Viewer Ignore",
];
/// 文件夹、普通文件的属性
const ATTR_DIR: u32 = 0x10;
const ATTR_NORMAL: u32 = 0x80;
/// FILEDESCRIPTORW 里的文件名是 Unicode
const FD_UNICODE: u32 = 0x8000_0000;

fn register(name: &str) -> u16 {
    unsafe { RegisterClipboardFormatW(&HSTRING::from(name)) as u16 }
}

/// 打开剪贴板做事：别的程序正占着的话等一会儿再试
fn with_clipboard<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    let mut tries = 0;
    loop {
        if unsafe { OpenClipboard(None) }.is_ok() {
            break;
        }
        tries += 1;
        if tries > 20 {
            bail!("剪贴板被别的程序占着");
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let result = f();
    let _ = unsafe { CloseClipboard() };
    result
}

fn global_bytes(h: HANDLE) -> Option<Vec<u8>> {
    let g = HGLOBAL(h.0);
    unsafe {
        let size = GlobalSize(g);
        if size == 0 {
            return None;
        }
        let p = GlobalLock(g) as *const u8;
        if p.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(p, size).to_vec();
        let _ = GlobalUnlock(g);
        Some(v)
    }
}

fn data(id: u32) -> Option<Vec<u8>> {
    let h = unsafe { GetClipboardData(id) }.ok()?;
    global_bytes(h)
}

fn format_name(id: u32) -> Option<String> {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClipboardFormatNameW(id, &mut buf) };
    (n > 0).then(|| String::from_utf16_lossy(&buf[..n as usize]))
}

/// 格式编号 → 两边都认的名字：标准格式是 #编号，登记的格式用名字（编号每次开机都不一样）
fn format_key(id: u32) -> Option<String> {
    if id < 0xC000 {
        return Some(format!("win:#{id}"));
    }
    format_name(id).map(|n| format!("win:{n}"))
}

fn key_format(key: &str) -> Option<u16> {
    let name = key.strip_prefix("win:")?;
    if let Some(n) = name.strip_prefix('#') {
        return n.parse().ok();
    }
    if SKIP_NAMES.contains(&name) {
        return None;
    }
    Some(register(name))
}

fn utf16_text(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).take_while(|&u| u != 0).collect();
    String::from_utf16_lossy(&units).replace("\r\n", "\n")
}

fn text_utf16(text: &str) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\n").replace('\n', "\r\n");
    normalized.encode_utf16().chain(std::iter::once(0)).flat_map(|u| u.to_le_bytes()).collect()
}

fn files(id_present: bool) -> Vec<PathBuf> {
    if !id_present {
        return Vec::new();
    }
    let Ok(h) = (unsafe { GetClipboardData(CF_HDROP.0 as u32) }) else { return Vec::new() };
    let drop = HDROP(h.0);
    let count = unsafe { DragQueryFileW(drop, u32::MAX, None) };
    (0..count)
        .filter_map(|i| {
            let len = unsafe { DragQueryFileW(drop, i, None) } as usize;
            let mut buf = vec![0u16; len + 1];
            let n = unsafe { DragQueryFileW(drop, i, Some(&mut buf)) } as usize;
            (n > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..n])))
        })
        .collect()
}

/// 两边通用的格式
fn portable(key: &str, ids: &[u32]) -> Option<Vec<u8>> {
    match key {
        "text" => data(CF_UNICODETEXT.0 as u32).map(|b| utf16_text(&b).into_bytes()),
        "html" => data(register("HTML Format") as u32).map(|b| html_fragment(&b).into_bytes()),
        "rtf" => data(register("Rich Text Format") as u32).map(|mut b| {
            while b.last() == Some(&0) {
                b.pop();
            }
            b
        }),
        "png" => data(register("PNG") as u32).or_else(|| {
            [CF_DIBV5.0 as u32, CF_DIB.0 as u32].into_iter().filter(|id| ids.contains(id)).find_map(|id| data(id).and_then(|d| dib_to_png(&d)))
        }),
        _ => None,
    }
}

fn format_ids() -> Vec<u32> {
    let mut ids = Vec::new();
    let mut f = 0;
    loop {
        f = unsafe { EnumClipboardFormats(f) };
        if f == 0 {
            return ids;
        }
        ids.push(f);
    }
}

/// 读起来便宜的格式（文字类）：复制时就读，不用等粘贴
fn cheap(key: &str) -> bool {
    matches!(key, "win:#13" | "win:#16" | "win:HTML Format" | "win:Rich Text Format" | "win:UniformResourceLocatorW" | "win:UniformResourceLocator")
}

impl ClipPlatform for WinClip {
    fn change_count(&self) -> u64 {
        unsafe { GetClipboardSequenceNumber() as u64 }
    }

    fn is_ours(&self) -> bool {
        unsafe {
            let Ok(owner) = GetClipboardOwner() else { return false };
            if owner.is_invalid() {
                return false;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(owner, Some(&mut pid));
            pid == GetCurrentProcessId()
        }
    }

    fn read(&self) -> Result<ClipContent> {
        with_clipboard(|| {
            let ids = format_ids();
            let files = files(ids.contains(&(CF_HDROP.0 as u32)));
            let mut formats: Vec<(String, Option<Vec<u8>>)> = Vec::new();
            if files.is_empty() {
                // 两边通用的：文字类的现在就读，图片只记名字，粘贴时再读（还要转格式）
                for key in ["text", "html", "rtf"] {
                    if let Some(d) = portable(key, &ids) {
                        formats.push((key.to_string(), Some(d)));
                    }
                }
                let image = [register("PNG") as u32, CF_DIB.0 as u32, CF_DIBV5.0 as u32];
                if ids.iter().any(|id| image.contains(id)) {
                    formats.push(("png".into(), None));
                }
                // 原样的格式：对方也是 Windows 时原样放回去。程序自己的格式（Excel 那一大堆）只记名字，
                // 复制时不逼它们全生成一遍
                for &id in &ids {
                    if SKIP_IDS.contains(&id) || id == CF_HDROP.0 as u32 {
                        continue;
                    }
                    let Some(key) = format_key(id) else { continue };
                    if SKIP_NAMES.contains(&&key[4..]) {
                        continue;
                    }
                    let d = if cheap(&key) { data(id) } else { None };
                    formats.push((key, d));
                }
            } else if let Some(text) = portable("text", &ids) {
                formats.push(("text".into(), Some(text)));
            }
            Ok(ClipContent { formats, files })
        })
    }

    fn read_format(&self, key: &str) -> Result<Vec<u8>> {
        with_clipboard(|| {
            if key.starts_with("win:") {
                let id = key_format(key).ok_or_else(|| anyhow!("不认识的格式 {key}"))?;
                return data(id as u32).ok_or_else(|| anyhow!("剪贴板里没有 {key}"));
            }
            portable(key, &format_ids()).ok_or_else(|| anyhow!("剪贴板里没有 {key}"))
        })
    }

    fn write(&self, source: Arc<dyn ClipSource>) -> Result<()> {
        sta(move || {
            let obj: IDataObject = DataObject::new(source).into();
            unsafe { OleSetClipboard(&obj) }.context("放不进剪贴板")
        })?
    }
}

// ---------------------------------------------------------------- OLE 线程

type Job = Box<dyn FnOnce() + Send>;

struct Sta {
    thread: u32,
    jobs: Arc<Mutex<VecDeque<Job>>>,
}

/// 在专门的 OLE 线程上做事：OLE 剪贴板要在单线程套间里设，别的程序来要数据时也在这个线程上调
fn sta<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T> {
    static STA: OnceLock<Sta> = OnceLock::new();
    let sta = STA.get_or_init(|| {
        let jobs: Arc<Mutex<VecDeque<Job>>> = Arc::default();
        let (tx, rx) = mpsc::channel();
        let queue = jobs.clone();
        std::thread::Builder::new()
            .name("p2pcli-clipboard".into())
            .spawn(move || unsafe {
                if let Err(e) = OleInitialize(None) {
                    warn!("OLE 初始化失败：{e}");
                }
                // 先建好消息队列，别的线程才能往这儿发消息
                let mut msg = MSG::default();
                let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
                let _ = tx.send(GetCurrentThreadId());
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    if msg.hwnd == HWND::default() && msg.message == WM_APP {
                        while let Some(job) = queue.lock().unwrap().pop_front() {
                            job();
                        }
                        continue;
                    }
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            })
            .expect("剪贴板线程");
        Sta { thread: rx.recv().unwrap_or(0), jobs }
    });
    let (tx, rx) = mpsc::channel();
    sta.jobs.lock().unwrap().push_back(Box::new(move || {
        let _ = tx.send(f());
    }));
    unsafe { PostThreadMessageW(sta.thread, WM_APP, WPARAM(0), LPARAM(0)) }.context("剪贴板线程没有响应")?;
    rx.recv().map_err(|_| anyhow!("剪贴板线程没有响应"))
}

// ---------------------------------------------------------------- 放进剪贴板的东西

#[derive(Clone)]
enum Kind {
    /// 原样的一块内存
    Raw(String),
    Text,
    Html,
    Png,
    Dib,
    Descriptor,
    Contents,
    DropEffect,
    Exclude,
}

#[implement(IDataObject)]
struct DataObject {
    source: Arc<dyn ClipSource>,
    formats: Vec<(FORMATETC, Kind)>,
}

fn etc(cf: u16, tymed: i32) -> FORMATETC {
    FORMATETC { cfFormat: cf, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: tymed as u32 }
}

impl DataObject {
    fn new(source: Arc<dyn ClipSource>) -> Self {
        let offer = source.offer();
        let mut formats: Vec<(FORMATETC, Kind)> = Vec::new();
        let mut add = |cf: u16, tymed: i32, kind: Kind| {
            if cf != 0 && !formats.iter().any(|(f, _)| f.cfFormat == cf) {
                formats.push((etc(cf, tymed), kind));
            }
        };
        let hg = TYMED_HGLOBAL.0;
        if !offer.files.is_empty() {
            add(register("FileGroupDescriptorW"), hg, Kind::Descriptor);
            add(register("FileContents"), TYMED_ISTREAM.0, Kind::Contents);
            add(register("Preferred DropEffect"), hg, Kind::DropEffect);
            if offer.format("text").is_some() {
                add(CF_UNICODETEXT.0, hg, Kind::Text);
            }
        } else if offer.has_prefix("win:") {
            // 对方也是 Windows：原样放回去
            for f in &offer.formats {
                if let Some(cf) = key_format(&f.key) {
                    add(cf, hg, Kind::Raw(f.key.clone()));
                }
            }
        } else {
            for f in &offer.formats {
                match f.key.as_str() {
                    "text" => add(CF_UNICODETEXT.0, hg, Kind::Text),
                    "html" => add(register("HTML Format"), hg, Kind::Html),
                    "rtf" => add(register("Rich Text Format"), hg, Kind::Raw("rtf".into())),
                    "png" => {
                        add(register("PNG"), hg, Kind::Png);
                        add(CF_DIB.0, hg, Kind::Dib);
                    }
                    _ => {}
                }
            }
        }
        // 要现传的：别让剪贴板历史、监视剪贴板的软件一放进来就全要一遍
        if offer.has_lazy() {
            add(register("ExcludeClipboardContentFromMonitorProcessing"), hg, Kind::Exclude);
        }
        Self { source, formats }
    }

    fn find(&self, fe: &FORMATETC) -> Option<&Kind> {
        self.formats.iter().find(|(f, _)| f.cfFormat == fe.cfFormat && fe.tymed & f.tymed != 0).map(|(_, k)| k)
    }

    fn render(&self, kind: &Kind, lindex: i32) -> Result<STGMEDIUM> {
        let s = &self.source;
        let bytes = match kind {
            Kind::Raw(key) => s.format(key)?,
            Kind::Text => text_utf16(&String::from_utf8_lossy(&s.format("text")?)),
            Kind::Html => cf_html(&String::from_utf8_lossy(&s.format("html")?)),
            Kind::Png => s.format("png")?,
            Kind::Dib => png_to_dib(&s.format("png")?).ok_or_else(|| anyhow!("对方的图片读不了"))?,
            Kind::Descriptor => descriptor(&s.file_list()?),
            Kind::DropEffect => 1u32.to_le_bytes().to_vec(),
            Kind::Exclude => 0u32.to_le_bytes().to_vec(),
            Kind::Contents => {
                let list = s.file_list()?;
                let index = usize::try_from(lindex).map_err(|_| anyhow!("没说要哪个文件"))?;
                let entry = list.get(index).ok_or_else(|| anyhow!("没有这个文件"))?;
                debug!(path = %entry.path, size = entry.size, "粘贴对方剪贴板里的文件");
                let stream: IStream = FileStream { reader: Mutex::new(s.open_file(index)?), size: entry.size, pos: Mutex::new(0) }.into();
                return Ok(STGMEDIUM {
                    tymed: TYMED_ISTREAM.0 as u32,
                    u: STGMEDIUM_0 { pstm: ManuallyDrop::new(Some(stream)) },
                    pUnkForRelease: ManuallyDrop::new(None),
                });
            }
        };
        Ok(STGMEDIUM { tymed: TYMED_HGLOBAL.0 as u32, u: STGMEDIUM_0 { hGlobal: to_global(&bytes)? }, pUnkForRelease: ManuallyDrop::new(None) })
    }
}

fn to_global(bytes: &[u8]) -> Result<HGLOBAL> {
    unsafe {
        let g = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))?;
        let p = GlobalLock(g) as *mut u8;
        if p.is_null() {
            bail!("内存不够");
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        let _ = GlobalUnlock(g);
        Ok(g)
    }
}

impl IDataObject_Impl for DataObject_Impl {
    fn GetData(&self, pformatetcin: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
        let fe = unsafe { pformatetcin.as_ref() }.ok_or(windows::core::Error::from(DV_E_FORMATETC))?;
        let kind = self.find(fe).ok_or(windows::core::Error::from(DV_E_FORMATETC))?.clone();
        // 有程序在粘贴：现在从对方传过来（文件内容是边读边传的流）
        self.render(&kind, fe.lindex).map_err(|e| {
            warn!("粘贴对方剪贴板里的内容失败：{e:#}");
            windows::core::Error::from(E_FAIL)
        })
    }

    fn GetDataHere(&self, _: *const FORMATETC, _: *mut STGMEDIUM) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn QueryGetData(&self, pformatetc: *const FORMATETC) -> HRESULT {
        match unsafe { pformatetc.as_ref() }.and_then(|fe| self.find(fe)) {
            Some(_) => S_OK,
            None => DV_E_FORMATETC,
        }
    }

    fn GetCanonicalFormatEtc(&self, _: *const FORMATETC, pformatetcout: *mut FORMATETC) -> HRESULT {
        if let Some(out) = unsafe { pformatetcout.as_mut() } {
            out.ptd = std::ptr::null_mut();
        }
        DATA_S_SAMEFORMATETC
    }

    fn SetData(&self, _: *const FORMATETC, pmedium: *const STGMEDIUM, frelease: BOOL) -> windows::core::Result<()> {
        // 资源管理器粘贴完会回报“已粘贴”之类的，收下就好；要我们释放的得释放
        if frelease.as_bool() && !pmedium.is_null() {
            unsafe { ReleaseStgMedium(pmedium as *mut STGMEDIUM) };
        }
        Ok(())
    }

    fn EnumFormatEtc(&self, dwdirection: u32) -> windows::core::Result<IEnumFORMATETC> {
        if dwdirection != DATADIR_GET.0 as u32 {
            return Err(E_NOTIMPL.into());
        }
        let list: Vec<FORMATETC> = self.formats.iter().map(|(f, _)| *f).collect();
        unsafe { SHCreateStdEnumFmtEtc(&list) }
    }

    fn DAdvise(&self, _: *const FORMATETC, _: u32, _: Ref<IAdviseSink>) -> windows::core::Result<u32> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn DUnadvise(&self, _: u32) -> windows::core::Result<()> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn EnumDAdvise(&self) -> windows::core::Result<IEnumSTATDATA> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
}

/// 虚拟文件的清单：资源管理器按它建文件夹、要每个文件的内容
fn descriptor(list: &[ClipEntry]) -> Vec<u8> {
    let size = std::mem::size_of::<FILEDESCRIPTORW>();
    let mut out = vec![0u8; 4 + size * list.len()];
    out[..4].copy_from_slice(&(list.len() as u32).to_le_bytes());
    for (i, e) in list.iter().enumerate() {
        let mut fd = FILEDESCRIPTORW { dwFlags: (FD_ATTRIBUTES.0 | FD_FILESIZE.0 | FD_PROGRESSUI.0) as u32 | FD_UNICODE, ..Default::default() };
        fd.dwFileAttributes = if e.dir { ATTR_DIR } else { ATTR_NORMAL };
        fd.nFileSizeHigh = (e.size >> 32) as u32;
        fd.nFileSizeLow = e.size as u32;
        let name: String = e
            .path
            .chars()
            .map(|c| match c {
                '/' => '\\',
                '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
                c if (c as u32) < 32 => '_',
                c => c,
            })
            .collect();
        let mut file_name = [0u16; 260];
        let units: Vec<u16> = name.encode_utf16().take(file_name.len() - 1).collect();
        file_name[..units.len()].copy_from_slice(&units);
        fd.cFileName = file_name;
        let at = 4 + i * size;
        unsafe { std::ptr::write_unaligned(out[at..].as_mut_ptr() as *mut FILEDESCRIPTORW, fd) };
    }
    out
}

/// 对方的一个文件：资源管理器一块一块地读，读到哪儿传到哪儿
#[implement(IStream)]
struct FileStream {
    reader: Mutex<Box<dyn Read + Send>>,
    size: u64,
    pos: Mutex<u64>,
}

impl ISequentialStream_Impl for FileStream_Impl {
    fn Read(&self, pv: *mut core::ffi::c_void, cb: u32, pcbread: *mut u32) -> HRESULT {
        let buf = unsafe { std::slice::from_raw_parts_mut(pv as *mut u8, cb as usize) };
        let mut reader = self.reader.lock().unwrap();
        let mut filled = 0;
        while filled < buf.len() {
            match reader.read(&mut buf[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) => {
                    warn!("传对方剪贴板里的文件失败：{e}");
                    return STG_E_READFAULT;
                }
            }
        }
        *self.pos.lock().unwrap() += filled as u64;
        if let Some(out) = unsafe { pcbread.as_mut() } {
            *out = filled as u32;
        }
        if filled < buf.len() { S_FALSE } else { S_OK }
    }

    fn Write(&self, _: *const core::ffi::c_void, _: u32, _: *mut u32) -> HRESULT {
        STG_E_ACCESSDENIED
    }
}

impl IStream_Impl for FileStream_Impl {
    fn Seek(&self, dlibmove: i64, dworigin: STREAM_SEEK, plibnewposition: *mut u64) -> windows::core::Result<()> {
        // 只能往前读：只认“现在在哪”
        let pos = *self.pos.lock().unwrap();
        let ok = (dworigin == STREAM_SEEK_CUR && dlibmove == 0) || (dworigin == STREAM_SEEK_SET && dlibmove as u64 == pos);
        if !ok {
            return Err(E_NOTIMPL.into());
        }
        if let Some(out) = unsafe { plibnewposition.as_mut() } {
            *out = pos;
        }
        Ok(())
    }

    fn SetSize(&self, _: u64) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn CopyTo(&self, _: Ref<IStream>, _: u64, _: *mut u64, _: *mut u64) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Commit(&self, _: &STGC) -> windows::core::Result<()> {
        Ok(())
    }

    fn Revert(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn LockRegion(&self, _: u64, _: u64, _: &LOCKTYPE) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn UnlockRegion(&self, _: u64, _: u64, _: u32) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Stat(&self, pstatstg: *mut STATSTG, _: &STATFLAG) -> windows::core::Result<()> {
        let out = unsafe { pstatstg.as_mut() }.ok_or(windows::core::Error::from(E_FAIL))?;
        *out = STATSTG { r#type: STGTY_STREAM.0 as u32, cbSize: self.size, ..Default::default() };
        Ok(())
    }

    fn Clone(&self) -> windows::core::Result<IStream> {
        Err(E_NOTIMPL.into())
    }
}

// ---------------------------------------------------------------- 格式转换

/// CF_HTML（带偏移量的头）里取出 HTML 片段
fn html_fragment(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let num = |name: &str| -> Option<usize> {
        let at = text.find(name)? + name.len();
        text[at..].lines().next()?.trim().parse().ok()
    };
    let raw = match (num("StartFragment:"), num("EndFragment:")) {
        (Some(a), Some(b)) if a <= b && b <= bytes.len() => &bytes[a..b],
        _ => match (num("StartHTML:"), num("EndHTML:")) {
            (Some(a), Some(b)) if a <= b && b <= bytes.len() => &bytes[a..b],
            _ => bytes,
        },
    };
    String::from_utf8_lossy(raw).trim_end_matches('\0').to_string()
}

/// HTML 片段包成 CF_HTML
fn cf_html(html: &str) -> Vec<u8> {
    const PRE: &str = "<html><body>\r\n<!--StartFragment-->";
    const POST: &str = "<!--EndFragment-->\r\n</body></html>";
    let head = |a: usize, b: usize, c: usize, d: usize| {
        format!("Version:0.9\r\nStartHTML:{a:010}\r\nEndHTML:{b:010}\r\nStartFragment:{c:010}\r\nEndFragment:{d:010}\r\n")
    };
    let h = head(0, 0, 0, 0).len();
    let start_frag = h + PRE.len();
    let end_frag = start_frag + html.len();
    let end = end_frag + POST.len();
    let mut out = head(h, end, start_frag, end_frag).into_bytes();
    out.extend_from_slice(PRE.as_bytes());
    out.extend_from_slice(html.as_bytes());
    out.extend_from_slice(POST.as_bytes());
    out.push(0);
    out
}

/// DIB（24、32 位）换成 PNG
fn dib_to_png(dib: &[u8]) -> Option<Vec<u8>> {
    let u32_at = |i: usize| dib.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let header = u32_at(0)? as usize;
    let width = u32_at(4)? as i32;
    let height = u32_at(8)? as i32;
    let bits = u16::from_le_bytes([*dib.get(14)?, *dib.get(15)?]);
    let compression = u32_at(16)?;
    if width <= 0 || height == 0 || !(bits == 24 || bits == 32) || !(compression == 0 || compression == 3) {
        return None;
    }
    let (w, h) = (width as usize, height.unsigned_abs() as usize);
    let masks = if compression == 3 && header == 40 { 12 } else { 0 };
    let colors = u32_at(32)? as usize;
    let start = header + masks + colors * 4;
    let stride = (w * bits as usize).div_ceil(32) * 4;
    if dib.len() < start + stride * h {
        return None;
    }
    let bpp = bits as usize / 8;
    let mut rgba = vec![0u8; w * h * 4];
    let mut any_alpha = false;
    for y in 0..h {
        let src_row = if height < 0 { y } else { h - 1 - y };
        let row = &dib[start + src_row * stride..];
        for x in 0..w {
            let p = &row[x * bpp..];
            let o = (y * w + x) * 4;
            rgba[o] = p[2];
            rgba[o + 1] = p[1];
            rgba[o + 2] = p[0];
            rgba[o + 3] = if bpp == 4 { p[3] } else { 255 };
            any_alpha |= bpp == 4 && p[3] != 0;
        }
    }
    // 32 位但 alpha 全是 0：其实是不透明的
    if bpp == 4 && !any_alpha {
        rgba.iter_mut().skip(3).step_by(4).for_each(|a| *a = 255);
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().ok()?;
        writer.write_image_data(&rgba).ok()?;
    }
    Some(out)
}

/// PNG 换成 32 位 DIB（不支持 PNG 格式的老程序用）
fn png_to_dib(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let channels = match info.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return None,
    };
    let mut out = Vec::with_capacity(40 + w * h * 4);
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&((w * h * 4) as u32).to_le_bytes());
    out.extend_from_slice(&[0u8; 16]);
    for y in (0..h).rev() {
        let row = &buf[y * info.line_size..];
        for x in 0..w {
            let p = &row[x * channels..];
            let (r, g, b, a) = match channels {
                1 => (p[0], p[0], p[0], 255),
                2 => (p[0], p[0], p[0], p[1]),
                3 => (p[0], p[1], p[2], 255),
                _ => (p[0], p[1], p[2], p[3]),
            };
            out.extend_from_slice(&[b, g, r, a]);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_round_trip() {
        let html = "<b>你好</b> world";
        assert_eq!(html_fragment(&cf_html(html)), html);
    }

    #[test]
    fn image_round_trip() {
        // 2×2 的 24 位 DIB，自下而上
        let mut dib = vec![0u8; 40];
        dib[0] = 40;
        dib[4] = 2;
        dib[8] = 2;
        dib[12] = 1;
        dib[14] = 24;
        // 每行 6 字节补到 8
        dib.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]); // 下面一行：红、绿
        dib.extend_from_slice(&[255, 0, 0, 255, 255, 255, 0, 0]); // 上面一行：蓝、白
        let png = dib_to_png(&dib).unwrap();
        let back = png_to_dib(&png).unwrap();
        // 下面一行第一个像素：红（BGRA）
        assert_eq!(&back[40..44], &[0, 0, 255, 255]);
        // 上面一行第二个像素：白
        assert_eq!(&back[52..56], &[255, 255, 255, 255]);
    }

    #[test]
    fn text_line_endings() {
        assert_eq!(utf16_text(&text_utf16("a\nb")), "a\nb");
        assert_eq!(text_utf16("a\nb").len(), (4 + 1) * 2);
    }
}
