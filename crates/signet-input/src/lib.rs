//! `signet-input` — raw reading of `/dev/input/event*`.
//!
//! The kernel exposes each input device as a stream of
//! `struct input_event` (16 bytes on 32-bit ARM: 8-byte `timeval`,
//! `type` u16, `code` u16, `value` i32). A touch contact is not *one*
//! event but a burst: `ABS_MT_TRACKING_ID`, `ABS_MT_POSITION_X`,
//! `ABS_MT_POSITION_Y`, `ABS_MT_TOUCH_MAJOR`… terminated by `SYN_REPORT`.
//! This module rebuilds high-level events (`Event`) from
//! these bursts, and applies the device-specific geometric transformation
//! (mirror / axis swap) to deliver screen coordinates.
//!
//! No dependency other than `libc` (for `poll`).

use std::fs::File;
use std::io::{self, Read};
use std::os::unix::io::AsRawFd;
use std::time::Duration;

pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_ABS: u16 = 0x03;
pub const EV_MSC: u16 = 0x04;

pub const SYN_REPORT: u16 = 0x00;
pub const SYN_MT_REPORT: u16 = 0x02;

pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;
pub const ABS_PRESSURE: u16 = 0x18;
pub const ABS_MT_TOUCH_MAJOR: u16 = 0x30;
pub const ABS_MT_POSITION_X: u16 = 0x35;
pub const ABS_MT_POSITION_Y: u16 = 0x36;
pub const ABS_MT_TRACKING_ID: u16 = 0x39;
pub const ABS_MT_PRESSURE: u16 = 0x3a;

pub const KEY_POWER: u16 = 116;
pub const KEY_HOME: u16 = 102;
pub const KEY_LIGHT: u16 = 90;
/// Magnetic cover (two codes depending on the generation).
pub const SLEEP_COVER: [u16; 2] = [59, 35];
pub const BTN_TOUCH: u16 = 330;

/// `struct input_event` as read from `/dev/input/eventN` (32-bit).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RawEvent {
    pub tv_sec: libc::c_long,
    pub tv_usec: libc::c_long,
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

/// Transformation from raw coordinates to the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transform {
    /// Dimensions of the raw touch space (before axis swap).
    pub raw_w: i32,
    pub raw_h: i32,
    pub swap_axes: bool,
    pub mirror_x: bool,
    pub mirror_y: bool,
}

impl std::fmt::Display for Transform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{},{},{},{},{}",
            self.raw_w, self.raw_h, self.swap_axes as u8, self.mirror_x as u8, self.mirror_y as u8
        )
    }
}

impl Transform {
    pub fn identity(w: i32, h: i32) -> Self {
        Transform {
            raw_w: w,
            raw_h: h,
            swap_axes: false,
            mirror_x: false,
            mirror_y: false,
        }
    }

    /// Deduces the transformation from two raw taps aimed at two
    /// screen targets diagonally opposite (top-left then bottom-right).
    pub fn calibrate(
        raw_w: i32,
        raw_h: i32,
        raw_a: (i32, i32),
        raw_b: (i32, i32),
        screen_w: i32,
        screen_h: i32,
    ) -> Transform {
        let dx = raw_b.0 - raw_a.0;
        let dy = raw_b.1 - raw_a.1;
        // The targets are farther apart vertically than horizontally on
        // a portrait screen: the raw axis that varies the most corresponds to screen Y.
        let swap_axes = if screen_h >= screen_w {
            dx.abs() > dy.abs()
        } else {
            dy.abs() > dx.abs()
        };
        let (sx, sy) = if swap_axes { (dy, dx) } else { (dx, dy) };
        Transform {
            raw_w,
            raw_h,
            swap_axes,
            mirror_x: sx < 0,
            mirror_y: sy < 0,
        }
    }

