use std::{fs, io::Cursor, path::PathBuf, thread};

use image::{imageops::FilterType, GenericImageView, ImageFormat, Rgba};
use md5::{Digest, Md5};
use ratatui::style::Color;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::protocol::{ImageProtocol, PreparedImage};

pub const AVATARS_ENABLED: bool = false;

pub struct AvatarManager {
    cache_dir: PathBuf,
    image_protocol: ImageProtocol,
    next_image_id: u32,
    prepared_image_map: FxHashMap<String, PreparedImage>,
    image_ids: FxHashSet<u32>,
    pending_uploads: Vec<String>,
    github_repos: Vec<String>,
}

impl AvatarManager {
    pub fn new(image_protocol: ImageProtocol, github_repos: Vec<String>) -> Self {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("gitui")
            .join("avatars-v4");
        let _ = fs::create_dir_all(&cache_dir);
        Self {
            cache_dir,
            image_protocol,
            next_image_id: 100000,
            prepared_image_map: FxHashMap::default(),
            image_ids: FxHashSet::default(),
            pending_uploads: Vec::new(),
            github_repos,
        }
    }

    pub fn is_enabled(&self) -> bool {
        AVATARS_ENABLED
    }

    fn email_to_path(&self, email: &str) -> PathBuf {
        let hash = format!("{:x}", Md5::digest(email.trim().to_lowercase()));
        self.cache_dir.join(format!("{hash}.png"))
    }

    fn next_image_id(&mut self) -> u32 {
        let id = self.next_image_id;
        self.next_image_id += 1;
        id
    }

    fn image_key(email: &str, height_cells: u16, selected: bool) -> String {
        format!("{}:{}:{}", height_cells, selected, email.trim().to_lowercase())
    }

    pub fn ensure_uploaded(&mut self, email: &str, height_cells: u16, selected: bool, bg: Color) {
        if !self.is_enabled() {
            return;
        }
        let key = Self::image_key(email, height_cells, selected);
        if self.prepared_image_map.contains_key(&key) {
            return;
        }
        let path = self.email_to_path(email);
        let Ok(bytes) = fs::read(&path) else {
            return;
        };
        if bytes.is_empty() {
            return;
        }
        if image::load_from_memory(&bytes).is_err() {
            let _ = fs::remove_file(path);
            return;
        }
        let bytes = match avatar_on_background_png(&bytes, ratatui_color_to_rgba(bg)) {
            Some(bytes) => bytes,
            None => return,
        };
        let width_cells = height_cells as usize * 2;
        let image_id = self.next_image_id();
        let mut image = self.image_protocol.prepare_image(&bytes, width_cells, image_id);
        if let Some(upload_data) = image.take_upload_data() {
            self.pending_uploads.push(upload_data);
        }
        self.prepared_image_map.insert(key, image);
        self.image_ids.insert(image_id);
    }

    pub fn prepared_image(&self, email: &str, height_cells: u16, selected: bool) -> Option<&PreparedImage> {
        self.prepared_image_map
            .get(&Self::image_key(email, height_cells, selected))
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
        // Disabled until GitHub authentication is implemented. The current unauthenticated
        // lookup experiments below are intentionally kept for later reference but should be
        // removed/reworked once avatar support becomes GitHub-authenticated.
        if !self.is_enabled() {
            return;
        }
        let path = self.email_to_path(email);
        if path.metadata().is_ok_and(|metadata| metadata.len() > 0) {
            return;
        }
        let email = email.trim().to_lowercase();
        let Some(url) = self.github_avatar_url(&email).or_else(|| {
            if self.github_repos.is_empty() {
                None
            } else {
                Some("github-commit".to_string())
            }
        }).or_else(|| Some(format!("github-email-search:{email}"))) else { return; };
        let path_clone = path.clone();
        let repos = self.github_repos.clone();
        let email_for_fallback = email.clone();
        thread::spawn(move || {
            let url = if url == "github-commit" {
                match resolve_github_commit_avatar_url(&repos, &commit_hashes) {
                    Some(url) => url,
                    None => match github_email_search_avatar_url(&email) {
                        Some(url) => url,
                        None => {
                            write_avatar_atomic(&path_clone, fallback_avatar_png(&email_for_fallback));
                            return;
                        }
                    },
                }
            } else if let Some(email) = url.strip_prefix("github-email-search:") {
                match github_email_search_avatar_url(email) {
                    Some(url) => url,
                    None => {
                        write_avatar_atomic(&path_clone, fallback_avatar_png(&email_for_fallback));
                        return;
                    }
                }
            } else {
                url
            };
            let result = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build();
            let Ok(client) = result else {
                write_avatar_atomic(&path_clone, fallback_avatar_png(&email_for_fallback));
                return;
            };
            match client.get(&url).send() {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(bytes) = resp.bytes() {
                        if bytes.len() > 100 {
                            if let Some(avatar) = rounded_avatar_png(&bytes) {
                                write_avatar_atomic(&path_clone, avatar);
                            } else {
                                write_avatar_atomic(&path_clone, fallback_avatar_png(&email_for_fallback));
                            }
                        } else {
                            write_avatar_atomic(&path_clone, fallback_avatar_png(&email_for_fallback));
                        }
                    }
                }
                _ => {
                    write_avatar_atomic(&path_clone, fallback_avatar_png(&email_for_fallback));
                }
            }
        });
    }

    pub fn github_avatar_url(&self, email: &str) -> Option<String> {
        let email = email.trim().to_lowercase();
        if let Some((before_at, domain)) = email.split_once('@') {
            if domain == "users.noreply.github.com" {
                if let Some((_, login)) = before_at.split_once('+') {
                    if !login.is_empty() {
                        return Some(format!("https://github.com/{login}.png?size=128"));
                    }
                }
                if !before_at.is_empty() {
                    return Some(format!("https://github.com/{before_at}.png?size=128"));
                }
            }
        }

        None
    }
}

