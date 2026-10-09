//! OpenGL (3.3 core on the Mac, ES 3.0 on the Pi). Two programs: one draws
//! waveform panels straight from a data texture, the other draws every
//! other shape and glyph in a single batch.

use anyhow::{Result, anyhow};
use glow::HasContext;

use crate::draw::{Fill, Frame, Vertex, count_f32, quad_vertices};
use kdj_engine::track::Bands;

/// Data textures wrap at this width (safe on the Pi's GPU).
const TEX_W: usize = 4096;

const QUAD_VS: &str = r"
layout(location = 0) in vec2 a_pos;
layout(location = 1) in vec2 a_uv;
layout(location = 2) in vec4 a_color;
layout(location = 3) in vec4 a_fill;
uniform vec2 u_screen;
out vec2 v_uv;
out vec2 v_pos;
out vec4 v_color;
flat out float v_fill;
void main() {
    v_uv = a_uv;
    v_pos = a_pos;
    v_color = a_color;
    v_fill = a_fill.x;
    vec2 p = a_pos / u_screen * 2.0 - 1.0;
    gl_Position = vec4(p.x, -p.y, 0.0, 1.0);
}
";

const QUAD_FS: &str = r"
uniform sampler2D u_atlas;
uniform float u_scale;
in vec2 v_uv;
in vec2 v_pos;
in vec4 v_color;
flat in float v_fill;
out vec4 o;
void main() {
    vec4 c = v_color;
    if (v_fill > 1.5) {
        // Hatch: diagonal stripes, 3 px on, 5 px off.
        if (mod(v_pos.x + v_pos.y, 8.0) > 3.0) discard;
    } else if (v_fill > 0.5) {
        c.a *= texture(u_atlas, v_uv).r;
    }
    o = c;
}
";

/// Pioneer 3-band: lows blue, mids amber, brown where they overlap, highs
/// white on top.
const WAVE_FS: &str = r"
uniform sampler2D u_tex;
uniform vec4 u_rect;
uniform float u_col0;
uniform float u_cpp;
uniform float u_played;
uniform float u_ncols;
in vec2 v_pos;
out vec4 o;
const vec3 LOW = vec3(0.12, 0.31, 0.85);
const vec3 MID = vec3(0.85, 0.56, 0.18);
const vec3 HIGH = vec3(0.95, 0.95, 0.95);
// Where lows and mids overlap: a dark brown, well below both LOW and MID in
// brightness, so it reads apart from the blue without relying on hue.
const vec3 OVERLAP = vec3(0.32, 0.19, 0.08);
vec3 bands(float col) {
    if (col < 0.0 || col >= u_ncols) return vec3(0.0);
    int c = int(col);
    return texelFetch(u_tex, ivec2(c % 4096, c / 4096), 0).rgb;
}
void main() {
    float c0 = u_col0 + (v_pos.x - u_rect.x) * u_cpp;
    float span = max(u_cpp, 1.0);
    int taps = int(clamp(ceil(span), 1.0, 8.0));
    vec3 b = vec3(0.0);
    for (int i = 0; i < 8; i++) {
        if (i >= taps) break;
        b = max(b, bands(c0 + span * (float(i) + 0.5) / float(taps)));
    }
    float d = abs(v_pos.y - (u_rect.y + u_rect.w * 0.5)) / (u_rect.w * 0.5);
    vec3 col;
    if (d <= b.b) col = HIGH;
    else if (d <= b.g && d <= b.r) col = OVERLAP;
    else if (d <= b.g) col = MID;
    else if (d <= b.r) col = LOW;
    else discard;
    if (c0 < u_played) col *= 0.42;
    o = vec4(col, 1.0);
}
";

#[derive(Debug)]
struct DataTex {
    tex: glow::Texture,
    cols: usize,
}

/// The quad program's per-frame uniforms, looked up once: glow builds a
/// `CString` for every lookup, which would allocate each frame.
#[derive(Debug)]
struct QuadUniforms {
    screen: Option<glow::UniformLocation>,
    scale: Option<glow::UniformLocation>,
}

