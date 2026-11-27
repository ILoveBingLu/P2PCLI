//! 抓几帧编码好，生成一个 HTML：用 WebCodecs 按远程画面窗口完全相同的配置解码，结果写进页面。
//! 用无界面的 Chromium/Edge 打开（--dump-dom）就能看到浏览器端能不能硬件解码这路画面。
//! cargo run -p p2pcli-screen --example webcodecs_check -- out.html
use base64::Engine as _;
use p2pcli_core::screen::{CaptureOptions, Quality};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let out = std::env::args().nth(1).unwrap_or_else(|| "webcodecs_check.html".into());
    let backend = p2pcli_screen::backend().expect("这个系统不支持");
    let display = backend.displays()?.into_iter().find(|d| d.primary).unwrap();
    let q = Quality::Balanced;
    let cap = backend.start(CaptureOptions { display: display.id, max_long_edge: q.max_long_edge(Default::default()), fps: q.fps(0, 0), bitrate: q.start_bitrate(), show_cursor: true, codec: Default::default(), full_range: false, wide_gamut: false, yuv444: false })?;
    let b64 = |d: &[u8]| base64::engine::general_purpose::STANDARD.encode(d);
    let mut config = None;
    let mut frames = Vec::new();
    for _ in 0..20 {
        let f = tokio::time::timeout(std::time::Duration::from_secs(5), cap.next_frame()).await??;
        if let Some(c) = f.config { config = Some(c); }
        frames.push(format!("{{\"key\":{},\"data\":\"{}\"}}", f.key, b64(&f.data)));
    }
    let c = config.expect("没有解码配置");
    let html = format!(r#"<!doctype html><meta charset="utf-8"><body><pre id="out">running</pre><script>
const cfg = {{codec: "{codec}", codedWidth: {w}, codedHeight: {h}, description: Uint8Array.from(atob("{desc}"), c => c.charCodeAt(0)), optimizeForLatency: true}};
const frames = [{frames}];
(async () => {{
  const out = [];
  const show = () => document.getElementById("out").textContent = out.join("\n") + "\n";
  out.push("secure=" + isSecureContext + " VideoDecoder=" + (typeof VideoDecoder)); show();
  for (const hw of ["prefer-hardware", "no-preference"]) {{
    VideoDecoder.isConfigSupported({{...cfg, hardwareAcceleration: hw}}).then(s => {{ out.push(hw + " supported=" + s.supported); show(); }});
  }}
  let decoded = 0, size = "", err = "";
  const d = new VideoDecoder({{output: f => {{ decoded++; size = f.displayWidth + "x" + f.displayHeight; f.close(); }}, error: e => err = String(e)}});
  d.configure({{...cfg, hardwareAcceleration: "prefer-hardware"}});
  frames.forEach((f, i) => d.decode(new EncodedVideoChunk({{type: f.key ? "key" : "delta", timestamp: i * 1000, data: Uint8Array.from(atob(f.data), c => c.charCodeAt(0))}})));
  out.push("queued"); show();
  setInterval(() => {{ document.getElementById("out").textContent = out.join("\n") + "\nprogress decoded=" + decoded + " size=" + size + " error=" + (err || "none") + " state=" + d.state; }}, 200);
  await d.flush().catch(e => err = String(e));
  out.push("decoded=" + decoded + "/" + frames.length + " size=" + size + " error=" + (err || "none"));
  document.getElementById("out").textContent = out.join("\n");
}})().catch(e => document.getElementById("out").textContent = "FAILED " + e);
</script>"#, codec = c.codec, w = c.width, h = c.height, desc = c.description, frames = frames.join(","));
    std::fs::write(&out, html)?;
    println!("写好了 {out}：{} {}x{}，{} 帧", c.codec, c.width, c.height, frames.len());
    Ok(())
}