    pub fn parse(s: &str) -> Option<Transform> {
        let v: Vec<i32> = s
            .split(',')
            .map(|x| x.trim().parse().ok())
            .collect::<Option<_>>()?;
        if v.len() != 5 {
            return None;
        }
        Some(Transform {
            raw_w: v[0],
            raw_h: v[1],
            swap_axes: v[2] != 0,
            mirror_x: v[3] != 0,
            mirror_y: v[4] != 0,
        })
    }
    #[inline]
    pub fn apply(&self, x: i32, y: i32) -> (i32, i32) {
        let (mut x, mut y) = if self.swap_axes { (y, x) } else { (x, y) };
        let (w, h) = if self.swap_axes {
            (self.raw_h, self.raw_w)
        } else {
            (self.raw_w, self.raw_h)
        };
        if self.mirror_x {
            x = w - 1 - x;
        }
        if self.mirror_y {
            y = h - 1 - y;
        }
        (x, y)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Down,
    Move,
    Up,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Touch {
        id: i32,
        phase: Phase,
        x: i32,
        y: i32,
    },
    Key {
        code: u16,
        pressed: bool,
    },
    /// Cover closed (`true`) or open.
    Cover(bool),
}

#[derive(Clone, Copy, Debug, Default)]
struct Slot {
    x: i32,
    y: i32,
    pressed: bool,
    seen_this_frame: bool,
    reported_down: bool,
}

/// Decoder for a touch device using the "type A multitouch protocol"
/// (each frame re-announces all active contacts, `ABS_MT_TRACKING_ID`
/// identifies the finger). This is what the IR screens of the Kobo Mark 7 speak.
pub struct TouchDecoder {
    pub transform: Transform,
    current: Option<i32>,
    slots: Vec<(i32, Slot)>,
    pending: Vec<Event>,
}

impl TouchDecoder {
    pub fn new(transform: Transform) -> Self {
        TouchDecoder {
            transform,
            current: None,
            slots: Vec::new(),
            pending: Vec::new(),
        }
    }

    fn slot(&mut self, id: i32) -> &mut Slot {
        if let Some(i) = self.slots.iter().position(|(sid, _)| *sid == id) {
            &mut self.slots[i].1
        } else {
            self.slots.push((id, Slot::default()));
            &mut self.slots.last_mut().unwrap().1
        }
    }

    pub fn feed(&mut self, ev: RawEvent) {
        match (ev.kind, ev.code) {
            (EV_ABS, ABS_MT_TRACKING_ID) => {
                if ev.value >= 0 {
                    self.current = Some(ev.value);
                    let s = self.slot(ev.value);
                    s.seen_this_frame = true;
                    s.pressed = true;
                } else {
                    // Protocol B: -1 = release of the current slot.
                    if let Some(id) = self.current {
                        self.slot(id).pressed = false;
                    }
                }
            }
            (EV_ABS, ABS_MT_POSITION_X) => {
                if let Some(id) = self.current {
                    self.slot(id).x = ev.value;
                }
            }
            (EV_ABS, ABS_MT_POSITION_Y) => {
                if let Some(id) = self.current {
                    self.slot(id).y = ev.value;
                }
            }
            (EV_ABS, ABS_MT_TOUCH_MAJOR) | (EV_ABS, ABS_MT_PRESSURE) => {
                // On Kobo, TOUCH_MAJOR/PRESSURE = 0 signals the release.
                if let Some(id) = self.current {
                    if ev.value == 0 {
                        self.slot(id).pressed = false;
                    }
                }
            }
            (EV_SYN, SYN_MT_REPORT) => {
                self.current = None;
            }
            (EV_SYN, SYN_REPORT) => self.end_frame(),
            _ => {}
        }
    }

