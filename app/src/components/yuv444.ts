// 远程画面的 4:4:4 还原：对方把全分辨率的 Y、Cb、Cr 从上到下叠成一张三倍高的灰度图编码（见 core 的 Layout::Yuv444Stacked），
// 这里把解出来的三段读回来，按 BT.709 全范围合成彩色。每个像素都有自己的颜色，彩色小字、细线不发虚。

const VERTEX = `#version 300 es
void main() {
  // 一个盖满画布的大三角形
  vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
  gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}`;

// 解码器按全范围、中性色度把灰度图转成 RGB，三个通道本该都等于 Y；色度被压缩得不完全是 128 时
// 按亮度系数加权还是 Y（R、G、B 是 Y、Cb、Cr 的线性组合，加权正好把色度消掉）
const FRAGMENT = `#version 300 es
precision highp float;
uniform sampler2D tex;
uniform int height;
out vec4 color;
const vec3 K = vec3(0.2126, 0.7152, 0.0722);
float plane(ivec2 p) { return dot(texelFetch(tex, p, 0).rgb, K); }
void main() {
  ivec2 p = ivec2(gl_FragCoord.xy);
  p.y = height - 1 - p.y;
  float y = plane(p);
  float cb = plane(p + ivec2(0, height)) - 0.5;
  float cr = plane(p + ivec2(0, 2 * height)) - 0.5;
  float r = y + 1.5748 * cr;
  float b = y + 1.8556 * cb;
  float g = (y - 0.2126 * r - 0.0722 * b) / 0.7152;
  color = vec4(clamp(vec3(r, g, b), 0.0, 1.0), 1.0);
}`;

/** 这台电脑能不能还原 4:4:4（要 WebGL2） */
export function canRender444(): boolean {
  try {
    return !!document.createElement("canvas").getContext("webgl2");
  } catch {
    return false;
  }
}

type ColorSpaced = { drawingBufferColorSpace?: PredefinedColorSpace; unpackColorSpace?: PredefinedColorSpace };

export class Yuv444Renderer {
  private constructor(
    readonly canvas: HTMLCanvasElement,
    private gl: WebGL2RenderingContext,
    private heightLoc: WebGLUniformLocation | null,
  ) {}

  /** 在这块画布上开 WebGL2；画布已经开过 2D 之类的就返回空 */
  static create(canvas: HTMLCanvasElement): Yuv444Renderer | null {
    const gl = canvas.getContext("webgl2", { alpha: false, antialias: false, depth: false, stencil: false, preserveDrawingBuffer: false });
    if (!gl) return null;
    const shader = (type: number, src: string) => {
      const s = gl.createShader(type)!;
      gl.shaderSource(s, src);
      gl.compileShader(s);
      if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s) ?? "着色器编译失败");
      return s;
    };
    const program = gl.createProgram()!;
    gl.attachShader(program, shader(gl.VERTEX_SHADER, VERTEX));
    gl.attachShader(program, shader(gl.FRAGMENT_SHADER, FRAGMENT));
    gl.linkProgram(program);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(program) ?? "着色器链接失败");
    gl.useProgram(program);
    gl.bindVertexArray(gl.createVertexArray());
    const tex = gl.createTexture();
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, tex);
    // 按像素取，不插值：三段各自对齐到原画面的像素
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.uniform1i(gl.getUniformLocation(program, "tex"), 0);
    // 传进来的是数据不是照片：不做任何颜色换算
    gl.pixelStorei(gl.UNPACK_COLORSPACE_CONVERSION_WEBGL, gl.NONE);
    gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, false);
    gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, false);
    const spaced = gl as unknown as ColorSpaced;
    if ("unpackColorSpace" in gl) spaced.unpackColorSpace = "srgb";
    return new Yuv444Renderer(canvas, gl, gl.getUniformLocation(program, "height"));
  }

  /** 画一帧（编码画面宽 W、高 3H），画布变成 W×H。p3：对方按 Display P3 抓的，画布按 P3 显示 */
  draw(frame: VideoFrame, p3: boolean) {
    const { gl, canvas } = this;
    const width = frame.displayWidth;
    const height = Math.floor(frame.displayHeight / 3);
    if (canvas.width !== width || canvas.height !== height) {
      canvas.width = width;
      canvas.height = height;
    }
    const spaced = gl as unknown as ColorSpaced;
    const want: PredefinedColorSpace = p3 ? "display-p3" : "srgb";
    if ("drawingBufferColorSpace" in gl && spaced.drawingBufferColorSpace !== want) spaced.drawingBufferColorSpace = want;
    gl.viewport(0, 0, width, height);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, gl.RGBA, gl.UNSIGNED_BYTE, frame);
    gl.uniform1i(this.heightLoc, height);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  }
}
