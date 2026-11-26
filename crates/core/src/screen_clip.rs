//! 远程控制时两边的剪贴板互通：文字、带格式的文字、图片、文件和文件夹；两边是同一种系统时，
//! 连各个程序自己的格式（表格、设计软件里复制的东西）也原样带上。大小不限。
//!
//! 一边的剪贴板变了，只把清单发过去：有哪些格式、各多大，小的顺便带上内容。另一边把清单放进自己的剪贴板，
//! 真有程序来粘贴时才去要内容（Windows 是 OLE 剪贴板和虚拟文件，macOS 是 NSPasteboardItem 的数据提供者），
//! 不会白传。内容走单独的 QUIC 流，不挤画面和键鼠。
//!
//! 流都由观看方开（被连方不往观看方开流）：观看方要被连方的内容开 [`ClipStreamReq::Get`]；
//! 被连方要观看方的内容时，在画面流里发 [`crate::screen::HostMsg::ClipWant`]，观看方开一条 [`ClipStreamReq::Put`] 送过去。

use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info};

use crate::host::BoxFuture;

/// 一个格式不超过这么大就直接带在清单里，粘贴时不用再要
pub const INLINE_EACH: usize = 256 << 10;
/// 清单里带的内容加起来不超过这么多：清单和画面走同一条流
pub const INLINE_TOTAL: usize = 1 << 20;
/// 内容流里每块的大小
const CHUNK: usize = 256 << 10;
/// 剪贴板内容流的优先级：比画面低，大文件不挤画面
pub const CLIP_PRIORITY: i32 = -20;
/// 多久看一眼本机剪贴板变没变
pub const POLL: Duration = Duration::from_millis(300);
/// 记住自己最近几份清单：对方粘贴到一半时本机又复制了别的，前一份的文件照样能传完
const KEEP_MINE: usize = 4;

/// 一份剪贴板清单
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipOffer {
    /// 这一边的第几份，向对方要内容时带上
    pub id: u64,
    pub formats: Vec<ClipFormat>,
    /// 复制的文件和文件夹（最上层的那几项）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<ClipFile>,
}

impl ClipOffer {
    pub fn format(&self, key: &str) -> Option<&ClipFormat> {
        self.formats.iter().find(|f| f.key == key)
    }

    /// 有没有带某个前缀的格式（mac:、win: 是两边系统自己的格式）
    pub fn has_prefix(&self, prefix: &str) -> bool {
        self.formats.iter().any(|f| f.key.starts_with(prefix))
    }

    /// 有要粘贴时才传的东西：文件，或者没带在清单里的大内容
    pub fn has_lazy(&self) -> bool {
        !self.files.is_empty() || self.formats.iter().any(|f| f.data.is_none())
    }
}

/// 一种格式。key 是两边通用的 text、html、rtf、png，或者系统自己的 mac:类型、win:格式名（win:#数字 是系统标准格式）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipFormat {
    pub key: String,
    /// 字节数；没读过的是 0
    pub size: u64,
    /// 小的直接带上
    #[serde(default, skip_serializing_if = "Option::is_none", with = "b64")]
    pub data: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipFile {
    pub name: String,
    pub dir: bool,
}

/// 文件清单里的一项：路径用 / 分隔，第一段是复制的那一项的名字；目录排在它里面的东西前面
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipEntry {
    pub path: String,
    pub dir: bool,
    pub size: u64,
}

/// 要哪一样
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum ClipWhat {
    Format { key: String },
    /// 所有文件和目录（展开文件夹）
    FileList,
    /// 文件清单里第几个文件的内容
    File { index: u32 },
}

/// 剪贴板内容流的第一条消息
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClipStreamReq {
    /// 观看方要被连方剪贴板里的内容，被连方在这条流上回
    Get { token: u64, offer: u64, what: ClipWhat },
    /// 回被连方的 ClipWant：观看方在这条流上把内容送过去
    Put { token: u64, req: u64 },
}

// ---------------------------------------------------------------- 平台接口

