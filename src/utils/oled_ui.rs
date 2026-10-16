//! v4 OLED drawing primitives (pure: no hardware, no heap) — host-testable.
//! Positions replicate the old embedded-layout / embedded-text output pixel for pixel.

use core::fmt::Write;
use embedded_graphics::{
    Drawable,
    geometry::AnchorPoint,
    image::{Image, ImageRaw},
    mono_font::{MonoFont, MonoTextStyle, ascii},
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Line, PrimitiveStyle, Rectangle},
    text::{Alignment, Baseline, Text, TextStyle, TextStyleBuilder},
};

pub const MAIN_RECT: Rectangle = Rectangle::new(Point::new(0, 11), Size::new(128, 53));
#[cfg_attr(any(feature = "e2e", feature = "qa"), allow(dead_code))]
const SCREEN_RECT: Rectangle = Rectangle::new(Point::zero(), Size::new(128, 64));
/// Timer/inspection digits area (cleared before every digit redraw).
const DIGITS_RECT: Rectangle = Rectangle::new(Point::new(0, 28), Size::new(128, 17));
pub const DETAILS_VISIBLE_LINES: usize = 5;

pub const NORMAL_FONT: MonoTextStyle<'_, BinaryColor> =
    MonoTextStyle::new(&ascii::FONT_7X13, BinaryColor::On);
pub const SMALL_FONT: MonoTextStyle<'_, BinaryColor> =
    MonoTextStyle::new(&ascii::FONT_6X9, BinaryColor::On);
const SMALL_FONT_INV: MonoTextStyle<'_, BinaryColor> =
    MonoTextStyle::new(&ascii::FONT_6X9, BinaryColor::Off);
pub const TIMER_FONT: MonoTextStyle<'_, BinaryColor> =
    MonoTextStyle::new(&profont::PROFONT_14_POINT, BinaryColor::On);
pub const SMALL_TIMER_FONT: MonoTextStyle<'_, BinaryColor> =
    MonoTextStyle::new(&profont::PROFONT_7_POINT, BinaryColor::On);

const TEXT_CENTER: TextStyle = TextStyleBuilder::new()
    .alignment(Alignment::Center)
    .baseline(Baseline::Middle)
    .build();
const TEXT_LEFT_MIDDLE: TextStyle = TextStyleBuilder::new()
    .alignment(Alignment::Left)
    .baseline(Baseline::Middle)
    .build();
const TEXT_TOP: TextStyle = TextStyleBuilder::new()
    .alignment(Alignment::Left)
    .baseline(Baseline::Top)
    .build();

// 1bpp, MSB first, rows padded to bytes (generated once from the old src/resources/*.png)
const ICON_CHARGING: ImageRaw<BinaryColor> =
    ImageRaw::new(&[0x08, 0x10, 0x20, 0x40, 0xF8, 0x10, 0xA0, 0xC0, 0xE0], 5);
const ICON_SERVER: ImageRaw<BinaryColor> =
    ImageRaw::new(&[0xFE, 0x82, 0x8A, 0xFE, 0x82, 0x8A, 0xFE], 7);
#[cfg_attr(feature = "timer-func", allow(dead_code))]
const ICON_TIMER: ImageRaw<BinaryColor> =
    ImageRaw::new(&[0x1E, 0x3C, 0x42, 0x85, 0x99, 0x91, 0x81, 0x42, 0x3C], 8);
const ICON_WIFI: ImageRaw<BinaryColor> = ImageRaw::new(
    &[
        0x1E, 0x00, 0x61, 0x80, 0x8C, 0x40, 0x33, 0x00, 0x40, 0x80, 0x1E, 0x00, 0x21, 0x00, 0x0C,
        0x00, 0x0C, 0x00,
    ],
    10,
);

pub type Line64 = heapless::String<64>;

/// 128×64 1bpp frame in SSD1309 page layout plus a copy of what the panel shows, so a flush
/// only sends the changed column span of each page.
pub struct Frame {
    buf: [u8; 1024],
    shown: [u8; 1024],
    /// `false` = panel content unknown (after init or a failed flush): resend everything
    synced: bool,
}

impl Frame {
    pub const fn new() -> Self {
        Self {
            buf: [0; 1024],
            shown: [0; 1024],
            synced: false,
        }
    }

    pub fn clear(&mut self) {
        self.buf.fill(0);
    }

    pub fn invalidate(&mut self) {
        self.synced = false;
    }

