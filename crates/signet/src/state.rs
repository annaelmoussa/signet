//! Persistent state: a `key=value` text file, read at startup, written on
//! every change. No database: a few dozen lines,
//! readable and repairable by hand from the Mac.

use std::collections::BTreeMap;
use std::fs;

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Index into `theme::READER_SIZES`.
    pub size: usize,
    /// Index into `theme::READER_MARGINS`.
    pub margins: usize,
    /// Index into `theme::READER_LEADINGS`.
    pub leading: usize,
    /// Frontlight 0–100.
    pub light: u8,
    /// Winter-time offset in minutes (the Kobo keeps time in UTC).
    pub utc_offset_minutes: i32,
    /// European summer time (last Sunday of March → last Sunday of October).
    pub eu_dst: bool,
    /// Automatic sleep after N minutes without interaction (0 = never).
    pub idle_minutes: u32,
    /// Automatic power-off after N hours asleep (0 = never).
    pub poweroff_hours: u32,
    /// Frontlight adapted to the time of day (full during the day, reduced at night).
    pub auto_light: bool,
    /// Fast mode: pure black and white text, pages turned in DU (~375 ms).
    pub fast_pages: bool,
    /// Ink: 0 = thin (raw), 1 = crisp (default), 2 = bold. See `theme::INKS`.
    pub ink: usize,
    /// Night mode: the page in white on black (images stay as they are).
    pub night: bool,
    /// Reading font: index into `theme::FAMILIES`.
    pub font: usize,
    /// Library order: 0 = recently read, 1 = title, 2 = author.
    pub library_sort: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            size: 1,
            margins: 1,
            leading: 1,
            light: 30,
            utc_offset_minutes: 60,
            eu_dst: true,
            idle_minutes: 15,
            poweroff_hours: 72,
            auto_light: true,
            fast_pages: false,
            ink: 1,
            night: false,
            font: 0,
            library_sort: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Position {
    pub chapter: usize,
    pub page: usize,
    /// Start of the page: (block, character). A new layout (text size,
    /// new compositor version) shifts the page breaks;
    /// the anchor finds the passage where the page number would be wrong.
    pub anchor: Option<(usize, u32)>,
}

#[derive(Debug, Default)]
pub struct State {
    path: String,
    pub settings: Settings,
    pub last_book: Option<String>,
    /// Serialized touch calibration (`Transform::to_string`).
    pub touch: Option<String>,
    pub positions: BTreeMap<String, Position>,
    pub finished: BTreeMap<String, bool>,
    /// Last opening of each book (Unix seconds): the "recent" order.
    pub seen: BTreeMap<String, u64>,
}

impl State {
    pub fn load(path: &str) -> State {
        let mut st = State {
            path: path.to_string(),
            ..Default::default()
        };
        let Ok(txt) = fs::read_to_string(path) else {
            return st;
        };
        for line in txt.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            match k {
                "size" => st.settings.size = v.parse().unwrap_or(1),
                "margins" => st.settings.margins = v.parse().unwrap_or(1),
                "leading" => st.settings.leading = v.parse().unwrap_or(1),
                "light" => st.settings.light = v.parse().unwrap_or(30),
                // The old `tz=120` was a fixed offset (summer time, which ends on
                // 25 October 2026): replaced by legal time, it is ignored.
                "tz" => {}
                "utc_offset" => st.settings.utc_offset_minutes = v.parse().unwrap_or(60),
                "dst" => st.settings.eu_dst = v == "1",
                "idle_minutes" => st.settings.idle_minutes = v.parse().unwrap_or(15),
                "poweroff_hours" => st.settings.poweroff_hours = v.parse().unwrap_or(72),
                "auto_light" => st.settings.auto_light = v == "1",
                "fast_pages" => st.settings.fast_pages = v == "1",
                "ink" => st.settings.ink = v.parse().unwrap_or(1).min(2),
                "night" => st.settings.night = v == "1",
                "font" => st.settings.font = v.parse().unwrap_or(0),
                "library_sort" => st.settings.library_sort = v.parse().unwrap_or(0).min(2),
                "last" => st.last_book = Some(v.to_string()),
                "touch" => st.touch = Some(v.to_string()),
                _ => {
                    if let Some(book) = k.strip_prefix("pos:") {
                        let f: Vec<&str> = v.split(',').collect();
                        if f.len() >= 2 {
                            let anchor = if f.len() >= 4 {
                                f[2].parse().ok().zip(f[3].parse().ok())
                            } else {
                                None
                            };
                            st.positions.insert(
                                book.to_string(),
                                Position {
                                    chapter: f[0].parse().unwrap_or(0),
                                    page: f[1].parse().unwrap_or(0),
                                    anchor,
                                },
                            );
                        }
                    } else if let Some(book) = k.strip_prefix("done:") {
                        st.finished.insert(book.to_string(), v == "1");
                    } else if let Some(book) = k.strip_prefix("seen:") {
                        if let Ok(t) = v.parse() {
                            st.seen.insert(book.to_string(), t);
                        }
                    }
                }
            }
        }
        st
    }

    pub fn save(&self) {
        let mut s = String::new();
        let st = &self.settings;
        s.push_str(&format!("size={}\nmargins={}\nleading={}\nlight={}\nutc_offset={}\ndst={}\nidle_minutes={}\npoweroff_hours={}\nauto_light={}\nfast_pages={}\nink={}\nnight={}\nfont={}\nlibrary_sort={}\n", st.size, st.margins, st.leading, st.light, st.utc_offset_minutes, if st.eu_dst { 1 } else { 0 }, st.idle_minutes, st.poweroff_hours, if st.auto_light { 1 } else { 0 }, if st.fast_pages { 1 } else { 0 }, st.ink, if st.night { 1 } else { 0 }, st.font, st.library_sort));
        if let Some(b) = &self.last_book {
            s.push_str(&format!("last={b}\n"));
        }
        if let Some(t) = &self.touch {
            s.push_str(&format!("touch={t}\n"));
        }
        for (b, p) in &self.positions {
            match p.anchor {
                Some((blk, ch)) => {
                    s.push_str(&format!("pos:{b}={},{},{blk},{ch}\n", p.chapter, p.page))
                }
                None => s.push_str(&format!("pos:{b}={},{}\n", p.chapter, p.page)),
            }
        }
        for (b, d) in &self.finished {
            s.push_str(&format!("done:{b}={}\n", if *d { 1 } else { 0 }));
        }
        for (b, t) in &self.seen {
            s.push_str(&format!("seen:{b}={t}\n"));
        }
        // During a USB export, /mnt/onboard is an empty folder of the rootfs:
        // writing into it would create a ghost state. We refrain.
        if self.path.starts_with("/mnt/onboard/")
            && !std::path::Path::new("/mnt/onboard/.kobo").is_dir()
        {
            return;
        }
        if let Some(dir) = std::path::Path::new(&self.path).parent() {
            let _ = fs::create_dir_all(dir);
        }
        let tmp = format!("{}.tmp", self.path);
        crate::device::with_rw(|| {
            if fs::write(&tmp, &s).is_ok() {
                let _ = fs::rename(&tmp, &self.path);
            }
        });
    }
}
