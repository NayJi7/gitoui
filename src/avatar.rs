use std::{fs, path::PathBuf, thread};

use md5::{Digest, Md5};

use crate::protocol::{ImageProtocol, PreparedImage};

pub struct AvatarManager {
    cache_dir: PathBuf,
    image_protocol: ImageProtocol,
    next_image_id: u32,
}

impl AvatarManager {
    pub fn new(image_protocol: ImageProtocol) -> Self {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("gitui")
            .join("avatars");
        let _ = fs::create_dir_all(&cache_dir);
        Self {
            cache_dir,
            image_protocol,
            next_image_id: 100000,
        }
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

    pub fn get_avatar(&mut self, email: &str, height_cells: u16) -> Option<PreparedImage> {
        let path = self.email_to_path(email);
        let bytes = fs::read(&path).ok()?;
        if bytes.is_empty() {
            return None;
        }
        let width_cells = height_cells as usize * 2;
        let image_id = self.next_image_id();
        Some(self.image_protocol.prepare_image(&bytes, width_cells, image_id))
    }

    pub fn prefetch(&self, email: &str) {
        let path = self.email_to_path(email);
        if path.exists() {
            return;
        }
        let url = format!("https://unavatar.io/github/{}", email.trim().to_lowercase());
        let path_clone = path.clone();
        thread::spawn(move || {
            let result = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build();
            let Ok(client) = result else {
                let _ = fs::write(&path_clone, &[]);
                return;
            };
            match client.get(&url).send() {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(bytes) = resp.bytes() {
                        if bytes.len() > 100 {
                            let _ = fs::write(&path_clone, &bytes);
                        } else {
                            let _ = fs::write(&path_clone, &[]);
                        }
                    }
                }
                _ => {
                    let _ = fs::write(&path_clone, &[]);
                }
            }
        });
    }
}
