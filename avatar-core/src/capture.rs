use std::error::Error;
use windows::Win32::Graphics::Gdi::{
    GetDC, CreateCompatibleDC, CreateCompatibleBitmap, SelectObject,
    BitBlt, GetDIBits, StretchBlt, SetStretchBltMode, DeleteObject, DeleteDC, ReleaseDC, SRCCOPY,
    BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, HBITMAP, HDC, COLORONCOLOR
};
use windows::Win32::Foundation::BOOL;
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
use log::{info, warn};

pub struct ScreenCapture {
    width: i32,
    height: i32,
}

impl ScreenCapture {
    pub fn new() -> Self {
        unsafe {
            let width = GetSystemMetrics(SM_CXSCREEN);
            let height = GetSystemMetrics(SM_CYSCREEN);
            info!("Initializing Screen Capture: Resolution {}x{}", width, height);
            ScreenCapture { width, height }
        }
    }

    /// Captures the primary monitor screen pixels in BGRA8888 format.
    pub fn capture_frame(&self) -> Result<Vec<u8>, Box<dyn Error>> {
        unsafe {
            let h_wnd = windows::Win32::Foundation::HWND(0); // HWND for entire screen
            let h_dc_screen: HDC = GetDC(h_wnd);
            if h_dc_screen.is_invalid() {
                return Err("Failed to get screen device context".into());
            }

            let h_dc_mem: HDC = CreateCompatibleDC(h_dc_screen);
            if h_dc_mem.is_invalid() {
                ReleaseDC(h_wnd, h_dc_screen);
                return Err("Failed to create compatible device context".into());
            }

            let h_bitmap: HBITMAP = CreateCompatibleBitmap(h_dc_screen, self.width, self.height);
            if h_bitmap.is_invalid() {
                let _ = DeleteDC(h_dc_mem);
                ReleaseDC(h_wnd, h_dc_screen);
                return Err("Failed to create compatible bitmap".into());
            }

            // Select bitmap into memory DC
            let h_old_bitmap = SelectObject(h_dc_mem, h_bitmap);

            // Copy screen contents into memory DC bitmap
            let success = BitBlt(
                h_dc_mem,
                0,
                0,
                self.width,
                self.height,
                h_dc_screen,
                0,
                0,
                SRCCOPY
            );

            if success.is_err() {
                let _ = SelectObject(h_dc_mem, h_old_bitmap);
                let _ = DeleteObject(h_bitmap);
                let _ = DeleteDC(h_dc_mem);
                ReleaseDC(h_wnd, h_dc_screen);
                return Err("BitBlt screen copy failed".into());
            }

            // Cleanup GDI selection before retrieving bits (required by GDI specification)
            let _ = SelectObject(h_dc_mem, h_old_bitmap);

            // Get bits of bitmap in BGRA format
            let mut bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: self.width,
                    biHeight: -self.height, // Negative height means top-down bitmap
                    biPlanes: 1,
                    biBitCount: 32, // 32-bit (BGRA)
                    biCompression: 0, // BI_RGB (uncompressed)
                    biSizeImage: 0,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 0,
                    biClrImportant: 0,
                },
                bmiColors: Default::default(),
            };

            let buf_size = (self.width * self.height * 4) as usize;
            let mut buffer = vec![0u8; buf_size];

            let lines_copied = GetDIBits(
                h_dc_screen,
                h_bitmap,
                0,
                self.height as u32,
                Some(buffer.as_mut_ptr() as *mut _),
                &mut bmi,
                DIB_RGB_COLORS
            );

            // Cleanup remaining GDI objects
            let _ = DeleteObject(h_bitmap);
            let _ = DeleteDC(h_dc_mem);
            ReleaseDC(h_wnd, h_dc_screen);

            if lines_copied == 0 {
                return Err("Failed to get bitmap bits (GetDIBits returned 0)".into());
            }

