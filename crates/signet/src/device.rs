//! Small hardware interfaces: time, battery, lighting.

use std::fs;

fn now_utc() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Civil day (year, month, day) for a number of days since 1 January 1970
/// (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Number of days since 1 January 1970 for a civil day (Hinnant).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// Last Sunday of a 31-day month, at 01:00 UTC (seconds since 1970).
fn last_sunday_1h_utc(year: i64, month: i64) -> i64 {
    let last = days_from_civil(year, month, 31);
    // 1 January 1970 was a Thursday: (days + 4) mod 7 is 0 on Sunday.
    (last - (last + 4).rem_euclid(7)) * 86_400 + 3_600
}

/// Legal-time offset at instant `utc`, in minutes: `std_minutes` in
/// winter time, one hour more in summer time when `eu_dst` (European
/// Union rule: from the last Sunday of March to the last Sunday
/// of October, switching at 01:00 UTC everywhere).
pub fn local_offset_minutes(std_minutes: i32, eu_dst: bool, utc: i64) -> i32 {
    if !eu_dst {
        return std_minutes;
    }
    let (year, _, _) = civil_from_days(utc.div_euclid(86_400));
    if (last_sunday_1h_utc(year, 3)..last_sunday_1h_utc(year, 10)).contains(&utc) {
        std_minutes + 60
    } else {
        std_minutes
    }
}

/// Local time (hh:mm) from the kernel's UTC clock (set at boot
/// from the hardware clock, no network) and the legal time.
pub fn clock(std_minutes: i32, eu_dst: bool) -> String {
    let now = now_utc();
    let local = now + local_offset_minutes(std_minutes, eu_dst, now) as i64 * 60;
    let day = local.rem_euclid(86_400);
    format!("{:02}:{:02}", day / 3600, (day % 3600) / 60)
}

/// Battery level 0–100, or `None` if unknown.
pub fn battery_percent() -> Option<u8> {
    for name in ["mc13892_bat", "bd71827_bat", "battery", "rn5t618-battery"] {
        if let Ok(s) = fs::read_to_string(format!("/sys/class/power_supply/{name}/capacity")) {
            return s.trim().parse().ok();
        }
    }
    None
}

static TERM: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn on_term(_: libc::c_int) {
    TERM.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// System shutdown (`reboot`, `poweroff`) or application shutdown: init sends
/// SIGTERM and leaves one second before SIGKILL. The signal interrupts the wait for
/// events; the loop then writes the position before quitting (previously the
/// last pages turned could be lost).
pub fn install_term_handler() {
    // SAFETY: handler reduced to an atomic write; valid sigaction.
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_term as extern "C" fn(libc::c_int) as usize;
        libc::sigemptyset(&mut sa.sa_mask);
        sa.sa_flags = 0;
        libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut());
        libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
    }
}

pub fn term_requested() -> bool {
    TERM.load(std::sync::atomic::Ordering::SeqCst)
}

/// Power button LED. The device tree makes it blink
/// (`timer` trigger) from boot until the firmware takes it over;
/// without Nickel, it blinked forever. The kernel's "charge" triggers do not
/// light up on the Nia (the battery driver does not report the start of
/// charging): Signet drives it itself, lit as long as the cable is plugged in.
pub fn set_power_led(on: bool) {
    let dir = "/sys/class/leds/GLED";
    if !std::path::Path::new(dir).exists() {
        return;
    }
    let _ = fs::write(format!("{dir}/trigger"), "none");
    let _ = fs::write(format!("{dir}/brightness"), if on { "255" } else { "0" });
}

/// External power present (mains charger or USB port): the
/// battery charges, or stays full on mains.
pub fn power_online() -> bool {
    // The Nia's kernel writes 3 (not 1) when the cable is plugged in: anything
    // that is not 0.
    ["acpwr", "usbpwr", "usb", "ac"].iter().any(|n| {
        fs::read_to_string(format!("/sys/class/power_supply/{n}/online"))
            .map(|v| {
                let v = v.trim();
                !v.is_empty() && v != "0"
            })
            .unwrap_or(false)
    })
}

