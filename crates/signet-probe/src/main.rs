//! Hardware probe: runs ON the Kobo and writes a text report to
//! /mnt/onboard/.signet/ (visible from the Mac over USB). No dependencies,
//! no `unsafe`: it only reads /proc and /sys, and copies a few
//! files from /etc useful for understanding how the firmware boots.
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

const OUT_DIR: &str = "/mnt/onboard/.signet";

fn read(p: &str) -> String {
    fs::read_to_string(p).unwrap_or_else(|e| format!("<erreur: {e}>"))
}

fn section(out: &mut String, title: &str, body: impl AsRef<str>) {
    let _ = writeln!(out, "\n==== {title} ====\n{}", body.as_ref().trim_end());
}

fn ls(dir: &str) -> String {
    let mut s = String::new();
    match fs::read_dir(dir) {
        Ok(rd) => {
            let mut names: Vec<String> = rd
                .filter_map(|e| e.ok())
                .map(|e| {
                    let p = e.path();
                    let kind = if p.is_dir() {
                        "d"
                    } else if p.is_symlink() {
                        "l"
                    } else {
                        "-"
                    };
                    let target = fs::read_link(&p)
                        .map(|t| format!(" -> {}", t.display()))
                        .unwrap_or_default();
                    format!("{kind} {}{target}", e.file_name().to_string_lossy())
                })
                .collect();
            names.sort();
            s = names.join("\n");
        }
        Err(e) => {
            let _ = write!(s, "<erreur: {e}>");
        }
    }
    s
}

/// Reads each file (non-recursive) of a sysfs directory and returns "name = value".
fn sysfs_dump(dir: &str) -> String {
    let mut s = String::new();
    if let Ok(rd) = fs::read_dir(dir) {
        let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            if p.is_file() {
                let v = fs::read_to_string(&p)
                    .map(|v| v.trim().to_string())
                    .unwrap_or_else(|e| format!("<{e}>"));
                let _ = writeln!(s, "{} = {}", e.file_name().to_string_lossy(), v);
            }
        }
    } else {
        let _ = write!(s, "<absent>");
    }
    s
}

fn copy_tree(src: &str, dst: &Path, max_bytes: u64) {
    let Ok(rd) = fs::read_dir(src) else { return };
    let _ = fs::create_dir_all(dst);
    for e in rd.filter_map(|e| e.ok()) {
        let p = e.path();
        let target = dst.join(e.file_name());
        if p.is_dir() {
            copy_tree(&p.to_string_lossy(), &target, max_bytes);
        } else if p.is_file()
            && fs::metadata(&p)
                .map(|m| m.len() <= max_bytes)
                .unwrap_or(false)
        {
            let _ = fs::copy(&p, &target);
        }
    }
}

