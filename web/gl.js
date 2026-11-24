// SPDX-License-Identifier: MIT
// Direct3D 7's fixed-function pixel pipeline on WebGL2, for the browser player: the batches the
// runtime's renderer writes (platform/src/gpu/web.rs has the format) are drawn over the render
// target's pixels and read back into guest memory, as the native wgpu renderer does
// (platform/src/gpu/native.rs, whose WGSL shader this one follows).
//
// The image is kept upside down on the GPU (row 0 of the guest's surface is row 0 of the
// framebuffer), so that uploading and reading back need no flip.

const VS = `#version 300 es
precision highp float;
layout(location = 0) in vec4 a_pos;
layout(location = 1) in vec4 a_diffuse;
layout(location = 2) in vec4 a_specular;
layout(location = 3) in vec2 a_uv0;
layout(location = 4) in vec2 a_uv1;
layout(location = 5) in float a_eye;
uniform vec2 u_view;
out vec4 v_diffuse;
out vec4 v_specular;
out vec2 v_uv0;
out vec2 v_uv1;
out float v_eye;
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
}`;

const FS = `#version 300 es
precision highp float;
precision highp int;
in vec4 v_diffuse;
in vec4 v_specular;
in vec2 v_uv0;
in vec2 v_uv1;
in float v_eye;
uniform sampler2D u_t0;
uniform sampler2D u_t1;
uniform ivec4 u_stage[4];
uniform vec4 u_factor;
uniform vec4 u_fog_color;
uniform vec2 u_fog_range;
uniform ivec4 u_misc; // alpha func, -, fog mode, flags (1 specular, 2 keyed, 4 tex0, 8 tex1)
uniform float u_alpha_ref;
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
  vec4 c0 = texture(u_t0, u_stage[1].z == 1 ? v_uv1 : v_uv0);
  vec4 c1 = texture(u_t1, u_stage[3].z == 1 ? v_uv1 : v_uv0);
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

/* The renderer's imports for the runtime (gl_init, gl_texture, gl_texture_drop, gl_render);
 * `memory()` gives the runtime's memory. Without WebGL2, gl_init answers 0 and the runtime
 * draws in software. */
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
    gl = canvas.getContext('webgl2', { antialias: false, depth: false, alpha: true, premultipliedAlpha: false });
    if (!gl) return 0;
    try {
      prog = gl.createProgram();
      gl.attachShader(prog, compile(gl.VERTEX_SHADER, VS));
      gl.attachShader(prog, compile(gl.FRAGMENT_SHADER, FS));
      gl.linkProgram(prog);
      if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(prog));
    } catch (e) {
      console.warn('WebGL2 renderer:', e);
      gl = null;
      return 0;
    }
    gl.useProgram(prog);
    loc = Object.fromEntries(['u_view', 'u_t0', 'u_t1', 'u_stage', 'u_factor', 'u_fog_color', 'u_fog_range',
      'u_misc', 'u_alpha_ref'].map((n) => [n, gl.getUniformLocation(prog, n)]));
    gl.uniform1i(loc.u_t0, 0);
    gl.uniform1i(loc.u_t1, 1);
    vbuf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, vbuf);
    const stride = 80;
    const attr = (i, n, off) => { gl.enableVertexAttribArray(i); gl.vertexAttribPointer(i, n, gl.FLOAT, false, stride, off); };
    attr(0, 4, 0); attr(1, 4, 16); attr(2, 4, 32); attr(3, 2, 48); attr(4, 2, 56); attr(5, 1, 64);
    white = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, white);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, 1, 1, 0, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array([255, 255, 255, 255]));
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    gl.pixelStorei(gl.PACK_ALIGNMENT, 1);
    console.log('[gl] WebGL2 renderer');
    return 1;
  }

  function target(w, h) {
    if (size[0] === w && size[1] === h) return;
    size = [w, h];
    if (fbo) { gl.deleteFramebuffer(fbo); gl.deleteTexture(color); gl.deleteRenderbuffer(depth); }
    color = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, color);
    gl.texStorage2D(gl.TEXTURE_2D, 1, gl.RGBA8, w, h);
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

  return {
    gl_init: () => (gl ? 1 : init()),
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
      const buf = memory().buffer;
      const words = new Uint32Array(buf, cmd, len);
      const floats = new Float32Array(buf, cmd, len);
      target(w, h);
      gl.bindFramebuffer(gl.FRAMEBUFFER, fbo);
      gl.viewport(0, 0, w, h);
      // The target's pixels as they are in guest memory.
      gl.activeTexture(gl.TEXTURE0);
      gl.bindTexture(gl.TEXTURE_2D, color);
      gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, w, h, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array(buf, rgba, w * h * 4));
      gl.useProgram(prog);
      gl.uniform2f(loc.u_view, w, h);
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
      gl.readPixels(0, 0, w, h, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array(memory().buffer, rgba, w * h * 4));
    },
  };
}
