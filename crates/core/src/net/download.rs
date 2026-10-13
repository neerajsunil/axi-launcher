use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::prelude::*;
use reqwest::blocking::Client;

use crate::{Error, Progress, Result};

const RETRIES: usize = 3;

/// A file to download. `size` lets an existing file of the right size be skipped.
pub struct Job {
    pub url: String,
    pub path: PathBuf,
    pub size: Option<u64>,
}

/// Downloads `url` to `path`, reusing an existing file when its size matches.
pub fn download(client: &Client, url: &str, path: &Path, size: Option<u64>) -> Result<()> {
    if let Ok(meta) = fs::metadata(path) {
        if size.is_none_or(|s| meta.len() == s) {
            return Ok(());
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".part");
    let tmp = PathBuf::from(tmp);

    let mut last_err = None;
    for _ in 0..RETRIES {
        let attempt = (|| -> Result<()> {
            let mut resp = client.get(url).send()?.error_for_status()?;
            let mut file = File::create(&tmp)?;
            io::copy(&mut resp, &mut file)?;
            Ok(())
        })();
        match attempt {
            Ok(()) => {
                fs::rename(&tmp, path)?;
                return Ok(());
            }
            Err(e) => last_err = Some(e),
        }
    }
    let _ = fs::remove_file(&tmp);
    Err(Error::msg(format!("downloading {url}: {}", last_err.unwrap())))
}

/// A thread pool for `threads` parallel network requests.
pub fn pool(threads: usize) -> Result<rayon::ThreadPool> {
    rayon::ThreadPoolBuilder::new().num_threads(threads.max(1)).build().map_err(|e| Error::msg(e.to_string()))
}

/// Downloads all `jobs` using `threads` parallel connections.
pub fn download_all(client: &Client, jobs: &[Job], threads: usize, label: &str, progress: Progress) -> Result<()> {
    let total = jobs.len();
    let done = AtomicUsize::new(0);
    let pool = pool(threads)?;
    progress(format!("{label} (0/{total})"), 0.0);
    pool.install(|| {
        jobs.par_iter().try_for_each(|job| {
            download(client, &job.url, &job.path, job.size)?;
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 20 == 0 || n == total {
                progress(format!("{label} ({n}/{total})"), n as f32 / total as f32);
            }
            Ok(())
        })
    })
}