/// 本机剪贴板里读出来的东西
#[derive(Debug, Default, Clone)]
pub struct ClipContent {
    /// 各格式；读起来便宜的（文字这些）带上内容，别的（图片、程序自己的格式）只有名字，对方粘贴时再读：
    /// 有些程序（Excel、Office）复制时只是登记一大堆格式，真读才去生成，每次复制都全读一遍又慢又占内存
    pub formats: Vec<(String, Option<Vec<u8>>)>,
    /// 复制的文件和文件夹
    pub files: Vec<PathBuf>,
}

/// 本机剪贴板，各平台实现
pub trait ClipPlatform: Send + Sync + 'static {
    /// 剪贴板每变一次就变一下
    fn change_count(&self) -> u64;
    /// 现在剪贴板里是不是自己放进去的（对方的内容），是的话不发回去
    fn is_ours(&self) -> bool;
    /// 读出现在的内容
    fn read(&self) -> Result<ClipContent>;
    /// 读一种格式（对方粘贴时要的大内容）
    fn read_format(&self, key: &str) -> Result<Vec<u8>>;
    /// 把对方的清单放进本机剪贴板；有程序要内容时从 source 拿
    fn write(&self, source: Arc<dyn ClipSource>) -> Result<()>;
}

/// 对方剪贴板的内容。平台实现在本机有程序粘贴时调，会一直阻塞到传完，别在异步任务里调
pub trait ClipSource: Send + Sync {
    fn offer(&self) -> &ClipOffer;
    fn format(&self, key: &str) -> Result<Vec<u8>>;
    fn file_list(&self) -> Result<Arc<Vec<ClipEntry>>>;
    fn open_file(&self, index: usize) -> Result<Box<dyn Read + Send>>;
    /// 把文件全部传到 dir 下（已经传过就直接用），返回最上层各项的路径。macOS 粘贴文件时用
    fn download(&self) -> Result<Vec<PathBuf>>;
}

/// 向对方要内容：观看方开 Get 流；被连方发 ClipWant，等观看方开 Put 流送来
pub trait ClipFetch: Send + Sync + 'static {
    fn fetch(&self, offer: u64, what: ClipWhat) -> BoxFuture<Result<Box<dyn AsyncRead + Send + Unpin>>>;
}

// ---------------------------------------------------------------- 一边的同步

struct Mine {
    id: u64,
    change: u64,
    paths: Vec<PathBuf>,
    list: Option<Arc<Vec<(ClipEntry, PathBuf)>>>,
}

/// 一边的剪贴板同步：看本机剪贴板变没变、把对方的放进来、给对方送内容
pub struct ClipSync {
    platform: Arc<dyn ClipPlatform>,
    rt: Handle,
    seen: Mutex<Option<u64>>,
    next_id: AtomicU64,
    mine: Mutex<VecDeque<Mine>>,
}

impl ClipSync {
    /// push_existing：一开始就把本机剪贴板现有的内容发过去（观看方：打开画面前复制的，进去就能粘贴）；
    /// 否则只发之后新复制的（被连方：不把原有的塞给观看方）
    pub fn new(platform: Arc<dyn ClipPlatform>, rt: Handle, push_existing: bool) -> Arc<Self> {
        clean_staging();
        let seen = if push_existing { None } else { Some(platform.change_count()) };
        Arc::new(Self { platform, rt, seen: Mutex::new(seen), next_id: AtomicU64::new(1), mine: Mutex::new(VecDeque::new()) })
    }

