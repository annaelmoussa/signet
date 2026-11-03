//! `signet-fb` — Signet's lowest layer: an e-ink framebuffer.
//!
//! Mental model:
//!
//! 1. We draw into an 8-bit grayscale **canvas** (`Canvas`),
//!    entirely in memory, independent of the hardware pixel format.
//! 2. We **blit** an area of the canvas to `/dev/fb0` (`Framebuffer::blit`),
//!    converting on the fly to the fb's format (8, 16 or 32 bpp).
//! 3. We ask the EPDC controller to **refresh** that area
//!    (`Framebuffer::refresh`) with a chosen waveform: this call
//!    is what physically moves the ink.
//! 4. We can **wait** for a refresh to finish (`Framebuffer::wait`)
//!    to chain cleanly or to measure its duration.
//!
//! No dependency other than `libc`.

pub mod mxcfb;

use mxcfb::*;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::io::AsRawFd;
use std::ptr;
use std::time::{Duration, Instant};

/// E-ink waveform: the trade-off between speed / quality / remanence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Waveform {
    /// Black & white, ~260 ms, no clearing: menus, highlights.
    Du,
    /// Very fast black & white (~120 ms), requires already-binary content:
    /// animations, scrolling, keyboard.
    A2,
    /// 16 grays, fast, "regal" (hardware anti-ghosting): **turning a page**.
    Glr16,
    /// 16 grays, fast, no regal: general interface.
    Gl16,
    /// 16 grays, slow (~450 ms) with flash, cleans the screen: every N pages.
    Gc16,
    /// 4 grays.
    Gc4,
    /// Lets the driver choose.
    Auto,
}

impl Waveform {
    fn code(self) -> u32 {
        match self {
            Waveform::Du => WFM_DU,
            Waveform::A2 => WFM_A2,
            Waveform::Glr16 => WFM_GLR16,
            Waveform::Gl16 => WFM_GL16,
            Waveform::Gc16 => WFM_GC16,
            Waveform::Gc4 => WFM_GC4,
            Waveform::Auto => WFM_AUTO,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Waveform::Du => "DU",
            Waveform::A2 => "A2",
            Waveform::Glr16 => "GLR16",
            Waveform::Gl16 => "GL16",
            Waveform::Gc16 => "GC16",
            Waveform::Gc4 => "GC4",
            Waveform::Auto => "AUTO",
        }
    }
}

/// Rectangular area (x, y, width, height) in screen pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub const fn new(x: u32, y: u32, w: u32, h: u32) -> Self {
        Rect { x, y, w, h }
    }
    pub fn clip(self, w: u32, h: u32) -> Rect {
        let x1 = self.x.min(w);
        let y1 = self.y.min(h);
        let x2 = (self.x + self.w).min(w);
        let y2 = (self.y + self.h).min(h);
        Rect {
            x: x1,
            y: y1,
            w: x2 - x1,
            h: y2 - y1,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }
}

/// 8-bit grayscale image (0 = black, 255 = white), row by row.
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Self {
        Canvas {
            width,
            height,
            data: vec![0xFF; (width * height) as usize],
        }
    }
    pub fn clear(&mut self, gray: u8) {
        self.data.fill(gray);
    }
    #[inline]
    pub fn put(&mut self, x: u32, y: u32, gray: u8) {
        if x < self.width && y < self.height {
            self.data[(y * self.width + x) as usize] = gray;
        }
    }
    #[inline]
    pub fn get(&self, x: u32, y: u32) -> u8 {
        self.data[(y * self.width + x) as usize]
    }
    pub fn fill_rect(&mut self, r: Rect, gray: u8) {
        let r = r.clip(self.width, self.height);
        for y in r.y..r.y + r.h {
            let row = (y * self.width) as usize;
            self.data[row + r.x as usize..row + (r.x + r.w) as usize].fill(gray);
        }
    }
    /// Inverts the grays of an area (instant visual feedback of a tap).
    pub fn invert_rect(&mut self, r: Rect) {
        let r = r.clip(self.width, self.height);
        for y in r.y..r.y + r.h {
            let row = (y * self.width) as usize;
            for v in &mut self.data[row + r.x as usize..row + (r.x + r.w) as usize] {
                *v = 255 - *v;
            }
        }
    }

    pub fn stroke_rect(&mut self, r: Rect, thickness: u32, gray: u8) {
        self.fill_rect(Rect::new(r.x, r.y, r.w, thickness), gray);
        self.fill_rect(
            Rect::new(r.x, r.y + r.h.saturating_sub(thickness), r.w, thickness),
            gray,
        );
        self.fill_rect(Rect::new(r.x, r.y, thickness, r.h), gray);
        self.fill_rect(
            Rect::new(r.x + r.w.saturating_sub(thickness), r.y, thickness, r.h),
            gray,
        );
    }
    /// Writes the image in binary PGM format (P5), readable everywhere.
    pub fn save_pgm(&self, path: &str) -> io::Result<()> {
        use std::io::Write;
        let mut f = File::create(path)?;
        write!(f, "P5\n{} {}\n255\n", self.width, self.height)?;
        f.write_all(&self.data)
    }
}