/// The waveform program's per-frame and per-panel uniforms.
#[derive(Debug)]
struct WaveUniforms {
    screen: Option<glow::UniformLocation>,
    rect: Option<glow::UniformLocation>,
    col0: Option<glow::UniformLocation>,
    cpp: Option<glow::UniformLocation>,
    played: Option<glow::UniformLocation>,
    ncols: Option<glow::UniformLocation>,
}

/// The offscreen framebuffer for snapshots, kept while its size holds.
#[derive(Debug)]
struct Target {
    fbo: glow::Framebuffer,
    tex: glow::Texture,
    size: [i32; 2],
}

#[derive(Debug)]
pub struct Gpu {
    pub gl: glow::Context,
    quad: glow::Program,
    wave: glow::Program,
    quad_u: QuadUniforms,
    wave_u: WaveUniforms,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    vbo_capacity: usize,
    atlas: glow::Texture,
    detail: Option<DataTex>,
    overview: Option<DataTex>,
    staging: Vec<Vertex>,
    offscreen: Option<Target>,
}

impl Gpu {
    /// `es`: the context is OpenGL ES (the Pi) rather than desktop GL.
    ///
    /// # Errors
    ///
    /// If a shader fails to compile or link, or GL can't create an object.
    pub fn new(gl: glow::Context, es: bool) -> Result<Self> {
        let header =
            if es { "#version 300 es\nprecision highp float;\nprecision highp int;\n" } else { "#version 330 core\n" };
        let stride = i32::try_from(size_of::<Vertex>())?;
        // SAFETY: the caller hands over a context that is current on this
        // thread; every object used here was just created on it, and the
        // attribute offsets match `Vertex`'s `repr(C)` layout.
        unsafe {
            let quad = program(&gl, header, QUAD_VS, QUAD_FS)?;
            let wave = program(&gl, header, QUAD_VS, WAVE_FS)?;
            let vao = gl.create_vertex_array().map_err(|e| anyhow!(e))?;
            let vbo = gl.create_buffer().map_err(|e| anyhow!(e))?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, stride, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, stride, 8);
            gl.enable_vertex_attrib_array(2);
            gl.vertex_attrib_pointer_f32(2, 4, glow::UNSIGNED_BYTE, true, stride, 16);
            gl.enable_vertex_attrib_array(3);
            gl.vertex_attrib_pointer_f32(3, 4, glow::UNSIGNED_BYTE, false, stride, 20);
            let atlas = gl.create_texture().map_err(|e| anyhow!(e))?;
            let loc = |p, name| gl.get_uniform_location(p, name);
            let quad_u = QuadUniforms { screen: loc(quad, "u_screen"), scale: loc(quad, "u_scale") };
            let wave_u = WaveUniforms {
                screen: loc(wave, "u_screen"),
                rect: loc(wave, "u_rect"),
                col0: loc(wave, "u_col0"),
                cpp: loc(wave, "u_cpp"),
                played: loc(wave, "u_played"),
                ncols: loc(wave, "u_ncols"),
            };
            // Both programs sample texture unit 0, for good.
            for (p, sampler) in [(quad, "u_atlas"), (wave, "u_tex")] {
                gl.use_program(Some(p));
                gl.uniform_1_i32(loc(p, sampler).as_ref(), 0);
            }
            Ok(Self {
                gl,
                quad,
                wave,
                quad_u,
                wave_u,
                vao,
                vbo,
                vbo_capacity: 0,
                atlas,
                detail: None,
                overview: None,
                staging: Vec::new(),
                offscreen: None,
            })
        }
    }

    pub fn upload_atlas(&mut self, width: usize, height: usize, pixels: &[u8]) {
        // SAFETY: our context is current on this thread (see `Gpu::new`), the
        // atlas texture belongs to it, and `pixels` outlives the upload call.
        unsafe {
            let gl = &self.gl;
            gl.bind_texture(glow::TEXTURE_2D, Some(self.atlas));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                gl_enum(glow::R8),
                gl_int(width),
                gl_int(height),
                0,
                glow::RED,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(pixels)),
            );
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, gl_enum(glow::LINEAR));
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, gl_enum(glow::LINEAR));
        }
    }

    /// Replace the waveform textures. If GL can't make one, that panel is
    /// left blank rather than taking the deck down.
    pub fn upload_waveforms(&mut self, detail: &[Bands], overview: &[Bands]) {
        let d = self.data_texture(detail).inspect_err(|e| eprintln!("detail waveform texture: {e}")).ok();
        let o = self.data_texture(overview).inspect_err(|e| eprintln!("overview waveform texture: {e}")).ok();
        let old = [std::mem::replace(&mut self.detail, d), std::mem::replace(&mut self.overview, o)];
        for old in old.into_iter().flatten() {
            // SAFETY: our context is current on this thread and the old
            // textures were created on it; nothing refers to them any more.
            unsafe { self.gl.delete_texture(old.tex) };
        }
    }

    fn data_texture(&self, cols: &[Bands]) -> Result<DataTex> {
        let rows = cols.len().div_ceil(TEX_W).max(1);
        let mut data = vec![0u8; TEX_W * rows * 4];
        for (i, b) in cols.iter().enumerate() {
            data[i * 4..i * 4 + 3].copy_from_slice(&[b.low, b.mid, b.high]);
        }
        // SAFETY: our context is current on this thread, and `data` holds
        // exactly TEX_W × rows RGBA texels and outlives the upload call.
        unsafe {
            let gl = &self.gl;
            let tex = gl.create_texture().map_err(|e| anyhow!(e))?;
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                gl_enum(glow::RGBA8),
                gl_int(TEX_W),
                gl_int(rows),
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&data)),
            );
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, gl_enum(glow::NEAREST));
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, gl_enum(glow::NEAREST));
            Ok(DataTex { tex, cols: cols.len() })
        }
    }

    /// Draw `frame` (in logical pixels) into `viewport` (device pixels).
    /// Allocates nothing once the staging and vertex buffers have grown to
    /// fit the busiest frame.
    pub fn draw(&mut self, frame: &Frame, viewport: [i32; 4], logical: [f32; 2], scale: f32) {
        self.stage(frame);
        let [x, y, w, h] = viewport;
        // SAFETY: our context is current on this thread (see `Gpu::new`).
        unsafe {
            let gl = &self.gl;
            gl.viewport(x, y, w, h);
            // Only clear our part of the window: two decks share one.
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(x, y, w, h);
            gl.clear_color(11.0 / 255.0, 11.0 / 255.0, 11.0 / 255.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
        }
        self.upload_staging();
        self.draw_waves(frame, logical);
        self.draw_shapes(frame, logical, scale);
        // SAFETY: as above.
        unsafe { self.gl.disable(glow::SCISSOR_TEST) };
    }

    /// Lay out this frame's vertices: one quad per waveform panel, then the shapes.
    fn stage(&mut self, frame: &Frame) {
        self.staging.clear();
        for w in &frame.waves {
            self.staging.extend_from_slice(&quad_vertices(w.rect, [0.0; 4], [0; 4], Fill::Solid));
        }
        self.staging.extend_from_slice(&frame.verts);
    }

    /// Copy the staged vertices into the vertex buffer, growing it if needed.
    fn upload_staging(&mut self) {
        // SAFETY: `Vertex` is `repr(C)` plain old data (f32s and u8s, no
        // padding), so its bytes may be read as `u8`; the slice covers exactly
        // the staged vertices and is dropped before `staging` changes.
        let bytes = unsafe {
            std::slice::from_raw_parts(self.staging.as_ptr().cast::<u8>(), self.staging.len() * size_of::<Vertex>())
        };
        // SAFETY: our context is current on this thread and the VAO and VBO
        // belong to it; `bytes` outlives the upload call and fits the buffer.
        unsafe {
            let gl = &self.gl;
            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            if bytes.len() > self.vbo_capacity {
                self.vbo_capacity = bytes.len().next_power_of_two();
            }
            // Orphan the old storage before writing: the GPU may still be
            // reading last frame's vertices, and a plain sub-data update
            // would wait for it (a known stall on the Pi's v3d driver).
            gl.buffer_data_size(glow::ARRAY_BUFFER, gl_int(self.vbo_capacity), glow::STREAM_DRAW);
            gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, 0, bytes);
        }
    }

    /// The waveform panels, straight from their data textures.
    fn draw_waves(&self, frame: &Frame, logical: [f32; 2]) {
        // SAFETY: our context is current on this thread; the program and
        // textures belong to it, and each panel's quad was staged first.
        unsafe {
            let gl = &self.gl;
            gl.disable(glow::BLEND);
            let u = &self.wave_u;
            gl.use_program(Some(self.wave));
            gl.uniform_2_f32(u.screen.as_ref(), logical[0], logical[1]);
            gl.active_texture(glow::TEXTURE0);
            for (i, w) in frame.waves.iter().enumerate() {
                let Some(t) = (if w.overview { &self.overview } else { &self.detail }) else { continue };
                gl.bind_texture(glow::TEXTURE_2D, Some(t.tex));
                let [rx, ry, rw, rh] = w.rect;
                gl.uniform_4_f32(u.rect.as_ref(), rx, ry, rw, rh);
                gl.uniform_1_f32(u.col0.as_ref(), w.col0);
                gl.uniform_1_f32(u.cpp.as_ref(), w.cols_per_px);
                gl.uniform_1_f32(u.played.as_ref(), w.played_col);
                gl.uniform_1_f32(u.ncols.as_ref(), count_f32(t.cols));
                gl.draw_arrays(glow::TRIANGLES, gl_int(i * 6), 6);
            }
        }
    }

    /// Every other shape and glyph, blended, in one draw call.
    fn draw_shapes(&self, frame: &Frame, logical: [f32; 2], scale: f32) {
        // SAFETY: our context is current on this thread; the program and
        // atlas belong to it, and the shapes follow the panel quads in the
        // staged buffer.
        unsafe {
            let gl = &self.gl;
            gl.enable(glow::BLEND);
            gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
            gl.use_program(Some(self.quad));
            gl.uniform_2_f32(self.quad_u.screen.as_ref(), logical[0], logical[1]);
            gl.uniform_1_f32(self.quad_u.scale.as_ref(), scale);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.atlas));
            gl.draw_arrays(glow::TRIANGLES, gl_int(frame.waves.len() * 6), gl_int(frame.verts.len()));
        }
    }

    /// Render into an offscreen buffer and read it back as RGBA rows, top first.
    ///
    /// # Errors
    ///
    /// If GL can't create the framebuffer or its texture.
    pub fn render_offscreen(&mut self, frame: &Frame, logical: [f32; 2], scale: f32) -> Result<(u32, u32, Vec<u8>)> {
        let (w, h) = (device_px(logical[0] * scale), device_px(logical[1] * scale));
        let fbo = self.offscreen_target([w, h])?;
        // SAFETY: our context is current on this thread and the framebuffer
        // belongs to it.
        unsafe { self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo)) };
        self.draw(frame, [0, 0, w, h], logical, scale);
        let (wu, hu) = (u32::try_from(w)?, u32::try_from(h)?);
        let row = usize::try_from(wu)? * 4;
        let mut px = vec![0u8; row * usize::try_from(hu)?];
        // SAFETY: our context is current on this thread, and `px` holds
        // exactly w × h RGBA pixels for the read to fill.
        unsafe {
            let gl = &self.gl;
            gl.read_pixels(0, 0, w, h, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut px)));
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        // GL reads bottom-up.
        let flipped: Vec<u8> = px.chunks_exact(row).rev().flatten().copied().collect();
        Ok((wu, hu, flipped))
    }

    /// The offscreen framebuffer at `size`, reusing the last one if it fits
    /// and deleting it if not, so repeated snapshots don't leak GPU memory.
    fn offscreen_target(&mut self, size: [i32; 2]) -> Result<glow::Framebuffer> {
        if let Some(t) = self.offscreen.as_ref().filter(|t| t.size == size) {
            return Ok(t.fbo);
        }
        // SAFETY: our context is current on this thread; the old target was
        // created on it and nothing else refers to it, and the new texture is
        // allocated here with no initial data to read.
        unsafe {
            let gl = &self.gl;
            if let Some(old) = self.offscreen.take() {
                gl.delete_framebuffer(old.fbo);
                gl.delete_texture(old.tex);
            }
            let fbo = gl.create_framebuffer().map_err(|e| anyhow!(e))?;
            let tex = match gl.create_texture() {
                Ok(t) => t,
                Err(e) => {
                    gl.delete_framebuffer(fbo);
                    return Err(anyhow!(e));
                }
            };
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                gl_enum(glow::RGBA8),
                size[0],
                size[1],
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex), 0);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            self.offscreen = Some(Target { fbo, tex, size });
            Ok(fbo)
        }
    }

    /// Clear the whole surface (the letterbox around the decks).
    pub fn clear_all(&self, size: (u32, u32)) {
        // SAFETY: our context is current on this thread (see `Gpu::new`).
        unsafe {
            self.gl.viewport(0, 0, gl_int(size.0), gl_int(size.1));
            self.gl.clear_color(0.0, 0.0, 0.0, 1.0);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
        }
    }

    /// Block until the GPU has finished, for timing.
    pub fn finish(&self) {
        // SAFETY: our context is current on this thread (see `Gpu::new`).
        unsafe { self.gl.finish() }
    }
}