fn main() {
    let mut out = String::new();
    let _ = writeln!(out, "signet-probe — rapport matériel Kobo");

    section(&mut out, "/proc/version", read("/proc/version"));
    section(&mut out, "/proc/cmdline", read("/proc/cmdline"));
    section(&mut out, "/proc/cpuinfo", read("/proc/cpuinfo"));
    section(&mut out, "/proc/meminfo", read("/proc/meminfo"));
    section(&mut out, "/proc/mounts", read("/proc/mounts"));
    section(&mut out, "/proc/partitions", read("/proc/partitions"));
    section(&mut out, "/proc/modules", read("/proc/modules"));
    section(
        &mut out,
        "/proc/device-tree/model",
        read("/proc/device-tree/model"),
    );
    section(
        &mut out,
        "/proc/device-tree/compatible",
        read("/proc/device-tree/compatible").replace('\0', " "),
    );
    section(
        &mut out,
        "/proc/bus/input/devices",
        read("/proc/bus/input/devices"),
    );

    // E-ink framebuffer
    section(
        &mut out,
        "ls /sys/class/graphics",
        ls("/sys/class/graphics"),
    );
    section(
        &mut out,
        "/sys/class/graphics/fb0",
        sysfs_dump("/sys/class/graphics/fb0"),
    );
    section(
        &mut out,
        "/sys/class/graphics/fb0/device",
        sysfs_dump("/sys/class/graphics/fb0/device"),
    );
    section(&mut out, "ls /dev (fb, input, mmc, ttymxc…)", {
        let mut v: Vec<String> = ls("/dev").lines().map(String::from).collect();
        v.retain(|l| {
            l.contains("fb")
                || l.contains("input")
                || l.contains("mmc")
                || l.contains("tty")
                || l.contains("mem")
                || l.contains("ntx")
        });
        v.join("\n")
    });
    section(&mut out, "ls /dev/input", ls("/dev/input"));

    // Inputs (touchscreen, buttons)
    for i in 0..4 {
        let base = format!("/sys/class/input/event{i}/device");
        if Path::new(&base).exists() {
            let mut s = String::new();
            for f in ["name", "phys", "uniq", "modalias"] {
                let _ = writeln!(s, "{f} = {}", read(&format!("{base}/{f}")).trim());
            }
            let _ = writeln!(
                s,
                "capabilities/abs = {}",
                read(&format!("{base}/capabilities/abs")).trim()
            );
            let _ = writeln!(
                s,
                "capabilities/key = {}",
                read(&format!("{base}/capabilities/key")).trim()
            );
            let _ = writeln!(
                s,
                "capabilities/ev  = {}",
                read(&format!("{base}/capabilities/ev")).trim()
            );
            section(&mut out, &format!("input event{i}"), s);
        }
    }

    // Frontlight, battery, LEDs, power
    section(
        &mut out,
        "ls /sys/class/backlight",
        ls("/sys/class/backlight"),
    );
    if let Ok(rd) = fs::read_dir("/sys/class/backlight") {
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path().to_string_lossy().to_string();
            section(&mut out, &format!("backlight {p}"), sysfs_dump(&p));
        }
    }
    section(
        &mut out,
        "ls /sys/class/power_supply",
        ls("/sys/class/power_supply"),
    );
    if let Ok(rd) = fs::read_dir("/sys/class/power_supply") {
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path().to_string_lossy().to_string();
            section(&mut out, &format!("power_supply {p}"), sysfs_dump(&p));
        }
    }
    section(&mut out, "ls /sys/class/leds", ls("/sys/class/leds"));
    section(&mut out, "/sys/power/state", read("/sys/power/state"));
    section(
        &mut out,
        "/sys/power (tous les fichiers)",
        sysfs_dump("/sys/power"),
    );
    section(
        &mut out,
        "ls /usr/local/Kobo/udev",
        ls("/usr/local/Kobo/udev"),
    );
    section(&mut out, "ls /lib/udev/rules.d", ls("/lib/udev/rules.d"));
    section(&mut out, "ls /drivers", ls("/drivers"));
    if let Ok(rd) = fs::read_dir("/drivers") {
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path();
            let mut tree = String::new();
            fn walk(p: &Path, depth: usize, out: &mut String) {
                if let Ok(rd) = fs::read_dir(p) {
                    let mut es: Vec<_> = rd.filter_map(|e| e.ok()).collect();
                    es.sort_by_key(|e| e.file_name());
                    for e in es {
                        let _ = writeln!(
                            out,
                            "{}{}",
                            "  ".repeat(depth),
                            e.file_name().to_string_lossy()
                        );
                        if e.path().is_dir() && depth < 4 {
                            walk(&e.path(), depth + 1, out);
                        }
                    }
                }
            }
            walk(&p, 0, &mut tree);
            section(&mut out, &format!("arbre {}", p.display()), tree);
        }
    }
    section(
        &mut out,
        "/sys/devices/system/cpu/cpu0/cpufreq",
        sysfs_dump("/sys/devices/system/cpu/cpu0/cpufreq"),
    );
    section(&mut out, "/sys/class/net", ls("/sys/class/net"));

    // Kobo userland
    section(&mut out, "ls /usr/local/Kobo", ls("/usr/local/Kobo"));
    section(&mut out, "ls /etc/init.d", ls("/etc/init.d"));
    section(&mut out, "ls /etc/udev/rules.d", ls("/etc/udev/rules.d"));
    section(&mut out, "ls /lib", ls("/lib"));
    section(&mut out, "ls /usr/lib", ls("/usr/lib"));
    section(
        &mut out,
        "ls /bin + /usr/bin (busybox ?)",
        format!("{}\n--\n{}", ls("/bin"), ls("/usr/bin")),
    );
    section(
        &mut out,
        "ls /sbin + /usr/sbin",
        format!("{}\n--\n{}", ls("/sbin"), ls("/usr/sbin")),
    );
    section(&mut out, "ls /usr/local", ls("/usr/local"));
    section(&mut out, "ls /mnt", ls("/mnt"));

    // Environment + processes
    let mut procs = String::new();
    if let Ok(rd) = fs::read_dir("/proc") {
        let mut pids: Vec<u32> = rd
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().to_string_lossy().parse().ok())
            .collect();
        pids.sort_unstable();
        for pid in pids {
            let cmd = read(&format!("/proc/{pid}/cmdline")).replace('\0', " ");
            let status = read(&format!("/proc/{pid}/status"));
            let rss = status
                .lines()
                .find(|l| l.starts_with("VmRSS"))
                .unwrap_or("")
                .trim();
            if !cmd.trim().is_empty() {
                let _ = writeln!(procs, "{pid:>5}  {rss:<22} {}", cmd.trim());
                if cmd.contains("nickel") {
                    let env = read(&format!("/proc/{pid}/environ")).replace('\0', "\n");
                    section(&mut out, "environnement de nickel", env);
                }
            }
        }
    }
    section(&mut out, "processus (pid, RSS, cmdline)", procs);

    // Copy of /etc and of Nickel's udev scripts (small files) for offline analysis.
    let etc_dst = Path::new(OUT_DIR).join("etc");
    copy_tree("/etc", &etc_dst, 256 * 1024);
    copy_tree(
        "/usr/local/Kobo/udev",
        &Path::new(OUT_DIR).join("kobo-udev"),
        256 * 1024,
    );
    copy_tree(
        "/lib/udev/rules.d",
        &Path::new(OUT_DIR).join("lib-udev-rules"),
        256 * 1024,
    );

    let _ = fs::create_dir_all(OUT_DIR);
    let report = Path::new(OUT_DIR).join("probe.txt");
    fs::write(&report, &out).expect("écriture du rapport");
    // Short output for the NickelMenu dialog box (cmd_output).
    println!(
        "Sonde OK → .signet/probe.txt ({} Ko)\n{}",
        out.len() / 1024,
        read("/proc/version").trim()
    );
}
