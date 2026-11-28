// SPDX-License-Identifier: MIT
// Direct3D 7's fixed-function pixel pipeline on WebGL2, for the browser player: the batches the
// runtime's renderer writes (platform/src/gpu/web.rs has the format) are drawn on the render
// target, which stays on the GPU as natively (platform/src/gpu/native.rs and resident.rs, whose
// WGSL shaders these follow): passes make it what guest memory would hold, the frame shown is
// made from it with the overlays and drawn on this module's canvas (which the page shows), and
// it is read into guest memory only when the game touches the surface. The old way, drawing
// over the pixels uploaded and reading the result back (gl_render), stays for
// RC_GPU_RESIDENT=0.
//
// The image is kept upside down on the GPU (row 0 of the guest's surface is row 0 of the
// framebuffer), so that uploading and reading back need no flip; only drawing on the canvas
// turns it over.

const VS = `#version 300 es
precision highp float;
layout(location = 0) in vec4 a_pos;
layout(location = 1) in vec4 a_diffuse;
layout(location = 2) in vec4 a_specular;
layout(location = 3) in vec2 a_uv0;
layout(location = 4) in vec2 a_uv1;
layout(location = 5) in float a_eye;
layout(location = 6) in float a_exact;
uniform vec2 u_view;
out vec4 v_diffuse;
out vec4 v_specular;
out vec2 v_uv0;
out vec2 v_uv1;
out float v_eye;
flat out float v_exact;
void main() {
  // Direct3D 7 has pixel centres on integer coordinates: half a pixel to WebGL's.
  float x = (a_pos.x + 0.5) / u_view.x * 2.0 - 1.0;
  float y = (a_pos.y + 0.5) / u_view.y * 2.0 - 1.0;
  float w = a_pos.w > 0.0 ? 1.0 / a_pos.w : 1.0;
  // Depth 0..1 to WebGL's -1..1.
  gl_Position = vec4(x * w, y * w, (clamp(a_pos.z, 0.0, 1.0) * 2.0 - 1.0) * w, w);
  v_diffuse = a_diffuse;
  v_specular = a_specular;
  v_uv0 = a_uv0;
  v_uv1 = a_uv1;
  v_eye = a_eye;
  v_exact = a_exact;
}`;

