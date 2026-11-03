//! `signetd` — the process our udev rule launches at every boot.
//!
//! It waits for the `onboard` partition to be mounted, logs, then depending on
//! the file `/mnt/onboard/.signet/mode`:
//!
//! - `probe` (default): runs the probe and the e-ink test bench, leaves
//!   Nickel alone;
//! - `signet`: runs the probe, waits for Nickel to be ready, **stops it**
//!   and launches Signet full screen. When Signet hands back control (power
//!   button), Nickel is restarted with its original environment.
//!
//! `/mnt/onboard/.signet/disable` neutralizes everything (safety switch).
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

const ONBOARD: &str = "/mnt/onboard";
const STATE: &str = "/mnt/onboard/.signet";
const BIN: &str = "/usr/local/signet";

fn onboard_mounted() -> bool {
    fs::read_to_string("/proc/mounts")
        .map(|m| {
            m.lines()
                .any(|l| l.split_whitespace().nth(1) == Some(ONBOARD))
        })
        .unwrap_or(false)
}

/// Daemon log: always in /tmp (tmpfs), and copied line by line
/// to `.signet/boot.log` when the onboard partition is mounted. We
/// never write to onboard while it is exported over USB.
struct Log(fs::File, Instant);
impl Log {
    fn line(&mut self, s: &str) {
        let line = format!("[{:>8.3}] {s}\n", self.1.elapsed().as_secs_f64());
        let _ = self.0.write_all(line.as_bytes());
        let _ = self.0.flush();
        if Path::new(&format!("{ONBOARD}/.kobo")).is_dir() {
            with_rw(|| {
                if let Ok(mut f) = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(format!("{STATE}/boot.log"))
                {
                    let _ = f.write_all(line.as_bytes());
                }
            });
        }
    }
}

/// UTC time "YYYY-MM-DD HH:MM:SS" (the reader's clock is in UTC), to
/// date each boot in boot.log.
fn utc_stamp() -> String {
    utc_format(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
    )
}

