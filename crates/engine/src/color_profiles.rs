//! Which colour profile each photo's file is read in (“Display P3 · embedded ICC”), for the Info
//! panel and `photo.inspect`'s `colorProfile`.
//!
//! The answer comes from the file's headers ([`lightcraft_codecs::read_color_profile`]: no pixels
//! are decoded for JPEG, PNG, TIFF and WebP) and is kept in memory per path and file size (a file
//! rewritten in place gets a new size, so it is read again). Like [`crate::availability`], the
//! app asks from the UI thread and has the files read on a worker
//! ([`ColorProfiles::run_in_background`]); elsewhere (CLI, MCP, tests) every unknown answer is
//! read on the spot.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use lightcraft_catalog::{MediaKind, Photo, Source};
use lightcraft_codecs::ProfileInfo;

use crate::availability::Waker;
use crate::merge::ByteReader;

/// A photo's colour profile: `None` while it is being read (background mode) or for a photo that
/// has none to show (video); `Some(Err(..))` when the file can't be read.
pub type Lookup = Option<Result<ProfileInfo, String>>;

/// Answers kept before the cache starts over (each is a few hundred bytes).
const MAX_ENTRIES: usize = 20_000;

type Key = (String, u64);

#[derive(Default)]
struct State {
    known: HashMap<Key, Result<ProfileInfo, String>>,
    queue: VecDeque<(Key, Option<ByteReader>)>,
    queued: HashSet<Key>,
    worker: bool,
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Read the profile from the file at `path` (through `reader` when given, else the file system).
fn read(path: &str, reader: Option<&ByteReader>) -> Result<ProfileInfo, String> {
    let bytes = match reader {
        Some(r) => r(path)?,
        None => std::fs::read(path).map_err(|e| format!("{path}: {e}"))?,
    };
    lightcraft_codecs::read_color_profile(&bytes).map_err(|e| e.to_string())
}

/// Colour profiles of photo files, read synchronously or on a worker thread. Cloning shares the cache.
#[derive(Clone, Default)]
pub struct ColorProfiles {
    background: Option<Waker>,
    state: Arc<Mutex<State>>,
}

impl std::fmt::Debug for ColorProfiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ColorProfiles").field("background", &self.background.is_some()).finish()
    }
}

impl ColorProfiles {
    /// Answer from the cache and read files on a worker thread from now on; `wake` runs when an
    /// answer arrives. No effect in the browser build (no threads).
    pub fn run_in_background(&mut self, wake: Waker) {
        if cfg!(target_arch = "wasm32") {
            return;
        }
        self.background = Some(wake);
    }

    fn store(&self, key: Key, answer: Result<ProfileInfo, String>) {
        let mut st = lock(&self.state);
        if st.known.len() >= MAX_ENTRIES {
            st.known.clear();
        }
        st.known.insert(key, answer);
    }

    /// The profile of the file at `path` (`size` bytes), read now when unknown, whatever the mode
    /// (agents asking through `photo.inspect` want an answer, not "pending").
    pub fn get_now(&self, path: &str, size: u64, reader: Option<&ByteReader>) -> Result<ProfileInfo, String> {
        let key = (path.to_string(), size);
        if let Some(a) = lock(&self.state).known.get(&key) {
            return a.clone();
        }
        let answer = read(path, reader);
        self.store(key, answer.clone());
        answer
    }

    /// The profile of the file at `path`: synchronous mode reads it now; background mode answers
    /// from the cache (`None` while unknown) and queues unknown files for the worker.
    pub fn get(&self, path: &str, size: u64, reader: Option<&ByteReader>) -> Option<Result<ProfileInfo, String>> {
        let Some(wake) = &self.background else {
            return Some(self.get_now(path, size, reader));
        };
        let key = (path.to_string(), size);
        let mut st = lock(&self.state);
        if let Some(a) = st.known.get(&key) {
            return Some(a.clone());
        }
        if st.queued.insert(key.clone()) {
            st.queue.push_back((key, reader.cloned()));
            if !st.worker {
                st.worker = true;
                drop(st);
                self.spawn_worker(wake.clone());
            }
        }
        None
    }

    fn spawn_worker(&self, wake: Waker) {
        let this = ColorProfiles { background: None, state: self.state.clone() };
        let started = std::thread::Builder::new().name("lc-color-profiles".into()).spawn(move || {
            loop {
                let (key, reader) = {
                    let mut st = lock(&this.state);
                    let Some(job) = st.queue.pop_front() else {
                        st.worker = false;
                        return;
                    };
                    job
                };
                let answer = read(&key.0, reader.as_ref());
                lock(&this.state).queued.remove(&key);
                this.store(key, answer);
                wake();
            }
        });
        if started.is_err() {
            // try again on the next question
            lock(&self.state).worker = false;
        }
    }
}