const FS = `#version 300 es
precision highp float;
precision highp int;
in vec4 v_diffuse;
in vec4 v_specular;
in vec2 v_uv0;
in vec2 v_uv1;
in float v_eye;
flat in float v_exact;
uniform sampler2D u_t0;
uniform sampler2D u_t1;
uniform ivec4 u_stage[4];
uniform vec4 u_factor;
uniform vec4 u_fog_color;
uniform vec2 u_fog_range;
uniform ivec4 u_misc; // alpha func, -, fog mode, flags (1 specular, 2 keyed, 4 tex0, 8 tex1)
uniform float u_alpha_ref;
// Target pixels per game pixel (x, y), whether the draw is pixel-exact on a larger target, how
// much larger texture 0 is on a pixel-exact draw (0: not).
uniform vec4 u_snap;
out vec4 o_color;

vec4 arg(int a, vec4 diffuse, vec4 current, vec4 tex, vec4 spec) {
  vec4 v = current;
  int k = a & 15;
  if (k == 0) v = diffuse;
  else if (k == 2) v = tex;
  else if (k == 3) v = u_factor;
  else if (k == 4) v = spec;
  if ((a & 32) != 0) v = vec4(v.a);
  if ((a & 16) != 0) v = vec4(1.0) - v;
  return v;
}

vec4 op(int o, vec4 a1, vec4 a2, float ad, float at, float ac) {
  if (o == 2) return a1;
  if (o == 3) return a2;
  if (o == 4) return a1 * a2;
  if (o == 5) return min(a1 * a2 * 2.0, vec4(1.0));
  if (o == 6) return min(a1 * a2 * 4.0, vec4(1.0));
  if (o == 7) return min(a1 + a2, vec4(1.0));
  if (o == 8) return clamp(a1 + a2 - 0.5, 0.0, 1.0);
  if (o == 9) return clamp((a1 + a2 - 0.5) * 2.0, 0.0, 1.0);
  if (o == 10) return a1 - a2;
  if (o == 11) return a1 + a2 - a1 * a2;
  if (o == 12) return a1 * ad + a2 * (1.0 - ad);
  if (o == 13) return a1 * at + a2 * (1.0 - at);
  if (o == 15) return a1 * ac + a2 * (1.0 - ac);
  return a1 * a2;
}

bool compare(int f, float a, float b) {
  if (f == 1) return false;
  if (f == 2) return a < b;
  if (f == 3) return a == b;
  if (f == 4) return a <= b;
  if (f == 5) return a > b;
  if (f == 6) return a != b;
  if (f == 7) return a >= b;
  return true;
}

void main() {
  // As natively: a pixel-exact draw on a larger target has its coordinates moved to the centre
  // of the game's pixel this pixel belongs to (along their gradients).
  vec2 frag = gl_FragCoord.xy;
  vec2 gx = dFdx(v_uv0);
  vec2 gy = dFdy(v_uv0);
  vec2 centre = (floor(frag / u_snap.xy) + 0.5) * u_snap.xy;
  vec2 d = (u_snap.z != 0.0 && v_exact > 0.5) ? centre - frag : vec2(0.0);
  vec2 uv0 = v_uv0 + gx * d.x + gy * d.y;
  vec2 uv1 = v_uv1 + dFdx(v_uv1) * d.x + dFdy(v_uv1) * d.y;
  // An enlarged texture on a pixel-exact triangle: the texel the game's pixel shows (at its
  // centre), sampled within at where this pixel lies in the game's pixel, so the enlarged
  // texture's detail (smoothing, an outline font's glyphs) shows; never a texel past the game's.
  vec2 game = frag / u_snap.xy;
  vec2 dc = (floor(game) + 0.5) * u_snap.xy - frag;
  vec2 atCentre = v_uv0 + gx * dc.x + gy * dc.y;
  vec2 size = vec2(textureSize(u_t0, 0)) / max(u_snap.w, 1.0);
  vec2 f = fract(game);
  vec2 within = vec2(gx.x < 0.0 ? 1.0 - f.x : f.x, gy.y < 0.0 ? 1.0 - f.y : f.y);
  if (u_snap.w > 1.5 && v_exact > 0.5) uv0 = (floor(atCentre * size) + within) / size;
  vec4 c0 = texture(u_t0, u_stage[1].z == 1 ? uv1 : uv0);
  vec4 c1 = texture(u_t1, u_stage[3].z == 1 ? uv1 : uv0);
  vec4 current = v_diffuse;
  bool keyed = false;
  int flags = u_misc.w;
  for (int s = 0; s < 2; s++) {
    ivec4 a = u_stage[2 * s];
    ivec4 b = u_stage[2 * s + 1];
    if (b.w == 0) break;
    vec4 tex = vec4(1.0);
    if (s == 0 && (flags & 4) != 0) tex = c0;
    if (s == 1 && (flags & 8) != 0) tex = c1;
    if ((flags & 2) != 0 && tex == vec4(0.0)) keyed = true;
    vec4 c = op(a.x, arg(a.y, v_diffuse, current, tex, v_specular), arg(a.z, v_diffuse, current, tex, v_specular), v_diffuse.a, tex.a, current.a);
    vec4 next = vec4(clamp(c.rgb, 0.0, 1.0), current.a);
    if (a.w != 1) {
      vec4 al = op(a.w, arg(b.x, v_diffuse, current, tex, v_specular), arg(b.y, v_diffuse, current, tex, v_specular), v_diffuse.a, tex.a, current.a);
      next.a = clamp(al.a, 0.0, 1.0);
    }
    current = next;
  }
  if ((flags & 1) != 0) current = vec4(min(current.rgb + v_specular.rgb, vec3(1.0)), current.a);
  if (keyed) discard;
  if (u_misc.x != 0 && !compare(u_misc.x, current.a, u_alpha_ref)) discard;
  if (u_misc.z == 1) {
    current = vec4(mix(u_fog_color.rgb, current.rgb, clamp(v_specular.a, 0.0, 1.0)), current.a);
  } else if (u_misc.z == 2) {
    float f = 1.0;
    if (u_fog_range.y > u_fog_range.x) f = clamp((u_fog_range.y - v_eye) / (u_fog_range.y - u_fog_range.x), 0.0, 1.0);
    current = vec4(mix(u_fog_color.rgb, current.rgb, f), current.a);
  }
  o_color = current;
}`;