    /// 本机剪贴板变了就读出清单（会读剪贴板，别在异步任务里调）。没变、是对方的、空的返回空
    pub fn poll(&self) -> Result<Option<ClipOffer>> {
        let change = self.platform.change_count();
        {
            let mut seen = self.seen.lock().unwrap();
            if *seen == Some(change) {
                return Ok(None);
            }
            *seen = Some(change);
        }
        if self.platform.is_ours() {
            return Ok(None);
        }
        let content = self.platform.read()?;
        if content.formats.is_empty() && content.files.is_empty() {
            return Ok(None);
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut budget = INLINE_TOTAL;
        let formats = content
            .formats
            .into_iter()
            .map(|(key, data)| {
                let size = data.as_ref().map_or(0, |d| d.len() as u64);
                let data = data.filter(|d| d.len() <= INLINE_EACH && d.len() <= budget);
                if let Some(d) = &data {
                    budget -= d.len();
                }
                ClipFormat { key, size, data }
            })
            .collect();
        let files = top_names(&content.files)
            .into_iter()
            .zip(&content.files)
            .map(|(name, p)| ClipFile { name, dir: p.is_dir() })
            .collect();
        let mut mine = self.mine.lock().unwrap();
        mine.push_back(Mine { id, change, paths: content.files, list: None });
        while mine.len() > KEEP_MINE {
            mine.pop_front();
        }
        Ok(Some(ClipOffer { id, formats, files }))
    }

    /// 把对方的清单放进本机剪贴板（会碰剪贴板，别在异步任务里调）
    pub fn apply(&self, offer: ClipOffer, fetch: Arc<dyn ClipFetch>) -> Result<()> {
        debug!(id = offer.id, formats = offer.formats.len(), files = offer.files.len(), "放进对方的剪贴板内容");
        let source = Arc::new(RemoteSource {
            offer,
            fetch,
            rt: self.rt.clone(),
            list: Mutex::new(None),
            staged: Mutex::new(None),
        });
        self.platform.write(source)?;
        // 自己放进去的这一下不算本机复制
        *self.seen.lock().unwrap() = Some(self.platform.change_count());
        Ok(())
    }

    /// 把自己某份清单里的一样东西写给对方：先一个字节的结果，成功的话后面是一块一块的内容
    pub async fn serve<W: AsyncWrite + Unpin + Send>(&self, offer: u64, what: ClipWhat, w: &mut W) -> Result<()> {
        match self.open(offer, what).await {
            Ok(mut r) => {
                w.write_u8(0).await?;
                pipe(&mut r, w).await
            }
            Err(e) => {
                debug!("给对方剪贴板内容失败：{e:#}");
                let msg = format!("{e:#}");
                w.write_u8(1).await?;
                w.write_u32(msg.len() as u32).await?;
                w.write_all(msg.as_bytes()).await?;
                Ok(())
            }
        }
    }

    async fn open(&self, offer: u64, what: ClipWhat) -> Result<Box<dyn AsyncRead + Send + Unpin>> {
        let (change, paths, list) = {
            let mine = self.mine.lock().unwrap();
            let m = mine.iter().find(|m| m.id == offer).ok_or_else(|| anyhow!("剪贴板已经换了内容"))?;
            (m.change, m.paths.clone(), m.list.clone())
        };
        match what {
            ClipWhat::Format { key } => {
                let platform = self.platform.clone();
                let data = tokio::task::spawn_blocking(move || {
                    if platform.change_count() != change {
                        bail!("剪贴板已经换了内容");
                    }
                    platform.read_format(&key)
                })
                .await??;
                Ok(Box::new(std::io::Cursor::new(data)))
            }
            ClipWhat::FileList => {
                let list = self.list(offer, list, paths).await?;
                let entries: Vec<&ClipEntry> = list.iter().map(|(e, _)| e).collect();
                Ok(Box::new(std::io::Cursor::new(serde_json::to_vec(&entries)?)))
            }
            ClipWhat::File { index } => {
                let list = self.list(offer, list, paths).await?;
                let (entry, path) = list.get(index as usize).ok_or_else(|| anyhow!("没有这个文件"))?;
                if entry.dir {
                    bail!("{} 是文件夹", entry.path);
                }
                let f = tokio::fs::File::open(path).await.with_context(|| format!("打不开 {}", path.display()))?;
                Ok(Box::new(f))
            }
        }
    }

    async fn list(&self, offer: u64, cached: Option<Arc<Vec<(ClipEntry, PathBuf)>>>, paths: Vec<PathBuf>) -> Result<Arc<Vec<(ClipEntry, PathBuf)>>> {
        if let Some(l) = cached {
            return Ok(l);
        }
        let list = Arc::new(tokio::task::spawn_blocking(move || walk(&paths)).await??);
        if let Some(m) = self.mine.lock().unwrap().iter_mut().find(|m| m.id == offer) {
            m.list = Some(list.clone());
        }
        Ok(list)
    }
}

/// 最上层各项的名字：重名的后面加 (2)、(3)
fn top_names(paths: &[PathBuf]) -> Vec<String> {
    let mut used: Vec<String> = Vec::new();
    paths
        .iter()
        .map(|p| {
            let base = p.file_name().map(|n| n.to_string_lossy().into_owned()).filter(|n| !n.is_empty()).unwrap_or_else(|| "file".into());
            let mut name = base.clone();
            let mut n = 2;
            while used.iter().any(|u| u.eq_ignore_ascii_case(&name)) {
                name = match base.rsplit_once('.') {
                    Some((stem, ext)) if !stem.is_empty() => format!("{stem} ({n}).{ext}"),
                    _ => format!("{base} ({n})"),
                };
                n += 1;
            }
            used.push(name.clone());
            name
        })
        .collect()
}

/// 展开复制的文件和文件夹。文件夹里的符号链接：指向文件的照常传，指向文件夹的跳过（免得绕圈）
fn walk(paths: &[PathBuf]) -> Result<Vec<(ClipEntry, PathBuf)>> {
    fn visit(rel: String, path: &Path, top: bool, out: &mut Vec<(ClipEntry, PathBuf)>) -> Result<()> {
        let link = std::fs::symlink_metadata(path)?.file_type().is_symlink();
        let meta = std::fs::metadata(path).with_context(|| format!("读不了 {}", path.display()))?;
        if meta.is_dir() {
            if link && !top {
                return Ok(());
            }
            out.push((ClipEntry { path: rel.clone(), dir: true, size: 0 }, path.to_path_buf()));
            let mut children: Vec<_> = std::fs::read_dir(path)?.filter_map(|e| e.ok()).collect();
            children.sort_by_key(|e| e.file_name());
            for c in children {
                let name = c.file_name().to_string_lossy().into_owned();
                // 读不了的（没权限）跳过，不让整个粘贴失败
                if let Err(e) = visit(format!("{rel}/{name}"), &c.path(), false, out) {
                    debug!("跳过 {}：{e:#}", c.path().display());
                }
            }
        } else {
            out.push((ClipEntry { path: rel, dir: false, size: meta.len() }, path.to_path_buf()));
        }
        Ok(())
    }
    let mut out = Vec::new();
    for (name, p) in top_names(paths).into_iter().zip(paths) {
        visit(name, p, true, &mut out)?;
    }
    Ok(out)
}

/// 对方路径里的一段段，去掉 ..、空的，免得写到别处去
pub fn safe_rel(path: &str) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains('\\') || part.contains('\0') {
            bail!("文件路径不对：{path}");
        }
        out.push(part);
    }
    if out.as_os_str().is_empty() {
        bail!("文件路径是空的");
    }
    Ok(out)
}