    /// Changed column range `[start, end)` of `page` with its new bytes.
    pub fn dirty(&self, page: usize) -> Option<(u8, &[u8])> {
        let range = page * 128..page * 128 + 128;
        let (new, old) = (&self.buf[range.clone()], &self.shown[range]);
        let (start, end) = if self.synced {
            let start = new.iter().zip(old).position(|(a, b)| a != b)?;
            let end = 128 - new.iter().zip(old).rev().position(|(a, b)| a != b)?;
            (start, end)
        } else {
            (0, 128)
        };
        Some((start as u8, &new[start..end]))
    }

    /// Records that `dirty(page)` reached the panel.
    pub fn sent(&mut self, page: usize) {
        let range = page * 128..page * 128 + 128;
        self.shown[range.clone()].copy_from_slice(&self.buf[range]);
    }

    /// After every page was sent.
    pub fn mark_synced(&mut self) {
        self.synced = true;
    }
}

impl OriginDimensions for Frame {
    fn size(&self) -> Size {
        Size::new(128, 64)
    }
}

impl DrawTarget for Frame {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;

    fn draw_iter<I: IntoIterator<Item = Pixel<BinaryColor>>>(&mut self, pixels: I) -> Result<(), Self::Error> {
        for Pixel(p, color) in pixels {
            if let (Ok(x @ 0..128), Ok(y @ 0..64)) = (usize::try_from(p.x), usize::try_from(p.y)) {
                let (byte, bit) = (&mut self.buf[y / 8 * 128 + x], 1 << (y % 8));
                if color.is_on() {
                    *byte |= bit;
                } else {
                    *byte &= !bit;
                }
            }
        }
        Ok(())
    }
}

/// Offset that centers `bb` inside `r` (same math as embedded-layout's Center alignment).
fn center_offset(bb: Rectangle, r: Rectangle) -> Point {
    r.anchor_point(AnchorPoint::Center) - bb.anchor_point(AnchorPoint::Center)
}

/// Draws `text` (may contain `\n`) as one block centered in `r`.
pub fn centered<D: DrawTarget<Color = BinaryColor>>(
    d: &mut D,
    text: &str,
    style: MonoTextStyle<'_, BinaryColor>,
    r: Rectangle,
) {
    let t = Text::with_text_style(text, Point::zero(), style, TEXT_CENTER);
    let by = center_offset(t.bounding_box(), r);
    _ = t.translate(by).draw(d);
}

pub fn centered_main<D: DrawTarget<Color = BinaryColor>>(d: &mut D, text: &str) {
    centered(d, text, NORMAL_FONT, MAIN_RECT);
}

#[cfg_attr(any(feature = "e2e", feature = "qa"), allow(dead_code))]
pub fn centered_screen<D: DrawTarget<Color = BinaryColor>>(
    d: &mut D,
    text: &str,
    style: MonoTextStyle<'_, BinaryColor>,
) {
    centered(d, text, style, SCREEN_RECT);
}

#[derive(Clone, Copy, PartialEq)]
pub enum VAlign {
    Middle,
    Bottom,
}

/// Greedy word wrap into lines of at most `cols` chars (`\n` forces a break, words longer
/// than a line are split). Yields trimmed slices of `text`.
pub fn wrap<'a>(text: &'a str, cols: usize, mut line: impl FnMut(&'a str)) {
    for para in text.split('\n') {
        let mut cur: Option<(usize, usize)> = None; // byte range of the current line in `para`
        let mut cur_len = 0;
        let mut pos = 0;
        for word in para.split(' ') {
            let start = pos;
            pos += word.len() + 1;
            if word.is_empty() {
                continue;
            }
            let wlen = word.chars().count();
            if let Some((s, _)) = cur {
                if cur_len + 1 + wlen <= cols {
                    cur = Some((s, start + word.len()));
                    cur_len += 1 + wlen;
                    continue;
                }
                line(&para[s..cur.map_or(s, |c| c.1)]);
            }
            // word starts a new line; split it when it is longer than a line
            let mut rest = word;
            let mut rest_start = start;
            while rest.chars().count() > cols {
                let cut = rest.char_indices().nth(cols).map_or(rest.len(), |(i, _)| i);
                line(&rest[..cut]);
                rest = &rest[cut..];
                rest_start += cut;
            }
            cur = Some((rest_start, rest_start + rest.len()));
            cur_len = rest.chars().count();
        }
        line(cur.map_or("", |(s, e)| &para[s..e]));
    }
}