/// A size or count as GL's signed int. Ours are far below `i32::MAX`;
/// anything larger saturates rather than wrapping negative.
fn gl_int(n: impl TryInto<i32>) -> i32 {
    n.try_into().unwrap_or(i32::MAX)
}

/// A GL enum where the API wants a `GLint` (internal formats, filter modes).
#[expect(clippy::cast_possible_wrap, reason = "GL enum values are all below 2^31, so they never wrap")]
const fn gl_enum(e: u32) -> i32 {
    e as i32
}

/// Logical size × scale as whole device pixels.
#[expect(
    clippy::cast_possible_truncation,
    reason = "dropping the fraction of a device pixel is intended; `as` saturates"
)]
const fn device_px(v: f32) -> i32 {
    v as i32
}

/// # Safety
///
/// `gl` must be current on this thread.
unsafe fn program(gl: &glow::Context, header: &str, vs: &str, fs: &str) -> Result<glow::Program> {
    // SAFETY: upheld by the caller; every object used is created here.
    unsafe {
        let p = gl.create_program().map_err(|e| anyhow!(e))?;
        let mut shaders = Vec::new();
        for (kind, src) in [(glow::VERTEX_SHADER, vs), (glow::FRAGMENT_SHADER, fs)] {
            let s = gl.create_shader(kind).map_err(|e| anyhow!(e))?;
            gl.shader_source(s, &format!("{header}{src}"));
            gl.compile_shader(s);
            if !gl.get_shader_compile_status(s) {
                return Err(anyhow!("shader: {}", gl.get_shader_info_log(s)));
            }
            gl.attach_shader(p, s);
            shaders.push(s);
        }
        gl.link_program(p);
        if !gl.get_program_link_status(p) {
            return Err(anyhow!("link: {}", gl.get_program_info_log(p)));
        }
        for s in shaders {
            gl.detach_shader(p, s);
            gl.delete_shader(s);
        }
        Ok(p)
    }
}
