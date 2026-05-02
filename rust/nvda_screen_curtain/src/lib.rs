/*
A part of NonVisual Desktop Access (NVDA)
This file is covered by the GNU General Public License.
See the file COPYING for more details.
Copyright (C) 2026 NV Access Limited

Utilities for Screen Curtain.

Captures the entire virtual screen via GDI and verifies, using a GDI+ histogram,
that every pixel is exactly RGB(0, 0, 0). Used by NVDA's screen curtain feature
to confirm the screen has been blacked out.

Ported from nvdaHelper/local/screenCurtain.cpp.
*/

use std::ptr;

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    GetObjectW, ReleaseDC, SelectObject, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HBITMAP, HDC, SRCCOPY,
};
use windows::Win32::Graphics::GdiPlus::{
    GdipBitmapGetHistogram, GdipBitmapGetHistogramSize, GdipCreateBitmapFromGdiDib,
    GdipDisposeImage, GpBitmap, GpImage, HistogramFormatRGB, Ok as GdipOk,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetDesktopWindow, GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN,
};

/// RAII guard for a desktop DC obtained via `GetDC(hwnd)`. Releases with
/// `ReleaseDC(hwnd, hdc)` on drop.
struct DesktopDcGuard {
    hwnd: HWND,
    hdc: HDC,
}

impl Drop for DesktopDcGuard {
    fn drop(&mut self) {
        if !self.hdc.is_invalid() {
            unsafe { ReleaseDC(self.hwnd, self.hdc) };
        }
    }
}

/// RAII guard for a memory DC created with `CreateCompatibleDC`. Releases with
/// `DeleteDC` on drop.
struct MemDcGuard(HDC);

impl Drop for MemDcGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = DeleteDC(self.0);
            }
        }
    }
}

/// RAII guard for an HBITMAP. Frees it with `DeleteObject` on drop.
struct BitmapGuard(HBITMAP);

impl Drop for BitmapGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = DeleteObject(self.0);
            }
        }
    }
}

/// RAII guard for a GDI+ `GpBitmap*`. Frees it with `GdipDisposeImage`.
struct GpBitmapGuard(*mut GpBitmap);

impl Drop for GpBitmapGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                GdipDisposeImage(self.0 as *mut GpImage);
            }
        }
    }
}

