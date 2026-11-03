//! Screen + input abstraction, with two implementations:
//! the Kobo (e-ink framebuffer + evdev) and the host (PGM captures + scripted
//! events), useful for developing and illustrating without the reader.

use signet_fb::{Canvas, Rect, Waveform};
use signet_input::Event;
use std::time::Duration;

pub trait Screen {
    fn size(&self) -> (u32, u32);
    /// Pushes `rect` from the canvas to the screen and triggers the refresh.
    fn present(&mut self, canvas: &Canvas, rect: Rect, wfm: Waveform, full: bool);
    /// Named breakpoint for captures (ignored on the reader).
    fn snapshot(&mut self, _canvas: &Canvas, _name: &str) {}
}

pub trait Input {
    /// Waits for the next event; `None` on timeout or at the end of the script.
    fn next(&mut self, timeout: Duration) -> Option<Event>;
    /// True when the source is exhausted (scripted mode).
    fn exhausted(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------- Kobo ----

pub struct KoboScreen {
    fb: signet_fb::Framebuffer,
}

impl KoboScreen {
    pub fn open() -> std::io::Result<Self> {
        let mut fb = signet_fb::Framebuffer::open()?;
        // 8-bit gray: direct line copy, half the memory of the
        // 16 bpp left by the kernel, a quarter of Nickel's 32 bpp.
        match fb.set_depth(8) {
            Ok(()) => eprintln!("framebuffer passé en 8 bpp"),
            Err(e) => eprintln!(
                "framebuffer : 8 bpp refusé ({e}), on garde {} bpp",
                fb.bits_per_pixel()
            ),
        }
        Ok(KoboScreen { fb })
    }
    pub fn describe(&self) -> String {
        self.fb.describe()
    }
}

impl Screen for KoboScreen {
    fn size(&self) -> (u32, u32) {
        (self.fb.width(), self.fb.height())
    }
    fn present(&mut self, canvas: &Canvas, rect: Rect, wfm: Waveform, full: bool) {
        self.fb.blit(canvas, rect);
        if let Ok(m) = self.fb.refresh(rect, wfm, full) {
            // A full GC16 deserves to be waited for (avoids collisions);
            // partial updates chain without blocking.
            if full {
                let _ = self.fb.wait(m);
            }
        }
    }
}

pub struct KoboInput {
    devices: Vec<signet_input::Device>,
    decoder: signet_input::TouchDecoder,
    touch_index: Option<usize>,
    queue: std::collections::VecDeque<Event>,
    raw: Vec<signet_input::RawEvent>,
}

impl KoboInput {
    /// Opens all `/dev/input/event*` and spots the touchscreen by its name.
    pub fn open(transform: signet_input::Transform) -> Self {
        let mut devices = Vec::new();
        let mut touch_index = None;
        for i in 0..8 {
            let path = format!("/dev/input/event{i}");
            if let Ok(d) = signet_input::Device::open(&path) {
                let n = d.name.to_ascii_lowercase();
                if n.contains("touch")
                    || n.contains("zforce")
                    || n.contains("elan")
                    || n.contains("cyttsp")
                    || n.contains("ir")
                {
                    touch_index = Some(devices.len());
                }
                devices.push(d);
            }
        }
        KoboInput {
            devices,
            decoder: signet_input::TouchDecoder::new(transform),
            touch_index,
            queue: Default::default(),
            raw: Vec::new(),
        }
    }
    pub fn set_transform(&mut self, t: signet_input::Transform) {
        self.decoder.transform = t;
    }
    /// Raw extent (max X, max Y) of the touchscreen according to the kernel.
    pub fn raw_extent(&self) -> Option<(i32, i32)> {
        let d = &self.devices[self.touch_index?];
        let x = d
            .abs_range(signet_input::ABS_MT_POSITION_X)
            .or_else(|| d.abs_range(signet_input::ABS_X))?;
        let y = d
            .abs_range(signet_input::ABS_MT_POSITION_Y)
            .or_else(|| d.abs_range(signet_input::ABS_Y))?;
        Some((x.1 + 1, y.1 + 1))
    }
    pub fn describe(&self) -> String {
        self.devices
            .iter()
            .enumerate()
            .map(|(i, d)| {
                format!(
                    "{}={:?}{}",
                    d.path,
                    d.name,
                    if Some(i) == self.touch_index {
                        " (tactile)"
                    } else {
                        ""
                    }
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl Input for KoboInput {
    fn next(&mut self, timeout: Duration) -> Option<Event> {
        if let Some(e) = self.queue.pop_front() {
            return Some(e);
        }
        let ready = signet_input::poll(&self.devices, Some(timeout)).ok()?;
        for i in ready {
            self.raw.clear();
            if self.devices[i].read_events(&mut self.raw).is_err() {
                continue;
            }
            let is_touch = Some(i) == self.touch_index
                || self.raw.iter().any(|e| e.kind == signet_input::EV_ABS);
            for &ev in &self.raw {
                if is_touch {
                    self.decoder.feed(ev);
                } else if let Some(e) = signet_input::decode_key(ev) {
                    self.queue.push_back(e);
                }
            }
            if is_touch {
                let evs: Vec<Event> = self.decoder.drain().collect();
                self.queue.extend(evs);
            }
        }
        self.queue.pop_front()
    }
}

// ---------------------------------------------------------------- Host ----

pub struct HostScreen {
    pub dir: String,
    w: u32,
    h: u32,
    pub presents: u32,
}

impl HostScreen {
    pub fn new(dir: &str, w: u32, h: u32) -> Self {
        let _ = std::fs::create_dir_all(dir);
        HostScreen {
            dir: dir.to_string(),
            w,
            h,
            presents: 0,
        }
    }
}

impl Screen for HostScreen {
    fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }
    fn present(&mut self, _canvas: &Canvas, rect: Rect, wfm: Waveform, full: bool) {
        self.presents += 1;
        eprintln!(
            "present #{:<3} {:?} {} {}",
            self.presents,
            rect,
            wfm.name(),
            if full { "FULL" } else { "partiel" }
        );
    }
    fn snapshot(&mut self, canvas: &Canvas, name: &str) {
        let path = format!("{}/{:02}-{name}.pgm", self.dir, self.presents + 1);
        if let Err(e) = canvas.save_pgm(&path) {
            eprintln!("capture {path}: {e}");
        } else {
            eprintln!("capture → {path}");
        }
    }
}

/// A scenario step: an event, or a pause (long press).
#[derive(Clone, Debug)]
pub enum Step {
    Ev(Event),
    Wait(Duration),
}

pub struct ScriptedInput {
    steps: std::collections::VecDeque<Step>,
}

impl ScriptedInput {
    pub fn new(steps: Vec<Step>) -> Self {
        ScriptedInput {
            steps: steps.into(),
        }
    }
    /// A complete tap (Down + Up) at the given point.
    pub fn tap(x: i32, y: i32) -> [Step; 2] {
        use signet_input::Phase;
        [
            Step::Ev(Event::Touch {
                id: 1,
                phase: Phase::Down,
                x,
                y,
            }),
            Step::Ev(Event::Touch {
                id: 1,
                phase: Phase::Up,
                x,
                y,
            }),
        ]
    }
    /// A long press (Down, pause, Up).
    pub fn long_press(x: i32, y: i32) -> [Step; 3] {
        use signet_input::Phase;
        [
            Step::Ev(Event::Touch {
                id: 1,
                phase: Phase::Down,
                x,
                y,
            }),
            Step::Wait(Duration::from_millis(750)),
            Step::Ev(Event::Touch {
                id: 1,
                phase: Phase::Up,
                x,
                y,
            }),
        ]
    }
    /// Press and hold, then drag to (x1, y1), in three moves.
    pub fn drag(x0: i32, y0: i32, x1: i32, y1: i32) -> Vec<Step> {
        use signet_input::Phase;
        let mut v = vec![
            Step::Ev(Event::Touch {
                id: 1,
                phase: Phase::Down,
                x: x0,
                y: y0,
            }),
            Step::Wait(Duration::from_millis(600)),
        ];
        for k in 1..=3 {
            let x = x0 + (x1 - x0) * k / 3;
            let y = y0 + (y1 - y0) * k / 3;
            v.push(Step::Ev(Event::Touch {
                id: 1,
                phase: Phase::Move,
                x,
                y,
            }));
            v.push(Step::Wait(Duration::from_millis(250)));
        }
        v.push(Step::Ev(Event::Touch {
            id: 1,
            phase: Phase::Up,
            x: x1,
            y: y1,
        }));
        v
    }
}

impl Input for ScriptedInput {
    /// A pause behaves like an elapsed delay: it returns `None`, which
    /// lets the application detect a held press.
    fn next(&mut self, _timeout: Duration) -> Option<Event> {
        match self.steps.pop_front()? {
            Step::Ev(e) => Some(e),
            Step::Wait(d) => {
                std::thread::sleep(d);
                None
            }
        }
    }
    fn exhausted(&self) -> bool {
        self.steps.is_empty()
    }
}
