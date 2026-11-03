//! Binary definitions of the `mxcfb` driver (Freescale/NXP i.MX EPDC) as
//! patched by Netronix for the "Mark 7" Kobo (i.MX6SL/SLL/ULL, kernel 4.1).
//!
//! References: `include/linux/mxcfb.h` from the Kobo kernel, `eink/mxcfb-kobo.h` from
//! FBInk, `framebuffer/mxcfb_sys.rs` from Plato. The structures must be
//! byte-for-byte identical to the kernel's: `#[repr(C)]` is mandatory.

/// Update rectangle, in pixels, in the fb's current orientation.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MxcfbRect {
    pub top: u32,
    pub left: u32,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct MxcfbAltBufferData {
    pub phys_addr: u32,
    pub width: u32,
    pub height: u32,
    pub alt_update_region: MxcfbRect,
}

/// `struct mxcfb_update_data`, Mark 7 version (72 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct MxcfbUpdateData {
    pub update_region: MxcfbRect,
    pub waveform_mode: u32,
    pub update_mode: u32,
    pub update_marker: u32,
    pub temp: libc::c_int,
    pub flags: libc::c_uint,
    pub dither_mode: libc::c_int,
    pub quant_bit: libc::c_int,
    pub alt_buffer_data: MxcfbAltBufferData,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct MxcfbUpdateMarkerData {
    pub update_marker: u32,
    pub collision_test: u32,
}

// --- ioctl numbers ------------------------------------------------------
// Linux encoding (ARM, generic): dir<<30 | size<<16 | type<<8 | nr
const IOC_WRITE: u32 = 1;
const IOC_READ: u32 = 2;
/// The numbers are kept as `u32`; musl declares `ioctl(int, int, ...)` whereas
/// glibc/macOS declare `unsigned long`: we cast at call time.
const fn ioc(dir: u32, ty: u8, nr: u8, size: usize) -> u32 {
    (dir << 30) | ((size as u32) << 16) | ((ty as u32) << 8) | nr as u32
}

pub const FBIOGET_VSCREENINFO: u32 = 0x4600;
pub const FBIOPUT_VSCREENINFO: u32 = 0x4601;
pub const FBIOGET_FSCREENINFO: u32 = 0x4602;

/// `MXCFB_SEND_UPDATE_V2 = _IOW('F', 0x2E, struct mxcfb_update_data)`
pub const MXCFB_SEND_UPDATE: u32 = ioc(
    IOC_WRITE,
    b'F',
    0x2E,
    core::mem::size_of::<MxcfbUpdateData>(),
);
/// `MXCFB_WAIT_FOR_UPDATE_COMPLETE_V3 = _IOWR('F', 0x2F, struct mxcfb_update_marker_data)`
pub const MXCFB_WAIT_FOR_UPDATE_COMPLETE: u32 = ioc(
    IOC_READ | IOC_WRITE,
    b'F',
    0x2F,
    core::mem::size_of::<MxcfbUpdateMarkerData>(),
);
/// `MXCFB_SET_UPDATE_SCHEME = _IOW('F', 0x32, uint32_t)`
pub const MXCFB_SET_UPDATE_SCHEME: u32 = ioc(IOC_WRITE, b'F', 0x32, 4);
/// `MXCFB_SET_PWRDOWN_DELAY = _IOW('F', 0x30, int32_t)`
pub const MXCFB_SET_PWRDOWN_DELAY: u32 = ioc(IOC_WRITE, b'F', 0x30, 4);

pub const UPDATE_SCHEME_SNAPSHOT: u32 = 0;
pub const UPDATE_SCHEME_QUEUE: u32 = 1;
pub const UPDATE_SCHEME_QUEUE_AND_MERGE: u32 = 2;

pub const UPDATE_MODE_PARTIAL: u32 = 0;
pub const UPDATE_MODE_FULL: u32 = 1;

pub const TEMP_USE_AMBIENT: libc::c_int = 0x1000;

// Waveforms (NTX numbering, EPDC v2)
pub const WFM_INIT: u32 = 0;
pub const WFM_DU: u32 = 1;
pub const WFM_GC16: u32 = 2;
pub const WFM_GC4: u32 = 3;
pub const WFM_A2: u32 = 4;
pub const WFM_GL16: u32 = 5;
pub const WFM_GLR16: u32 = 6;
pub const WFM_GLD16: u32 = 7;
pub const WFM_AUTO: u32 = 257;

pub const EPDC_FLAG_ENABLE_INVERSION: u32 = 0x01;
pub const EPDC_FLAG_FORCE_MONOCHROME: u32 = 0x02;
pub const EPDC_FLAG_USE_DITHERING_Y1: u32 = 0x2000;
pub const EPDC_FLAG_USE_DITHERING_Y4: u32 = 0x4000;
pub const EPDC_FLAG_USE_REGAL: u32 = 0x8000;

pub const EPDC_DITHER_PASSTHROUGH: libc::c_int = 0;
pub const EPDC_DITHER_FLOYD_STEINBERG: libc::c_int = 1;
pub const EPDC_DITHER_ATKINSON: libc::c_int = 2;
pub const EPDC_DITHER_ORDERED: libc::c_int = 3;
pub const EPDC_DITHER_QUANT_ONLY: libc::c_int = 4;

// --- linux/fb.h -----------------------------------------------------------
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct FbBitfield {
    pub offset: u32,
    pub length: u32,
    pub msb_right: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct FbVarScreenInfo {
    pub xres: u32,
    pub yres: u32,
    pub xres_virtual: u32,
    pub yres_virtual: u32,
    pub xoffset: u32,
    pub yoffset: u32,
    pub bits_per_pixel: u32,
    pub grayscale: u32,
    pub red: FbBitfield,
    pub green: FbBitfield,
    pub blue: FbBitfield,
    pub transp: FbBitfield,
    pub nonstd: u32,
    pub activate: u32,
    pub height: u32,
    pub width: u32,
    pub accel_flags: u32,
    pub pixclock: u32,
    pub left_margin: u32,
    pub right_margin: u32,
    pub upper_margin: u32,
    pub lower_margin: u32,
    pub hsync_len: u32,
    pub vsync_len: u32,
    pub sync: u32,
    pub vmode: u32,
    pub rotate: u32,
    pub colorspace: u32,
    pub reserved: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FbFixScreenInfo {
    pub id: [u8; 16],
    pub smem_start: u32,
    pub smem_len: u32,
    pub type_: u32,
    pub type_aux: u32,
    pub visual: u32,
    pub xpanstep: u16,
    pub ypanstep: u16,
    pub ywrapstep: u16,
    pub line_length: u32,
    pub mmio_start: u32,
    pub mmio_len: u32,
    pub accel: u32,
    pub capabilities: u16,
    pub reserved: [u16; 2],
}

impl Default for FbFixScreenInfo {
    fn default() -> Self {
        // All zeros is a valid value for this POD structure.
        unsafe { core::mem::zeroed() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn struct_sizes_match_kernel() {
        assert_eq!(core::mem::size_of::<MxcfbRect>(), 16);
        assert_eq!(core::mem::size_of::<MxcfbUpdateData>(), 72);
        assert_eq!(core::mem::size_of::<MxcfbUpdateMarkerData>(), 8);
        assert_eq!(core::mem::size_of::<FbVarScreenInfo>(), 160);
    }
    #[test]
    fn ioctl_numbers() {
        assert_eq!(MXCFB_SEND_UPDATE, 0x4048_462E);
        assert_eq!(MXCFB_WAIT_FOR_UPDATE_COMPLETE, 0xC008_462F);
    }
}
