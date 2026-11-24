//! Persistent 32-bpp DIB section used as the source for
//! `UpdateLayeredWindow` (spec §4.5 render pipeline).
//!
//! The DIB is top-down BGRA with premultiplied alpha, the format layered
//! windows expect. It is allocated once and reallocated only when the frame
//! size changes (on DPI change).

use std::ffi::c_void;

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, HBITMAP, HDC, HGDIOBJ, SelectObject,
};

use sohi_core::geometry::RectPx;

use crate::WinError;

pub(crate) struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u8,
    width: u32,
    height: u32,
}

impl Surface {
    pub(crate) fn new(width: u32, height: u32) -> Result<Self, WinError> {
        let w = i32::try_from(width).map_err(|_| WinError::invalid("CreateDIBSection", "width"))?;
        let h =
            i32::try_from(height).map_err(|_| WinError::invalid("CreateDIBSection", "height"))?;
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: u32::try_from(size_of::<BITMAPINFOHEADER>()).unwrap_or(0),
                biWidth: w,
                biHeight: -h, // negative: top-down rows
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: a null DC creates a memory DC compatible with the screen.
        let dc = unsafe { CreateCompatibleDC(None) };
        if dc.is_invalid() {
            return Err(WinError::last("CreateCompatibleDC"));
        }
        let mut bits: *mut c_void = std::ptr::null_mut();
        // SAFETY: `info` describes a valid 32-bpp DIB and `bits` is a valid
        // out-pointer. No file mapping is used.
        let bitmap = match unsafe {
            CreateDIBSection(
                Some(dc),
                &raw const info,
                DIB_RGB_COLORS,
                &raw mut bits,
                None,
                0,
            )
        } {
            Ok(b) if !bits.is_null() => b,
            Ok(_) | Err(_) => {
                let err = WinError::last("CreateDIBSection");
                // SAFETY: `dc` was created above and selected nothing yet.
                let _ = unsafe { DeleteDC(dc) };
                return Err(err);
            }
        };
        // SAFETY: both handles are valid; the DC keeps the bitmap selected
        // until Drop restores the previous object.
        let previous = unsafe { SelectObject(dc, bitmap.into()) };
        Ok(Self {
            dc,
            bitmap,
            previous,
            bits: bits.cast(),
            width,
            height,
        })
    }

    pub(crate) fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub(crate) fn dc(&self) -> HDC {
        self.dc
    }

