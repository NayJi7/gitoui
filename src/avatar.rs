use std::{
    fs,
    io::Cursor,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
};

use image::{imageops::FilterType, GenericImageView, ImageFormat, Rgba, RgbaImage};
use md5::{Digest, Md5};
use ratatui::style::Color;
#[cfg(test)]
use ratatui::style::Style;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    event::{AppEvent, Sender},
    protocol::{ImageProtocol, PreparedImage},
};

#[cfg(test)]
use crate::protocol::PreparedImageCell;

/// Max concurrent HTTP avatar fetches.  Keeps the thread count bounded and
/// avoids saturating the connection pool or triggering GitHub rate limits.
const MAX_CONCURRENT_FETCHES: usize = 4;

/// State shared between the main thread and fetch threads.  A single Mutex
/// instead of two avoids double-locking on every `prefetch()` call.
struct FetchState {
    in_flight: FxHashSet<String>,
    missing: FxHashSet<String>,
}

pub struct AvatarManager {
    cache_dir: PathBuf,
    image_protocol: ImageProtocol,
    next_image_id: u32,
    prepared_image_map: FxHashMap<String, PreparedImage>,
    image_ids: FxHashSet<u32>,
    pending_uploads: Vec<String>,
    github_repos: Vec<String>,
    github_token: Option<String>,
    github_avatars: bool,
    fetch_state: Arc<Mutex<FetchState>>,
    active_fetches: Arc<AtomicUsize>,
    http_client: Arc<reqwest::blocking::Client>,
    event_sender: Option<Sender>,
}

impl Clone for AvatarManager {
    fn clone(&self) -> Self {
        Self {
            cache_dir: self.cache_dir.clone(),
            image_protocol: self.image_protocol,
            next_image_id: self.next_image_id,
            prepared_image_map: self.prepared_image_map.clone(),
            image_ids: self.image_ids.clone(),
            pending_uploads: Vec::new(),
            github_repos: self.github_repos.clone(),
            github_token: self.github_token.clone(),
            github_avatars: self.github_avatars,
            fetch_state: self.fetch_state.clone(),
            active_fetches: self.active_fetches.clone(),
            http_client: self.http_client.clone(),
            event_sender: self.event_sender.clone(),
        }
    }
}