/// Front light. The Nia exposes `mxc_msp430.0` (0–100) in sysfs;
/// we use it first, then ioctl 241 of `/dev/ntx_io` as a fallback.
pub fn set_frontlight(level: u8) -> std::io::Result<()> {
    let level = level.min(100);
    if fs::write(
        "/sys/class/backlight/mxc_msp430.0/brightness",
        format!("{level}\n"),
    )
    .is_ok()
    {
        return Ok(());
    }
    use std::os::unix::io::AsRawFd;
    const CM_FRONT_LIGHT_SET: u32 = 241;
    let f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/ntx_io")?;
    // SAFETY: ioctl with an integer argument, documented by the ntx_io driver.
    let rv = unsafe { libc::ioctl(f.as_raw_fd(), CM_FRONT_LIGHT_SET as _, level as libc::c_int) };
    if rv < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Result of a sleep attempt.
pub struct SleepReport {
    pub steps: Vec<String>,
    /// Time spent in the write to `mem`: if it is more than half a second,
    /// the reader really slept, whatever the return code.
    pub slept: std::time::Duration,
}

/// Puts the reader to sleep (suspend-to-RAM) and returns only on wake-up.
///
/// Sequence on Kobo i.MX: `state-extended = 1` warns the Netronix kernel,
/// `sync`, then `mem`. A peculiarity observed on the Nia: the write to
/// `mem` **succeeds** (dmesg shows the full suspend/resume cycle) but
/// returns an error code (ENOSYS, ENOENT…) on wake-up. We therefore rely
/// only on elapsed time.
pub fn suspend() -> SleepReport {
    let mut steps = Vec::new();
    let states = fs::read_to_string("/sys/power/state").unwrap_or_default();
    // RTC alarm: we touch nothing (the caller sets it,
    // for the automatic power-off); we only note it for the log.
    let alarm = fs::read_to_string("/sys/class/rtc/rtc0/wakealarm").unwrap_or_default();
    let now = fs::read_to_string("/sys/class/rtc/rtc0/since_epoch").unwrap_or_default();
    steps.push(format!(
        "rtc0 : wakealarm={:?} since_epoch={}",
        alarm.trim(),
        now.trim()
    ));
    steps.push(format!(
        "wakeup_count={}",
        fs::read_to_string("/sys/power/wakeup_count")
            .unwrap_or_default()
            .trim()
    ));
    match fs::write("/sys/power/state-extended", "1\n") {
        Ok(()) => steps.push("state-extended ← 1".into()),
        Err(e) => steps.push(format!("state-extended ← 1 : {e}")),
    }
    std::thread::sleep(std::time::Duration::from_millis(2000));
    // SAFETY: libc call with no arguments.
    unsafe { libc::sync() };
    let st = if states.split_whitespace().any(|s| s == "mem") {
        "mem"
    } else {
        "standby"
    };
    let t0 = std::time::Instant::now();
    let r = fs::write("/sys/power/state", format!("{st}\n"));
    let slept = t0.elapsed();
    let _ = fs::write("/sys/power/state-extended", "0\n");
    steps.push(format!(
        "{st} : {:.1} s, retour {}",
        slept.as_secs_f64(),
        r.err()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "OK".into())
    ));
    SleepReport { steps, slept }
}

/// Last lines of dmesg (diagnostics).
pub fn dmesg_tail(n: usize) -> Vec<String> {
    std::process::Command::new("/bin/dmesg")
        .output()
        .map(|o| {
            let txt = String::from_utf8_lossy(&o.stdout);
            let lines: Vec<&str> = txt.lines().collect();
            lines
                .iter()
                .rev()
                .take(n)
                .rev()
                .map(|l| l.to_string())
                .collect()
        })
        .unwrap_or_default()
}

// ------------------------------------------------------------------ USB ----

/// Netronix platform (`mx6ull-ntx`, `mx6sll-ntx`…) deduced from the device tree,
/// to find the kernel modules in `/drivers/<platform>/`.
pub fn platform() -> String {
    let compat = fs::read_to_string("/proc/device-tree/compatible").unwrap_or_default();
    for (needle, name) in [
        ("imx6ull", "mx6ull-ntx"),
        ("imx6sll", "mx6sll-ntx"),
        ("imx6sl", "mx6sl-ntx"),
        ("imx50", "mx50-ntx"),
    ] {
        if compat.contains(needle) {
            return name.to_string();
        }
    }
    "mx6ull-ntx".to_string()
}