/// 从对方剪贴板粘贴文件时先传到这里
pub fn staging_root() -> PathBuf {
    std::env::temp_dir().join("P2PCLI-clipboard")
}

/// 删掉一天以前粘贴过的临时文件
fn clean_staging() {
    let Ok(dir) = std::fs::read_dir(staging_root()) else { return };
    for e in dir.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > Duration::from_secs(24 * 3600));
        if old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

// ---------------------------------------------------------------- 内容流的格式

async fn pipe<R: AsyncRead + Unpin + ?Sized, W: AsyncWrite + Unpin + ?Sized>(r: &mut R, w: &mut W) -> Result<()> {
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = r.read(&mut buf).await?;
        w.write_u32(n as u32).await?;
        if n == 0 {
            return Ok(());
        }
        w.write_all(&buf[..n]).await?;
    }
}

/// 读开头的结果：失败时带着对方说的原因
pub async fn read_status<R: AsyncRead + Unpin + ?Sized>(r: &mut R) -> Result<()> {
    match r.read_u8().await.context("对方没有回剪贴板内容")? {
        0 => Ok(()),
        _ => {
            let len = r.read_u32().await? as usize;
            if len > 64 << 10 {
                bail!("对方回的原因太长");
            }
            let mut msg = vec![0u8; len];
            r.read_exact(&mut msg).await?;
            bail!("{}", String::from_utf8_lossy(&msg))
        }
    }
}

/// 读完所有块
pub async fn read_all<R: AsyncRead + Unpin + ?Sized>(r: &mut R) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let n = r.read_u32().await.context("剪贴板内容没传完")? as usize;
        if n == 0 {
            return Ok(out);
        }
        if n > CHUNK * 4 {
            bail!("剪贴板内容格式不对");
        }
        let start = out.len();
        out.resize(start + n, 0);
        r.read_exact(&mut out[start..]).await.context("剪贴板内容没传完")?;
    }
}