impl AvatarManager {
    pub fn new(
        _image_protocol: ImageProtocol,
        github_repos: Vec<String>,
        github_token: Option<String>,
        event_sender: Option<Sender>,
    ) -> Self {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("gitui")
            .join("avatars-v5");
        let _ = fs::create_dir_all(&cache_dir);

        let http_client = Arc::new(
            reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .redirect(reqwest::redirect::Policy::limited(3))
                .build()
                .unwrap_or_default(),
        );

        Self {
            cache_dir,
            image_protocol: _image_protocol,
            next_image_id: 1,
            prepared_image_map: FxHashMap::default(),
            image_ids: FxHashSet::default(),
            pending_uploads: Vec::new(),
            github_repos,
            github_token,
            github_avatars: true,
            fetch_state: Arc::new(Mutex::new(FetchState {
                in_flight: FxHashSet::default(),
                missing: FxHashSet::default(),
            })),
            active_fetches: Arc::new(AtomicUsize::new(0)),
            http_client,
            event_sender,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.github_avatars
            && self
                .github_token
                .as_deref()
                .is_some_and(|token| !token.trim().is_empty())
    }

    pub fn set_github_avatars(&mut self, enabled: bool) {
        self.github_avatars = enabled;
        if !enabled {
            self.prepared_image_map.clear();
            self.pending_uploads.clear();
            self.image_ids.clear();
        }
    }

    pub fn set_github_token(&mut self, github_token: Option<String>) {
        self.github_token = github_token;
        let mut state = self.fetch_state.lock().unwrap();
        state.in_flight.clear();
        state.missing.clear();
        drop(state);
        if !self.is_enabled() {
            self.prepared_image_map.clear();
            self.pending_uploads.clear();
        }
    }

    fn email_to_path(&self, email: &str) -> PathBuf {
        let hash = format!("{:x}", Md5::digest(email.trim().to_lowercase()));
        self.cache_dir.join(format!("{hash}.png"))
    }

    fn image_key(email: &str, height_cells: u16, selected: bool) -> String {
        format!(
            "{}:{}:{}",
            height_cells,
            if selected { "s" } else { "n" },
            email.trim().to_lowercase()
        )
    }

    pub fn ensure_uploaded(
        &mut self,
        email: &str,
        height_cells: u16,
        selected: bool,
        bg: Color,
    ) -> bool {
        if !self.is_enabled() {
            return false;
        }
        let key = Self::image_key(email, height_cells, selected);
        if self.prepared_image_map.contains_key(&key) {
            return false;
        }
        let path = self.email_to_path(email);
        let Ok(bytes) = fs::read(&path) else {
            return false;
        };
        if bytes.is_empty() {
            return false;
        }
        let png_data = if selected {
            let Some(source) = image::load_from_memory(&bytes).ok() else {
                let _ = fs::remove_file(path);
                return false;
            };
            let Some(avatar) = rounded_avatar_on_background(&source, ratatui_color_to_rgba(bg))
            else {
                return false;
            };
            let mut out = Cursor::new(Vec::new());
            if avatar.write_to(&mut out, ImageFormat::Png).is_err() {
                return false;
            }
            out.into_inner()
        } else {
            let Ok(source) = image::load_from_memory(&bytes) else {
                let _ = fs::remove_file(path);
                return false;
            };
            let Some(avatar) = rounded_avatar_on_background(&source, ratatui_color_to_rgba(bg))
            else {
                return false;
            };
            let mut out = Cursor::new(Vec::new());
            if avatar.write_to(&mut out, ImageFormat::Png).is_err() {
                return false;
            }
            out.into_inner()
        };
        let cell_width = height_cells as usize * 2;
        let image_id = self.next_image_id;
        self.next_image_id = self.next_image_id.wrapping_add(1);
        let mut prepared = self
            .image_protocol
            .prepare_image(&png_data, cell_width, image_id);
        if let Some(upload_data) = prepared.take_upload_data() {
            self.pending_uploads.push(upload_data);
        }
        self.image_ids.insert(image_id);
        self.prepared_image_map.insert(key, prepared);
        true
    }

    pub fn prepared_image(
        &self,
        email: &str,
        height_cells: u16,
        selected: bool,
    ) -> Option<&PreparedImage> {
        self.prepared_image_map
            .get(&Self::image_key(email, height_cells, selected))
    }

    pub fn cached_avatar_exists(&self, email: &str) -> bool {
        self.email_to_path(email)
            .metadata()
            .is_ok_and(|metadata| metadata.len() > 0)
    }

    pub fn clear_prepared_images(&mut self) {
        self.prepared_image_map.clear();
        self.pending_uploads.clear();
        self.image_ids.clear();
        // Allow previously-failed lookups to be retried on the next upload cycle.
        self.fetch_state.lock().unwrap().missing.clear();
    }

    pub fn drain_pending_uploads(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_uploads)
    }

    pub fn image_ids_sorted(&self) -> Vec<u32> {
        let mut ids = self.image_ids.iter().copied().collect::<Vec<u32>>();
        ids.sort();
        ids
    }