/// What a raw file's colours are read through: not an image profile, the camera's colour model.
pub fn camera_raw_profile(preview_only: bool) -> ProfileInfo {
    if preview_only {
        return ProfileInfo {
            name: "Embedded JPEG preview".into(),
            origin: "camera".into(),
            origin_label: "colour space declared by the raw file".into(),
            description: Some("The raw data can't be decoded: the camera's own JPEG is shown, read in the colour space the raw file states".into()),
            unsupported: false,
        };
    }
    ProfileInfo {
        name: "Camera raw".into(),
        origin: "camera".into(),
        origin_label: "camera colour matrix → working space".into(),
        description: Some(
            "Raw sensor data has no image profile: it is converted with the camera's colour matrix into the linear Rec. 2020 working space".into(),
        ),
        unsupported: false,
    }
}

/// A generated demo photo: rendered straight into the working space.
pub fn generated_profile() -> ProfileInfo {
    ProfileInfo {
        name: "Rec. 2020 (linear)".into(),
        origin: "generated".into(),
        origin_label: "generated demo photo".into(),
        description: None,
        unsupported: false,
    }
}

impl crate::media::MediaCache {
    /// `p`'s colour profile. `now`: read the file on this thread when unknown (agents); otherwise
    /// background mode answers `None` until the worker has read it (the UI).
    pub fn color_profile(&self, p: &Photo, now: bool) -> Lookup {
        if p.kind == MediaKind::Video {
            return None;
        }
        let path = match &p.source {
            Source::Demo { .. } => return Some(Ok(generated_profile())),
            Source::File { path } => path,
        };
        if p.kind == MediaKind::Raw {
            return Some(Ok(camera_raw_profile(p.preview_only.is_some())));
        }
        if now {
            Some(self.color_profiles.get_now(path, p.file_size, self.file_bytes.as_ref()))
        } else {
            self.color_profiles.get(path, p.file_size, self.file_bytes.as_ref())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn p3_jpeg() -> Vec<u8> {
        let icc = lightcraft_codecs::icc::write_named(lightcraft_codecs::NamedSpace::DisplayP3);
        let data = [200u8; 8 * 8 * 3];
        let img = lightcraft_codecs::EncodeImage::new(8, 8, 3, lightcraft_codecs::Samples::U8(&data));
        let meta = lightcraft_codecs::EncodeMeta { icc: Some(&icc), ..Default::default() };
        lightcraft_codecs::encode_jpeg(&img, 90, lightcraft_codecs::ChromaSubsampling::S444, &meta).unwrap()
    }

    fn counting_reader(bytes: Vec<u8>) -> (ByteReader, Arc<AtomicUsize>) {
        let n = Arc::new(AtomicUsize::new(0));
        let c = n.clone();
        (
            Arc::new(move |_p: &str| {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(bytes.clone())
            }),
            n,
        )
    }

    #[test]
    fn synchronous_reads_once_per_path_and_size() {
        let (reader, n) = counting_reader(p3_jpeg());
        let c = ColorProfiles::default();
        let p = c.get("a.jpg", 10, Some(&reader)).unwrap().unwrap();
        assert_eq!(p.summary(), "Display P3 · embedded ICC");
        assert_eq!(c.get("a.jpg", 10, Some(&reader)).unwrap().unwrap(), p);
        assert_eq!(n.load(Ordering::SeqCst), 1, "cached");
        c.get("a.jpg", 11, Some(&reader));
        assert_eq!(n.load(Ordering::SeqCst), 2, "a different size is a different file");
        let bad: ByteReader = Arc::new(|_p: &str| Ok(b"not an image".to_vec()));
        assert!(c.get("b.jpg", 1, Some(&bad)).unwrap().is_err());
    }

    #[test]
    fn background_answers_later_without_blocking() {
        let (reader, _) = counting_reader(p3_jpeg());
        let slow: ByteReader = Arc::new(move |p: &str| {
            std::thread::sleep(Duration::from_millis(100));
            reader(p)
        });
        let mut c = ColorProfiles::default();
        let woke = Arc::new(AtomicUsize::new(0));
        let w = woke.clone();
        c.run_in_background(Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }));
        assert!(c.get("a.jpg", 1, Some(&slow)).is_none(), "unknown at first");
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while c.get("a.jpg", 1, Some(&slow)).is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(c.get("a.jpg", 1, Some(&slow)).unwrap().unwrap().name, "Display P3");
        assert!(woke.load(Ordering::SeqCst) >= 1);
    }
}