/// The hardware framebuffer, memory-mapped.
pub struct Framebuffer {
    file: File,
    mem: *mut u8,
    mem_len: usize,
    pub var: FbVarScreenInfo,
    pub fix: FbFixScreenInfo,
    marker: u32,
    bytes_per_pixel: usize,
}

// The mmap pointer is used only from this struct; we allow it to
// cross threads.
unsafe impl Send for Framebuffer {}

impl Framebuffer {
    pub fn open() -> io::Result<Self> {
        Self::open_path("/dev/fb0")
    }

    pub fn open_path(path: &str) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let fd = file.as_raw_fd();
        let mut var = FbVarScreenInfo::default();
        let mut fix = FbFixScreenInfo::default();
        // SAFETY: standard ioctls from linux/fb.h, structures of the right size.
        unsafe {
            ioctl(fd, FBIOGET_VSCREENINFO, &mut var as *mut _)?;
            ioctl(fd, FBIOGET_FSCREENINFO, &mut fix as *mut _)?;
        }
        if var.bits_per_pixel % 8 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "bpp non multiple de 8",
            ));
        }
        let mem_len = fix.smem_len as usize;
        // SAFETY: shared mapping of the fb, length given by the driver.
        let mem = unsafe {
            libc::mmap(
                ptr::null_mut(),
                mem_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if mem == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Framebuffer {
            file,
            mem: mem as *mut u8,
            mem_len,
            var,
            fix,
            marker: 1,
            bytes_per_pixel: (var.bits_per_pixel / 8) as usize,
        })
    }

    /// Switches the framebuffer to `bpp` bits per pixel (8 = Y8 gray, the most
    /// economical for the EPDC: one row = one memory copy). The kernel
    /// may reallocate the memory: we re-read the info and remap.
    pub fn set_depth(&mut self, bpp: u32) -> io::Result<()> {
        if self.var.bits_per_pixel == bpp {
            return Ok(());
        }
        let fd = self.file.as_raw_fd();
        let mut var = self.var;
        var.bits_per_pixel = bpp;
        var.grayscale = if bpp == 8 { 1 } else { 0 };
        if bpp == 8 {
            var.red = FbBitfield {
                offset: 0,
                length: 8,
                msb_right: 0,
            };
            var.green = var.red;
            var.blue = var.red;
            var.transp = FbBitfield::default();
        }
        var.activate = 0; // FB_ACTIVATE_NOW
                          // SAFETY: standard ioctl from linux/fb.h.
        unsafe { ioctl(fd, FBIOPUT_VSCREENINFO, &mut var as *mut _)? };
        let mut nvar = FbVarScreenInfo::default();
        let mut nfix = FbFixScreenInfo::default();
        unsafe {
            ioctl(fd, FBIOGET_VSCREENINFO, &mut nvar as *mut _)?;
            ioctl(fd, FBIOGET_FSCREENINFO, &mut nfix as *mut _)?;
        }
        if nvar.bits_per_pixel != bpp {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("le noyau a gardé {} bpp", nvar.bits_per_pixel),
            ));
        }
        // Remap if the size changed.
        let new_len = nfix.smem_len as usize;
        if new_len != self.mem_len {
            // SAFETY: we unmap the old area before mapping a new one.
            unsafe { libc::munmap(self.mem as *mut _, self.mem_len) };
            let mem = unsafe {
                libc::mmap(
                    ptr::null_mut(),
                    new_len,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fd,
                    0,
                )
            };
            if mem == libc::MAP_FAILED {
                return Err(io::Error::last_os_error());
            }
            self.mem = mem as *mut u8;
            self.mem_len = new_len;
        }
        self.var = nvar;
        self.fix = nfix;
        self.bytes_per_pixel = (bpp / 8) as usize;
        Ok(())
    }

    // `var.width` / `var.height` are the panel's physical size in mm; pixels are in `xres` / `yres`.
    #[allow(clippy::misnamed_getters)]
    pub fn width(&self) -> u32 {
        self.var.xres
    }
    #[allow(clippy::misnamed_getters)]
    pub fn height(&self) -> u32 {
        self.var.yres
    }
    pub fn bits_per_pixel(&self) -> u32 {
        self.var.bits_per_pixel
    }
    pub fn rotation(&self) -> u32 {
        self.var.rotate
    }
    pub fn id(&self) -> String {
        String::from_utf8_lossy(&self.fix.id)
            .trim_end_matches('\0')
            .to_string()
    }

    /// Describes the framebuffer in one line (for logs / the journal).
    pub fn describe(&self) -> String {
        format!(
            "{} {}x{} (virt {}x{}) {}bpp gray={} rotate={} line_length={} smem={}K r@{}/{} g@{}/{} b@{}/{}",
            self.id(),
            self.var.xres,
            self.var.yres,
            self.var.xres_virtual,
            self.var.yres_virtual,
            self.var.bits_per_pixel,
            self.var.grayscale,
            self.var.rotate,
            self.fix.line_length,
            self.fix.smem_len / 1024,
            self.var.red.offset,
            self.var.red.length,
            self.var.green.offset,
            self.var.green.length,
            self.var.blue.offset,
            self.var.blue.length,
        )
    }

    /// Writes `gray` at pixel (x, y) directly into video memory.
    #[inline]
    pub fn put(&mut self, x: u32, y: u32, gray: u8) {
        if x >= self.var.xres || y >= self.var.yres {
            return;
        }
        let off = y as usize * self.fix.line_length as usize + x as usize * self.bytes_per_pixel;
        if off + self.bytes_per_pixel > self.mem_len {
            return;
        }
        // SAFETY: offset bounded above.
        unsafe {
            let p = self.mem.add(off);
            match self.bytes_per_pixel {
                1 => *p = gray,
                2 => {
                    let g = gray as u16;
                    let v = ((g >> 3) << 11) | ((g >> 2) << 5) | (g >> 3);
                    ptr::write_unaligned(p as *mut u16, v);
                }
                _ => {
                    *p = gray;
                    *p.add(1) = gray;
                    *p.add(2) = gray;
                    *p.add(3) = 0xFF;
                }
            }
        }
    }

    /// Copies area `r` of the canvas to video memory (without refreshing).
    pub fn blit(&mut self, canvas: &Canvas, r: Rect) {
        let r = r.clip(
            self.var.xres.min(canvas.width),
            self.var.yres.min(canvas.height),
        );
        if r.is_empty() {
            return;
        }
        let ll = self.fix.line_length as usize;
        match self.bytes_per_pixel {
            1 => {
                for y in r.y..r.y + r.h {
                    let src = &canvas.data[(y * canvas.width + r.x) as usize..][..r.w as usize];
                    let off = y as usize * ll + r.x as usize;
                    // SAFETY: area bounded by clip() and by smem_len.
                    unsafe {
                        ptr::copy_nonoverlapping(src.as_ptr(), self.mem.add(off), r.w as usize)
                    };
                }
            }
            _ => {
                for y in r.y..r.y + r.h {
                    for x in r.x..r.x + r.w {
                        self.put(x, y, canvas.get(x, y));
                    }
                }
            }
        }
    }

    pub fn fill(&mut self, r: Rect, gray: u8) {
        let r = r.clip(self.var.xres, self.var.yres);
        for y in r.y..r.y + r.h {
            for x in r.x..r.x + r.w {
                self.put(x, y, gray);
            }
        }
    }

    /// Asks the e-ink controller to redraw `r`. Returns the marker to
    /// pass to [`wait`](Self::wait). `full = true` forces a full
    /// refresh (flash) of the area.
    pub fn refresh(&mut self, r: Rect, wfm: Waveform, full: bool) -> io::Result<u32> {
        let r = r.clip(self.var.xres, self.var.yres);
        let marker = self.marker;
        self.marker = self.marker.wrapping_add(1).max(1);

        let waveform_mode = wfm.code();
        let mut flags = 0u32;
        if waveform_mode == WFM_A2 {
            flags |= EPDC_FLAG_FORCE_MONOCHROME;
        }
        if waveform_mode == WFM_GLR16 || waveform_mode == WFM_GLD16 {
            flags |= EPDC_FLAG_USE_REGAL;
        }

        let data = MxcfbUpdateData {
            update_region: MxcfbRect {
                top: r.y,
                left: r.x,
                width: r.w,
                height: r.h,
            },
            waveform_mode,
            update_mode: if full {
                UPDATE_MODE_FULL
            } else {
                UPDATE_MODE_PARTIAL
            },
            update_marker: marker,
            temp: TEMP_USE_AMBIENT,
            flags,
            dither_mode: EPDC_DITHER_PASSTHROUGH,
            quant_bit: 0,
            alt_buffer_data: MxcfbAltBufferData::default(),
        };
        // SAFETY: #[repr(C)] struct of the size the driver expects.
        unsafe {
            ioctl(
                self.file.as_raw_fd(),
                MXCFB_SEND_UPDATE,
                &data as *const _ as *mut MxcfbUpdateData,
            )?
        };
        Ok(marker)
    }

    /// Blocks until refresh `marker` completes (kernel timeout 5 s).
    /// Returns the wait duration measured on the userland side.
    pub fn wait(&self, marker: u32) -> io::Result<Duration> {
        let mut m = MxcfbUpdateMarkerData {
            update_marker: marker,
            collision_test: 0,
        };
        let t0 = Instant::now();
        // SAFETY: idem.
        unsafe {
            ioctl(
                self.file.as_raw_fd(),
                MXCFB_WAIT_FOR_UPDATE_COMPLETE,
                &mut m as *mut _,
            )?
        };
        Ok(t0.elapsed())
    }

    /// Driver queue mode (SNAPSHOT / QUEUE / QUEUE_AND_MERGE).
    pub fn set_update_scheme(&self, scheme: u32) -> io::Result<()> {
        // SAFETY: idem.
        unsafe {
            ioctl(
                self.file.as_raw_fd(),
                MXCFB_SET_UPDATE_SCHEME,
                &scheme as *const _ as *mut u32,
            )
            .map(|_| ())
        }
    }

    /// Captures the current video memory content as grayscale.
    pub fn snapshot(&self) -> Canvas {
        let (w, h) = (self.var.xres, self.var.yres);
        let mut c = Canvas::new(w, h);
        let ll = self.fix.line_length as usize;
        for y in 0..h {
            for x in 0..w {
                let off = y as usize * ll + x as usize * self.bytes_per_pixel;
                if off + self.bytes_per_pixel > self.mem_len {
                    continue;
                }
                // SAFETY: bounded.
                let g = unsafe {
                    match self.bytes_per_pixel {
                        1 => *self.mem.add(off),
                        2 => {
                            let v = ptr::read_unaligned(self.mem.add(off) as *const u16);
                            (((v >> 5) & 0x3F) << 2) as u8
                        }
                        _ => *self.mem.add(off + 1), // green channel
                    }
                };
                c.put(x, y, g);
            }
        }
        c
    }
}

impl Drop for Framebuffer {
    fn drop(&mut self) {
        // SAFETY: we unmap only what we mapped.
        unsafe { libc::munmap(self.mem as *mut _, self.mem_len) };
    }
}

/// `ioctl` call with conversion of the number to the type the libc expects
/// (`c_int` on musl, `c_ulong` elsewhere).
unsafe fn ioctl<T>(fd: libc::c_int, req: u32, arg: *mut T) -> io::Result<libc::c_int> {
    check(libc::ioctl(fd, req as _, arg))
}

fn check(rv: libc::c_int) -> io::Result<libc::c_int> {
    if rv < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(rv)
    }
}