/// Word-wrapped text, each line horizontally centered, block aligned in `r`
/// (replaces embedded-text's centered `TextBox`).
pub fn text_box<D: DrawTarget<Color = BinaryColor>>(
    d: &mut D,
    text: &str,
    r: Rectangle,
    style: MonoTextStyle<'_, BinaryColor>,
    valign: VAlign,
) {
    let font: &MonoFont = style.font;
    let cw = font.character_size.width + font.character_spacing;
    let lh = font.character_size.height as i32;
    let cols = (r.size.width / cw) as usize;

    let mut n = 0;
    wrap(text, cols, |_| n += 1);
    let text_h = n * lh;
    let mut y = r.top_left.y
        + match valign {
            VAlign::Middle => (r.size.height as i32 - text_h) / 2,
            VAlign::Bottom => r.size.height as i32 - text_h,
        };

    // like embedded-text: the block is aligned as a whole, only fully visible rows are drawn
    let bottom = r.top_left.y + r.size.height as i32;
    wrap(text, cols, |l| {
        if y >= r.top_left.y && y + lh <= bottom {
            let w = l.chars().count() as i32 * cw as i32;
            let x = r.top_left.x + (r.size.width as i32 - w + 1) / 2;
            _ = Text::with_text_style(l, Point::new(x, y), style, TEXT_TOP).draw(d);
        }
        y += lh;
    });
}

pub fn text_main<D: DrawTarget<Color = BinaryColor>>(d: &mut D, text: &str) {
    text_box(d, text, MAIN_RECT, NORMAL_FONT, VAlign::Middle);
}

pub struct TopBar<'a> {
    pub battery: (u8, bool),
    pub wifi: bool,
    pub server: bool,
    #[cfg_attr(feature = "timer-func", allow(dead_code))]
    pub stackmat: bool,
    pub title: Option<&'a str>,
}

fn crossed_icon<D: DrawTarget<Color = BinaryColor>>(
    d: &mut D,
    icon: &ImageRaw<BinaryColor>,
    at: Point,
    crossed: bool,
) {
    _ = Image::new(icon, at).draw(d);
    if crossed {
        let size = icon.size();
        let tl = at + Point::new((size.width as i32 - 9) / 2, (size.height as i32 - 9) / 2);
        let stroke = PrimitiveStyle::with_stroke(BinaryColor::On, 1);
        _ = Line::new(tl, tl + Point::new(8, 8)).into_styled(stroke).draw(d);
        _ = Line::new(tl + Point::new(0, 8), tl + Point::new(8, 0))
            .into_styled(stroke)
            .draw(d);
    }
}

pub fn top_bar<D: DrawTarget<Color = BinaryColor>>(d: &mut D, bar: &TopBar) {
    let mut pct: heapless::String<5> = heapless::String::new();
    _ = write!(pct, "{}%", bar.battery.0);

    // battery: right aligned "[charging icon] NN%"
    let text = Text::with_text_style(&pct, Point::zero(), NORMAL_FONT, TEXT_TOP);
    let tb = text.bounding_box();
    let text_x = 128 - tb.size.width as i32;
    _ = text
        .translate(Point::new(text_x, -2) - tb.top_left)
        .draw(d);
    if bar.battery.1 {
        _ = Image::new(&ICON_CHARGING, Point::new(text_x - 1 - 5, 0)).draw(d);
    }

    // connection icons, left aligned with 2 px spacing
    crossed_icon(d, &ICON_WIFI, Point::new(0, 0), !bar.wifi);
    crossed_icon(d, &ICON_SERVER, Point::new(12, 1), !bar.server);
    #[cfg(not(feature = "timer-func"))]
    crossed_icon(d, &ICON_TIMER, Point::new(21, 0), !bar.stackmat);

    if let Some(title) = bar.title {
        _ = Text::with_text_style(title, Point::new(64, 5), SMALL_FONT, TEXT_CENTER).draw(d);
    }

    _ = Line::new(Point::new(0, 10), Point::new(128, 10))
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(d);
}