// The passes over the target (resident.rs's WGSL, as GLSL). Uniforms p[0..3] as there.
const PASS_VS = `#version 300 es
precision highp float;
precision highp int;
uniform uvec4 p[4];
uniform int u_quad;
void main() {
  int i = gl_VertexID;
  if (u_quad == 0) {
    vec2 q = vec2(float((i << 1) & 2), float(i & 2));
    gl_Position = vec4(q * 2.0 - 1.0, 0.0, 1.0);
  } else {
    // An overlay picture: p[0] frame width, height, picture x, y; p[1].xy its size.
    vec2 q = vec2(float(i & 1), float((i >> 1) & 1));
    vec2 at = vec2(float(int(p[0].z)), float(int(p[0].w))) + q * vec2(p[1].xy);
    gl_Position = vec4(at / vec2(p[0].xy) * 2.0 - 1.0, 0.0, 1.0);
  }
}`;

const PASS_FS = `#version 300 es
precision highp float;
precision highp int;
uniform uvec4 p[4];
uniform sampler2D u_src;
uniform sampler2D u_lut;
uniform sampler2D u_img;
out vec4 o_color;

// p[0]: target width, height; guest width, height
// p[1]: margin, edge strip x, edge strip y, flags (1 scaled, 2 sixteen bits, 4 gamma, 8 rgb565)
// p[2]: bits of red, green, blue, alpha
// p[3]: the guest's zero pixel as RGBA8

uvec4 px(vec4 c) { return uvec4(roundEven(clamp(c, 0.0, 1.0) * 255.0)); }
vec4 outp(uvec4 v) { return vec4(v) / 255.0; }
uvec4 load(sampler2D t, uint x, uint y) { return px(texelFetch(t, ivec2(int(x), int(y)), 0)); }
uvec2 pos() { return uvec2(gl_FragCoord.xy); }

uint quant1(uint v, uint n) {
  if (n == 0u) return 255u;
  uint m = (1u << n) - 1u;
  uint c = (v * m + 127u) / 255u;
  return c * 255u / m;
}
uvec4 quant(uvec4 v) {
  return uvec4(quant1(v.x, p[2].x), quant1(v.y, p[2].y), quant1(v.z, p[2].z), quant1(v.w, p[2].w));
}
uint low(uint x, uint size, uint gsize) {
  if ((p[1].w & 1u) == 0u) return x;
  return min(x * gsize / size, gsize - 1u);
}
uint centre(uint l, uint size, uint gsize) {
  if ((p[1].w & 1u) == 0u) return l;
  return min((2u * l + 1u) * size / (2u * gsize), size - 1u);
}
bool in_margin(uint lx) { return p[1].x > 0u && (lx < p[1].x || lx >= p[0].z - p[1].x); }

vec4 edges() {
  uvec2 f = pos();
  return outp(load(u_src, max(f.x, p[1].y), max(f.y, p[1].z)));
}
vec4 quantize() {
  uvec2 f = pos();
  return outp(quant(load(u_src, f.x, f.y)));
}
vec4 margins() {
  uvec2 f = pos();
  if (in_margin(low(f.x, p[0].x, p[0].z))) return outp(p[3]);
  return outp(load(u_src, f.x, f.y));
}
vec4 reduce() {
  uvec2 f = pos();
  uint lx = low(f.x, p[0].x, p[0].z);
  uint ly = low(f.y, p[0].y, p[0].w);
  if (in_margin(lx)) return outp(p[3]);
  return outp(quant(load(u_src, centre(lx, p[0].x, p[0].z), centre(ly, p[0].y, p[0].w))));
}
vec4 guest() {
  uvec2 f = pos();
  return outp(load(u_src, centre(f.x, p[0].x, p[0].z), centre(f.y, p[0].y, p[0].w)));
}
uvec4 gamma(uvec4 v) {
  if ((p[1].w & 4u) == 0u) return v;
  return uvec4(load(u_lut, v.x, 0u).x, load(u_lut, v.y, 0u).y, load(u_lut, v.z, 0u).z, v.w);
}
uvec4 shown(uint x, uint y) {
  uvec4 v = load(u_src, x, y);
  if ((p[1].w & 1u) == 0u) {
    if ((p[1].w & 8u) != 0u) {
      uint r = (v.x * 31u + 127u) / 255u;
      uint g = (v.y * 63u + 127u) / 255u;
      uint b = (v.z * 31u + 127u) / 255u;
      v = uvec4((r << 3u) | (r >> 2u), (g << 2u) | (g >> 4u), (b << 3u) | (b >> 2u), 255u);
    } else {
      v = uvec4(v.xyz, 255u);
    }
  }
  return gamma(v);
}
vec4 present() {
  uvec2 f = pos();
  return outp(shown(f.x, f.y));
}
// p[1].z 1 for a gradient: p[2] its region x0, x1, y0, y1; p[3].xy the columns left, right.
vec4 overlay() {
  uvec2 f = pos();
  vec4 base = vec4(shown(f.x, f.y));
  if (p[1].z == 1u && f.x >= p[2].x && f.x < p[2].y && f.y >= p[2].z && f.y < p[2].w) {
    vec4 l = vec4(shown(p[3].x, f.y));
    vec4 r = vec4(shown(p[3].y, f.y));
    float t = float(f.x - p[2].x) / float(max(p[2].y - p[2].x, 1u));
    base = l * (1.0 - t) + r * t;
  }
  vec4 c = texelFetch(u_img, ivec2(int(f.x) - int(p[0].z), int(f.y) - int(p[0].w)), 0);
  vec4 o = base / 255.0 * (1.0 - c.a) + vec4(c.rgb, 0.0);
  return vec4(o.rgb, 1.0);
}
`;

