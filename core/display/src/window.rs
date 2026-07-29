use std::io::Cursor;

use anyhow::{anyhow, bail, ensure, Context, Result};
use sdl2::event::{Event, WindowEvent};
use sdl2::pixels::PixelFormatEnum;
use sdl2::render::{Canvas, TextureCreator};
use sdl2::video::{Window, WindowContext};
use sdl2::{Sdl, VideoSubsystem};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DisplayConfig {
    pub width: u32,
    pub height: u32,
    pub scale: u32,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            width: 320,
            height: 240,
            scale: 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DisplayEvent {
    Close {
        instance_id: String,
    },
    Resized {
        instance_id: String,
        width: u32,
        height: u32,
    },
}

pub struct DisplayWindow {
    pub id: String,
    pub width: u32,
    pub height: u32,
    pub scale: u32,
    canvas: Canvas<Window>,
    texture_creator: TextureCreator<WindowContext>,
    framebuffer: Vec<u8>,
    standalone_context: Option<Sdl>,
}

impl DisplayWindow {
    pub fn create(title: &str, width: u32, height: u32, scale: u32) -> Result<Self> {
        let sdl_context = sdl2::init().map_err(sdl_error)?;
        let video = sdl_context.video().map_err(sdl_error)?;
        let mut window = Self::create_with_video(&video, title, width, height, scale)?;
        window.standalone_context = Some(sdl_context);
        Ok(window)
    }

    pub(crate) fn create_with_video(
        video: &VideoSubsystem,
        title: &str,
        width: u32,
        height: u32,
        scale: u32,
    ) -> Result<Self> {
        validate_dimensions(width, height, scale)?;
        let window_width = width
            .checked_mul(scale)
            .context("scaled display width overflowed")?;
        let window_height = height
            .checked_mul(scale)
            .context("scaled display height overflowed")?;
        let framebuffer_len = rgb565_len(width, height)?;

        let window = video
            .window(title, window_width, window_height)
            .position_centered()
            .resizable()
            .build()
            .map_err(|error| anyhow!(error))?;
        let mut canvas = window
            .into_canvas()
            .build()
            .map_err(|error| anyhow!(error))?;
        canvas
            .set_logical_size(width, height)
            .map_err(|error| anyhow!(error))?;
        let texture_creator = canvas.texture_creator();

        Ok(Self {
            id: title.to_owned(),
            width,
            height,
            scale,
            canvas,
            texture_creator,
            framebuffer: vec![0; framebuffer_len],
            standalone_context: None,
        })
    }

    /// Update the window with tightly packed RGB565 pixel data.
    pub fn present_rgb565(
        &mut self,
        pixels: &[u8],
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<()> {
        ensure!(width > 0 && height > 0, "display area cannot be empty");
        let right = x.checked_add(width).context("display area x overflowed")?;
        let bottom = y.checked_add(height).context("display area y overflowed")?;
        ensure!(
            right <= self.width && bottom <= self.height,
            "display area is outside the framebuffer"
        );
        let expected = rgb565_len(width, height)?;
        ensure!(
            pixels.len() == expected,
            "RGB565 data length is {}, expected {expected}",
            pixels.len()
        );

        let source_pitch = width as usize * 2;
        let target_pitch = self.width as usize * 2;
        for row in 0..height as usize {
            let source_start = row * source_pitch;
            let target_start = (y as usize + row) * target_pitch + x as usize * 2;
            self.framebuffer[target_start..target_start + source_pitch]
                .copy_from_slice(&pixels[source_start..source_start + source_pitch]);
        }
        self.render()
    }

    /// Capture the current logical framebuffer as PNG bytes.
    pub fn capture_screenshot(&self) -> Vec<u8> {
        self.encode_png().unwrap_or_default()
    }

    /// Return a copy of the current logical RGB565 framebuffer.
    pub fn capture_rgb565(&self) -> Vec<u8> {
        self.framebuffer.clone()
    }

    /// Handle SDL events for a window created directly with [`Self::create`].
    pub fn poll_events(&mut self) -> Vec<DisplayEvent> {
        let Some(context) = self.standalone_context.as_ref() else {
            return Vec::new();
        };
        let Ok(mut event_pump) = context.event_pump() else {
            return Vec::new();
        };
        let window_id = self.window_id();
        event_pump
            .poll_iter()
            .filter_map(|event| event_for_window(&self.id, window_id, event))
            .collect()
    }

    pub(crate) fn window_id(&self) -> u32 {
        self.canvas.window().id()
    }

    fn render(&mut self) -> Result<()> {
        let mut texture = self
            .texture_creator
            .create_texture_streaming(PixelFormatEnum::RGB565, self.width, self.height)
            .map_err(|error| anyhow!(error))?;
        texture
            .update(None, &self.framebuffer, self.width as usize * 2)
            .map_err(|error| anyhow!(error))?;
        self.canvas.clear();
        self.canvas.copy(&texture, None, None).map_err(sdl_error)?;
        self.canvas.present();
        Ok(())
    }

    fn encode_png(&self) -> Result<Vec<u8>> {
        let pixel_count = (self.width as usize)
            .checked_mul(self.height as usize)
            .context("display dimensions overflowed")?;
        let mut rgb = Vec::with_capacity(pixel_count * 3);
        for bytes in self.framebuffer.chunks_exact(2) {
            let pixel = u16::from_ne_bytes([bytes[0], bytes[1]]);
            let red = ((pixel >> 11) & 0x1f) as u8;
            let green = ((pixel >> 5) & 0x3f) as u8;
            let blue = (pixel & 0x1f) as u8;
            rgb.extend_from_slice(&[
                (red << 3) | (red >> 2),
                (green << 2) | (green >> 4),
                (blue << 3) | (blue >> 2),
            ]);
        }

        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(Cursor::new(&mut png), self.width, self.height);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header()?;
            writer.write_image_data(&rgb)?;
        }
        Ok(png)
    }
}

pub(crate) fn event_for_window(
    instance_id: &str,
    expected_window_id: u32,
    event: Event,
) -> Option<DisplayEvent> {
    match event {
        Event::Quit { .. } => Some(DisplayEvent::Close {
            instance_id: instance_id.to_owned(),
        }),
        Event::Window {
            window_id,
            win_event: WindowEvent::Close,
            ..
        } if window_id == expected_window_id => Some(DisplayEvent::Close {
            instance_id: instance_id.to_owned(),
        }),
        Event::Window {
            window_id,
            win_event: WindowEvent::Resized(width, height) | WindowEvent::SizeChanged(width, height),
            ..
        } if window_id == expected_window_id && width > 0 && height > 0 => {
            Some(DisplayEvent::Resized {
                instance_id: instance_id.to_owned(),
                width: width as u32,
                height: height as u32,
            })
        }
        _ => None,
    }
}

fn validate_dimensions(width: u32, height: u32, scale: u32) -> Result<()> {
    if width == 0 || height == 0 || scale == 0 {
        bail!("display width, height, and scale must be nonzero");
    }
    Ok(())
}

fn rgb565_len(width: u32, height: u32) -> Result<usize> {
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(2))
        .context("display framebuffer size overflowed")
}

