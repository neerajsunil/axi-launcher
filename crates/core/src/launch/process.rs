//! A running game: output capture, stopping and waiting.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::util::now_unix;

/// Receives each line the game prints.
pub type OutputSink = Arc<dyn Fn(String) + Send + Sync>;

pub struct GameProcess {
    child: Mutex<Child>,
    /// Unix seconds when the game started.
    pub started: u64,
}

impl GameProcess {
    /// Spawns `cmd`, copying its output into `log_file` and `output`.
    pub fn spawn(mut cmd: Command, log_file: &Path, output: OutputSink) -> io::Result<Arc<Self>> {
        let log = Arc::new(Mutex::new(File::create(log_file)?));
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        hide_console(&mut cmd);
        let mut child = cmd.spawn()?;
        if let Some(out) = child.stdout.take() {
            pump(out, Some(log.clone()), output.clone());
        }
        if let Some(err) = child.stderr.take() {
            pump(err, Some(log), output);
        }
        Ok(Arc::new(Self { child: Mutex::new(child), started: now_unix() }))
    }

    pub fn kill(&self) -> io::Result<()> {
        self.child.lock().unwrap().kill()
    }

    /// Blocks until the game exits. `kill` may be called meanwhile from another thread.
    pub fn wait(&self) -> io::Result<ExitStatus> {
        loop {
            if let Some(status) = self.child.lock().unwrap().try_wait()? {
                return Ok(status);
            }
            thread::sleep(Duration::from_millis(250));
        }
    }
}

/// Runs a helper program (e.g. a loader installer) to completion, forwarding its output.
pub fn run_logged(mut cmd: Command, output: &OutputSink) -> io::Result<ExitStatus> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    hide_console(&mut cmd);
    let mut child = cmd.spawn()?;
    let pumps = [
        child.stdout.take().map(|s| pump(s, None, output.clone())),
        child.stderr.take().map(|s| pump(s, None, output.clone())),
    ];
    let status = child.wait()?;
    for pump in pumps.into_iter().flatten() {
        let _ = pump.join();
    }
    Ok(status)
}

fn hide_console(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let _ = cmd;
}

fn pump(stream: impl Read + Send + 'static, log: Option<Arc<Mutex<File>>>, output: OutputSink) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut buf = Vec::new();
        while reader.read_until(b'\n', &mut buf).unwrap_or(0) > 0 {
            if let Some(log) = &log {
                let _ = log.lock().unwrap().write_all(&buf);
            }
            let line = String::from_utf8_lossy(&buf).trim_end().to_string();
            output(line);
            buf.clear();
        }
    })
}
