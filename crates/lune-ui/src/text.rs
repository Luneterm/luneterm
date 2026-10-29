//! Text: a shaped-buffer cache keyed by content, so the UI can be drawn immediate-mode.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use glyphon::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, Weight, Wrap};

use crate::color::Rgba;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Font {
    Ui,
    Mono,
}

/// Font, size and colour for a run of text. Sizes are logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub font: Font,
    pub size: f32,
    pub line: f32,
    pub weight: u16,
    pub color: Rgba,
}

impl Style {
    pub fn ui(size: f32, color: Rgba) -> Self {
        Self {
            font: Font::Ui,
            size,
            line: (size * 1.3).round(),
            weight: 400,
            color,
        }
    }

    pub fn mono(size: f32, line: f32, color: Rgba) -> Self {
        Self {
            font: Font::Mono,
            size,
            line,
            weight: 400,
            color,
        }
    }

    pub fn medium(self) -> Self {
        Self {
            weight: 500,
            ..self
        }
    }

    pub fn bold(self) -> Self {
        Self {
            weight: 700,
            ..self
        }
    }

    fn hash_into(&self, h: &mut impl Hasher) {
        self.font.hash(h);
        self.size.to_bits().hash(h);
        self.line.to_bits().hash(h);
        self.weight.hash(h);
        self.color.hash(h);
    }
}

/// Text to draw: one or more styled runs; `wrap` is a width in logical px.
#[derive(Debug, Clone)]
pub struct Spec {
    pub runs: Vec<(String, Style)>,
    pub wrap: Option<f32>,
}

impl Spec {
    pub fn plain(text: impl Into<String>, style: Style) -> Self {
        Self {
            runs: vec![(text.into(), style)],
            wrap: None,
        }
    }

    pub fn wrapped(mut self, width: f32) -> Self {
        self.wrap = Some(width);
        self
    }

    fn key(&self, scale: f32) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        scale.to_bits().hash(&mut h);
        self.wrap.map(f32::to_bits).hash(&mut h);
        for (text, style) in &self.runs {
            text.hash(&mut h);
            style.hash_into(&mut h);
        }
        h.finish()
    }
}

#[derive(Debug)]
struct Entry {
    buffer: Buffer,
    size: (f32, f32),
    last_used: u64,
}

#[derive(Debug)]
pub struct TextCache {
    pub fonts: FontSystem,
    ui_family: String,
    mono_family: String,
    entries: HashMap<u64, Entry>,
    frame: u64,
    pub scale: f32,
}

impl TextCache {
    /// `font_data` is loaded in order; `ui_family` and `mono_family` are the family names
    /// that [`Font::Ui`] and [`Font::Mono`] resolve to. The caller supplies all of it; this
    /// crate embeds no fonts.
    pub fn new(scale: f32, font_data: &[&[u8]], ui_family: &str, mono_family: &str) -> Self {
        let mut fonts = FontSystem::new();
        let db = fonts.db_mut();
        for data in font_data {
            db.load_font_data(data.to_vec());
        }
        Self {
            fonts,
            ui_family: ui_family.to_owned(),
            mono_family: mono_family.to_owned(),
            entries: HashMap::new(),
            frame: 0,
            scale,
        }
    }

    pub fn set_scale(&mut self, scale: f32) {
        if scale != self.scale {
            self.scale = scale;
            self.entries.clear();
        }
    }

    /// Shapes `spec` if needed; returns its cache key and logical size.
    pub fn prepare(&mut self, spec: &Spec) -> (u64, (f32, f32)) {
        let key = spec.key(self.scale);
        let frame = self.frame;
        if let Some(e) = self.entries.get_mut(&key) {
            e.last_used = frame;
            return (key, e.size);
        }
        let s = self.scale;
        let first = spec
            .runs
            .first()
            .map(|r| r.1)
            .unwrap_or(Style::ui(13.0, [255; 4]));
        let mut buffer = Buffer::new(
            &mut self.fonts,
            Metrics::new(first.size * s, first.line * s),
        );
        buffer.set_size(spec.wrap.map(|w| w * s), None);
        buffer.set_wrap(if spec.wrap.is_some() {
            Wrap::WordOrGlyph
        } else {
            Wrap::None
        });
        let attrs = |st: &Style| {
            let family = match st.font {
                Font::Ui => Family::Name(&self.ui_family),
                Font::Mono => Family::Name(&self.mono_family),
            };
            let [r, g, b, a] = st.color;
            Attrs::new()
                .family(family)
                .weight(Weight(st.weight))
                .color(Color::rgba(r, g, b, a))
        };
        let default = attrs(&first);
        buffer.set_rich_text(
            spec.runs.iter().map(|(t, st)| (t.as_str(), attrs(st))),
            &default,
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut self.fonts, false);
        let mut w: f32 = 0.0;
        let mut lines = 0;
        for run in buffer.layout_runs() {
            w = w.max(run.line_w);
            lines += 1;
        }
        let size = (w / s, lines.max(1) as f32 * first.line);
        self.entries.insert(
            key,
            Entry {
                buffer,
                size,
                last_used: frame,
            },
        );
        (key, size)
    }

    /// The font system (for glyphon's `prepare`) alongside a lookup of shaped buffers.
    pub fn split<'a>(
        &'a mut self,
    ) -> (&'a mut FontSystem, impl Fn(u64) -> Option<&'a Buffer> + 'a) {
        let entries = &self.entries;
        (&mut self.fonts, move |key| {
            entries.get(&key).map(|e| &e.buffer)
        })
    }

    /// Drop buffers unused for a while; call once per frame.
    pub fn end_frame(&mut self) {
        let frame = self.frame;
        self.entries
            .retain(|_, e| frame.saturating_sub(e.last_used) < 180);
        self.frame += 1;
    }
}
