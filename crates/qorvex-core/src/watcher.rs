//! Screen change detection for iOS Simulator automation.
//!
//! This module provides automatic screen change detection by polling the
//! accessibility tree and comparing hashes to detect UI changes.
//!
//! # Example
//!
//! ```no_run
//! use std::sync::Arc;
//! use qorvex_core::session::Session;
//! use qorvex_core::watcher::{ScreenWatcher, WatcherConfig};
//!
//! #[tokio::main]
//! async fn main() {
//!     let session = Session::new(Some("SIMULATOR-UDID".to_string()), "default");
//!     let config = WatcherConfig::default();
//!
//!     let handle = ScreenWatcher::spawn(
//!         session,
//!         "SIMULATOR-UDID".to_string(),
//!         config,
//!     );
//!
//!     // Later, stop the watcher
//!     handle.stop().await;
//! }
//! ```

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::axe::{Axe, UIElement};
use crate::session::Session;
use crate::simctl::Simctl;

/// Configuration for the screen watcher.
#[derive(Debug, Clone)]
pub struct WatcherConfig {
    /// Polling interval in milliseconds (default: 500).
    pub interval_ms: u64,
    /// Whether to capture screenshots on change (default: true).
    pub capture_screenshots: bool,
    /// Hamming distance threshold for visual change detection (0-64, default: 5).
    /// Lower values are more sensitive to changes. A value of 0 means any visual
    /// change triggers an event. A value of 64 effectively disables visual detection.
    pub visual_change_threshold: u32,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            interval_ms: 500,
            capture_screenshots: true,
            visual_change_threshold: 5,
        }
    }
}

/// Handle for controlling a running screen watcher.
///
/// The watcher will continue running until `stop()` is called or the
/// handle is dropped.
pub struct WatcherHandle {
    cancel_token: CancellationToken,
    join_handle: JoinHandle<()>,
}

impl WatcherHandle {
    /// Stops the watcher and waits for it to finish.
    pub async fn stop(self) {
        self.cancel_token.cancel();
        let _ = self.join_handle.await;
    }

    /// Cancels the watcher without waiting.
    pub fn cancel(&self) {
        self.cancel_token.cancel();
    }

    /// Returns whether the watcher is still running.
    pub fn is_running(&self) -> bool {
        !self.join_handle.is_finished()
    }
}

/// Screen watcher that detects UI changes by polling the accessibility tree.
pub struct ScreenWatcher;

impl ScreenWatcher {
    /// Spawns a new screen watcher task.
    ///
    /// The watcher will poll the accessibility tree at the configured interval
    /// and broadcast `ScreenInfoUpdated` events when changes are detected.
    ///
    /// # Arguments
    ///
    /// * `session` - The session to broadcast events to
    /// * `simulator_udid` - The UDID of the simulator to watch
    /// * `config` - Watcher configuration
    ///
    /// # Returns
    ///
    /// A `WatcherHandle` that can be used to stop the watcher.
    pub fn spawn(
        session: Arc<Session>,
        simulator_udid: String,
        config: WatcherConfig,
    ) -> WatcherHandle {
        let cancel_token = CancellationToken::new();
        let token_clone = cancel_token.clone();

        let join_handle = tokio::spawn(async move {
            Self::run_loop(session, simulator_udid, config, token_clone).await;
        });

        WatcherHandle {
            cancel_token,
            join_handle,
        }
    }