    pub fn prefetch(&self, commit_hashes: Vec<String>, email: &str) {
        let Some(token) = self.github_token.clone() else {
            return;
        };
        if self.github_repos.is_empty() {
            return;
        }
        let email_key = email.trim().to_lowercase();

        // Single lock for all state checks — avoids double-locking
        {
            let mut state = self.fetch_state.lock().unwrap();
            if state.missing.contains(&email_key) {
                return;
            }
            // Don't add to in_flight if we're already at the concurrency limit —
            // the next prefetch call on the following frame will retry.
            if self.active_fetches.load(Ordering::Relaxed) >= MAX_CONCURRENT_FETCHES {
                return;
            }
            if !state.in_flight.insert(email_key.clone()) {
                return; // already fetching
            }
        }

        let path = self.email_to_path(email);
        if let Ok(metadata) = path.metadata() {
            if metadata.len() > 0 {
                // Already on disk — remove from in_flight without fetch
                self.fetch_state
                    .lock()
                    .unwrap()
                    .in_flight
                    .remove(&email_key);
                return;
            }
            let _ = fs::remove_file(&path);
        }

        self.active_fetches.fetch_add(1, Ordering::Relaxed);

        let path_clone = path.clone();
        let repos = self.github_repos.clone();
        let fetch_state = self.fetch_state.clone();
        let active_fetches = self.active_fetches.clone();
        let event_sender = self.event_sender.clone();
        let client = self.http_client.clone();

        thread::spawn(move || {
            let mut found_avatar = false;

            let url =
                resolve_github_commit_avatar_url(&client, &repos, &commit_hashes, &token);

            if let Some(url) = url {
                match client.get(&url).send() {
                    Ok(resp) if resp.status().is_success() => {
                        if let Ok(bytes) = resp.bytes() {
                            if bytes.len() > 100 && image::load_from_memory(&bytes).is_ok() {
                                write_avatar_atomic(&path_clone, bytes.to_vec());
                                found_avatar = true;
                            }
                        }
                    }
                    _ => {}
                }
            }

            active_fetches.fetch_sub(1, Ordering::Relaxed);
            finish_avatar_fetch(&fetch_state, &email_key, found_avatar);

            if found_avatar {
                if let Some(sender) = event_sender {
                    sender.try_send(AppEvent::AvatarsUpdated);
                }
            }
        });
    }
}

fn finish_avatar_fetch(
    fetch_state: &Arc<Mutex<FetchState>>,
    email_key: &str,
    found_avatar: bool,
) {
    let mut state = fetch_state.lock().unwrap();
    state.in_flight.remove(email_key);
    if !found_avatar {
        state.missing.insert(email_key.to_string());
    }
}

fn resolve_github_commit_avatar_url(
    client: &reqwest::blocking::Client,
    repos: &[String],
    commit_hashes: &[String],
    token: &str,
) -> Option<String> {
    for repo in repos {
        for commit_hash in commit_hashes.iter().take(8) {
            let url = format!("https://api.github.com/repos/{repo}/commits/{commit_hash}");
            let Ok(response) = client
                .get(&url)
                .header("Accept", "application/vnd.github+json")
                .header("User-Agent", "gitui")
                .bearer_auth(token)
                .send()
            else {
                continue;
            };
            if !response.status().is_success() {
                continue;
            }
            let Ok(body) = response.text() else {
                continue;
            };
            if let Some(avatar_url) = extract_avatar_url(&body) {
                return Some(avatar_url);
            }
        }
    }
    None
}

fn extract_avatar_url(body: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    json.get("author")
        .and_then(|a| a.get("avatar_url"))
        .and_then(|u| u.as_str())
        .map(|s| s.to_string())
}

fn write_avatar_atomic(path: &PathBuf, bytes: Vec<u8>) {
    if bytes.is_empty() {
        return;
    }
    let temp_path = path.with_extension("png.tmp");
    if fs::write(&temp_path, bytes).is_ok() {
        let _ = fs::rename(&temp_path, path);
    }
}

#[allow(dead_code)]
fn fallback_avatar_png(seed: &str) -> Vec<u8> {
    let digest = Md5::digest(seed.trim().to_lowercase());
    let bg = [
        80u8.saturating_add(digest[0] % 120),
        80u8.saturating_add(digest[1] % 120),
        80u8.saturating_add(digest[2] % 120),
        255,
    ];
    let fg = [
        220u8.saturating_sub(digest[3] % 80),
        220u8.saturating_sub(digest[4] % 80),
        220u8.saturating_sub(digest[5] % 80),
        255,
    ];
    let mut avatar = image::RgbaImage::new(128, 128);
    let radius_sq = 64.0f32 * 64.0f32; // squared — no sqrt needed
    let center = 63.5f32;
    for (x, y, pixel) in avatar.enumerate_pixels_mut() {
        let dx = x as f32 - center;
        let dy = y as f32 - center;
        if dx * dx + dy * dy > radius_sq {
            *pixel = Rgba([0, 0, 0, 0]);
        } else if ((x / 16 + y / 16 + digest[((x / 16) % 16) as usize] as u32) % 3) == 0 {
            *pixel = Rgba(fg);
        } else {
            *pixel = Rgba(bg);
        }
    }
    let mut out = Cursor::new(Vec::new());
    avatar.write_to(&mut out, ImageFormat::Png).unwrap();
    out.into_inner()
}

