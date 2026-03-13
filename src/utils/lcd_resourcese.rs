use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Line, PrimitiveStyle, Rectangle},
};

macros::load_lcd_resources!("src/resources");

#[derive(Clone, Copy)]
pub struct PixelArt {
    data: &'static [u8], // packed bits, MSB first, row by row
    width: u32,
    height: u32,
    top_left: Point,
}

impl PixelArt {
    pub const fn new(data: &'static [u8], width: u32, height: u32) -> Self {
        Self {
            data,
            width,
            height,
            top_left: Point::zero(),
        }
    }

    fn bytes_per_row(&self) -> u32 {
        (self.width + 7) / 8
    }
}

impl Dimensions for PixelArt {
    fn bounding_box(&self) -> Rectangle {
        Rectangle::new(self.top_left, Size::new(self.width, self.height))
    }
}

impl Transform for PixelArt {
    fn translate(&self, by: Point) -> Self {
        Self {
            top_left: self.top_left + by,
            ..*self
        }
    }
    fn translate_mut(&mut self, by: Point) -> &mut Self {
        self.top_left += by;
        self
    }
}

impl Drawable for PixelArt {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D: DrawTarget<Color = BinaryColor>>(&self, target: &mut D) -> Result<(), D::Error> {
        let origin = self.top_left;
        let bpr = self.bytes_per_row();
        let (w, h) = (self.width, self.height);
        let data = self.data;

        target.draw_iter((0..h).flat_map(move |y| {
            (0..w).map(move |x| {
                let byte = data[(y * bpr + x / 8) as usize];
                let on = byte & (0x80 >> (x % 8)) != 0;
                Pixel(
                    origin + Point::new(x as i32, y as i32),
                    if on {
                        BinaryColor::On
                    } else {
                        BinaryColor::Off
                    },
                )
            })
        }))
    }
}

#[allow(dead_code)]
pub struct Overlay<A, B> {
    base: A,
    overlay: B,
    top_left: Point,
}

#[allow(dead_code)]
impl<A: Dimensions, B: Dimensions + Transform> Overlay<A, B> {
    pub fn new(base: A, overlay: B) -> Self {
        let base_box = base.bounding_box();
        let overlay_box = overlay.bounding_box();

        // center overlay over base
        let offset = Point::new(
            (base_box.size.width as i32 - overlay_box.size.width as i32) / 2,
            (base_box.size.height as i32 - overlay_box.size.height as i32) / 2,
        );
        let overlay = overlay.translate(base_box.top_left + offset);

        Self {
            top_left: base_box.top_left,
            base,
            overlay,
        }
    }
}

impl<A: Dimensions, B> Dimensions for Overlay<A, B> {
    fn bounding_box(&self) -> Rectangle {
        Rectangle::new(self.top_left, self.base.bounding_box().size)
    }
}

impl<A: Transform, B: Transform> Transform for Overlay<A, B> {
    fn translate(&self, by: Point) -> Self {
        Self {
            top_left: self.top_left + by,
            base: self.base.translate(by),
            overlay: self.overlay.translate(by),
        }
    }
    fn translate_mut(&mut self, by: Point) -> &mut Self {
        self.top_left += by;
        self.base.translate_mut(by);
        self.overlay.translate_mut(by);
        self
    }
}

impl<A, B> Drawable for Overlay<A, B>
where
    A: Drawable<Color = BinaryColor>,
    B: Drawable<Color = BinaryColor>,
{
    type Color = BinaryColor;
    type Output = ();

    fn draw<D: DrawTarget<Color = BinaryColor>>(&self, target: &mut D) -> Result<(), D::Error> {
        self.base.draw(target)?;
        self.overlay.draw(target)?;
        Ok(())
    }
}

pub struct CrossedIcon<A> {
    base: A,
    top_left: Point,
    crossed: bool,
    cross_size: u32,
}

impl<A: Dimensions> CrossedIcon<A> {
    pub fn new(base: A, crossed: bool, cross_size: u32) -> Self {
        let base_box = base.bounding_box();
        Self {
            base,
            top_left: base_box.top_left,
            crossed,
            cross_size,
        }
    }
}

impl<A: Dimensions> Dimensions for CrossedIcon<A> {
    fn bounding_box(&self) -> Rectangle {
        Rectangle::new(self.top_left, self.base.bounding_box().size)
    }
}

impl<A: Transform> Transform for CrossedIcon<A> {
    fn translate(&self, by: Point) -> Self {
        Self {
            top_left: self.top_left + by,
            base: self.base.translate(by),
            crossed: self.crossed,
            cross_size: self.cross_size,
        }
    }
    fn translate_mut(&mut self, by: Point) -> &mut Self {
        self.top_left += by;
        self.base.translate_mut(by);
        self
    }
}

impl<A> Drawable for CrossedIcon<A>
where
    A: Drawable<Color = BinaryColor> + Dimensions,
{
    type Color = BinaryColor;
    type Output = ();
    fn draw<D: DrawTarget<Color = BinaryColor>>(&self, target: &mut D) -> Result<(), D::Error> {
        self.base.draw(target)?;
        if self.crossed {
            let thin_stroke = PrimitiveStyle::with_stroke(BinaryColor::On, 1);
            let bb = self.bounding_box();

            // Center the cross over the bounding box
            let offset = Point::new(
                (bb.size.width as i32 - self.cross_size as i32) / 2,
                (bb.size.height as i32 - self.cross_size as i32) / 2,
            );
            let tl = bb.top_left + offset;
            let s = self.cross_size as i32 - 1;

            Line::new(tl, tl + Point::new(s, s))
                .into_styled(thin_stroke)
                .draw(target)?;
            Line::new(tl + Point::new(0, s), tl + Point::new(s, 0))
                .into_styled(thin_stroke)
                .draw(target)?;
        }
        Ok(())
    }
}