fn utc_format(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Days since 1970 → civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Is the onboard partition mounted read-only? (Signet leaves it
/// that way at rest; see `device::with_rw` in the application.)
fn onboard_readonly() -> bool {
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

/// Writes to onboard even if it is read-only: remounts read-write
/// just long enough to write, `sync`, then makes it read-only again.
fn with_rw(f: impl FnOnce()) {
    let ro = onboard_readonly();
    if ro {
        let _ = std::process::Command::new("/bin/mount")
            .args(["-o", "remount,rw", ONBOARD])
            .status();
    }
    f();
    if ro {
        let _ = std::process::Command::new("/bin/sync").status();
        let _ = std::process::Command::new("/bin/mount")
            .args(["-o", "remount,ro", ONBOARD])
            .status();
    }
}

/// Copies /tmp/signet.log (Signet's stderr) to `.signet/signet.log`.
fn flush_signet_log() {
    if !Path::new(&format!("{ONBOARD}/.kobo")).is_dir() {
        return;
    }
    let data = fs::read("/tmp/signet.log").unwrap_or_default();
    if data.is_empty() {
        return;
    }
    with_rw(|| {
        if let Ok(mut f) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(format!("{STATE}/signet.log"))
        {
            if f.write_all(&data).is_ok() {
                let _ = fs::OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open("/tmp/signet.log");
            }
        }
    });
}

fn run(log: &mut Log, name: &str) -> Option<i32> {
    let t0 = Instant::now();
    match Command::new(format!("{BIN}/{name}")).output() {
        Ok(o) => {
            log.line(&format!(
                "{name}: code {:?} en {:.0} ms",
                o.status.code(),
                t0.elapsed().as_secs_f64() * 1000.0
            ));
            for l in String::from_utf8_lossy(&o.stdout).lines().take(60) {
                log.line(&format!("  | {l}"));
            }
            for l in String::from_utf8_lossy(&o.stderr).lines().take(60) {
                log.line(&format!("  ! {l}"));
            }
            o.status.code()
        }
        Err(e) => {
            log.line(&format!("{name}: impossible de lancer : {e}"));
            None
        }
    }
}

/// Finds the pids whose command name is `name`.
fn pids_of(name: &str) -> Vec<i32> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir("/proc") else {
        return out;
    };
    for e in rd.filter_map(|e| e.ok()) {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        let comm = fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
        if comm.trim() == name {
            out.push(pid);
        }
    }
    out
}

fn environ_of(pid: i32) -> HashMap<String, String> {
    fs::read(format!("/proc/{pid}/environ"))
        .map(|b| {
            b.split(|c| *c == 0)
                .filter_map(|kv| {
                    let s = String::from_utf8_lossy(kv);
                    let (k, v) = s.split_once('=')?;
                    Some((k.to_string(), v.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn kill_all(names: &[&str], sig: i32) {
    for n in names {
        for pid in pids_of(n) {
            // SAFETY: sending a signal to a known pid.
            unsafe { libc::kill(pid, sig) };
        }
    }
}

const NICKEL_FRIENDS: &[&str] = &[
    "nickel",
    "hindenburg",
    "sickel",
    "fickel",
    "fontickel",
    "adobehost",
    "foxitpdf",
    "iink",
    "dhcpcd-dbus",
    "dhcpcd",
    "udhcpc",
    "wpa_supplicant",
    "fmon",
];

fn sh(cmd: &str, args: &[&str]) -> bool {
    Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn platform() -> String {
    let compat = fs::read_to_string("/proc/device-tree/compatible").unwrap_or_default();
    for (needle, name) in [
        ("imx6ull", "mx6ull-ntx"),
        ("imx6sll", "mx6sll-ntx"),
        ("imx6sl", "mx6sl-ntx"),
    ] {
        if compat.contains(needle) {
            return name.to_string();
        }
    }
    "mx6ull-ntx".to_string()
}

/// Reads `.signet/wifi.conf` (`ssid=` and `psk=` lines).
fn wifi_conf() -> Option<(String, String)> {
    let txt = fs::read_to_string(format!("{STATE}/wifi.conf")).ok()?;
    let mut ssid = None;
    let mut psk = None;
    for l in txt.lines() {
        if let Some(v) = l.strip_prefix("ssid=") {
            ssid = Some(v.trim().to_string());
        }
        if let Some(v) = l.strip_prefix("psk=") {
            psk = Some(v.trim().to_string());
        }
    }
    Some((ssid?, psk?))
}

/// Development mode: Wi-Fi brought up by ourselves (driver, wpa_supplicant,
/// firmware DHCP) and `sshd` with our configuration and our key. The
/// reader is then reachable on the local network; the USB cable remains a
/// disk, as always.
fn dev_network(log: &mut Log) {
    let Some((ssid, psk)) = wifi_conf() else {
        log.line("dev : pas de .signet/wifi.conf (ssid=/psk=), Wi-Fi non monté");
        return;
    };
    let plat = platform();
    for m in ["sdio_wifi_pwr", "8189fs"] {
        let ok = sh("/sbin/insmod", &[&format!("/drivers/{plat}/wifi/{m}.ko")]);
        log.line(&format!(
            "dev : insmod {m} : {}",
            if ok {
                "ok"
            } else {
                "échec (déjà chargé ?)"
            }
        ));
    }
    let iface = if Path::new("/sys/class/net/eth0").exists() {
        "eth0"
    } else {
        "wlan0"
    };
    for _ in 0..20 {
        if Path::new(&format!("/sys/class/net/{iface}")).exists() {
            break;
        }
        sleep(Duration::from_millis(250));
    }
    let _ = sh("/sbin/ifconfig", &[iface, "up"]);
    let conf = format!("ctrl_interface=/var/run/wpa_supplicant\nnetwork={{\n\tssid=\"{ssid}\"\n\tpsk=\"{psk}\"\n}}\n");
    let _ = fs::write("/tmp/signet-wpa.conf", conf);
    let _ = fs::create_dir_all("/var/run/wpa_supplicant");
    kill_all(&["wpa_supplicant", "udhcpc"], libc::SIGTERM);
    let ok = sh(
        "/sbin/wpa_supplicant",
        &[
            "-D",
            "wext",
            "-s",
            "-i",
            iface,
            "-O",
            "/var/run/wpa_supplicant",
            "-c",
            "/tmp/signet-wpa.conf",
            "-B",
        ],
    ) || sh(
        "/usr/sbin/wpa_supplicant",
        &[
            "-D",
            "wext",
            "-s",
            "-i",
            iface,
            "-O",
            "/var/run/wpa_supplicant",
            "-c",
            "/tmp/signet-wpa.conf",
            "-B",
        ],
    ) || sh(
        "/bin/wpa_supplicant",
        &[
            "-D",
            "wext",
            "-s",
            "-i",
            iface,
            "-O",
            "/var/run/wpa_supplicant",
            "-c",
            "/tmp/signet-wpa.conf",
            "-B",
        ],
    );
    log.line(&format!(
        "dev : wpa_supplicant ({ssid}) : {}",
        if ok { "lancé" } else { "échec" }
    ));
    // DHCP in the background (the firmware script configures the address and the route).
    let _ = Command::new("/sbin/udhcpc")
        .args([
            "-S",
            "-i",
            iface,
            "-s",
            "/etc/udhcpc.d/default.script",
            "-t",
            "15",
            "-T",
            "10",
            "-A",
            "3",
            "-b",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    // The firmware's sshd, with our own configuration and authorized key.
    let _ = fs::create_dir_all("/var/empty");
    let _ = sh("/bin/addgroup", &["--gid", "999", "sshd"]);
    let _ = sh("/bin/mkdir", &["-p", "/dev/pts"]);
    let _ = sh("/bin/mount", &["-t", "devpts", "devpts", "/dev/pts"]);
    let ok = sh("/usr/sbin/sshd", &["-f", "/usr/local/signet/sshd_config"]);
    log.line(&format!(
        "dev : sshd : {}",
        if ok { "démarré" } else { "échec" }
    ));
    // IP address: wait for DHCP, note it for Signet and for the Mac.
    for _ in 0..60 {
        sleep(Duration::from_millis(500));
        if let Some(ip) = iface_ip(iface) {
            log.line(&format!("dev : {iface} = {ip}"));
            let _ = fs::write("/tmp/signet-ip", &ip);
            with_rw(|| {
                let _ = fs::write(format!("{STATE}/ip.txt"), &ip);
            });
            return;
        }
    }
    log.line("dev : pas d'adresse IP obtenue (Wi-Fi hors de portée ou mot de passe faux ?)");
}

/// IPv4 address of an interface, via ifconfig's output (busybox).
fn iface_ip(iface: &str) -> Option<String> {
    let o = Command::new("/sbin/ifconfig").arg(iface).output().ok()?;
    let txt = String::from_utf8_lossy(&o.stdout);
    for l in txt.lines() {
        if let Some(i) = l.find("inet addr:") {
            let rest = &l[i + 10..];
            return rest.split_whitespace().next().map(String::from);
        }
    }
    None
}

/// Stops Nickel and its satellites. Returns Nickel's environment.
fn stop_nickel(log: &mut Log) -> HashMap<String, String> {
    let env = pids_of("nickel")
        .first()
        .map(|p| environ_of(*p))
        .unwrap_or_default();
    log.line(&format!(
        "arrêt de Nickel (PLATFORM={:?}, PRODUCT={:?}, INTERFACE={:?})",
        env.get("PLATFORM"),
        env.get("PRODUCT"),
        env.get("INTERFACE")
    ));
    let _ = Command::new("/bin/sync").status();
    log.line("sync fait");
    kill_all(NICKEL_FRIENDS, libc::SIGTERM);
    for _ in 0..40 {
        if pids_of("nickel").is_empty() {
            break;
        }
        sleep(Duration::from_millis(100));
    }
    if !pids_of("nickel").is_empty() {
        log.line("Nickel ne répond pas à SIGTERM, SIGKILL");
        kill_all(&["nickel"], libc::SIGKILL);
    }
    log.line("Nickel arrêté");
    env
}

/// Restarts Nickel the way the firmware's init script does.
fn start_nickel(log: &mut Log, env: &HashMap<String, String>) {
    log.line("relance de Nickel");
    const KEYS: [&str; 13] = [
        "PLATFORM",
        "PRODUCT",
        "INTERFACE",
        "WIFI_MODULE",
        "WIFI_MODULE_PATH",
        "DBUS_SESSION_BUS_ADDRESS",
        "NICKEL_HOME",
        "LANG",
        "PATH",
        "HOME",
        "QT_QPA_PLATFORM",
        "QWS_DISPLAY",
        "QT_GSTREAMER_PLAYBIN_AUDIOSINK",
    ];
    // Our own environment first (that of rcS if launched by rcS),
    // then whatever we had captured from Nickel if it ran.
    let mut base: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| KEYS.contains(&k.as_str()))
        .collect();
    for (k, v) in env.iter() {
        if KEYS.contains(&k.as_str()) && !base.iter().any(|(bk, _)| bk == k) {
            base.push((k.clone(), v.clone()));
        }
    }
    for (k, v) in [
        ("PLATFORM", platform()),
        ("PRODUCT", "luna".to_string()),
        ("INTERFACE", "eth0".to_string()),
        ("WIFI_MODULE", "8189fs".to_string()),
        ("NICKEL_HOME", "/mnt/onboard/.kobo".to_string()),
        ("LANG", "en_US.UTF-8".to_string()),
        ("HOME", "/".to_string()),
    ] {
        if !base.iter().any(|(bk, _)| bk == k) {
            base.push((k.to_string(), v));
        }
    }
    if !base.iter().any(|(k, _)| k == "PATH") {
        base.push((
            "PATH".into(),
            "/usr/local/Kobo:/usr/bin:/bin:/usr/sbin:/sbin".into(),
        ));
    }
    base.push(("LD_LIBRARY_PATH".into(), "/usr/local/Kobo".into()));
    let spawn = |cmd: &str, args: &[&str]| {
        Command::new(cmd)
            .args(args)
            .envs(base.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    };
    if let Err(e) = spawn("/usr/local/Kobo/hindenburg", &[]) {
        log.line(&format!("hindenburg : {e}"));
    }
    match spawn(
        "/usr/local/Kobo/nickel",
        &["-platform", "kobo", "-skipFontLoad"],
    ) {
        Ok(_) => log.line("Nickel relancé"),
        Err(e) => log.line(&format!("nickel : {e}")),
    }
    let _ = spawn("/sbin/udevadm", &["trigger"]);
}

/// Marker in /tmp (tmpfs, erased at every boot): the udev rule
/// may launch us several times per boot (Nickel runs `udevadm trigger`
/// at every start, which re-emits the loop0 event); we work only
/// once.
const ONCE: &str = "/tmp/signetd.ran";

/// Kernel lines about the RC5T619's power-off history.
fn poweroff_cause() -> Option<String> {
    let out = std::process::Command::new("/bin/dmesg").output().ok()?;
    parse_poweroff(&String::from_utf8_lossy(&out.stdout))
}

fn parse_poweroff(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let i = lines.iter().position(|l| l.contains("POWER off history"))?;
    let clean = |l: &str| {
        l.split(" : ")
            .skip(2)
            .collect::<Vec<_>>()
            .join(" : ")
            .trim()
            .to_string()
    };
    let mut cause = clean(lines[i]);
    // The next line names the cause ("Long power on key press"…), if there is one.
    if let Some(next) = lines
        .get(i + 1)
        .filter(|l| l.contains("ricoh61x_init_battery") && !l.contains("0x"))
    {
        cause = format!("{cause} ({})", clean(next));
    }
    Some(cause)
}

/// Reboot. Two shutdowns stayed stuck until a long press on the
/// button (cause unknown, both in dev mode, Wi-Fi loaded; the Wi-Fi SDIO
/// driver is the prime suspect). Before `reboot`: the partition is made
/// read-only, the hardware watchdog is armed, then the Wi-Fi is
/// unloaded. Opening `/dev/watchdog` stops the kernel timer that
/// was feeding it; the daemon disappears with the shutdown without closing it
/// cleanly, so the watchdog stays armed: if the reboot has not completed
/// by its timeout (60 s), the chip restarts on its own, onto a clean
/// partition. Nothing more is written to the partition once the watchdog is armed.
fn safe_reboot(log: &mut Log) {
    let timeout = fs::read_to_string("/sys/class/watchdog/watchdog0/timeout")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "?".into());
    log.line(&format!(
        "redémarrage : partition en lecture seule, chien de garde ({timeout} s), Wi-Fi déchargé"
    ));
    let _ = Command::new("/bin/sync").status();
    if onboard_mounted() && !onboard_readonly() {
        let _ = Command::new("/bin/mount")
            .args(["-o", "remount,ro", ONBOARD])
            .status();
    }
    match fs::OpenOptions::new().write(true).open("/dev/watchdog") {
        Ok(mut f) => {
            let _ = f.write_all(b"1");
            // Never closed by us: closing without a "V" leaves it armed.
            std::mem::forget(f);
        }
        Err(e) => {
            let _ = writeln!(log.0, "chien de garde indisponible : {e}");
        }
    }
    kill_all(&["wpa_supplicant", "udhcpc", "dhcpcd"], libc::SIGTERM);
    for iface in ["eth0", "wlan0"] {
        if Path::new(&format!("/sys/class/net/{iface}")).exists() {
            let _ = sh("/sbin/ifconfig", &[iface, "down"]);
        }
    }
    for m in ["8189fs", "8192es", "sdio_wifi_pwr"] {
        let _ = sh("/sbin/rmmod", &[m]);
    }
    let _ = Command::new("/sbin/reboot").status();
}

fn main() {
    let boot = Instant::now();
    // `signetd reboot`: the protected reboot, for tools (SSH).
    if std::env::args().nth(1).as_deref() == Some("reboot") {
        let Ok(f) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/signetd.log")
        else {
            return;
        };
        let mut log = Log(f, boot);
        safe_reboot(&mut log);
        return;
    }
    // When rcS is patched, the instance launched by rcS (with its
    // environment: PLATFORM, PRODUCT, DBUS…) must do the work; the one
    // from the udev rule steps aside. Without the patch, it is the opposite.
    let from_rcs = std::env::var_os("SIGNET_FROM_RCS").is_some();
    if Path::new("/usr/local/signet/rcs-patched").exists() && !from_rcs {
        return;
    }
    if Path::new(ONCE).exists() {
        return;
    }
    let _ = fs::write(ONCE, "1");
    while !onboard_mounted() {
        if boot.elapsed() > Duration::from_secs(60) {
            return;
        }
        sleep(Duration::from_millis(250));
    }
    if Path::new(&format!("{STATE}/disable")).exists() {
        return;
    }
    let _ = fs::create_dir_all(STATE);
    let Ok(f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/signetd.log")
    else {
        return;
    };
    let mut log = Log(f, boot);
    let mode = fs::read_to_string(format!("{STATE}/mode"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "probe".into());
    log.line(&format!(
        "---- signetd {} démarre (mode {mode}, lancé par {}) {} UTC ----",
        option_env!("SIGNET_BUILD").unwrap_or("dev"),
        if from_rcs { "rcS" } else { "udev" },
        utc_stamp()
    ));
    log.line(&format!(
        "onboard monté après {:.0} ms ; uptime {}",
        boot.elapsed().as_secs_f64() * 1000.0,
        fs::read_to_string("/proc/uptime")
            .unwrap_or_default()
            .trim()
    ));
    // Cause of the last power-off, kept by the power controller
    // (RC5T619 POFFHIS register, which the kernel prints at boot): an
    // unexplained shutdown leaves at least this trace. `dmesg` runs separately and boot
    // does not wait for it more than half a second; the write stays here, in the
    // main thread (two simultaneous writes would fight over the partition).
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(poweroff_cause());
    });
    if let Ok(Some(cause)) = rx.recv_timeout(Duration::from_millis(500)) {
        log.line(&format!("dernière mise hors tension : {cause}"));
    }

    // Init script rollback requested by the disk: .signet/restore-rcs.
    if Path::new(&format!("{STATE}/restore-rcs")).exists() {
        match fs::copy("/etc/init.d/rcS.orig", "/etc/init.d/rcS") {
            Ok(_) => log.line("rcS d'origine restauré (demande .signet/restore-rcs)"),
            Err(e) => log.line(&format!("restauration rcS : {e}")),
        }
        let _ = fs::remove_file(format!("{STATE}/restore-rcs"));
        let _ = fs::remove_file("/usr/local/signet/rcs-patched");
        let _ = Command::new("/bin/sync").status();
    }

    // Cleanup of a previous NickelMenu installation (initial stage 0).
    for p in [
        "/usr/local/Kobo/imageformats/libnm.so",
        "/usr/local/signet/signet-hello",
    ] {
        if Path::new(p).exists() && fs::remove_file(p).is_ok() {
            log.line(&format!("supprimé {p}"));
        }
    }
    let nm = format!("{ONBOARD}/.adds/nm");
    if Path::new(&nm).exists() && fs::remove_dir_all(&nm).is_ok() {
        let _ = fs::remove_dir(format!("{ONBOARD}/.adds"));
        log.line("supprimé .adds/nm");
    }

    // The probe (7 s) runs only once: its report is on the partition.
    if !Path::new(&format!("{STATE}/probe.txt")).exists() {
        run(&mut log, "signet-probe");
    }

    let dev = Path::new(&format!("{STATE}/dev")).exists();

    match mode.as_str() {
        "signet" => {
            // Nickel has nothing to do for us: rcS already mounted onboard and set
            // the time. We stop it as soon as its process appears, and we also cut
            // the boot animation so it does not paint over
            // Signet. Wait at most 30 s, then go on without it.
            // If rcS is patched, Nickel will never be launched: no need to wait for it.
            if !Path::new("/usr/local/signet/rcs-patched").exists() {
                for _ in 0..300 {
                    if !pids_of("nickel").is_empty() {
                        break;
                    }
                    sleep(Duration::from_millis(100));
                }
            } else {
                log.line("rcS patché : Nickel non attendu");
            }
            kill_all(&["on-animator.sh", "update-animator.sh"], libc::SIGTERM);
            let env = stop_nickel(&mut log);
            if dev {
                // Nickel stopped: Wi-Fi + sshd, in parallel with Signet (DHCP takes
                // 15 s, the reader need not wait). Partition hook copied
                // to /tmp (a script read from the partition would block the USB export).
                let hook = format!("{STATE}/dev.sh");
                if Path::new(&hook).exists() && fs::copy(&hook, "/tmp/dev.sh").is_ok() {
                    match Command::new("/bin/sh")
                        .arg("/tmp/dev.sh")
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()
                    {
                        Ok(_) => log.line("dev : hook dev.sh lancé en arrière-plan"),
                        Err(e) => log.line(&format!("dev : hook : {e}")),
                    }
                } else {
                    dev_network(&mut log);
                }
            }
            if !dev {
                // Wi-Fi left active by Nickel prevents sleep (SDIO driver).
                kill_all(&["wpa_supplicant", "udhcpc", "dhcpcd"], libc::SIGTERM);
                for iface in ["eth0", "wlan0"] {
                    if Path::new(&format!("/sys/class/net/{iface}")).exists() {
                        let _ = sh("/sbin/ifconfig", &[iface, "down"]);
                    }
                }
                for m in ["8189fs", "8192es", "sdio_wifi_pwr"] {
                    if sh("/sbin/rmmod", &[m]) {
                        log.line(&format!("wifi : {m} déchargé"));
                    }
                }
            }
            // E-ink test bench only once (as long as fbtest.txt does not exist), screen free.
            if !Path::new(&format!("{STATE}/fbtest.txt")).exists() {
                run(&mut log, "signet-fbtest");
            }
            log.line(&format!(
                "uptime {} : lancement de Signet",
                fs::read_to_string("/proc/uptime")
                    .unwrap_or_default()
                    .trim()
            ));
            // Signet's output streamed to /tmp/signet.log; Signet and the
            // daemon copy it to the partition when it is mounted.
            // Signet runs; if it crashes (code ≠ 0 and ≠ 3, or signal), we
            // restart it up to three times before falling back to Nickel.
            let mut attempts = 0;
            loop {
                attempts += 1;
                let slog = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open("/tmp/signet.log")
                    .ok();
                let t0 = Instant::now();
                let status = match slog {
                    Some(f) => Command::new(format!("{BIN}/signet"))
                        .current_dir("/")
                        .stdin(Stdio::null())
                        .stdout(Stdio::from(f.try_clone().unwrap()))
                        .stderr(Stdio::from(f))
                        .status(),
                    None => Command::new(format!("{BIN}/signet"))
                        .current_dir("/")
                        .status(),
                };
                let code = status.as_ref().ok().and_then(|s| s.code());
                log.line(&format!(
                    "Signet terminé ({code:?}) après {:.0} s",
                    t0.elapsed().as_secs_f64()
                ));
                flush_signet_log();
                match code {
                    Some(3) => {
                        log.line("redémarrage demandé");
                        safe_reboot(&mut log);
                        return;
                    }
                    Some(0) => break,
                    // New version installed: restart requested by Signet itself
                    // (file /tmp/signet-restart), without rebooting the reader.
                    Some(4) => {
                        log.line("relance de Signet demandée (nouvelle version)");
                        attempts = 0;
                        continue;
                    }
                    _ if attempts < 3 && t0.elapsed() > Duration::from_secs(5) => {
                        log.line(&format!(
                            "Signet s'est arrêté anormalement, relance ({attempts}/3)"
                        ));
                        sleep(Duration::from_secs(1));
                        continue;
                    }
                    _ => {
                        log.line("Signet s'arrête anormalement en boucle : repli sur Nickel");
                        break;
                    }
                }
            }
            start_nickel(&mut log, &env);
        }
        _ => {
            if dev {
                dev_network(&mut log);
            }
            sleep(Duration::from_secs(25));
            log.line(&format!(
                "uptime {}",
                fs::read_to_string("/proc/uptime")
                    .unwrap_or_default()
                    .trim()
            ));
            run(&mut log, "signet-fbtest");
        }
    }
    log.line("---- signetd terminé ----");
}

#[cfg(test)]
mod tests {
    #[test]
    fn utc_format_matches_civil_calendar() {
        assert_eq!(super::utc_format(0), "1970-01-01 00:00:00");
        assert_eq!(super::utc_format(951_782_400), "2000-02-29 00:00:00");
        assert_eq!(super::utc_format(1_761_000_000), "2025-10-20 22:40:00");
        assert_eq!(super::utc_format(4_102_444_799), "2099-12-31 23:59:59");
    }
}

#[cfg(test)]
mod tests_poweroff {
    /// Real kernel lines from the Nia.
    #[test]
    fn reads_the_power_off_history() {
        let dmesg = "usbhid: USB HID core driver\nPMU : ricoh61x_init_battery : POWER off history 0x0a is 0x01 \nPMU : ricoh61x_init_battery : Long power on key press\nPMU : ricoh61x_init_battery : POWER control function 0x0d is 0x00 \n";
        assert_eq!(
            super::parse_poweroff(dmesg).as_deref(),
            Some("POWER off history 0x0a is 0x01 (Long power on key press)")
        );
        let plain = "PMU : ricoh61x_init_battery : POWER off history 0x0a is 0x00 \nPMU : ricoh61x_init_battery : POWER control function 0x0d is 0x00\n";
        assert_eq!(
            super::parse_poweroff(plain).as_deref(),
            Some("POWER off history 0x0a is 0x00")
        );
        assert_eq!(super::parse_poweroff("rien"), None);
    }
}
