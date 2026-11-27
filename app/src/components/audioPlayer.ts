// 远程画面的声音：后端解好的 48 kHz 立体声 PCM（左右交错的 f32），一段段排进 Web Audio 放出来。
// 留一点缓冲防网络抖动，攒多了（延迟变大）就丢掉一些追上来，声音不会越拖越晚。

const RATE = 48_000;
/// 一开始、或者断了一会儿以后，第一段从多久之后开始放（秒）：给网络抖动留的余量
const START_LEAD = 0.06;
/// 排着的超过这么久（秒）就丢掉新来的这段，把延迟压回来
const MAX_LEAD = 0.25;

export class PcmPlayer {
  private ctx: AudioContext;
  private gain: GainNode;
  /// 下一段从什么时候开始放（AudioContext 的时间）
  private next = 0;
  private muted = false;
  /** 声音被系统拦着（还没点过这个窗口，浏览器不让自动放）：界面上提示点一下 */
  onBlocked: ((blocked: boolean) => void) | null = null;
  private blocked = false;

  constructor() {
    this.ctx = new AudioContext({ sampleRate: RATE, latencyHint: "interactive" });
    this.gain = this.ctx.createGain();
    this.gain.connect(this.ctx.destination);
    this.ctx.onstatechange = () => this.setBlocked(this.ctx.state === "suspended");
  }

  private setBlocked(blocked: boolean) {
    if (blocked === this.blocked) return;
    this.blocked = blocked;
    this.onBlocked?.(blocked);
  }

  /** 用户点了一下、按了个键：浏览器这时才允许开声音 */
  resume() {
    if (this.ctx.state === "suspended") void this.ctx.resume().catch(() => {});
  }

  setMuted(muted: boolean) {
    this.muted = muted;
    this.gain.gain.value = muted ? 0 : 1;
    if (muted) this.next = 0;
  }

  push(pcm: Float32Array) {
    if (this.muted) return;
    if (this.ctx.state === "suspended") {
      this.setBlocked(true);
      this.resume();
      return;
    }
    const frames = pcm.length >> 1;
    if (frames === 0) return;
    const now = this.ctx.currentTime;
    // 断了一会儿（或者刚开始）：从现在往后留一点余量开始排
    if (this.next < now + 0.005) this.next = now + START_LEAD;
    // 排得太多：这段不放了，延迟压回来
    if (this.next > now + MAX_LEAD) return;
    const buffer = this.ctx.createBuffer(2, frames, RATE);
    const left = new Float32Array(frames);
    const right = new Float32Array(frames);
    for (let i = 0; i < frames; i++) {
      left[i] = pcm[i * 2];
      right[i] = pcm[i * 2 + 1];
    }
    buffer.copyToChannel(left, 0);
    buffer.copyToChannel(right, 1);
    const source = this.ctx.createBufferSource();
    source.buffer = buffer;
    source.connect(this.gain);
    source.start(this.next);
    this.next += frames / RATE;
  }

  /** 现在排着多少（毫秒），诊断用 */
  queuedMs(): number {
    return Math.max(0, (this.next - this.ctx.currentTime) * 1000);
  }

  close() {
    void this.ctx.close().catch(() => {});
  }
}