#[allow(dead_code)]
fn ratatui_color_to_rgba(color: Color) -> [u8; 4] {
    match color {
        Color::Rgb(r, g, b) => [r, g, b, 255],
        Color::Black => [0, 0, 0, 255],
        Color::Red => [255, 0, 0, 255],
        Color::Green => [0, 255, 0, 255],
        Color::Yellow => [255, 255, 0, 255],
        Color::Blue => [0, 0, 255, 255],
        Color::Magenta => [255, 0, 255, 255],
        Color::Cyan => [0, 255, 255, 255],
        Color::Gray => [128, 128, 128, 255],
        Color::DarkGray => [64, 64, 64, 255],
        Color::LightRed => [255, 128, 128, 255],
        Color::LightGreen => [128, 255, 128, 255],
        Color::LightYellow => [255, 255, 128, 255],
        Color::LightBlue => [128, 128, 255, 255],
        Color::LightMagenta => [255, 128, 255, 255],
        Color::LightCyan => [128, 255, 255, 255],
        Color::White => [255, 255, 255, 255],
        Color::Indexed(index) => indexed_color_to_rgba(index),
        Color::Reset => [0x1a, 0x1b, 0x26, 255],
    }
}

fn indexed_color_to_rgba(index: u8) -> [u8; 4] {
    const BASIC: [[u8; 3]; 16] = [
        [0, 0, 0],
        [128, 0, 0],
        [0, 128, 0],
        [128, 128, 0],
        [0, 0, 128],
        [128, 0, 128],
        [0, 128, 128],
        [192, 192, 192],
        [128, 128, 128],
        [255, 0, 0],
        [0, 255, 0],
        [255, 255, 0],
        [0, 0, 255],
        [255, 0, 255],
        [0, 255, 255],
        [255, 255, 255],
    ];

    if index < 16 {
        let [r, g, b] = BASIC[index as usize];
        return [r, g, b, 255];
    }

    if index < 232 {
        let i = index - 16;
        let channel = |value: u8| if value == 0 { 0 } else { 55 + value * 40 };
        return [channel(i / 36), channel((i % 36) / 6), channel(i % 6), 255];
    }

    let gray = 8 + (index - 232) * 10;
    [gray, gray, gray, 255]
}