/// 把一块一块的内容当普通的阻塞读：粘贴大文件时平台实现边收边写
struct ChunkReader {
    rt: Handle,
    r: Box<dyn AsyncRead + Send + Unpin>,
    left: usize,
    done: bool,
}

impl Read for ChunkReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        let r = &mut self.r;
        if self.left == 0 {
            let n = self.rt.block_on(r.read_u32())? as usize;
            if n == 0 {
                self.done = true;
                return Ok(0);
            }
            self.left = n;
        }
        let want = buf.len().min(self.left);
        let n = self.rt.block_on(r.read(&mut buf[..want]))?;
        if n == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "剪贴板里的文件没传完，连接断了"));
        }
        self.left -= n;
        Ok(n)
    }
}

// ---------------------------------------------------------------- 对方的内容

struct RemoteSource {
    offer: ClipOffer,
    fetch: Arc<dyn ClipFetch>,
    rt: Handle,
    list: Mutex<Option<Arc<Vec<ClipEntry>>>>,
    staged: Mutex<Option<Vec<PathBuf>>>,
}

impl RemoteSource {
    fn get(&self, what: ClipWhat) -> Result<Box<dyn AsyncRead + Send + Unpin>> {
        let fetch = self.fetch.fetch(self.offer.id, what);
        self.rt.block_on(async move {
            let mut r = fetch.await?;
            read_status(&mut r).await?;
            Ok(r)
        })
    }
}

impl ClipSource for RemoteSource {
    fn offer(&self) -> &ClipOffer {
        &self.offer
    }

    fn format(&self, key: &str) -> Result<Vec<u8>> {
        if let Some(data) = self.offer.format(key).and_then(|f| f.data.clone()) {
            return Ok(data);
        }
        let mut r = self.get(ClipWhat::Format { key: key.into() })?;
        let data = self.rt.block_on(read_all(&mut r))?;
        info!(key, bytes = data.len(), "粘贴对方剪贴板里的内容");
        Ok(data)
    }

    fn file_list(&self) -> Result<Arc<Vec<ClipEntry>>> {
        if let Some(l) = self.list.lock().unwrap().clone() {
            return Ok(l);
        }
        let mut r = self.get(ClipWhat::FileList)?;
        let bytes = self.rt.block_on(read_all(&mut r))?;
        let list: Arc<Vec<ClipEntry>> = Arc::new(serde_json::from_slice(&bytes).context("对方的文件清单格式不对")?);
        *self.list.lock().unwrap() = Some(list.clone());
        Ok(list)
    }

    fn open_file(&self, index: usize) -> Result<Box<dyn Read + Send>> {
        let r = self.get(ClipWhat::File { index: index as u32 })?;
        Ok(Box::new(ChunkReader { rt: self.rt.clone(), r, left: 0, done: false }))
    }

    fn download(&self) -> Result<Vec<PathBuf>> {
        // 同一时间只传一份：几个程序同时来要，后来的等前面传完直接用
        let mut staged = self.staged.lock().unwrap();
        if let Some(p) = staged.as_ref() {
            return Ok(p.clone());
        }
        let list = self.file_list()?;
        let dir = staging_root().join(format!("{}-{:08x}", self.offer.id, rand::random::<u32>()));
        std::fs::create_dir_all(&dir).with_context(|| format!("建不了临时文件夹 {}", dir.display()))?;
        let total: u64 = list.iter().map(|e| e.size).sum();
        info!(files = list.len(), bytes = total, dir = %dir.display(), "粘贴对方剪贴板里的文件");
        for (i, e) in list.iter().enumerate() {
            let p = dir.join(safe_rel(&e.path)?);
            if e.dir {
                std::fs::create_dir_all(&p)?;
                continue;
            }
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut f = std::fs::File::create(&p).with_context(|| format!("写不了 {}", p.display()))?;
            std::io::copy(&mut self.open_file(i)?, &mut f).with_context(|| format!("传 {} 失败", e.path))?;
        }
        let paths: Vec<PathBuf> = self.offer.files.iter().map(|f| dir.join(&f.name)).collect();
        *staged = Some(paths.clone());
        Ok(paths)
    }
}

// ---------------------------------------------------------------- 被连方