    fn end_frame(&mut self) {
        let t = self.transform;
        let mut i = 0;
        while i < self.slots.len() {
            let (id, s) = &mut self.slots[i];
            let (x, y) = t.apply(s.x, s.y);
            let alive = s.pressed && s.seen_this_frame;
            if alive && !s.reported_down {
                s.reported_down = true;
                self.pending.push(Event::Touch {
                    id: *id,
                    phase: Phase::Down,
                    x,
                    y,
                });
                i += 1;
            } else if alive {
                self.pending.push(Event::Touch {
                    id: *id,
                    phase: Phase::Move,
                    x,
                    y,
                });
                i += 1;
            } else if s.reported_down {
                self.pending.push(Event::Touch {
                    id: *id,
                    phase: Phase::Up,
                    x,
                    y,
                });
                self.slots.remove(i);
            } else {
                self.slots.remove(i);
            }
        }
        for (_, s) in self.slots.iter_mut() {
            s.seen_this_frame = false;
        }
        self.current = None;
    }

    pub fn drain(&mut self) -> impl Iterator<Item = Event> + '_ {
        self.pending.drain(..)
    }
}

/// Decoder for the physical buttons and the cover.
pub fn decode_key(ev: RawEvent) -> Option<Event> {
    if ev.kind != EV_KEY {
        return None;
    }
    if SLEEP_COVER.contains(&ev.code) {
        return Some(Event::Cover(ev.value != 0));
    }
    match ev.value {
        0 => Some(Event::Key {
            code: ev.code,
            pressed: false,
        }),
        1 => Some(Event::Key {
            code: ev.code,
            pressed: true,
        }),
        _ => None, // auto-repeat
    }
}

/// An open evdev device.
pub struct Device {
    file: File,
    pub path: String,
    pub name: String,
}

impl Device {
    pub fn open(path: &str) -> io::Result<Self> {
        let file = File::open(path)?;
        let n = path.rsplit('/').next().unwrap_or("");
        let name = std::fs::read_to_string(format!("/sys/class/input/{n}/device/name"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        Ok(Device {
            file,
            path: path.to_string(),
            name,
        })
    }

    /// Range (min, max) of an absolute axis, via `EVIOCGABS(code)`.
    pub fn abs_range(&self, code: u16) -> Option<(i32, i32)> {
        #[repr(C)]
        #[derive(Default)]
        struct AbsInfo {
            value: i32,
            minimum: i32,
            maximum: i32,
            fuzz: i32,
            flat: i32,
            resolution: i32,
        }
        let mut info = AbsInfo::default();
        // _IOR('E', 0x40 + code, struct input_absinfo) = dir 2<<30 | 24<<16 | 'E'<<8 | nr
        let req: u32 = (2u32 << 30) | (24u32 << 16) | ((b'E' as u32) << 8) | (0x40 + code as u32);
        // SAFETY: standard evdev ioctl, structure of the expected size.
        let rv = unsafe { libc::ioctl(self.file.as_raw_fd(), req as _, &mut info as *mut AbsInfo) };
        if rv < 0 {
            None
        } else {
            Some((info.minimum, info.maximum))
        }
    }

    /// Reads all available events (the fd must be ready, see `poll`).
    pub fn read_events(&mut self, out: &mut Vec<RawEvent>) -> io::Result<()> {
        const SZ: usize = std::mem::size_of::<RawEvent>();
        let mut buf = [0u8; SZ * 64];
        let n = self.file.read(&mut buf)?;
        for chunk in buf[..n].chunks_exact(SZ) {
            // SAFETY: RawEvent is POD and chunk is exactly its size.
            let ev: RawEvent =
                unsafe { std::ptr::read_unaligned(chunk.as_ptr() as *const RawEvent) };
            out.push(ev);
        }
        Ok(())
    }
}

/// Waits until at least one device has data. Returns the ready indices.
pub fn poll(devices: &[Device], timeout: Option<Duration>) -> io::Result<Vec<usize>> {
    let mut fds: Vec<libc::pollfd> = devices
        .iter()
        .map(|d| libc::pollfd {
            fd: d.file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        })
        .collect();
    let ms = timeout
        .map(|t| t.as_millis().min(i32::MAX as u128) as i32)
        .unwrap_or(-1);
    // SAFETY: valid array of pollfd.
    let rv = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, ms) };
    if rv < 0 {
        let e = io::Error::last_os_error();
        if e.kind() == io::ErrorKind::Interrupted {
            return Ok(Vec::new());
        }
        return Err(e);
    }
    Ok(fds
        .iter()
        .enumerate()
        .filter(|(_, f)| f.revents & libc::POLLIN != 0)
        .map(|(i, _)| i)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: u16, code: u16, value: i32) -> RawEvent {
        RawEvent {
            tv_sec: 0,
            tv_usec: 0,
            kind,
            code,
            value,
        }
    }