fn github_email_search_avatar_url(email: &str) -> Option<String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()?;
    let url = format!(
        "https://api.github.com/search/users?q={}+in%3Aemail",
        email.replace('@', "%40")
    );
    let response = client
        .get(url)
        .header("User-Agent", "gitui")
        .send()
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body = response.text().ok()?;
    extract_avatar_url(&body)
}

fn resolve_github_commit_avatar_url(repos: &[String], commit_hashes: &[String]) -> Option<String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()?;
    for repo in repos {
        for commit_hash in commit_hashes.iter().take(8) {
            let url = format!("https://api.github.com/repos/{repo}/commits/{commit_hash}");
            let Ok(response) = client
                .get(url)
                .header("User-Agent", "gitui")
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
    let marker = "\"avatar_url\": \"";
    let start = body.find(marker)? + marker.len();
    let rest = &body[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn write_avatar_atomic(path: &PathBuf, bytes: Vec<u8>) {
    let temp_path = path.with_extension("png.tmp");
    if fs::write(&temp_path, bytes).is_ok() {
        let _ = fs::rename(&temp_path, path);
    }
}

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
    let radius = 64.0f32;
    let center = 63.5f32;
    for (x, y, pixel) in avatar.enumerate_pixels_mut() {
        let dx = x as f32 - center;
        let dy = y as f32 - center;
        let distance = (dx * dx + dy * dy).sqrt();
        if distance > radius {
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
        Color::Reset | Color::Indexed(_) => [0x1a, 0x1b, 0x26, 255],
    }
}

fn rounded_avatar_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let image = image::load_from_memory(bytes).ok()?;
    let (width, height) = image.dimensions();
    let side = width.min(height);
    let x = (width - side) / 2;
    let y = (height - side) / 2;
    let mut avatar = image
        .crop_imm(x, y, side, side)
        .resize_exact(128, 128, FilterType::Lanczos3)
        .to_rgba8();
    let radius = 64.0f32;
    let center = 63.5f32;
    for (x, y, pixel) in avatar.enumerate_pixels_mut() {
        let dx = x as f32 - center;
        let dy = y as f32 - center;
        if (dx * dx + dy * dy).sqrt() > radius {
            *pixel = Rgba([pixel[0], pixel[1], pixel[2], 0]);
        }
    }
    let mut out = Cursor::new(Vec::new());
    avatar.write_to(&mut out, ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

fn avatar_on_background_png(bytes: &[u8], bg: [u8; 4]) -> Option<Vec<u8>> {
    let mut image = image::load_from_memory(bytes).ok()?.to_rgba8();
    for pixel in image.pixels_mut() {
        let alpha = pixel[3] as u16;
        let inv_alpha = 255 - alpha;
        pixel[0] = ((pixel[0] as u16 * alpha + bg[0] as u16 * inv_alpha) / 255) as u8;
        pixel[1] = ((pixel[1] as u16 * alpha + bg[1] as u16 * inv_alpha) / 255) as u8;
        pixel[2] = ((pixel[2] as u16 * alpha + bg[2] as u16 * inv_alpha) / 255) as u8;
        pixel[3] = 255;
    }
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, ImageFormat::Png).ok()?;
    Some(out.into_inner())
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
    fn github_avatar_url_resolves_only_confident_sources() {
        let manager = AvatarManager::new(
            ImageProtocol::Iterm2,
            vec!["example-owner/example-repo".to_string()],
        );

        assert_eq!(
            manager.github_avatar_url("123456+octocat@users.noreply.github.com"),
            Some("https://github.com/octocat.png?size=128".to_string())
        );
        assert_eq!(manager.github_avatar_url("someone@example.org"), None);
    }

    #[test]
    fn avatar_on_background_replaces_transparent_corners_with_background() {
        let mut source = image::RgbaImage::new(32, 32);
        for pixel in source.pixels_mut() {
            *pixel = Rgba([255, 0, 0, 255]);
        }
        let mut input = Cursor::new(Vec::new());
        source.write_to(&mut input, ImageFormat::Png).unwrap();

        let rounded = rounded_avatar_png(input.get_ref()).unwrap();
        let output = avatar_on_background_png(&rounded, [10, 20, 30, 255]).unwrap();
        let composited = image::load_from_memory(&output).unwrap().to_rgba8();

        assert_eq!(composited.get_pixel(0, 0).0, [10, 20, 30, 255]);
        assert_eq!(composited.get_pixel(64, 64).0, [255, 0, 0, 255]);
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
    fn avatars_are_disabled_until_github_authentication_exists() {
        let manager = AvatarManager::new(ImageProtocol::Iterm2, Vec::new());

        assert!(!manager.is_enabled());
    }
}
