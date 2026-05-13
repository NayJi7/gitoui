//! Live smoke test for login-based avatars. Triggers a real GitHub
//! avatar fetch for a couple of well-known logins, polls until the
//! disk cache is populated, then verifies `ensure_uploaded_login`
//! returns a prepared image.

use gitoui::avatar::AvatarManager;
use gitoui::protocol::ImageProtocol;
use ratatui::style::Color;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn main() {
    let token = std::fs::read_to_string(
        std::env::var("HOME").unwrap() + "/.config/gitoui/github_token.toml",
    )
    .expect("read token file")
    .lines()
    .find_map(|l| {
        l.strip_prefix("token = \"")
            .and_then(|s| s.strip_suffix('"'))
    })
    .expect("token line")
    .to_string();

    // Pick an arbitrary protocol — we're driving the disk-cache +
    // prepare-image pipeline, not actually painting to a TTY. The
    // prepare path produces in-memory cells regardless.
    let manager = Arc::new(std::sync::Mutex::new(AvatarManager::new(
        ImageProtocol::Iterm2,
        vec!["NayJi7/test".into()],
        Some(token),
        None,
    )));

    let logins = ["NayJi7", "torvalds", "dtolnay"];

    println!("== Triggering prefetches ==");
    for login in &logins {
        manager.lock().unwrap().prefetch_login(login);
        println!("  prefetch_login({})", login);
    }

    let deadline = Instant::now() + Duration::from_secs(12);
    let mut remaining: Vec<&&str> = logins.iter().collect();
    while !remaining.is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
        let m = manager.lock().unwrap();
        remaining.retain(|login| {
            let cached = m.cached_avatar_exists_login(login);
            if cached {
                println!("  ✓ cached  {}", login);
            }
            !cached
        });
    }

    let mut pass = 0u32;
    let mut fail = 0u32;
    println!("\n== Disk cache verification ==");
    for login in &logins {
        let exists = manager.lock().unwrap().cached_avatar_exists_login(login);
        if exists {
            pass += 1;
            println!("  PASS  cached_avatar_exists_login({})", login);
        } else {
            fail += 1;
            println!("  FAIL  cached_avatar_exists_login({})", login);
        }
    }

    println!("\n== ensure_uploaded_login (no terminal protocol so this only proves the disk → image path doesn't panic) ==");
    for login in &logins {
        let mut m = manager.lock().unwrap();
        // height=1 cells (matches the conventional avatar size used by
        // commit list). `selected=false` is the common path.
        let uploaded = m.ensure_uploaded_login(login, 1, false, Color::Reset);
        // `ImageProtocol::None` returns a prepared image with empty
        // cells, but the call must not panic and must register the
        // login in the prepared-image map.
        let prepared = m.prepared_image_login(login, 1, false).is_some();
        if prepared {
            pass += 1;
            println!(
                "  PASS  ensure_uploaded_login({}) returned {}",
                login, uploaded
            );
        } else {
            fail += 1;
            println!(
                "  FAIL  ensure_uploaded_login({}) returned {} but prepared_image_login is None",
                login, uploaded
            );
        }
    }

    println!("\n=== RESULT: {} pass / {} fail ===", pass, fail);
    if fail > 0 {
        std::process::exit(1);
    }
}