/// Is the USB cable plugged in? (`usbpwr` is the PMIC's USB power.)
pub fn usb_plugged() -> bool {
    for p in [
        "/sys/class/power_supply/usbpwr/online",
        "/sys/class/power_supply/usb/online",
    ] {
        if let Ok(s) = fs::read_to_string(p) {
            return s.trim() != "0";
        }
    }
    false
}

fn sh(cmd: &str, args: &[&str]) -> std::io::Result<bool> {
    let st = std::process::Command::new(cmd)
        .args(args)
        .env("PATH", "/sbin:/bin:/usr/sbin:/usr/bin")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    Ok(st.success())
}

const ONBOARD_DEV: &str = "/dev/mmcblk0p3";

/// State of the USB controller on the device side: "not attached", "powered",
/// "default", "addressed", "configured"… A computer moves the
/// gadget to "configured" by enumerating it; a charger never enumerates anything.
pub fn udc_state() -> String {
    fs::read_dir("/sys/class/udc")
        .ok()
        .and_then(|mut d| d.next())
        .and_then(|e| e.ok())
        .and_then(|e| fs::read_to_string(e.path().join("state")).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// File that carries the gadget's storage medium (empty: no medium).
fn lun_file() -> Option<std::path::PathBuf> {
    fs::read_dir("/sys/class/udc")
        .ok()?
        .flatten()
        .map(|e| e.path().join("device/gadget/lun0/file"))
        .find(|p| p.exists())
}

/// Loads the USB storage gadget *without a medium*, the partition staying mounted:
/// a computer enumerates it as an empty card reader (see [`udc_state`]),
/// a charger does nothing. The medium is inserted only once the computer
/// is recognized ([`usb_insert_onboard`]). Previously, the partition was exported as soon as
/// the USB port was powered, so a charger was enough.
pub fn usb_gadget_load(
    fw_version: &str,
    serial: &str,
) -> Result<Vec<String>, (Vec<String>, String)> {
    let mut log = Vec::new();
    let dir = format!("/drivers/{}/usb/gadget", platform());
    for m in ["configfs", "libcomposite", "usb_f_mass_storage"] {
        let path = format!("{dir}/{m}.ko");
        match sh("/sbin/insmod", &[&path]) {
            Ok(true) => log.push(format!("{m} chargé")),
            Ok(false) => log.push(format!("{m} : insmod a échoué (déjà chargé ?)")),
            Err(e) => return Err((log, format!("insmod {m} : {e}"))),
        }
    }
    let params = [
        format!("{dir}/g_file_storage.ko"),
        "stall=0".into(),
        "removable=1".into(),
        "idVendor=0x2237".into(),
        "idProduct=0x4230".into(),
        "iManufacturer=Kobo".into(),
        format!("iProduct=eReader-{fw_version}"),
        format!("iSerialNumber={serial}"),
    ];
    let args: Vec<&str> = params.iter().map(String::as_str).collect();
    match sh("/sbin/insmod", &args) {
        Ok(true) => {
            log.push("g_file_storage chargé, sans support".into());
            Ok(log)
        }
        Ok(false) => Err((log, "g_file_storage : insmod a échoué".into())),
        Err(e) => Err((log, format!("insmod g_file_storage : {e}"))),
    }
}

/// Unloads the gadget (the partition was not exported).
pub fn usb_gadget_unload() -> Vec<String> {
    let mut log = Vec::new();
    for m in [
        "g_file_storage",
        "usb_f_mass_storage",
        "libcomposite",
        "configfs",
    ] {
        match sh("/sbin/rmmod", &[m]) {
            Ok(true) => log.push(format!("{m} déchargé")),
            Ok(false) => log.push(format!("{m} : rmmod a échoué")),
            Err(e) => log.push(format!("rmmod {m} : {e}")),
        }
    }
    log
}

/// The computer is there: unmounts the onboard partition and gives it to the gadget
/// as its medium, exactly what the firmware did.
pub fn usb_insert_onboard() -> Result<Vec<String>, (Vec<String>, String)> {
    let mut log = Vec::new();
    let Some(lun) = lun_file() else {
        return Err((log, "gadget sans fichier lun0 : pas chargé ?".into()));
    };
    // Safeguard no. 1: no process may hold a file open on the
    // partition. Exporting a partition still in use = two systems
    // writing the same FAT = corruption, then a factory reset by
    // the firmware at the next boot. Opens are often
    // transient (network script, log): we wait up to 8 s.
    let mut holders = onboard_holders();
    let mut waited = 0;
    while !holders.is_empty() && waited < 16 {
        std::thread::sleep(std::time::Duration::from_millis(500));
        waited += 1;
        holders = onboard_holders();
    }
    if waited > 0 {
        log.push(format!(
            "attendu {:.1} s que la partition se libère",
            waited as f32 * 0.5
        ));
    }
    if !holders.is_empty() {
        for h in &holders {
            log.push(format!("fichier ouvert sur onboard : {h}"));
        }
        return Err((
            log,
            "des fichiers sont encore ouverts sur la partition, export refusé".into(),
        ));
    }
    // SAFETY: libc call with no arguments.
    unsafe { libc::sync() };
    // Safeguard no. 2: *non*-lazy unmount. If it fails, we insert nothing.
    match sh("/bin/umount", &["/mnt/onboard"]) {
        Ok(true) => log.push("onboard démonté".into()),
        Ok(false) => return Err((log, "umount onboard a échoué (partition occupée ?)".into())),
        Err(e) => return Err((log, format!("umount : {e}"))),
    }
    if onboard_mounted() {
        return Err((log, "onboard toujours montée après umount".into()));
    }
    match fs::write(&lun, ONBOARD_DEV) {
        Ok(()) => {
            log.push("partition insérée : exportée".into());
            Ok(log)
        }
        Err(e) => Err((log, format!("insertion du support : {e}"))),
    }
}

/// The computer ejected the disk (the gadget's medium is removed).
pub fn usb_medium_ejected() -> bool {
    lun_file()
        .map(|p| {
            fs::read_to_string(p)
                .map(|v| v.trim().is_empty())
                .unwrap_or(false)
        })
        .unwrap_or(false)
}

/// End of the USB export: unloads the gadget stack, checks and remounts onboard.
pub fn usb_unexport() -> Vec<String> {
    let mut log = Vec::new();
    for m in [
        "g_file_storage",
        "usb_f_mass_storage",
        "libcomposite",
        "configfs",
    ] {
        match sh("/sbin/rmmod", &[m]) {
            Ok(true) => log.push(format!("{m} déchargé")),
            Ok(false) => log.push(format!("{m} : rmmod a échoué")),
            Err(e) => log.push(format!("rmmod {m} : {e}")),
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(500));
    let fsck = [
        "/sbin/dosfsck",
        "/usr/sbin/dosfsck",
        "/bin/dosfsck",
        "/usr/bin/dosfsck",
        "/sbin/fsck.vfat",
        "/usr/sbin/fsck.vfat",
    ]
    .into_iter()
    .find(|p| std::path::Path::new(p).exists());
    match fsck {
        Some(f) => {
            let mut ok = false;
            for _ in 0..4 {
                if sh(f, &["-a", "-w", ONBOARD_DEV]).unwrap_or(false) {
                    ok = true;
                    break;
                }
            }
            log.push(if ok {
                format!("{f} OK")
            } else {
                format!("{f} : échec")
            });
        }
        None => log.push("dosfsck introuvable".into()),
    }
    match sh(
        "/bin/mount",
        &[
            "-o",
            "noatime,nodiratime,shortname=mixed,utf8",
            "-t",
            "vfat",
            ONBOARD_DEV,
            "/mnt/onboard",
        ],
    ) {
        Ok(true) => log.push("onboard remonté".into()),
        Ok(false) => log.push("mount onboard : échec".into()),
        Err(e) => log.push(format!("mount : {e}")),
    }
    if let Some(m) = protect_onboard() {
        log.push(m);
    }
    for p in ["/mnt/onboard/fsck0000.rec", "/mnt/onboard/FSCK0000.REC"] {
        let _ = with_rw(|| fs::remove_file(p));
    }
    log
}

/// (firmware version, serial number) read from `.kobo/version`.
pub fn kobo_version() -> (String, String) {
    let v = fs::read_to_string("/mnt/onboard/.kobo/version").unwrap_or_default();
    let f: Vec<&str> = v.trim().split(',').collect();
    (
        f.get(2).unwrap_or(&"0.0.0").to_string(),
        f.first().unwrap_or(&"N306000000000").to_string(),
    )
}

/// Signet's log is written to `/tmp` (tmpfs): never to the
/// onboard partition, which can be exported over USB at any time (writing
/// to it while the Mac holds it would corrupt the FAT). This function
/// copies what has accumulated to `.signet/signet.log` when onboard
/// is mounted, then empties the buffer.
pub fn flush_log() -> std::io::Result<usize> {
    use std::io::Write;
    const TMP: &str = "/tmp/signet.log";
    const DST: &str = "/mnt/onboard/.signet/signet.log";
    if !onboard_mounted() {
        return Ok(0);
    }
    let data = fs::read(TMP).unwrap_or_default();
    if data.is_empty() {
        return Ok(0);
    }
    with_rw(|| -> std::io::Result<()> {
        let _ = fs::create_dir_all("/mnt/onboard/.signet");
        let mut f = fs::OpenOptions::new().create(true).append(true).open(DST)?;
        f.write_all(&data)?;
        f.flush()
    })?;
    // Truncate the buffer; stderr is opened with O_APPEND, so later writes
    // start again from the beginning.
    let _ = fs::OpenOptions::new().write(true).truncate(true).open(TMP);
    Ok(data.len())
}

/// List of "pid:cmd → path" for files open under /mnt/onboard,
/// all processes included (ourselves too: an open book, a log…).
pub fn onboard_holders() -> Vec<String> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir("/proc") else {
        return out;
    };
    for e in rd.filter_map(|e| e.ok()) {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.parse::<u32>().is_err() {
            continue;
        }
        let comm = fs::read_to_string(format!("/proc/{name}/comm"))
            .unwrap_or_default()
            .trim()
            .to_string();
        let mut targets = Vec::new();
        if let Ok(fds) = fs::read_dir(format!("/proc/{name}/fd")) {
            for fd in fds.filter_map(|f| f.ok()) {
                if let Ok(t) = fs::read_link(fd.path()) {
                    let t = t.to_string_lossy().into_owned();
                    if t.starts_with("/mnt/onboard") {
                        targets.push(t);
                    }
                }
            }
        }
        for t in ["cwd", "exe"] {
            if let Ok(l) = fs::read_link(format!("/proc/{name}/{t}")) {
                let l = l.to_string_lossy().into_owned();
                if l.starts_with("/mnt/onboard") {
                    targets.push(format!("{t}={l}"));
                }
            }
        }
        for t in targets {
            out.push(format!("{name}:{comm} → {t}"));
        }
    }
    out
}

/// Is the onboard partition mounted? (`.kobo` exists only on the FAT.)
pub fn onboard_mounted() -> bool {
    std::path::Path::new("/mnt/onboard/.kobo").is_dir()
}

/// Is the onboard partition mounted read-only?
pub fn onboard_readonly() -> bool {
    fs::read_to_string("/proc/mounts")
        .map(|m| {
            m.lines().any(|l| {
                l.contains(" /mnt/onboard ")
                    && l.split_whitespace()
                        .nth(3)
                        .map(|o| o.split(',').any(|f| f == "ro"))
                        .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn remount(opts: &str) -> bool {
    sh("/bin/mount", &["-o", opts, "/mnt/onboard"]).unwrap_or(false)
}

static RW_DEPTH: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Write window on the onboard partition. At rest, the FAT is mounted
/// read-only: a brutal power cut (button held
/// ten seconds, dead battery) never leaves it "dirty", and the
/// firmware has no reason to offer its factory reset at the
/// next boot. We switch to read-write just long enough to write, `sync`, then
/// go back to read-only. No effect on the Mac (no partition).
pub fn with_rw<R>(f: impl FnOnce() -> R) -> R {
    use std::sync::atomic::Ordering;
    if !onboard_mounted()
        || !onboard_readonly()
            && RW_DEPTH.load(Ordering::SeqCst) == 0
            && !PROTECTED.load(Ordering::SeqCst)
    {
        return f();
    }
    let depth = RW_DEPTH.fetch_add(1, Ordering::SeqCst);
    if depth == 0 && !remount("remount,rw") {
        eprintln!("onboard : remount rw impossible");
    }
    let r = f();
    if depth == 0 {
        let _ = sh("/bin/sync", &[]);
        if !remount("remount,ro") {
            eprintln!("onboard : remount ro impossible (fichier ouvert en écriture ?)");
        }
    }
    RW_DEPTH.fetch_sub(1, Ordering::SeqCst);
    r
}

static PROTECTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Puts the partition in read-only mode at rest (see [`with_rw`]).
pub fn protect_onboard() -> Option<String> {
    if !onboard_mounted() {
        return None;
    }
    let _ = sh("/bin/sync", &[]);
    PROTECTED.store(true, std::sync::atomic::Ordering::SeqCst);
    Some(if remount("remount,ro") {
        "onboard en lecture seule au repos".into()
    } else {
        "onboard : remount ro impossible".into()
    })
}

/// Turns off Wi-Fi the way Nickel does before sleep: network processes,
/// interface, then unloading the SDIO driver. As long as the Realtek driver
/// is loaded, the kernel refuses to suspend ("mmc1:0001:1 cannot remain
/// alive while host is suspended", error -38).
pub fn wifi_off() -> Vec<String> {
    let mut log = Vec::new();
    for name in ["wpa_supplicant", "udhcpc", "dhcpcd", "wpa_cli"] {
        for pid in pids_of(name) {
            // SAFETY: signal to a known pid.
            unsafe { libc::kill(pid, libc::SIGTERM) };
            log.push(format!("{name} ({pid}) arrêté"));
        }
    }
    let modules = fs::read_to_string("/proc/modules").unwrap_or_default();
    for iface in ["eth0", "wlan0"] {
        if std::path::Path::new(&format!("/sys/class/net/{iface}")).exists() {
            let _ = sh("/sbin/ifconfig", &[iface, "down"]);
            log.push(format!("{iface} down"));
        }
    }
    for m in [
        "8189fs",
        "8192es",
        "8723ds",
        "8821cs",
        "bcmdhd",
        "dhd",
        "sdio_wifi_pwr",
    ] {
        if modules
            .lines()
            .any(|l| l.split_whitespace().next() == Some(m))
        {
            match sh("/sbin/rmmod", &[m]) {
                Ok(true) => log.push(format!("{m} déchargé")),
                Ok(false) => log.push(format!("{m} : rmmod a échoué")),
                Err(e) => log.push(format!("rmmod {m} : {e}")),
            }
        }
    }
    log
}

fn pids_of(name: &str) -> Vec<i32> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir("/proc") else {
        return out;
    };
    for e in rd.filter_map(|e| e.ok()) {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        if fs::read_to_string(format!("/proc/{pid}/comm"))
            .map(|c| c.trim() == name)
            .unwrap_or(false)
        {
            out.push(pid);
        }
    }
    out
}

/// Development mode: `.signet/dev` file present → the reader is a
/// USB network card (SSH), not a disk.
pub fn dev_mode() -> bool {
    std::path::Path::new("/mnt/onboard/.signet/dev").exists()
}

/// Launches the development hook from a copy in /tmp, detached: the
/// script must never stay open on the partition (USB export).
pub fn run_dev_hook() {
    if fs::copy("/mnt/onboard/.signet/dev.sh", "/tmp/dev.sh").is_err() {
        return;
    }
    let _ = std::process::Command::new("/bin/sh")
        .arg("/tmp/dev.sh")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Seconds since the epoch according to the kernel's real-time clock.
pub fn rtc_now() -> Option<i64> {
    fs::read_to_string("/sys/class/rtc/rtc0/since_epoch")
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Sets a wake-up by the real-time clock in `seconds` seconds.
/// Returns the wake-up instant (epoch).
pub fn set_rtc_alarm(seconds: u32) -> std::io::Result<i64> {
    let _ = fs::write("/sys/class/rtc/rtc0/wakealarm", "0\n");
    fs::write("/sys/class/rtc/rtc0/wakealarm", format!("+{seconds}\n"))?;
    let set: i64 = fs::read_to_string("/sys/class/rtc/rtc0/wakealarm")?
        .trim()
        .parse()
        .unwrap_or(0);
    Ok(set)
}

pub fn clear_rtc_alarm() {
    let _ = fs::write("/sys/class/rtc/rtc0/wakealarm", "0\n");
}

/// Cuts the power (the image on screen stays: it is e-ink).
///
/// The FAT partition must be cleanly unmounted first: cut while mounted,
/// it stays marked "dirty" and the firmware offers a factory reset
/// at the next boot (experienced first-hand). We unmount, check, then
/// request an orderly shutdown; as a last resort only, `-f`.
pub fn poweroff() {
    // SAFETY: libc call with no arguments.
    unsafe { libc::sync() };
    let _ = sh("/bin/umount", &["/mnt/onboard"]);
    if onboard_mounted() {
        let _ = sh("/bin/umount", &["-l", "/mnt/onboard"]);
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    let _ = sh("/bin/dosfsck", &["-a", "-w", ONBOARD_DEV]);
    unsafe { libc::sync() };
    let _ = std::process::Command::new("/sbin/poweroff").status();
    std::thread::sleep(std::time::Duration::from_secs(20));
    let _ = std::process::Command::new("/sbin/poweroff")
        .arg("-f")
        .status();
}

/// Local date spelled out: "jeudi 22 octobre".
pub fn local_date(std_minutes: i32, eu_dst: bool) -> String {
    const JOURS: [&str; 7] = [
        "dimanche", "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi",
    ];
    const MOIS: [&str; 12] = [
        "janvier",
        "février",
        "mars",
        "avril",
        "mai",
        "juin",
        "juillet",
        "août",
        "septembre",
        "octobre",
        "novembre",
        "décembre",
    ];
    let now = now_utc();
    let days =
        (now + local_offset_minutes(std_minutes, eu_dst, now) as i64 * 60).div_euclid(86_400);
    let (_, m, d) = civil_from_days(days);
    format!(
        "{} {d} {}",
        JOURS[(days + 4).rem_euclid(7) as usize],
        MOIS[(m - 1) as usize]
    )
}

/// Decimal local time (0.0 → 23.99).
pub fn local_hour(std_minutes: i32, eu_dst: bool) -> f32 {
    let now = now_utc();
    let local =
        (now + local_offset_minutes(std_minutes, eu_dst, now) as i64 * 60).rem_euclid(86_400);
    local as f32 / 3600.0
}

/// Lighting level adapted to the time of day, from the day level `day`:
/// full from 7 am to 8 pm, linear descent until 11 pm to the
/// night level (a quarter of the day level, at least 3), rising again between 6 am and 7 am.
pub fn auto_light_level(day: u8, hour: f32) -> u8 {
    let day_f = day as f32;
    let night = (day_f * 0.25).max(3.0).min(day_f);
    let level = if (7.0..20.0).contains(&hour) {
        day_f
    } else if (20.0..23.0).contains(&hour) {
        day_f - (day_f - night) * (hour - 20.0) / 3.0
    } else if (6.0..7.0).contains(&hour) {
        night + (day_f - night) * (hour - 6.0)
    } else {
        night
    };
    level.round().clamp(0.0, 100.0) as u8
}

#[cfg(test)]
mod tests {
    use super::auto_light_level;
    #[test]
    fn auto_light_curve() {
        assert_eq!(auto_light_level(80, 12.0), 80);
        assert_eq!(auto_light_level(80, 21.5), 50);
        assert_eq!(auto_light_level(80, 2.0), 20);
        assert_eq!(auto_light_level(80, 6.5), 50);
        assert_eq!(auto_light_level(0, 12.0), 0);
        assert_eq!(auto_light_level(8, 2.0), 3);
    }
}

#[cfg(test)]
mod tests_heure {
    use super::*;

    /// French legal time: 2026 and 2027 changes to the second
    /// (01:00 UTC on the last Sundays of March and October).
    #[test]
    fn european_summer_time() {
        let at = |t: i64| local_offset_minutes(60, true, t);
        // 29 March 2026, 25 October 2026, 28 March 2027, 31 October 2027 at 01:00 UTC.
        for (start, end) in [
            (1_774_746_000i64, 1_792_890_000i64),
            (1_806_195_600, 1_824_944_400),
        ] {
            assert_eq!((at(start - 1), at(start)), (60, 120));
            assert_eq!((at(end - 1), at(end)), (120, 60));
        }
        // On 22 October 2026 at 09:42 UTC: 11:42 in Paris; on the 26th: 10:42.
        assert_eq!(at(1_792_662_135), 120);
        assert_eq!(at(1_792_662_135 + 4 * 86_400), 60);
        assert_eq!(local_offset_minutes(60, false, 1_792_662_135), 60);
        for days in [-1_000_000i64, -1, 0, 20_748, 1_000_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
    }
}