// The passes by the number the runtime gives them.
const PASSES = ['edges', 'quantize', 'margins', 'reduce', 'guest', 'present', 'overlay'];

/* The renderer: `imports` for the runtime (gl_init, gl_texture, gl_texture_drop, gl_render,
 * and for the target kept on the GPU gl_draw, gl_pass, gl_read_guest, gl_read_target,
 * gl_keep_front, gl_present, gl_read_present, gl_wait); `canvas()` is what gl_present drew on,
 * `readPresent(w, h)` its picture as RGBA8. `memory()` gives the runtime's memory. Without
 * WebGL2, gl_init answers 0 and the runtime draws in software. */
export function createGL(memory) {
  let gl = null;
  let prog = null;
  let loc = null;
  let fbo = null;
  let color = null;
  let depth = null;
  let size = [0, 0];
  let vbuf = null;
  let white = null;
  const textures = new Map();
  // The passes' programs, their empty vertex array; the scratch a pass over the target draws
  // into (then swapped with the target's colour); the front surface's picture, the frame shown,
  // and the guest-sized picture read back (each a texture with its framebuffer and size).
  const passes = [];
  let pvao = null;
  let scratch = null;
  let front = null;
  let shown = null;
  let low = null;
  let readFbo = null;

  function compile(type, src) {
    const s = gl.createShader(type);
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s));
    return s;
  }

  function init() {
    let canvas;
    if (typeof OffscreenCanvas !== 'undefined') canvas = new OffscreenCanvas(1, 1);
    else if (typeof document !== 'undefined') canvas = document.createElement('canvas');
    else return 0;
    gl = canvas.getContext('webgl2', {
      antialias: false, depth: false, alpha: false, premultipliedAlpha: false, preserveDrawingBuffer: true,
    });
    if (!gl) return 0;
    const link = (vs, fs) => {
      const p = gl.createProgram();
      gl.attachShader(p, compile(gl.VERTEX_SHADER, vs));
      gl.attachShader(p, compile(gl.FRAGMENT_SHADER, fs));
      gl.linkProgram(p);
      if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(p));
      return p;
    };
    try {
      prog = link(VS, FS);
      for (const name of PASSES) {
        const p = link(PASS_VS, `${PASS_FS}void main() { o_color = ${name}(); }\n`);
        const at = (n) => gl.getUniformLocation(p, n);
        gl.useProgram(p);
        gl.uniform1i(at('u_src'), 0);
        gl.uniform1i(at('u_lut'), 1);
        gl.uniform1i(at('u_img'), 2);
        gl.uniform1i(at('u_quad'), name === 'overlay' ? 1 : 0);
        passes.push({ prog: p, p: at('p[0]') });
      }
    } catch (e) {
      console.warn('WebGL2 renderer:', e);
      gl = null;
      return 0;
    }
    gl.useProgram(prog);
    loc = Object.fromEntries(['u_view', 'u_t0', 'u_t1', 'u_stage', 'u_factor', 'u_fog_color', 'u_fog_range',
      'u_misc', 'u_alpha_ref', 'u_snap'].map((n) => [n, gl.getUniformLocation(prog, n)]));
    gl.uniform1i(loc.u_t0, 0);
    gl.uniform1i(loc.u_t1, 1);
    vbuf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, vbuf);
    const stride = 80;
    const attr = (i, n, off) => { gl.enableVertexAttribArray(i); gl.vertexAttribPointer(i, n, gl.FLOAT, false, stride, off); };
    attr(0, 4, 0); attr(1, 4, 16); attr(2, 4, 32); attr(3, 2, 48); attr(4, 2, 56); attr(5, 1, 64); attr(6, 1, 68);
    white = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, white);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, 1, 1, 0, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array([255, 255, 255, 255]));
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    gl.pixelStorei(gl.PACK_ALIGNMENT, 1);
    pvao = gl.createVertexArray();
    readFbo = gl.createFramebuffer();
    console.log('[gl] WebGL2 renderer');
    return 1;
  }

  function target(w, h) {
    if (size[0] === w && size[1] === h) return;
    size = [w, h];
    if (fbo) { gl.deleteFramebuffer(fbo); gl.deleteTexture(color); gl.deleteRenderbuffer(depth); }
    color = colorTexture(w, h);
    depth = gl.createRenderbuffer();
    gl.bindRenderbuffer(gl.RENDERBUFFER, depth);
    gl.renderbufferStorage(gl.RENDERBUFFER, gl.DEPTH_COMPONENT32F, w, h);
    fbo = gl.createFramebuffer();
    gl.bindFramebuffer(gl.FRAMEBUFFER, fbo);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, color, 0);
    gl.framebufferRenderbuffer(gl.FRAMEBUFFER, gl.DEPTH_ATTACHMENT, gl.RENDERBUFFER, depth);
    gl.viewport(0, 0, w, h);
    gl.clearDepth(1);
    gl.clear(gl.DEPTH_BUFFER_BIT);
  }

  // An RGBA8 texture the passes read texel by texel.
  function colorTexture(w, h) {
    const t = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, t);
    gl.texStorage2D(gl.TEXTURE_2D, 1, gl.RGBA8, w, h);
    nearest();
    return t;
  }

  function nearest() {
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
  }

  // A texture with its framebuffer, `old`'s again if it has the size.
  function picture(old, w, h) {
    if (old && old.w === w && old.h === h) return old;
    if (old) { gl.deleteFramebuffer(old.fbo); gl.deleteTexture(old.tex); }
    const tex = colorTexture(w, h);
    const fbo = gl.createFramebuffer();
    gl.bindFramebuffer(gl.FRAMEBUFFER, fbo);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, tex, 0);
    return { tex, fbo, w, h };
  }

  function bindPassTexture(unit, t) {
    gl.activeTexture(gl.TEXTURE0 + unit);
    gl.bindTexture(gl.TEXTURE_2D, t || white);
    nearest();
  }

  // Pass `n` from `src` into the framebuffer `fbo` (w × h) with the uniforms `u` (16 words);
  // `img` the overlay picture's texture (drawn as its quad over what `fbo` holds).
  function runPass(n, u, src, fbo, w, h, lut, img) {
    const pass = passes[n];
    gl.bindFramebuffer(gl.FRAMEBUFFER, fbo);
    gl.viewport(0, 0, w, h);
    gl.disable(gl.BLEND);
    gl.disable(gl.DEPTH_TEST);
    gl.useProgram(pass.prog);
    gl.uniform4uiv(pass.p, u);
    bindPassTexture(0, src);
    bindPassTexture(1, lut);
    bindPassTexture(2, img);
    gl.bindVertexArray(pvao);
    if (img) gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    else gl.drawArrays(gl.TRIANGLES, 0, 3);
    gl.bindVertexArray(null);
  }

  // Read `w` × `h` RGBA8 of the texture `tex` into the runtime's memory at `out`.
  function readTexture(tex, w, h, out) {
    gl.bindFramebuffer(gl.FRAMEBUFFER, readFbo);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, tex, 0);
    gl.readPixels(0, 0, w, h, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array(memory().buffer, out, w * h * 4));
  }

  // Draw the batch's commands on the target (w × h), over the pixels at `rgba` if not 0.

  function blendFactor(f) {
    switch (f) {
      case 1: return gl.ZERO;
      case 2: return gl.ONE;
      case 3: return gl.SRC_COLOR;
      case 4: return gl.ONE_MINUS_SRC_COLOR;
      case 5: return gl.SRC_ALPHA;
      case 6: return gl.ONE_MINUS_SRC_ALPHA;
      case 7: return gl.DST_ALPHA;
      case 8: return gl.ONE_MINUS_DST_ALPHA;
      case 9: return gl.DST_COLOR;
      case 10: return gl.ONE_MINUS_DST_COLOR;
      case 11: return gl.SRC_ALPHA_SATURATE;
      default: return gl.ONE;
    }
  }
  const DEPTH = () => [gl.ALWAYS, gl.NEVER, gl.LESS, gl.EQUAL, gl.LEQUAL, gl.GREATER, gl.NOTEQUAL, gl.GEQUAL, gl.ALWAYS];

  function bindTexture(unit, id, flags) {
    gl.activeTexture(gl.TEXTURE0 + unit);
    const t = id ? textures.get(id) : null;
    gl.bindTexture(gl.TEXTURE_2D, t || white);
    if (!t) return;
    const filter = flags & 1 ? gl.LINEAR : gl.NEAREST;
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, filter);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, filter);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, flags & 2 ? gl.CLAMP_TO_EDGE : gl.REPEAT);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, flags & 4 ? gl.CLAMP_TO_EDGE : gl.REPEAT);
  }

  function drawCommands(cmd, len, rgba, w, h, vw = w, vh = h) {
    const buf = memory().buffer;
    const words = new Uint32Array(buf, cmd, len);
    const floats = new Float32Array(buf, cmd, len);
    target(w, h);
    gl.bindFramebuffer(gl.FRAMEBUFFER, fbo);
    gl.viewport(0, 0, w, h);
    // The target's pixels as they are in guest memory (none: it holds them).
    if (rgba) {
      gl.activeTexture(gl.TEXTURE0);
      gl.bindTexture(gl.TEXTURE_2D, color);
      gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, w, h, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array(buf, rgba, w * h * 4));
    }
    gl.useProgram(prog);
    // Positions are in the game's screen pixels (`vw × vh`), the target may be larger.
    gl.uniform2f(loc.u_view, vw, vh);
    gl.bindBuffer(gl.ARRAY_BUFFER, vbuf);
    const depthFn = DEPTH();
    let p = 0;
    while (p < len) {
      const kind = words[p++];
      if (kind === 1) {
        gl.depthMask(true);
        gl.clearDepth(floats[p++]);
        gl.clear(gl.DEPTH_BUFFER_BIT);
        continue;
      }
      const n = words[p++];
      gl.bufferData(gl.ARRAY_BUFFER, floats.subarray(p, p + 20 * n), gl.STREAM_DRAW);
      p += 20 * n;
      const t0 = words[p], f0 = words[p + 1], t1 = words[p + 2], f1 = words[p + 3];
      p += 4;
      bindTexture(0, t0, f0);
      bindTexture(1, t1, f1);
      const ns = words[p++];
      const stage = new Int32Array(16);
      for (let s = 0; s < ns; s += 1) {
        const q = p + 7 * s;
        stage.set([words[q], words[q + 1], words[q + 2], words[q + 3]], 8 * s);
        stage.set([words[q + 4], words[q + 5], words[q + 6], 1], 8 * s + 4);
      }
      p += 7 * ns;
      gl.uniform4iv(loc.u_stage, stage);
      gl.uniform4f(loc.u_factor, floats[p], floats[p + 1], floats[p + 2], floats[p + 3]);
      p += 4;
      const flags = words[p++];
      const alphaFn = words[p], alphaRef = floats[p + 1];
      p += 2;
      const src = words[p], dst = words[p + 1];
      p += 2;
      const dfn = words[p], dwrite = words[p + 1];
      p += 2;
      const fog = words[p];
      gl.uniform4f(loc.u_fog_color, floats[p + 1], floats[p + 2], floats[p + 3], floats[p + 4]);
      gl.uniform2f(loc.u_fog_range, floats[p + 5], floats[p + 6]);
      p += 7;
      gl.uniform1f(loc.u_alpha_ref, alphaRef);
      gl.uniform4i(loc.u_misc, alphaFn, 0, fog, (flags & 1) | (flags & 2) | (t0 ? 4 : 0) | (t1 ? 8 : 0));
      gl.uniform4f(loc.u_snap, w / vw, h / vh, (flags & 4) && (w !== vw || h !== vh) ? 1 : 0, flags >> 8);
      if (src) {
        gl.enable(gl.BLEND);
        gl.blendFunc(blendFactor(src), blendFactor(dst));
      } else {
        gl.disable(gl.BLEND);
      }
      if (dfn) {
        gl.enable(gl.DEPTH_TEST);
        gl.depthFunc(depthFn[dfn] ?? gl.LEQUAL);
        gl.depthMask(!!dwrite);
      } else {
        gl.disable(gl.DEPTH_TEST);
        gl.depthMask(false);
      }
      if (n) gl.drawArrays(gl.TRIANGLES, 0, n);
    }
  }

  const imports = {
    gl_init: () => (gl ? 1 : init()),
    gl_max_texture: () => (gl ? gl.getParameter(gl.MAX_TEXTURE_SIZE) : 2048),
    gl_texture(id, w, h, ptr) {
      const t = gl.createTexture();
      gl.activeTexture(gl.TEXTURE0);
      gl.bindTexture(gl.TEXTURE_2D, t);
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE,
        new Uint8Array(memory().buffer, ptr, w * h * 4));
      textures.set(id, t);
    },
    gl_texture_drop(id) {
      const t = textures.get(id);
      if (t) gl.deleteTexture(t);
      textures.delete(id);
    },
    gl_render(cmd, len, rgba, w, h) {
      drawCommands(cmd, len, rgba, w, h);
      gl.readPixels(0, 0, w, h, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array(memory().buffer, rgba, w * h * 4));
    },
    gl_draw: (cmd, len, rgba, w, h, vw, vh) => drawCommands(cmd, len, rgba, w, h, vw, vh),
    gl_pass(n, u) {
      const [w, h] = size;
      scratch = picture(scratch, w, h);
      runPass(n, new Uint32Array(memory().buffer, u, 16), color, scratch.fbo, w, h, null, null);
      // The scratch is the target now, the target's colour the scratch.
      const t = scratch.tex;
      scratch.tex = color;
      color = t;
      gl.bindFramebuffer(gl.FRAMEBUFFER, scratch.fbo);
      gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, scratch.tex, 0);
      gl.bindFramebuffer(gl.FRAMEBUFFER, fbo);
      gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, color, 0);
    },
    gl_read_guest(u, fromFront, out, gw, gh) {
      const src = fromFront ? front : { tex: color, w: size[0], h: size[1] };
      if (!src) return;
      if (src.w === gw && src.h === gh) {
        readTexture(src.tex, gw, gh, out);
        return;
      }
      low = picture(low, gw, gh);
      runPass(4, new Uint32Array(memory().buffer, u, 16), src.tex, low.fbo, gw, gh, null, null);
      readTexture(low.tex, gw, gh, out);
    },
    gl_read_target(out) {
      gl.bindFramebuffer(gl.FRAMEBUFFER, fbo);
      gl.readPixels(0, 0, size[0], size[1], gl.RGBA, gl.UNSIGNED_BYTE,
        new Uint8Array(memory().buffer, out, size[0] * size[1] * 4));
    },
    gl_keep_front() {
      const [w, h] = size;
      front = picture(front, w, h);
      gl.bindFramebuffer(gl.READ_FRAMEBUFFER, fbo);
      gl.bindFramebuffer(gl.DRAW_FRAMEBUFFER, front.fbo);
      gl.blitFramebuffer(0, 0, w, h, 0, 0, w, h, gl.COLOR_BUFFER_BIT, gl.NEAREST);
      gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    },
    gl_present(u, lut, list, n) {
      const [w, h] = size;
      const buf = memory().buffer;
      const lutTex = lut ? textures.get(lut) : null;
      shown = picture(shown, w, h);
      runPass(5, new Uint32Array(buf, u, 16), color, shown.fbo, w, h, lutTex, null);
      const words = new Uint32Array(buf, list, 17 * n);
      for (let i = 0; i < n; i += 1) {
        const img = textures.get(words[17 * i]);
        if (img) runPass(6, words.subarray(17 * i + 1, 17 * i + 17), color, shown.fbo, w, h, lutTex, img);
      }
      // On the canvas, the right way up.
      const c = gl.canvas;
      if (c.width !== w || c.height !== h) { c.width = w; c.height = h; }
      gl.bindFramebuffer(gl.READ_FRAMEBUFFER, shown.fbo);
      gl.bindFramebuffer(gl.DRAW_FRAMEBUFFER, null);
      gl.blitFramebuffer(0, 0, w, h, 0, h, w, 0, gl.COLOR_BUFFER_BIT, gl.NEAREST);
      gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    },
    gl_read_present(out, w, h) {
      if (shown && shown.w === w && shown.h === h) readTexture(shown.tex, w, h, out);
    },
    gl_wait: () => gl.finish(),
  };
  return {
    imports,
    canvas: () => gl && gl.canvas,
    readPresent(w, h) {
      const out = new Uint8Array(w * h * 4);
      if (!shown || shown.w !== w || shown.h !== h) return out;
      gl.bindFramebuffer(gl.FRAMEBUFFER, shown.fbo);
      gl.readPixels(0, 0, w, h, gl.RGBA, gl.UNSIGNED_BYTE, out);
      return out;
    },
  };
}
