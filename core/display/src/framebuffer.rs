use anyhow::{ensure, Context, Result};

use crate::{Rect, BYTES_PER_PIXEL};

/// Byte order used by Mycelium's capture and presentation ABI.
///
/// `HostNative` matches an in-memory `u16` used by LVGL. `St7789Wire` is the
/// high-byte-first sequence transferred over SPI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rgb565ByteOrder {
    HostNative,
    St7789Wire,
}

pub(crate) fn update_rgb565(
    framebuffer: &mut [u8],
    framebuffer_width: u32,
    framebuffer_height: u32,
    pixels: &[u8],
    area: Rect,
) -> Result<()> {
    ensure!(
        area.width > 0 && area.height > 0,
        "display area cannot be empty"
    );
    let right = area
        .x
        .checked_add(area.width)
        .context("display area x overflowed")?;
    let bottom = area
        .y
        .checked_add(area.height)
        .context("display area y overflowed")?;
    ensure!(
        right <= framebuffer_width && bottom <= framebuffer_height,
        "display area is outside the framebuffer"
    );
    let expected_framebuffer = crate::framebuffer_size(framebuffer_width, framebuffer_height)
        .context("display framebuffer size overflowed")?;
    ensure!(
        framebuffer.len() == expected_framebuffer,
        "framebuffer length is {}, expected {expected_framebuffer}",
        framebuffer.len()
    );
    let expected_pixels = crate::framebuffer_size(area.width, area.height)
        .context("display update size overflowed")?;
    ensure!(
        pixels.len() == expected_pixels,
        "RGB565 data length is {}, expected {expected_pixels}",
        pixels.len()
    );

    let source_pitch = area.width as usize * BYTES_PER_PIXEL;
    let target_pitch = framebuffer_width as usize * BYTES_PER_PIXEL;
    for row in 0..area.height as usize {
        let source_start = row * source_pitch;
        let target_start =
            (area.y as usize + row) * target_pitch + area.x as usize * BYTES_PER_PIXEL;
        framebuffer[target_start..target_start + source_pitch]
            .copy_from_slice(&pixels[source_start..source_start + source_pitch]);
    }
    Ok(())
}

/// Convert native-endian packed RGB565 pixels to ST7789 SPI wire order.
pub fn host_rgb565_to_st7789_wire(pixels: &[u8]) -> Option<Vec<u8>> {
    convert_rgb565(
        pixels,
        Rgb565ByteOrder::HostNative,
        Rgb565ByteOrder::St7789Wire,
    )
}

/// Convert high-byte-first ST7789 SPI bytes to native-endian packed RGB565.
pub fn st7789_wire_to_host_rgb565(pixels: &[u8]) -> Option<Vec<u8>> {
    convert_rgb565(
        pixels,
        Rgb565ByteOrder::St7789Wire,
        Rgb565ByteOrder::HostNative,
    )
}

fn convert_rgb565(
    pixels: &[u8],
    source: Rgb565ByteOrder,
    target: Rgb565ByteOrder,
) -> Option<Vec<u8>> {
    if !pixels.len().is_multiple_of(BYTES_PER_PIXEL) {
        return None;
    }
    let mut converted = Vec::with_capacity(pixels.len());
    for bytes in pixels.chunks_exact(BYTES_PER_PIXEL) {
        let value = match source {
            Rgb565ByteOrder::HostNative => u16::from_ne_bytes([bytes[0], bytes[1]]),
            Rgb565ByteOrder::St7789Wire => u16::from_be_bytes([bytes[0], bytes[1]]),
        };
        converted.extend_from_slice(&match target {
            Rgb565ByteOrder::HostNative => value.to_ne_bytes(),
            Rgb565ByteOrder::St7789Wire => value.to_be_bytes(),
        });
    }
    Some(converted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn native_pixels(values: &[u16]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_ne_bytes())
            .collect()
    }

    #[test]
    fn preserves_disjoint_edge_single_pixel_and_multi_row_updates() {
        let mut framebuffer = vec![0; 5 * 4 * 2];

        update_rgb565(
            &mut framebuffer,
            5,
            4,
            &native_pixels(&[0xf800]),
            Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
        )
        .unwrap();
        update_rgb565(
            &mut framebuffer,
            5,
            4,
            &native_pixels(&[0x07e0]),
            Rect {
                x: 4,
                y: 3,
                width: 1,
                height: 1,
            },
        )
        .unwrap();
        update_rgb565(
            &mut framebuffer,
            5,
            4,
            &native_pixels(&[1, 2, 3, 4, 5, 6]),
            Rect {
                x: 1,
                y: 1,
                width: 3,
                height: 2,
            },
        )
        .unwrap();

        let pixel = |x: usize, y: usize| {
            let offset = (y * 5 + x) * 2;
            u16::from_ne_bytes([framebuffer[offset], framebuffer[offset + 1]])
        };
        assert_eq!(pixel(0, 0), 0xf800);
        assert_eq!(pixel(4, 3), 0x07e0);
        assert_eq!((pixel(1, 1), pixel(3, 1)), (1, 3));
        assert_eq!((pixel(1, 2), pixel(3, 2)), (4, 6));
        assert_eq!(pixel(4, 0), 0);
        assert_eq!(pixel(0, 3), 0);
    }

    #[test]
    fn converts_native_rgb565_to_big_endian_wire_bytes_and_back() {
        let host = native_pixels(&[0xf800, 0x07e0, 0x001f, 0xffff]);
        let wire = host_rgb565_to_st7789_wire(&host).unwrap();
        assert_eq!(wire, [0xf8, 0x00, 0x07, 0xe0, 0x00, 0x1f, 0xff, 0xff]);
        assert_eq!(st7789_wire_to_host_rgb565(&wire).unwrap(), host);
        assert!(host_rgb565_to_st7789_wire(&[0xff]).is_none());
    }
}