    /// The pixel memory: `width * height` BGRA premultiplied pixels.
    pub(crate) fn pixels_mut(&mut self) -> &mut [u8] {
        let len = self.width as usize * self.height as usize * 4;
        // SAFETY: CreateDIBSection allocated exactly `len` bytes at `bits`
        // (32 bpp, rows are DWORD-aligned with no padding), the memory lives
        // until DeleteObject in Drop, and `&mut self` makes this the only
        // view. GDI does not touch the bits concurrently: UpdateLayeredWindow
        // reads them synchronously on this thread.
        unsafe { std::slice::from_raw_parts_mut(self.bits, len) }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        // SAFETY: restore the DC's original object before deleting the
        // bitmap, then delete the DC; each handle is released exactly once.
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

/// Copies RGBA (tiny-skia's order) into BGRA (GDI's order). Both buffers
/// hold premultiplied pixels, so only the channel order changes.
pub(crate) fn swizzle_rgba_to_bgra(src: &[u8], dst: &mut [u8]) {
    let (src_px, _) = src.as_chunks::<4>();
    let (dst_px, _) = dst.as_chunks_mut::<4>();
    for (&[r, g, b, a], d) in src_px.iter().zip(dst_px) {
        *d = [b, g, r, a];
    }
}

/// Enlarges premultiplied RGBA, `width` pixels wide, to twice its size in
/// BGRA, smoothing between pixels. `dst` is `dst_width` pixels wide; an odd
/// last row or column is cut off.
pub(crate) fn double_into(src: &[u8], width: usize, dst: &mut [u8], dst_width: usize) {
    let (stride, dst_stride) = (width * 4, dst_width * 4);
    if stride == 0 || dst_stride == 0 {
        return;
    }
    // Every second row from one source row...
    for (row, out) in src.chunks_exact(stride).zip(dst.chunks_mut(dst_stride * 2)) {
        let (pixels, _) = row.as_chunks::<4>();
        let (out, _) = out
            .get_mut(..dst_stride)
            .unwrap_or_default()
            .as_chunks_mut::<8>();
        let bgra = |&[r, g, b, a]: &[u8; 4]| u32::from_le_bytes([b, g, r, a]);
        let next = pixels.iter().skip(1).chain(pixels.last());
        for ((here, next), out) in pixels.iter().zip(next).zip(out) {
            let here = bgra(here);
            let pair = u64::from(here) | (u64::from(average(here, bgra(next))) << 32);
            *out = pair.to_le_bytes();
        }
    }
    // ...and the rows between as their averages.
    let mut rows = dst.chunks_mut(dst_stride * 2).peekable();
    while let Some(pair) = rows.next() {
        let (here, between) = pair.split_at_mut(dst_stride.min(pair.len()));
        let below = rows.peek().and_then(|p| p.get(..dst_stride));
        let (between, _) = between.as_chunks_mut::<4>();
        let (here, _) = here.as_chunks::<4>();
        let (below, _) = below.unwrap_or(here.as_flattened()).as_chunks::<4>();
        for ((&a, &b), out) in here.iter().zip(below).zip(between) {
            *out = average(u32::from_le_bytes(a), u32::from_le_bytes(b)).to_le_bytes();
        }
    }
}

/// The average of two pixels, channel by channel, rounded down.
fn average(a: u32, b: u32) -> u32 {
    (a & b) + (((a ^ b) & 0xFEFE_FEFE) >> 1)
}

/// Like [`swizzle_rgba_to_bgra`], but moves the image `shift` pixels to
/// the right: the left `shift` columns become transparent and the
/// rightmost columns of `src` are clipped. Both buffers are `width` pixels
/// wide.
pub(crate) fn swizzle_shifted(src: &[u8], dst: &mut [u8], width: usize, shift: usize) {
    let stride = width * 4;
    if stride == 0 {
        return;
    }
    let shift = shift.min(width) * 4;
    for (src_row, dst_row) in src.chunks_exact(stride).zip(dst.chunks_exact_mut(stride)) {
        let (clear, rest) = dst_row.split_at_mut(shift);
        clear.fill(0);
        swizzle_rgba_to_bgra(&src_row[..rest.len()], rest);
    }
}

/// Like [`swizzle_rgba_to_bgra`], but only for the pixels inside `rect`
/// (already clipped to the `width`-pixel-wide buffers).
pub(crate) fn swizzle_region(src: &[u8], dst: &mut [u8], width: usize, rect: RectPx) {
    let (Ok(left), Ok(right), Ok(top), Ok(bottom)) = (
        usize::try_from(rect.left),
        usize::try_from(rect.right),
        usize::try_from(rect.top),
        usize::try_from(rect.bottom),
    ) else {
        return;
    };
    if left >= right {
        return;
    }
    let stride = width * 4;
    for y in top..bottom {
        let span = y * stride + left * 4..y * stride + right * 4;
        if let (Some(s), Some(d)) = (src.get(span.clone()), dst.get_mut(span)) {
            swizzle_rgba_to_bgra(s, d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_swizzle_touches_only_the_rectangle() {
        // 3x2 image; update only the middle pixel of the second row.
        let src: Vec<u8> = (1..=24).collect();
        let mut dst = [0u8; 24];
        let rect = RectPx {
            left: 1,
            top: 1,
            right: 2,
            bottom: 2,
        };
        swizzle_region(&src, &mut dst, 3, rect);
        let mut expected = [0u8; 24];
        expected[16..20].copy_from_slice(&[19, 18, 17, 20]);
        assert_eq!(dst, expected);
    }

    #[test]
    fn shifted_swizzle_clears_left_and_clips_right() {
        // One row, three pixels: A B C shifted by one gives _ A B.
        let src = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let mut dst = [0xFF; 12];
        swizzle_shifted(&src, &mut dst, 3, 1);
        assert_eq!(dst, [0, 0, 0, 0, 3, 2, 1, 4, 7, 6, 5, 8]);
        swizzle_shifted(&src, &mut dst, 3, 9);
        assert_eq!(dst, [0; 12]);
        swizzle_shifted(&src, &mut dst, 3, 0);
        assert_eq!(dst, [3, 2, 1, 4, 7, 6, 5, 8, 11, 10, 9, 12]);
    }

    #[test]
    fn swizzle_swaps_red_and_blue() {
        let src = [1, 2, 3, 4, 10, 20, 30, 40];
        let mut dst = [0; 8];
        swizzle_rgba_to_bgra(&src, &mut dst);
        assert_eq!(dst, [3, 2, 1, 4, 30, 20, 10, 40]);
    }

    #[test]
    fn surface_has_expected_size_and_is_writable() {
        let mut s = Surface::new(168, 126).unwrap();
        assert_eq!(s.size(), (168, 126));
        let px = s.pixels_mut();
        assert_eq!(px.len(), 168 * 126 * 4);
        px.fill(0xAB);
        assert!(px.iter().all(|&b| b == 0xAB));
    }

    #[test]
    fn doubling_smooths_and_swizzles() {
        // Two pixels side by side become 4 × 2.
        let src = [10, 20, 30, 40, 30, 40, 50, 60];
        let mut dst = [0; 4 * 2 * 4];
        double_into(&src, 2, &mut dst, 4);
        let row = [
            [30, 20, 10, 40],
            [40, 30, 20, 50],
            [50, 40, 30, 60],
            [50, 40, 30, 60],
        ];
        assert_eq!(dst[..16], *row.as_flattened());
        assert_eq!(dst[16..], *row.as_flattened());
    }

    #[test]
    fn doubling_averages_rows_and_cuts_odd_sizes() {
        let src = [0, 0, 0, 0, 8, 8, 8, 8];
        let mut dst = [1; 2 * 3 * 4];
        double_into(&src, 1, &mut dst, 2);
        assert_eq!(dst[..8], [0; 8]);
        assert_eq!(dst[8..16], [4; 8]);
        assert_eq!(dst[16..], [8; 8]);
        let mut odd = [1; 4];
        double_into(&[7; 4], 1, &mut odd, 1);
        assert_eq!(odd, [1; 4]);
    }
}