fn sdl_error(error: String) -> anyhow::Error {
    anyhow!(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configure_headless_sdl() {
        std::env::set_var("SDL_VIDEODRIVER", "dummy");
        std::env::set_var("SDL_RENDER_DRIVER", "software");
    }

    #[test]
    fn creates_window_and_renders_rgb565_framebuffer() {
        let _serial = crate::SDL_TEST_LOCK.lock().unwrap();
        configure_headless_sdl();
        let mut window = DisplayWindow::create("test", 4, 3, 2).unwrap();
        let red = 0xf800_u16.to_ne_bytes();
        let pixels = [red, red, red, red].concat();

        window.present_rgb565(&pixels, 1, 1, 2, 2).unwrap();

        let framebuffer = window.capture_rgb565();
        assert_eq!(&framebuffer[10..14], &pixels[..4]);
        assert_eq!(&framebuffer[18..22], &pixels[4..]);
    }

    #[test]
    fn screenshot_capture_is_a_valid_png() {
        let _serial = crate::SDL_TEST_LOCK.lock().unwrap();
        configure_headless_sdl();
        let mut window = DisplayWindow::create("capture", 2, 1, 1).unwrap();
        let pixels = [0xf800_u16.to_ne_bytes(), 0x07e0_u16.to_ne_bytes()].concat();
        window.present_rgb565(&pixels, 0, 0, 2, 1).unwrap();

        let screenshot = window.capture_screenshot();
        assert!(screenshot.starts_with(b"\x89PNG\r\n\x1a\n"));
        let decoder = png::Decoder::new(Cursor::new(screenshot));
        let mut reader = decoder.read_info().unwrap();
        let mut decoded = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut decoded).unwrap();
        assert_eq!((info.width, info.height), (2, 1));
        assert_eq!(&decoded[..info.buffer_size()], &[255, 0, 0, 0, 255, 0]);
    }

    #[test]
    fn rejects_invalid_and_out_of_bounds_updates() {
        let _serial = crate::SDL_TEST_LOCK.lock().unwrap();
        configure_headless_sdl();
        assert!(DisplayWindow::create("invalid", 0, 1, 1).is_err());

        let mut window = DisplayWindow::create("bounds", 2, 2, 1).unwrap();
        assert!(window.present_rgb565(&[0; 2], 2, 0, 1, 1).is_err());
        assert!(window.present_rgb565(&[0; 1], 0, 0, 1, 1).is_err());
    }
}