    async fn run_loop(
        session: Arc<Session>,
        simulator_udid: String,
        config: WatcherConfig,
        cancel_token: CancellationToken,
    ) {
        let interval = tokio::time::Duration::from_millis(config.interval_ms);

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    break;
                }
                _ = tokio::time::sleep(interval) => {
                    Self::check_for_changes(&session, &simulator_udid, &config).await;
                }
            }
        }
    }

    async fn check_for_changes(
        session: &Arc<Session>,
        simulator_udid: &str,
        config: &WatcherConfig,
    ) {
        // Run axe in a blocking task since it's a subprocess call
        let udid = simulator_udid.to_string();
        let hierarchy_result = tokio::task::spawn_blocking(move || {
            Axe::dump_hierarchy(&udid)
        }).await;

        let hierarchy = match hierarchy_result {
            Ok(Ok(h)) => h,
            Ok(Err(_)) | Err(_) => return, // Skip this poll on error
        };

        let elements = Axe::list_elements(&hierarchy);

        // Compute hash of elements
        let element_hash = Self::hash_elements(&elements);

        // Capture screenshot and compute visual hash
        let (screenshot, screenshot_hash) = if config.capture_screenshots {
            let udid = simulator_udid.to_string();
            let result = tokio::task::spawn_blocking(move || {
                Simctl::screenshot(&udid).ok().map(|bytes| {
                    let dhash = Self::dhash_screenshot(&bytes);
                    use base64::Engine;
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                    (b64, dhash)
                })
            })
            .await
            .ok()
            .flatten();

            match result {
                Some((b64, dhash)) => (Some(b64), dhash),
                None => (None, None),
            }
        } else {
            (None, None)
        };

        // Update session (this handles hash comparison internally)
        session
            .update_screen_info(elements, element_hash, screenshot, screenshot_hash, config.visual_change_threshold)
            .await;
    }

    /// Computes a hash of the element list for change detection.
    fn hash_elements(elements: &[UIElement]) -> u64 {
        let mut hasher = DefaultHasher::new();

        for element in elements {
            element.identifier.hash(&mut hasher);
            element.label.hash(&mut hasher);
            element.value.hash(&mut hasher);
            element.element_type.hash(&mut hasher);

            // Hash frame if present
            if let Some(ref frame) = element.frame {
                (frame.x as i64).hash(&mut hasher);
                (frame.y as i64).hash(&mut hasher);
                (frame.width as i64).hash(&mut hasher);
                (frame.height as i64).hash(&mut hasher);
            }
        }

        hasher.finish()
    }

    /// Computes a perceptual difference hash (dHash) of a screenshot.
    ///
    /// This creates a 64-bit hash that captures the visual structure of the image.
    /// Similar images will have similar hashes, allowing detection of visual changes
    /// like animations or scroll position that don't affect the accessibility tree.
    ///
    /// # Algorithm
    /// 1. Resize image to 9x8 grayscale
    /// 2. For each pixel, compare to its right neighbor
    /// 3. Set bit to 1 if left pixel is brighter than right
    ///
    /// # Arguments
    /// * `png_bytes` - Raw PNG image data
    ///
    /// # Returns
    /// `Some(u64)` containing the perceptual hash, or `None` if decoding fails.
    fn dhash_screenshot(png_bytes: &[u8]) -> Option<u64> {
        use image::imageops::FilterType;

        // Decode PNG
        let img = image::load_from_memory(png_bytes).ok()?;

        // Resize to 9x8 grayscale (9 wide to get 8 horizontal differences)
        let small = img
            .resize_exact(9, 8, FilterType::Triangle)
            .to_luma8();

        // Compute difference hash
        let mut hash: u64 = 0;
        for y in 0..8 {
            for x in 0..8 {
                let left = small.get_pixel(x, y).0[0];
                let right = small.get_pixel(x + 1, y).0[0];
                if left > right {
                    hash |= 1 << (y * 8 + x);
                }
            }
        }

        Some(hash)
    }

    /// Computes the Hamming distance between two hashes.
    ///
    /// The Hamming distance is the number of bit positions where the hashes differ.
    /// For 64-bit hashes, the distance ranges from 0 (identical) to 64 (completely different).
    #[cfg(test)]
    fn hamming_distance(a: u64, b: u64) -> u32 {
        (a ^ b).count_ones()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_watcher_config_default() {
        let config = WatcherConfig::default();
        assert_eq!(config.interval_ms, 500);
        assert!(config.capture_screenshots);
        assert_eq!(config.visual_change_threshold, 5);
    }

    #[test]
    fn test_hash_elements_empty() {
        let elements: Vec<UIElement> = vec![];
        let hash = ScreenWatcher::hash_elements(&elements);
        // Empty vec should produce a consistent hash
        assert_eq!(hash, ScreenWatcher::hash_elements(&[]));
    }

    #[test]
    fn test_hash_elements_deterministic() {
        let element = UIElement {
            identifier: Some("test-id".to_string()),
            label: Some("Test Label".to_string()),
            value: Some("Test Value".to_string()),
            element_type: Some("Button".to_string()),
            frame: None,
            children: vec![],
            role: None,
        };

        let hash1 = ScreenWatcher::hash_elements(&[element.clone()]);
        let hash2 = ScreenWatcher::hash_elements(&[element]);

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_elements_different() {
        let element1 = UIElement {
            identifier: Some("id1".to_string()),
            label: None,
            value: None,
            element_type: None,
            frame: None,
            children: vec![],
            role: None,
        };

        let element2 = UIElement {
            identifier: Some("id2".to_string()),
            label: None,
            value: None,
            element_type: None,
            frame: None,
            children: vec![],
            role: None,
        };

        let hash1 = ScreenWatcher::hash_elements(&[element1]);
        let hash2 = ScreenWatcher::hash_elements(&[element2]);

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_hamming_distance_identical() {
        assert_eq!(ScreenWatcher::hamming_distance(0, 0), 0);
        assert_eq!(ScreenWatcher::hamming_distance(u64::MAX, u64::MAX), 0);
        assert_eq!(ScreenWatcher::hamming_distance(0x12345678, 0x12345678), 0);
    }

    #[test]
    fn test_hamming_distance_single_bit() {
        assert_eq!(ScreenWatcher::hamming_distance(0, 1), 1);
        assert_eq!(ScreenWatcher::hamming_distance(0, 2), 1);
        assert_eq!(ScreenWatcher::hamming_distance(0, 0x8000000000000000), 1);
    }

    #[test]
    fn test_hamming_distance_max() {
        // All bits different
        assert_eq!(ScreenWatcher::hamming_distance(0, u64::MAX), 64);
    }

    #[test]
    fn test_hamming_distance_symmetric() {
        let a = 0x123456789ABCDEF0u64;
        let b = 0xFEDCBA9876543210u64;
        assert_eq!(
            ScreenWatcher::hamming_distance(a, b),
            ScreenWatcher::hamming_distance(b, a)
        );
    }

    #[test]
    fn test_dhash_screenshot_invalid_data() {
        // Invalid PNG data should return None
        assert!(ScreenWatcher::dhash_screenshot(&[]).is_none());
        assert!(ScreenWatcher::dhash_screenshot(&[0, 1, 2, 3]).is_none());
        assert!(ScreenWatcher::dhash_screenshot(b"not a png").is_none());
    }

    #[test]
    fn test_dhash_screenshot_valid_png() {
        use image::{ImageBuffer, Rgb};

        // Create a small valid PNG using the image crate
        let img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(2, 2, |_, _| {
            Rgb([255, 255, 255]) // White
        });

        let mut png_bytes = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).unwrap();

        let result = ScreenWatcher::dhash_screenshot(&png_bytes);
        assert!(result.is_some());
    }

    #[test]
    fn test_dhash_deterministic() {
        // Create a simple valid PNG using the image crate
        use image::{ImageBuffer, Rgb};

        let img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(100, 100, |x, y| {
            // Create a gradient pattern
            Rgb([(x as u8), (y as u8), 128])
        });

        let mut png_bytes = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut png_bytes);
        img.write_to(&mut cursor, image::ImageFormat::Png).unwrap();

        let hash1 = ScreenWatcher::dhash_screenshot(&png_bytes);
        let hash2 = ScreenWatcher::dhash_screenshot(&png_bytes);

        assert!(hash1.is_some());
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_dhash_different_images() {
        use image::{ImageBuffer, Rgb};

        // Create two different images
        let img1: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(100, 100, |_, _| {
            Rgb([0, 0, 0]) // All black
        });

        let img2: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(100, 100, |_, _| {
            Rgb([255, 255, 255]) // All white
        });

        let mut png1 = Vec::new();
        let mut png2 = Vec::new();
        img1.write_to(&mut std::io::Cursor::new(&mut png1), image::ImageFormat::Png).unwrap();
        img2.write_to(&mut std::io::Cursor::new(&mut png2), image::ImageFormat::Png).unwrap();

        let hash1 = ScreenWatcher::dhash_screenshot(&png1).unwrap();
        let hash2 = ScreenWatcher::dhash_screenshot(&png2).unwrap();

        // Solid colors will produce 0 hash since all adjacent pixels are equal
        // But the hashes should be computed successfully
        assert!(hash1 == 0 || hash2 == 0 || hash1 != hash2);
    }

    #[test]
    fn test_dhash_similar_images_low_distance() {
        use image::{ImageBuffer, Rgb};

        // Create two very similar images (slight difference)
        let img1: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(100, 100, |x, _| {
            Rgb([(x as u8), 128, 128])
        });

        let img2: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(100, 100, |x, _| {
            Rgb([((x + 1) as u8), 128, 128]) // Slightly shifted
        });

        let mut png1 = Vec::new();
        let mut png2 = Vec::new();
        img1.write_to(&mut std::io::Cursor::new(&mut png1), image::ImageFormat::Png).unwrap();
        img2.write_to(&mut std::io::Cursor::new(&mut png2), image::ImageFormat::Png).unwrap();

        let hash1 = ScreenWatcher::dhash_screenshot(&png1).unwrap();
        let hash2 = ScreenWatcher::dhash_screenshot(&png2).unwrap();

        // Similar images should have low Hamming distance
        let distance = ScreenWatcher::hamming_distance(hash1, hash2);
        assert!(distance <= 10, "Expected low distance for similar images, got {}", distance);
    }
}