/// 被连方这一路画面的剪贴板：观看方开来的内容流按 token 找到它
pub struct HostClip {
    remote: iroh::EndpointId,
    pub sync: Arc<ClipSync>,
    token: u64,
    next_req: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<iroh::endpoint::RecvStream>>>,
    /// 要发给观看方的 ClipOffer、ClipWant
    out: mpsc::UnboundedSender<crate::screen::HostMsg>,
}

static HOST_CLIPS: Mutex<Option<HashMap<u64, Weak<HostClip>>>> = Mutex::new(None);

impl HostClip {
    pub fn register(remote: iroh::EndpointId, sync: Arc<ClipSync>, out: mpsc::UnboundedSender<crate::screen::HostMsg>) -> Arc<Self> {
        let token = loop {
            let t: u64 = rand::random();
            if t != 0 {
                break t;
            }
        };
        let clip = Arc::new(Self { remote, sync, token, next_req: AtomicU64::new(1), pending: Mutex::new(HashMap::new()), out });
        HOST_CLIPS.lock().unwrap().get_or_insert_with(HashMap::new).insert(token, Arc::downgrade(&clip));
        clip
    }

    pub fn token(&self) -> u64 {
        self.token
    }

    /// 每隔一会儿看一眼本机剪贴板，变了就把清单发给观看方；画面结束（HostClip 丢掉）就停
    pub fn spawn_poll(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(POLL).await;
                let Some(clip) = weak.upgrade() else { return };
                let sync = clip.sync.clone();
                match tokio::task::spawn_blocking(move || sync.poll()).await {
                    Ok(Ok(Some(offer))) => {
                        if clip.out.send(crate::screen::HostMsg::ClipOffer { offer }).is_err() {
                            return;
                        }
                    }
                    Ok(Err(e)) => debug!("读剪贴板失败：{e:#}"),
                    _ => {}
                }
            }
        });
    }

    pub fn fetcher(self: &Arc<Self>) -> Arc<dyn ClipFetch> {
        Arc::new(HostFetch(Arc::downgrade(self)))
    }

    fn find(token: u64, remote: iroh::EndpointId) -> Result<Arc<HostClip>> {
        let clip = HOST_CLIPS.lock().unwrap().as_ref().and_then(|m| m.get(&token)).and_then(Weak::upgrade);
        match clip {
            Some(c) if c.remote == remote => Ok(c),
            _ => bail!("这一路画面已经结束了"),
        }
    }
}

impl Drop for HostClip {
    fn drop(&mut self) {
        if let Some(m) = HOST_CLIPS.lock().unwrap().as_mut() {
            m.remove(&self.token);
        }
    }
}

struct HostFetch(Weak<HostClip>);

impl ClipFetch for HostFetch {
    fn fetch(&self, offer: u64, what: ClipWhat) -> BoxFuture<Result<Box<dyn AsyncRead + Send + Unpin>>> {
        let clip = self.0.upgrade();
        Box::pin(async move {
            let clip = clip.ok_or_else(|| anyhow!("远程画面已经结束了，对方剪贴板里的内容拿不到了"))?;
            let req = clip.next_req.fetch_add(1, Ordering::Relaxed);
            let (tx, rx) = oneshot::channel();
            clip.pending.lock().unwrap().insert(req, tx);
            clip.out.send(crate::screen::HostMsg::ClipWant { req, offer, what }).map_err(|_| anyhow!("远程画面已经结束了"))?;
            drop(clip);
            let recv = tokio::time::timeout(Duration::from_secs(30), rx)
                .await
                .map_err(|_| anyhow!("对方一直没有送来剪贴板内容"))?
                .map_err(|_| anyhow!("远程画面已经结束了"))?;
            Ok(Box::new(recv) as Box<dyn AsyncRead + Send + Unpin>)
        })
    }
}

