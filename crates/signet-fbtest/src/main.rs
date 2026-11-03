//! E-ink test bench: draws a test pattern and times each waveform.
//! Results in /mnt/onboard/.signet/fbtest.txt + capture fbtest.pgm.
use signet_fb::{Canvas, Framebuffer, Rect, Waveform};
use std::fmt::Write as _;
use std::fs;
use std::thread::sleep;
use std::time::Duration;

const OUT: &str = "/mnt/onboard/.signet";

fn main() {
    let mut log = String::new();
    let _ = fs::create_dir_all(OUT);
    let mut fb = match Framebuffer::open() {
        Ok(fb) => fb,
        Err(e) => {
            let _ = fs::write(format!("{OUT}/fbtest.txt"), format!("open /dev/fb0: {e}"));
            println!("ERREUR fb0: {e}");
            return;
        }
    };
    let _ = writeln!(log, "framebuffer: {}", fb.describe());
    let (w, h) = (fb.width(), fb.height());

    // --- 1. Test pattern ----------------------------------------------------
    let mut c = Canvas::new(w, h);
    c.clear(0xFF);
    // Ramp of 16 grays across the full width
    let band = w / 16;
    for i in 0..16u32 {
        c.fill_rect(Rect::new(i * band, 40, band, 120), (i * 17) as u8);
    }
    // Frame + black block + checkerboard
    c.stroke_rect(Rect::new(20, 200, w - 40, h - 260), 4, 0x00);
    c.fill_rect(Rect::new(60, 240, 200, 200), 0x00);
    for y in (240..440).step_by(8) {
        for x in (300..500).step_by(8) {
            let g = if ((x / 8) + (y / 8)) % 2 == 0 {
                0x00
            } else {
                0xFF
            };
            c.fill_rect(Rect::new(x, y, 8, 8), g);
        }
    }
    // Fine lines from 1 to 6 px
    for (i, t) in (1..=6u32).enumerate() {
        c.fill_rect(Rect::new(60, 480 + i as u32 * 24, w - 120, t), 0x00);
    }

    // --- 2. Full-screen GC16 (reference) -------------------------------------
    fb.blit(&c, Rect::new(0, 0, w, h));
    time(
        &mut fb,
        &mut log,
        "GC16 full (mire complète)",
        Rect::new(0, 0, w, h),
        Waveform::Gc16,
        true,
    );
    sleep(Duration::from_millis(600));

    // --- 3. Same area, each waveform, partial ---------------------------------
    let zone = Rect::new(60, 640, w - 120, 160);
    for (i, wfm) in [
        Waveform::Du,
        Waveform::A2,
        Waveform::Gl16,
        Waveform::Glr16,
        Waveform::Gc16,
        Waveform::Auto,
    ]
    .into_iter()
    .enumerate()
    {
        // Alternates black / white so that each pass actually changes the ink.
        let g = if i % 2 == 0 { 0x00 } else { 0xFF };
        c.fill_rect(zone, g);
        fb.blit(&c, zone);
        time(
            &mut fb,
            &mut log,
            &format!("{} partiel {}x{}", wfm.name(), zone.w, zone.h),
            zone,
            wfm,
            false,
        );
        sleep(Duration::from_millis(400));
    }

    // --- 4. Small area (button size) -----------------------------------------
    let small = Rect::new(w - 200, h - 120, 160, 80);
    for wfm in [Waveform::Du, Waveform::A2, Waveform::Glr16] {
        c.fill_rect(small, 0x00);
        fb.blit(&c, small);
        time(
            &mut fb,
            &mut log,
            &format!("{} petit 160x80", wfm.name()),
            small,
            wfm,
            false,
        );
        c.fill_rect(small, 0xFF);
        fb.blit(&c, small);
        time(
            &mut fb,
            &mut log,
            &format!("{} petit 160x80 (retour blanc)", wfm.name()),
            small,
            wfm,
            false,
        );
    }

    // --- 5. Capture + log ----------------------------------------------------
    let _ = fb.snapshot().save_pgm(&format!("{OUT}/fbtest.pgm"));
    let _ = fs::write(format!("{OUT}/fbtest.txt"), &log);
    println!("{log}");
}

fn time(fb: &mut Framebuffer, log: &mut String, label: &str, r: Rect, wfm: Waveform, full: bool) {
    let t0 = std::time::Instant::now();
    match fb.refresh(r, wfm, full) {
        Ok(m) => {
            let submit = t0.elapsed();
            match fb.wait(m) {
                Ok(d) => {
                    let _ = writeln!(
                        log,
                        "{label:<36} submit {:>5.1} ms  wait {:>6.1} ms",
                        ms(submit),
                        ms(d)
                    );
                }
                Err(e) => {
                    let _ = writeln!(
                        log,
                        "{label:<36} submit {:>5.1} ms  wait ERREUR {e}",
                        ms(submit)
                    );
                }
            }
        }
        Err(e) => {
            let _ = writeln!(log, "{label:<36} refresh ERREUR {e}");
        }
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}