#[allow(dead_code)]
fn rounded_avatar_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let image = image::load_from_memory(bytes).ok()?;
    let (width, height) = image.dimensions();
    let side = width.min(height);
    let x = (width - side) / 2;
    let y = (height - side) / 2;
    let mut avatar = image
        .crop_imm(x, y, side, side)
        .resize_exact(128, 128, FilterType::Triangle) // bilinear — much faster than Lanczos3
        .to_rgba8();
    let radius_sq = 64.0f32 * 64.0f32; // compare squared to avoid sqrt
    let center = 63.5f32;
    for (x, y, pixel) in avatar.enumerate_pixels_mut() {
        let dx = x as f32 - center;
        let dy = y as f32 - center;
        if dx * dx + dy * dy > radius_sq {
            *pixel = Rgba([pixel[0], pixel[1], pixel[2], 0]);
        }
    }
    let mut out = Cursor::new(Vec::new());
    avatar.write_to(&mut out, ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

#[cfg(test)]
fn rounded_avatar_on_background_png(bytes: &[u8], bg: [u8; 4]) -> Option<Vec<u8>> {
    let source = image::load_from_memory(bytes).ok()?;
    let avatar = rounded_avatar_on_background(&source, bg)?;
    let mut out = Cursor::new(Vec::new());
    avatar.write_to(&mut out, ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

fn rounded_avatar_on_background(source: &image::DynamicImage, bg: [u8; 4]) -> Option<RgbaImage> {
    let (width, height) = source.dimensions();
    let side = width.min(height);
    let x = (width - side) / 2;
    let y = (height - side) / 2;
    let mut image = source
        .crop_imm(x, y, side, side)
        .resize_exact(128, 128, FilterType::Triangle) // bilinear — much faster than Lanczos3
        .to_rgba8();
    let radius_sq = 64.0f32 * 64.0f32; // compare squared to avoid sqrt
    let center = 63.5f32;
    // Single pass: composite alpha onto bg AND apply circular mask
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let dx = x as f32 - center;
        let dy = y as f32 - center;
        if dx * dx + dy * dy > radius_sq {
            *pixel = Rgba(bg);
        } else {
            let alpha = pixel[3] as u16;
            let inv_alpha = 255 - alpha;
            pixel[0] = ((pixel[0] as u16 * alpha + bg[0] as u16 * inv_alpha) / 255) as u8;
            pixel[1] = ((pixel[1] as u16 * alpha + bg[1] as u16 * inv_alpha) / 255) as u8;
            pixel[2] = ((pixel[2] as u16 * alpha + bg[2] as u16 * inv_alpha) / 255) as u8;
            pixel[3] = 255;
        }
    }
    Some(image)
}

#[cfg(test)]
fn prepare_avatar_cells(bytes: &[u8], cell_width: usize) -> Option<PreparedImage> {
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    prepare_avatar_cells_from_image(&image, cell_width)
}

#[cfg(test)]
fn prepare_avatar_cells_from_image(image: &RgbaImage, cell_width: usize) -> Option<PreparedImage> {
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 || cell_width == 0 {
        return None;
    }

    let mut cells = Vec::with_capacity(cell_width);
    for column in 0..cell_width {
        let x_start = (column as u32 * width) / cell_width as u32;
        let x_end = ((column as u32 + 1) * width / cell_width as u32).max(x_start + 1);
        let y_mid = height / 2;
        let top = average_region(&image, x_start, x_end, 0, y_mid.max(1));
        let bottom = average_region(&image, x_start, x_end, y_mid, height);
        cells.push(PreparedImageCell::new(
            "▀".to_string(),
            Style::default()
                .fg(Color::Rgb(top[0], top[1], top[2]))
                .bg(Color::Rgb(bottom[0], bottom[1], bottom[2])),
            false,
        ));
    }

    Some(PreparedImage::from_cells(cells))
}

#[cfg(test)]
fn average_region(
    image: &image::RgbaImage,
    x_start: u32,
    x_end: u32,
    y_start: u32,
    y_end: u32,
) -> [u8; 3] {
    let mut total = [0u64; 3];
    let mut count = 0u64;
    for y in y_start..y_end.max(y_start + 1).min(image.height()) {
        for x in x_start..x_end.max(x_start + 1).min(image.width()) {
            let pixel = image.get_pixel(x, y);
            total[0] += pixel[0] as u64;
            total[1] += pixel[1] as u64;
            total[2] += pixel[2] as u64;
            count += 1;
        }
    }
    if count == 0 {
        return [0, 0, 0];
    }
    [
        (total[0] / count) as u8,
        (total[1] / count) as u8,
        (total[2] / count) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounded_avatar_png_masks_corners_and_keeps_center() {
        let mut source = image::RgbaImage::new(32, 32);
        for pixel in source.pixels_mut() {
            *pixel = Rgba([255, 0, 0, 255]);
        }
        let mut input = Cursor::new(Vec::new());
        source.write_to(&mut input, ImageFormat::Png).unwrap();

        let output = rounded_avatar_png(input.get_ref()).unwrap();
        let rounded = image::load_from_memory(&output).unwrap().to_rgba8();

        assert_eq!(rounded.get_pixel(0, 0)[3], 0);
        assert_eq!(rounded.get_pixel(127, 0)[3], 0);
        assert_eq!(rounded.get_pixel(64, 64)[3], 255);
    }

    #[test]
    fn rounded_avatar_on_background_replaces_corners_with_background() {
        let mut source = image::RgbaImage::new(32, 32);
        for pixel in source.pixels_mut() {
            *pixel = Rgba([255, 0, 0, 255]);
        }
        let mut input = Cursor::new(Vec::new());
        source.write_to(&mut input, ImageFormat::Png).unwrap();

        let output = rounded_avatar_on_background_png(input.get_ref(), [10, 20, 30, 255]).unwrap();
        let composited = image::load_from_memory(&output).unwrap().to_rgba8();

        assert_eq!(composited.get_pixel(0, 0).0, [10, 20, 30, 255]);
        assert_eq!(composited.get_pixel(64, 64).0, [255, 0, 0, 255]);
    }

    #[test]
    fn prepared_avatar_cells_use_terminal_colors_without_image_payloads() {
        let mut source = image::RgbaImage::new(32, 32);
        for (x, y, pixel) in source.enumerate_pixels_mut() {
            *pixel = if y < 16 {
                Rgba([200, 10, 20, 255])
            } else if x < 16 {
                Rgba([30, 180, 40, 255])
            } else {
                Rgba([40, 50, 190, 255])
            };
        }
        let mut input = Cursor::new(Vec::new());
        source.write_to(&mut input, ImageFormat::Png).unwrap();

        let rounded = rounded_avatar_on_background_png(input.get_ref(), [10, 20, 30, 255]).unwrap();
        let mut prepared = prepare_avatar_cells(&rounded, 2).unwrap();

        assert_eq!(prepared.cells().len(), 2);
        assert!(prepared.cells().iter().all(|cell| !cell.skip()));
        assert!(prepared.cells().iter().all(|cell| cell.symbol() == "▀"));
        assert_eq!(prepared.take_upload_data(), None);
    }

    #[test]
    fn fallback_avatar_png_is_round_and_deterministic() {
        let first = fallback_avatar_png("someone@example.org");
        let second = fallback_avatar_png("someone@example.org");
        let other = fallback_avatar_png("other@example.org");

        assert_eq!(first, second);
        assert_ne!(first, other);

        let avatar = image::load_from_memory(&first).unwrap().to_rgba8();
        assert_eq!(avatar.get_pixel(0, 0)[3], 0);
        assert_eq!(avatar.get_pixel(64, 64)[3], 255);
    }

    #[test]
    fn avatars_are_disabled_without_github_auth() {
        let manager = AvatarManager::new(ImageProtocol::Iterm2, Vec::new(), None, None);

        assert!(!manager.is_enabled());
    }

    #[test]
    fn avatars_are_enabled_with_github_auth() {
        let manager = AvatarManager::new(
            ImageProtocol::Iterm2,
            Vec::new(),
            Some("token".into()),
            None,
        );

        assert!(manager.is_enabled());
    }

    #[test]
    fn empty_avatar_writes_do_not_create_cache_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("avatar.png");

        write_avatar_atomic(&path, Vec::new());

        assert!(!path.exists());
    }

    #[test]
    fn extract_avatar_url_does_not_use_committer_for_author_avatar() {
        let body = r#"{
            "author": null,
            "committer": { "avatar_url": "https://avatars.githubusercontent.com/u/1?v=4" }
        }"#;

        assert_eq!(extract_avatar_url(body), None);
    }

    #[test]
    fn indexed_colors_use_xterm_palette_for_avatar_backgrounds() {
        assert_eq!(
            ratatui_color_to_rgba(Color::Indexed(60)),
            [95, 95, 135, 255]
        );
        assert_eq!(
            ratatui_color_to_rgba(Color::Indexed(235)),
            [38, 38, 38, 255]
        );
    }

    #[test]
    fn avatar_cache_key_differs_for_selected_and_normal() {
        assert_ne!(
            AvatarManager::image_key("user@example.com", 1, false),
            AvatarManager::image_key("user@example.com", 1, true)
        );
    }

    #[test]
    fn cached_avatar_exists_only_for_non_empty_cache_files() {
        let temp = tempfile::tempdir().unwrap();
        let mut manager = AvatarManager::new(
            ImageProtocol::Iterm2,
            Vec::new(),
            Some("token".into()),
            None,
        );
        manager.cache_dir = temp.path().to_path_buf();

        let cached_path = manager.email_to_path("cached@example.com");
        fs::write(&cached_path, fallback_avatar_png("cached@example.com")).unwrap();
        let empty_path = manager.email_to_path("empty@example.com");
        fs::write(&empty_path, []).unwrap();

        assert!(manager.cached_avatar_exists("cached@example.com"));
        assert!(!manager.cached_avatar_exists("empty@example.com"));
        assert!(!manager.cached_avatar_exists("missing@example.com"));
    }
}