/// 被连方：观看方开来的剪贴板内容流
pub async fn handle_stream(remote: iroh::EndpointId, req: ClipStreamReq, mut send: iroh::endpoint::SendStream, recv: iroh::endpoint::RecvStream) {
    match req {
        ClipStreamReq::Get { token, offer, what } => {
            let _ = send.set_priority(CLIP_PRIORITY);
            let result = match HostClip::find(token, remote) {
                Ok(clip) => clip.sync.serve(offer, what, &mut send).await,
                Err(e) => {
                    let msg = format!("{e:#}");
                    let _ = send.write_u8(1).await;
                    let _ = send.write_u32(msg.len() as u32).await;
                    send.write_all(msg.as_bytes()).await.map_err(Into::into)
                }
            };
            if let Err(e) = result {
                debug!("给观看方剪贴板内容失败：{e:#}");
            }
            let _ = send.finish();
        }
        ClipStreamReq::Put { token, req } => {
            let Ok(clip) = HostClip::find(token, remote) else { return };
            let tx = clip.pending.lock().unwrap().remove(&req);
            if let Some(tx) = tx {
                let _ = tx.send(recv);
            }
            let _ = send.finish();
        }
    }
}

// ---------------------------------------------------------------- 观看方

/// 观看方这一路画面的剪贴板
pub struct ViewerClip {
    pub sync: Arc<ClipSync>,
    remote: Arc<crate::client::RemoteHost>,
    token: u64,
    /// 观看窗口里关掉了互通
    pub enabled: AtomicBool,
}

impl ViewerClip {
    pub fn new(platform: Arc<dyn ClipPlatform>, remote: Arc<crate::client::RemoteHost>, token: u64) -> Arc<Self> {
        Arc::new(Self { sync: ClipSync::new(platform, Handle::current(), true), remote, token, enabled: AtomicBool::new(true) })
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn fetcher(&self) -> Arc<dyn ClipFetch> {
        Arc::new(ViewerFetch { remote: self.remote.clone(), token: self.token })
    }

    /// 被连方要本机剪贴板里的内容：开一条流送过去
    pub async fn put(&self, req: u64, offer: u64, what: ClipWhat) -> Result<()> {
        let (mut send, _recv) = self.remote.open_stream(&crate::proto::Msg::ScreenClip { req: ClipStreamReq::Put { token: self.token, req } }).await?;
        let _ = send.set_priority(CLIP_PRIORITY);
        self.sync.serve(offer, what, &mut send).await?;
        send.finish()?;
        // 等对方收完再丢掉这条流
        let _ = send.stopped().await;
        Ok(())
    }
}

struct ViewerFetch {
    remote: Arc<crate::client::RemoteHost>,
    token: u64,
}

impl ClipFetch for ViewerFetch {
    fn fetch(&self, offer: u64, what: ClipWhat) -> BoxFuture<Result<Box<dyn AsyncRead + Send + Unpin>>> {
        let remote = self.remote.clone();
        let req = ClipStreamReq::Get { token: self.token, offer, what };
        Box::pin(async move {
            let (mut send, recv) = remote.open_stream(&crate::proto::Msg::ScreenClip { req }).await?;
            let _ = send.finish();
            Ok(Box::new(recv) as Box<dyn AsyncRead + Send + Unpin>)
        })
    }
}

mod b64 {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(b) => s.serialize_str(&STANDARD.encode(b)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        let s: Option<String> = Option::deserialize(d)?;
        s.map(|s| STANDARD.decode(s).map_err(serde::de::Error::custom)).transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_paths() {
        let names = top_names(&[PathBuf::from("/a/x.txt"), PathBuf::from("/b/x.txt"), PathBuf::from("/c/X.txt"), PathBuf::from("/d/dir")]);
        assert_eq!(names, vec!["x.txt", "x (2).txt", "X (3).txt", "dir"]);
        assert!(safe_rel("a/../b").is_err());
        assert!(safe_rel("/a").is_err());
        assert!(safe_rel("a\\b").is_err());
        assert_eq!(safe_rel("a/b").unwrap(), PathBuf::from("a").join("b"));
    }

    #[test]
    fn offer_json() {
        let o = ClipOffer {
            id: 3,
            formats: vec![
                ClipFormat { key: "text".into(), size: 2, data: Some(b"hi".to_vec()) },
                ClipFormat { key: "png".into(), size: 9_000_000, data: None },
            ],
            files: vec![],
        };
        let s = serde_json::to_string(&o).unwrap();
        assert!(s.contains("\"aGk=\""));
        assert_eq!(serde_json::from_str::<ClipOffer>(&s).unwrap(), o);
        assert!(o.has_lazy());
    }
}