/// Scrollable list in the main area; `item(i, buf)` writes item `i` (only visible ones
/// are requested).
pub fn menu<D: DrawTarget<Color = BinaryColor>>(
    d: &mut D,
    total: usize,
    selected: usize,
    mut item: impl FnMut(usize, &mut Line64),
) {
    const LINE_HEIGHT: i32 = 10;
    const VISIBLE: usize = 5;
    const PADDING_X: i32 = 4;

    let scroll_start = if selected + 1 >= VISIBLE {
        (selected + 1 - VISIBLE).min(total.saturating_sub(VISIBLE))
    } else {
        0
    };

    let mut buf = Line64::new();
    for row in 0..VISIBLE.min(total.saturating_sub(scroll_start)) {
        let idx = scroll_start + row;
        let y = MAIN_RECT.top_left.y + row as i32 * LINE_HEIGHT;
        let text_y = y + LINE_HEIGHT / 2;
        buf.clear();
        item(idx, &mut buf);

        let style = if idx == selected {
            _ = Rectangle::new(Point::new(0, y), Size::new(128, LINE_HEIGHT as u32))
                .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
                .draw(d);
            SMALL_FONT_INV
        } else {
            SMALL_FONT
        };
        _ = Text::with_text_style(&buf, Point::new(PADDING_X, text_y), style, TEXT_LEFT_MIDDLE)
            .draw(d);

        if scroll_start > 0 && row == 0 {
            _ = Text::with_text_style("^", Point::new(122, text_y), SMALL_FONT, TEXT_CENTER)
                .draw(d);
        }
        if scroll_start + VISIBLE < total && row == VISIBLE - 1 {
            _ = Text::with_text_style("v", Point::new(122, text_y), SMALL_FONT, TEXT_CENTER)
                .draw(d);
        }
    }
}

/// Multi-line text scrolled by `scroll` lines.
pub fn details<D: DrawTarget<Color = BinaryColor>>(d: &mut D, text: &str, scroll: usize) {
    const LINE_HEIGHT: i32 = 10;
    const PADDING_X: i32 = 2;

    let start_y = MAIN_RECT.top_left.y;
    for (row, line) in text
        .split('\n')
        .skip(scroll)
        .take(DETAILS_VISIBLE_LINES)
        .enumerate()
    {
        let text_y = start_y + row as i32 * LINE_HEIGHT + LINE_HEIGHT / 2;
        _ = Text::with_text_style(line, Point::new(PADDING_X, text_y), SMALL_FONT, TEXT_LEFT_MIDDLE)
            .draw(d);
    }

    if scroll > 0 {
        let y = start_y + LINE_HEIGHT / 2;
        _ = Text::with_text_style("^", Point::new(122, y), SMALL_FONT, TEXT_CENTER).draw(d);
    }
    if scroll + DETAILS_VISIBLE_LINES < text.split('\n').count() {
        let y = start_y + (DETAILS_VISIBLE_LINES as i32 - 1) * LINE_HEIGHT + LINE_HEIGHT / 2;
        _ = Text::with_text_style("v", Point::new(122, y), SMALL_FONT, TEXT_CENTER).draw(d);
    }
}

