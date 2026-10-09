//! A frame as data: coloured triangles plus up to two waveform panels. Built
//! on the CPU each frame into reused buffers, then handed to the GPU in one go.

pub type Rgba = [u8; 4];

#[must_use]
pub const fn grey(v: u8) -> Rgba {
    [v, v, v, 255]
}

#[must_use]
pub const fn alpha(c: Rgba, a: u8) -> Rgba {
    [c[0], c[1], c[2], a]
}

/// How the fragment shader treats a vertex.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Fill {
    Solid = 0,
    /// Coverage from the glyph atlas.
    Glyph = 1,
    /// Diagonal stripes, for loop regions.
    Hatch = 2,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Vertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: Rgba,
    pub fill: [u8; 4],
}

/// A waveform panel: which texture, which columns land where.
#[derive(Clone, Copy, Debug)]
pub struct Wave {
    pub overview: bool,
    /// x, y, w, h in logical pixels.
    pub rect: [f32; 4],
    /// Column at the left edge.
    pub col0: f32,
    pub cols_per_px: f32,
    /// Columns before this are drawn dimmed (already played).
    pub played_col: f32,
}

#[derive(Default, Debug)]
pub struct Frame {
    pub waves: Vec<Wave>,
    pub verts: Vec<Vertex>,
}

impl Frame {
    pub fn clear(&mut self) {
        self.waves.clear();
        self.verts.clear();
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Rgba) {
        self.quad([x, y, w, h], [0.0; 4], color, Fill::Solid);
    }

    pub fn hatch(&mut self, x: f32, y: f32, w: f32, h: f32, color: Rgba) {
        self.quad([x, y, w, h], [0.0; 4], color, Fill::Hatch);
    }

    /// A rectangle outline (x, y, w, h) drawn inside the given bounds.
    pub fn outline(&mut self, [x, y, w, h]: [f32; 4], thickness: f32, color: Rgba) {
        let side = (-2.0f32).mul_add(thickness, h);
        self.rect(x, y, w, thickness, color);
        self.rect(x, y + h - thickness, w, thickness, color);
        self.rect(x, y + thickness, thickness, side, color);
        self.rect(x + w - thickness, y + thickness, thickness, side, color);
    }

    pub fn tri(&mut self, a: [f32; 2], b: [f32; 2], c: [f32; 2], col: Rgba) {
        for pos in [a, b, c] {
            self.verts.push(Vertex { pos, uv: [0.0; 2], color: col, fill: [Fill::Solid as u8, 0, 0, 0] });
        }
    }

    /// `rect` is [x, y, w, h]; `uv` is [u0, v0, u1, v1].
    pub fn quad(&mut self, rect: [f32; 4], uv: [f32; 4], color: Rgba, fill: Fill) {
        self.verts.extend_from_slice(&quad_vertices(rect, uv, color, fill));
    }
}

/// The two triangles of a quad. `rect` is [x, y, w, h]; `uv` is [u0, v0, u1, v1].
#[must_use]
pub fn quad_vertices([x, y, w, h]: [f32; 4], uv: [f32; 4], color: Rgba, fill: Fill) -> [Vertex; 6] {
    let fill = [fill as u8, 0, 0, 0];
    let vertex = |px, py, tu, tv| Vertex { pos: [px, py], uv: [tu, tv], color, fill };
    let (x1, y1) = (x + w, y + h);
    [
        vertex(x, y, uv[0], uv[1]),
        vertex(x1, y, uv[2], uv[1]),
        vertex(x, y1, uv[0], uv[3]),
        vertex(x1, y, uv[2], uv[1]),
        vertex(x1, y1, uv[2], uv[3]),
        vertex(x, y1, uv[0], uv[3]),
    ]
}

// Numeric conversions for drawing. Geometry is `f32`, counts are `usize` and
// track time is `f64`; the deliberate lossy casts between them live here.

/// A count or index as `f32`, for placing things on screen.
#[expect(
    clippy::cast_precision_loss,
    reason = "list lengths, atlas coordinates and waveform columns stay far below 2^24, where f32 is exact"
)]
pub(crate) const fn count_f32(n: usize) -> f32 {
    n as f32
}

/// A frame count or audio clock as `f64`.
#[expect(
    clippy::cast_precision_loss,
    reason = "frame counts and clocks stay far below 2^53 (years of audio), where f64 is exact"
)]
pub(crate) const fn count_f64(n: u64) -> f64 {
    n as f64
}

/// A track-time quantity narrowed to `f32` for drawing.
#[expect(
    clippy::cast_possible_truncation,
    reason = "screen positions and waveform columns only need f32 precision; out-of-range values saturate"
)]
pub(crate) const fn narrow(v: f64) -> f32 {
    v as f32
}

/// Whole units of a non-negative quantity, for clock readouts.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "readouts are non-negative and far below u32::MAX; truncating toward zero is how a clock counts"
)]
pub(crate) const fn whole(v: f64) -> u32 {
    v as u32
}