/// Captures the entire virtual screen and returns true iff every pixel is
/// exactly RGB(0, 0, 0).
///
/// Returns false on any failure (no logging is performed; the Python caller
/// treats failure the same as "screen is not black").
///
/// GDI+ note: this function relies on GDI+ already being initialised in the
/// host process (the host process performs `GdiplusStartup` elsewhere).
/// Because nvdaRust.pyd loads into the same process as the existing
/// `nvdaHelperLocal.dll` consumer, the same global GDI+ token is available
/// without us calling `GdiplusStartup` again.
pub fn is_screen_fully_black() -> bool {
    // The virtual screen is the bounding rectangle of all of the monitors on the system.
    let screen_width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let screen_height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
    // While the primary monitor's top left corner is at the origin, it is
    // not necessarily at the top left of the virtual screen.
    // Thus the top left of the virtual screen may be negative.
    let screen_origin_x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let screen_origin_y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    // Screen coordinates are 16-bit integers and 2^16 * 2^16 = 2^32, so the
    // area of the screen is guaranteed to fit in an i32 on supported
    // platforms. Use u32 here because histogram counts are unsigned.
    let screen_area: u32 = (screen_width as i64 * screen_height as i64) as u32;

    // The desktop window covers the entire virtual screen.
    let desktop_wnd = unsafe { GetDesktopWindow() };
    if desktop_wnd.0 == 0 as _ {
        return false;
    }

    let desktop_dc = unsafe { GetDC(desktop_wnd) };
    if desktop_dc.is_invalid() {
        return false;
    }
    let _desktop_dc_guard = DesktopDcGuard {
        hwnd: desktop_wnd,
        hdc: desktop_dc,
    };

    let capture_dc = unsafe { CreateCompatibleDC(desktop_dc) };
    if capture_dc.is_invalid() {
        return false;
    }
    let _capture_dc_guard = MemDcGuard(capture_dc);

    let capture_bitmap =
        unsafe { CreateCompatibleBitmap(desktop_dc, screen_width, screen_height) };
    if capture_bitmap.is_invalid() {
        return false;
    }
    let _capture_bitmap_guard = BitmapGuard(capture_bitmap);

    // Set capture_dc to draw to capture_bitmap.
    let old_obj = unsafe { SelectObject(capture_dc, capture_bitmap) };
    if old_obj.is_invalid() {
        return false;
    }

    // Replace the contents of capture_dc with those of desktop_dc.
    let blt_result = unsafe {
        BitBlt(
            capture_dc,
            0,
            0,
            screen_width,
            screen_height,
            desktop_dc,
            screen_origin_x,
            screen_origin_y,
            SRCCOPY,
        )
    };
    // Restore capture_dc for safety (matches the C++).
    unsafe {
        let _ = SelectObject(capture_dc, old_obj);
    }
    if blt_result.is_err() {
        return false;
    }

    // Get properties of capture_bitmap.
    let mut dd_screenshot = BITMAP::default();
    let bytes_written = unsafe {
        GetObjectW(
            capture_bitmap,
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut dd_screenshot as *mut _ as *mut _),
        )
    };
    if bytes_written == 0 {
        return false;
    }

    // Build a BITMAPINFO describing a 32-bit, uncompressed DIB matching the
    // device-dependent bitmap's dimensions.
    let mut di_screenshot_info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: dd_screenshot.bmWidth,
            biHeight: dd_screenshot.bmHeight,
            biPlanes: 1,    // Can only ever be 1.
            biBitCount: 32, // High byte unused.
            biCompression: BI_RGB.0,
            biSizeImage: 0, // Unneeded as uncompressed.
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0, // All colours are needed.
        },
        bmiColors: [Default::default()],
    };

    // Calculate the size (in bytes) of the DIB. Each scan line must be a
    // multiple of 32 bits long, including padding if necessary. So add 31 to
    // push us to that boundary; if no padding is needed, adding 31 makes no
    // difference since (32n + 31) / 32 = n R 31.
    let bit_count = di_screenshot_info.bmiHeader.biBitCount as i32;
    let row_bytes = ((dd_screenshot.bmWidth * bit_count + 31) / 32) * 4;
    let di_screenshot_size = (row_bytes as usize) * (dd_screenshot.bmHeight as usize);

    // Initialise each byte to 1 as a canary (matches C++).
    let mut di_screenshot_bits: Vec<u8> = vec![1u8; di_screenshot_size];

    let lines_copied = unsafe {
        GetDIBits(
            capture_dc,
            capture_bitmap,
            0,
            dd_screenshot.bmHeight as u32,
            Some(di_screenshot_bits.as_mut_ptr() as *mut _),
            &mut di_screenshot_info,
            DIB_RGB_COLORS,
        )
    };
    // GetDIBits returns 0 on failure; ERROR_INVALID_PARAMETER is also
    // documented as a possible non-success return per the C++.
    if lines_copied == 0 {
        return false;
    }

    // Create a GDI+ bitmap wrapping the DIB.
    let mut gp_bitmap_ptr: *mut GpBitmap = ptr::null_mut();
    let status = unsafe {
        GdipCreateBitmapFromGdiDib(
            &di_screenshot_info,
            di_screenshot_bits.as_mut_ptr() as *mut _,
            &mut gp_bitmap_ptr,
        )
    };
    if status != GdipOk || gp_bitmap_ptr.is_null() {
        return false;
    }
    let _gp_bitmap_guard = GpBitmapGuard(gp_bitmap_ptr);

    // Calculate histogram size.
    let mut histogram_size: u32 = 0;
    let status =
        unsafe { GdipBitmapGetHistogramSize(HistogramFormatRGB, &mut histogram_size) };
    if status != GdipOk || histogram_size == 0 {
        return false;
    }

    // Allocate per-channel histograms.
    let mut hist_r = vec![0u32; histogram_size as usize];
    let mut hist_g = vec![0u32; histogram_size as usize];
    let mut hist_b = vec![0u32; histogram_size as usize];

    let status = unsafe {
        GdipBitmapGetHistogram(
            gp_bitmap_ptr,
            HistogramFormatRGB,
            histogram_size,
            hist_r.as_mut_ptr(),
            hist_g.as_mut_ptr(),
            hist_b.as_mut_ptr(),
            ptr::null_mut(),
        )
    };
    if status != GdipOk {
        return false;
    }

    // If the entire screen is black, then the only colour in the histogram
    // must be (0, 0, 0). Since the sum of values in each channel must be the
    // number of pixels in the image, if the screen is entirely black the
    // 0-th entry in each channel must be the number of pixels in the image.
    hist_r[0] == screen_area && hist_g[0] == screen_area && hist_b[0] == screen_area
}