pub enum QrData<'a> {
    Text(&'a str),
    Binary(&'a [u8]),
}

/// Version 6 (41×41 modules) at 1 px per module, centered on the screen.
pub fn qr<D: DrawTarget<Color = BinaryColor>>(d: &mut D, data: QrData<'_>) {
    use qrcodegen_no_heap::{QrCode, QrCodeEcc, Version};
    const MAX: Version = Version::new(6);

    let mut out = [0u8; MAX.buffer_len()];
    let mut tmp = [0u8; MAX.buffer_len()];
    let qr = match data {
        QrData::Text(text) => QrCode::encode_text(
            text,
            &mut tmp,
            &mut out,
            QrCodeEcc::Quartile,
            Version::MIN,
            MAX,
            None,
            true,
        ),
        QrData::Binary(data) => {
            let Some(dst) = tmp.get_mut(..data.len()) else {
                return;
            };
            dst.copy_from_slice(data);
            QrCode::encode_binary(
                &mut tmp,
                data.len(),
                &mut out,
                QrCodeEcc::Quartile,
                Version::MIN,
                MAX,
                None,
                true,
            )
        }
    };
    let Ok(qr) = qr else {
        return;
    };

    let size = qr.size();
    let origin = Point::new((128 - size) / 2, (64 - size) / 2);
    let on = PrimitiveStyle::with_fill(BinaryColor::On);
    for y in 0..size {
        for x in 0..size {
            if qr.get_module(x, y) {
                _ = Rectangle::new(origin + Point::new(x, y), Size::new(1, 1))
                    .into_styled(on)
                    .draw(d);
            }
        }
    }
}

pub fn group_select<D: DrawTarget<Color = BinaryColor>>(d: &mut D, text: &str) {
    for (s, right) in [("<", false), (">", true)] {
        let t = Text::with_text_style(s, Point::zero(), TIMER_FONT, TEXT_CENTER);
        let bb = t.bounding_box();
        let x = if right {
            MAIN_RECT.anchor_point(AnchorPoint::BottomRight).x - bb.anchor_point(AnchorPoint::BottomRight).x
        } else {
            MAIN_RECT.top_left.x - bb.top_left.x
        };
        let y = center_offset(bb, MAIN_RECT).y;
        _ = t.translate(Point::new(x, y)).draw(d);
    }
    text_main(d, text);
}

/// Finished screen: big time (optionally inspection time below) at the top, status at the bottom.
pub fn finished<D: DrawTarget<Color = BinaryColor>>(
    d: &mut D,
    time: &str,
    inspection: Option<&str>,
    status: Option<&str>,
) {
    let font = if time.len() > 13 { NORMAL_FONT } else { TIMER_FONT };
    let main = Text::with_text_style(time, Point::zero(), font, TEXT_CENTER);
    let mb = main.bounding_box();

    match inspection {
        Some(insp) => {
            let it = Text::with_text_style(insp, Point::zero(), SMALL_TIMER_FONT, TEXT_CENTER);
            let ib = it.bounding_box();
            let width = mb.size.width.max(ib.size.width);
            let group = Rectangle::new(Point::zero(), Size::new(width, mb.size.height + ib.size.height));
            let gx = center_offset(group, MAIN_RECT).x;
            let top = MAIN_RECT.top_left.y + 2;
            let mx = gx + (width as i32 - mb.size.width as i32) / 2;
            let ix = gx + (width as i32 - ib.size.width as i32) / 2;
            _ = main.translate(Point::new(mx, top) - mb.top_left).draw(d);
            _ = it
                .translate(Point::new(ix, top + mb.size.height as i32) - ib.top_left)
                .draw(d);
        }
        None => {
            let x = center_offset(mb, MAIN_RECT).x;
            let y = MAIN_RECT.top_left.y + 10 - mb.top_left.y;
            _ = main.translate(Point::new(x, y)).draw(d);
        }
    }

    if let Some(status) = status {
        text_box(d, status, MAIN_RECT, NORMAL_FONT, VAlign::Bottom);
    }
}

/// Timer / inspection digits (fast path: only this area changes between frames).
pub fn digits<D: DrawTarget<Color = BinaryColor>>(d: &mut D, ms: u64) {
    _ = d.fill_solid(&DIGITS_RECT, BinaryColor::Off);
    let time = crate::utils::text::time_str(ms);
    _ = Text::with_text_style(&time, Point::new(64, 36), TIMER_FONT, TEXT_CENTER).draw(d);
}

pub fn inspection_label<D: DrawTarget<Color = BinaryColor>>(d: &mut D, label: &str) {
    _ = Text::with_text_style(label, Point::new(64, 50), NORMAL_FONT, TEXT_CENTER).draw(d);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrapped(text: &str, cols: usize) -> std::vec::Vec<std::string::String> {
        let mut v = std::vec::Vec::new();
        wrap(text, cols, |l| v.push(l.into()));
        v
    }

    #[test]
    fn wrap_breaks_words_and_lines() {
        assert_eq!(wrapped("Scan the card", 18), ["Scan the card"]);
        assert_eq!(wrapped("a\nb", 18), ["a", "b"]);
        assert_eq!(wrapped("hello world foo", 11), ["hello world", "foo"]);
        assert_eq!(wrapped("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrapped("x\n\ny", 4), ["x", "", "y"]);
        assert_eq!(wrapped("zażółć gęślą", 6), ["zażółć", "gęślą"]);
    }

    #[test]
    fn frame_sends_only_changed_spans() {
        let mut f = Frame::new();
        // unknown panel content: every page is sent in full
        assert!((0..8).all(|p| f.dirty(p).is_some_and(|(c, b)| c == 0 && b.len() == 128)));
        (0..8).for_each(|p| f.sent(p));
        f.mark_synced();
        assert!((0..8).all(|p| f.dirty(p).is_none()));

        _ = Pixel(Point::new(10, 20), BinaryColor::On).draw(&mut f);
        _ = Pixel(Point::new(13, 23), BinaryColor::On).draw(&mut f);
        _ = Pixel(Point::new(200, 20), BinaryColor::On).draw(&mut f); // off screen: ignored
        assert_eq!(f.dirty(2), Some((10, &[0x10, 0, 0, 0x80][..])));
        assert!(f.dirty(1).is_none());
        f.sent(2);
        assert!(f.dirty(2).is_none());

        // redrawing identical content sends nothing
        f.clear();
        _ = Pixel(Point::new(10, 20), BinaryColor::On).draw(&mut f);
        _ = Pixel(Point::new(13, 23), BinaryColor::On).draw(&mut f);
        assert!((0..8).all(|p| f.dirty(p).is_none()));

        f.invalidate();
        assert!(f.dirty(5).is_some_and(|(c, b)| c == 0 && b.len() == 128));
    }
}