    #[test]
    fn raw_event_is_16_bytes_on_32bit() {
        if std::mem::size_of::<libc::c_long>() == 4 {
            assert_eq!(std::mem::size_of::<RawEvent>(), 16);
        }
    }

    #[test]
    fn type_a_down_move_up() {
        let mut d = TouchDecoder::new(Transform::identity(758, 1024));
        for &(c, v) in &[
            (ABS_MT_TRACKING_ID, 7),
            (ABS_MT_POSITION_X, 100),
            (ABS_MT_POSITION_Y, 200),
            (ABS_MT_TOUCH_MAJOR, 1),
        ] {
            d.feed(ev(EV_ABS, c, v));
        }
        d.feed(ev(EV_SYN, SYN_MT_REPORT, 0));
        d.feed(ev(EV_SYN, SYN_REPORT, 0));
        assert_eq!(
            d.drain().collect::<Vec<_>>(),
            vec![Event::Touch {
                id: 7,
                phase: Phase::Down,
                x: 100,
                y: 200
            }]
        );

        for &(c, v) in &[
            (ABS_MT_TRACKING_ID, 7),
            (ABS_MT_POSITION_X, 110),
            (ABS_MT_POSITION_Y, 205),
            (ABS_MT_TOUCH_MAJOR, 1),
        ] {
            d.feed(ev(EV_ABS, c, v));
        }
        d.feed(ev(EV_SYN, SYN_MT_REPORT, 0));
        d.feed(ev(EV_SYN, SYN_REPORT, 0));
        assert_eq!(
            d.drain().collect::<Vec<_>>(),
            vec![Event::Touch {
                id: 7,
                phase: Phase::Move,
                x: 110,
                y: 205
            }]
        );

        // Empty frame (or TOUCH_MAJOR = 0) → release
        d.feed(ev(EV_ABS, ABS_MT_TRACKING_ID, 7));
        d.feed(ev(EV_ABS, ABS_MT_TOUCH_MAJOR, 0));
        d.feed(ev(EV_SYN, SYN_MT_REPORT, 0));
        d.feed(ev(EV_SYN, SYN_REPORT, 0));
        assert_eq!(
            d.drain().collect::<Vec<_>>(),
            vec![Event::Touch {
                id: 7,
                phase: Phase::Up,
                x: 110,
                y: 205
            }]
        );
    }

    #[test]
    fn calibrate_detects_swap_and_mirror() {
        // 1024×758 sensor "lying down": raw X ↔ screen Y, and screen X inverted.
        // Target A (60,60) screen → raw (60, 697); target B (698, 964) → raw (964, 59).
        let t = Transform::calibrate(1024, 758, (60, 697), (964, 59), 758, 1024);
        assert!(t.swap_axes && t.mirror_x && !t.mirror_y, "{t:?}");
        assert_eq!(t.apply(60, 697), (60, 60));
        assert_eq!(t.apply(964, 59), (698, 964));
        let id = Transform::calibrate(758, 1024, (60, 60), (698, 964), 758, 1024);
        assert!(!id.swap_axes && !id.mirror_x && !id.mirror_y);
        assert_eq!(Transform::parse(&t.to_string()), Some(t));
    }

    #[test]
    fn transform_swap_and_mirror() {
        let t = Transform {
            raw_w: 1024,
            raw_h: 758,
            swap_axes: true,
            mirror_x: true,
            mirror_y: false,
        };
        // raw (x=10, y=20) → swap → (20, 10) in a 758×1024 space → mirror x → (737, 10)
        assert_eq!(t.apply(10, 20), (737, 10));
    }
}