            Ok(buffer)
        }
    }

    pub fn dimensions(&self) -> (i32, i32) {
        (self.width, self.height)
    }

    /// Captures the screen and compresses it into JPEG bytes using jpeg-encoder.
    pub fn capture_frame_jpeg(&self, quality: u8) -> Result<Vec<u8>, Box<dyn Error>> {
        let bgra_pixels = self.capture_frame()?;
        let mut jpeg_bytes = Vec::new();
        let encoder = jpeg_encoder::Encoder::new(&mut jpeg_bytes, quality);
        encoder.encode(&bgra_pixels, self.width as u16, self.height as u16, jpeg_encoder::ColorType::Bgra)?;
        Ok(jpeg_bytes)
    }

    /// Captures at a reduced resolution for low-latency streaming.
    /// `scale` = 0.5 means capture at half resolution (e.g. 960x540 for a 1920x1080 screen).
    /// This dramatically reduces JPEG encode time and frame byte size.
    pub fn capture_frame_jpeg_scaled(&self, scale: f32, quality: u8) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
        let out_w = ((self.width as f32) * scale) as i32;
        let out_h = ((self.height as f32) * scale) as i32;

        unsafe {
            let h_wnd = windows::Win32::Foundation::HWND(0);
            let h_dc_screen: HDC = GetDC(h_wnd);
            if h_dc_screen.is_invalid() {
                return Err("Failed to get screen DC".into());
            }

            let h_dc_mem: HDC = CreateCompatibleDC(h_dc_screen);
            if h_dc_mem.is_invalid() {
                ReleaseDC(h_wnd, h_dc_screen);
                return Err("Failed to create mem DC".into());
            }

            // Bitmap at output (scaled) size
            let h_bitmap: HBITMAP = CreateCompatibleBitmap(h_dc_screen, out_w, out_h);
            if h_bitmap.is_invalid() {
                let _ = DeleteDC(h_dc_mem);
                ReleaseDC(h_wnd, h_dc_screen);
                return Err("Failed to create scaled bitmap".into());
            }

            let h_old = SelectObject(h_dc_mem, h_bitmap);

            // Set stretch mode to COLORONCOLOR (fastest, no blending artifacts)
            SetStretchBltMode(h_dc_mem, COLORONCOLOR);

            // StretchBlt: downscale screen into the smaller mem DC
            let ok = StretchBlt(
                h_dc_mem, 0, 0, out_w, out_h,
                h_dc_screen, 0, 0, self.width, self.height,
                SRCCOPY,
            );
            if !ok.as_bool() {
                let _ = SelectObject(h_dc_mem, h_old);
                let _ = DeleteObject(h_bitmap);
                let _ = DeleteDC(h_dc_mem);
                ReleaseDC(h_wnd, h_dc_screen);
                // Fallback: full-resolution capture then encode (slower but reliable)
                warn!("StretchBlt failed — falling back to full capture");
                return self.capture_frame_jpeg(quality)
                    .map_err(|e| -> Box<dyn Error + Send + Sync> { e.to_string().into() });
            }

            // De-select the bitmap before retrieving bits (required by GDI specification)
            let _ = SelectObject(h_dc_mem, h_old);

            let mut bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: out_w,
                    biHeight: -out_h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: 0,
                    biSizeImage: 0,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 0,
                    biClrImportant: 0,
                },
                bmiColors: Default::default(),
            };

            let buf_size = (out_w * out_h * 4) as usize;
            let mut buffer = vec![0u8; buf_size];

            let lines = GetDIBits(
                h_dc_screen, h_bitmap, 0, out_h as u32,
                Some(buffer.as_mut_ptr() as *mut _),
                &mut bmi,
                DIB_RGB_COLORS,
            );

            // Cleanup remaining GDI objects
            let _ = DeleteObject(h_bitmap);
            let _ = DeleteDC(h_dc_mem);
            ReleaseDC(h_wnd, h_dc_screen);

            if lines == 0 {
                return Err("GetDIBits returned 0 lines".into());
            }

            // Encode scaled BGRA pixels to JPEG
            let mut jpeg_bytes = Vec::new();
            let encoder = jpeg_encoder::Encoder::new(&mut jpeg_bytes, quality);
            encoder.encode(&buffer, out_w as u16, out_h as u16, jpeg_encoder::ColorType::Bgra)?;

            info!("Scaled frame: {}x{} → {} bytes JPEG", out_w, out_h, jpeg_bytes.len());
            Ok(jpeg_bytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;

    #[test]
    fn test_capture_jpeg() {
        let capture = ScreenCapture::new();
        let result = capture.capture_frame_jpeg(50);
        assert!(result.is_ok(), "Failed to capture full-res frame: {:?}", result.err());
        let bytes = result.unwrap();
        println!("Full-res capture JPEG size: {} bytes", bytes.len());
        assert!(!bytes.is_empty(), "JPEG bytes should not be empty");
        
        let mut file = File::create("c:\\Avatar desktop\\test_full.jpg").unwrap();
        file.write_all(&bytes).unwrap();
        println!("Saved test_full.jpg");
    }

    #[test]
    fn test_capture_jpeg_scaled() {
        let capture = ScreenCapture::new();
        let result = capture.capture_frame_jpeg_scaled(0.5, 40);
        assert!(result.is_ok(), "Failed to capture scaled frame: {:?}", result.err());
        let bytes = result.unwrap();
        println!("Scaled capture JPEG size: {} bytes", bytes.len());
        assert!(!bytes.is_empty(), "Scaled JPEG bytes should not be empty");

        let mut file = File::create("c:\\Avatar desktop\\test_scaled.jpg").unwrap();
        file.write_all(&bytes).unwrap();
        println!("Saved test_scaled.jpg");
    }
}

