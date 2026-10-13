//! Finds Java installations already on this machine.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::runtime::java_in;

/// A Java installation found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaInstall {
    pub path: PathBuf,
    /// Major version (8, 17, 21, ...); 0 when it could not be determined.
    pub major: u32,
    /// Full version string, e.g. `21.0.4` or `1.8.0_412`.
    pub version: String,
    /// Who built it, e.g. `Eclipse Adoptium`.
    pub vendor: String,
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

/// Folders whose subdirectories are usually JDK/JRE installs.
fn install_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(pf) = std::env::var_os(var).map(PathBuf::from) {
                for vendor in [
                    "Java",
                    "Eclipse Adoptium",
                    "Eclipse Foundation",
                    "Microsoft",
                    "Zulu",
                    "BellSoft",
                    "Amazon Corretto",
                    "Semeru",
                ] {
                    roots.push(pf.join(vendor));
                }
            }
        }
    } else if cfg!(target_os = "macos") {
        roots.push("/Library/Java/JavaVirtualMachines".into());
    } else {
        roots.extend(["/usr/lib/jvm".into(), "/usr/lib64/jvm".into()]);
    }
    if let Some(home) = home_dir() {
        // IntelliJ and SDKMAN downloads.
        roots.push(home.join(".jdks"));
        roots.push(home.join(".sdkman/candidates/java"));
    }
    roots
}

/// `runtime` folders of the official Minecraft launcher, laid out as
/// `<root>/<component>/<platform>/<component>`.
fn mojang_launcher_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if cfg!(windows) {
        if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
            // Microsoft Store / Xbox app install.
            roots.push(local.join(r"Packages\Microsoft.4297127D64EC6_8wekyb3d8bbwe\LocalCache\Local\runtime"));
        }
        if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
            roots.push(appdata.join(r".minecraft\runtime"));
        }
        if let Some(pf) = std::env::var_os("ProgramFiles(x86)").map(PathBuf::from) {
            roots.push(pf.join(r"Minecraft Launcher\runtime"));
        }
    } else if let Some(home) = home_dir() {
        roots.push(if cfg!(target_os = "macos") {
            home.join("Library/Application Support/minecraft/runtime")
        } else {
            home.join(".minecraft/runtime")
        });
    }
    roots
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default()
}

/// The java executable of an install rooted at `java_home`, whatever its layout.
pub(crate) fn find_java(java_home: &Path) -> Option<PathBuf> {
    let candidates = [
        java_in(java_home),
        java_home.join("bin").join(if cfg!(windows) { "java.exe" } else { "java" }),
        java_home.join("Contents/Home/bin/java"),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

/// Major version from a Java version string: `1.8.0_412` → 8, `21.0.4` → 21.
pub(crate) fn parse_major(version: &str) -> u32 {
    let mut parts = version.split(|c: char| !c.is_ascii_digit());
    match parts.next().and_then(|p| p.parse().ok()) {
        Some(1) => parts.next().and_then(|p| p.parse().ok()).unwrap_or(0),
        Some(n) => n,
        None => 0,
    }
}

/// Reads `JAVA_VERSION` and `IMPLEMENTOR` from the `release` file next to `bin`.
pub(crate) fn read_release(exe: &Path) -> Option<(String, String)> {
    let text = fs::read_to_string(exe.parent()?.parent()?.join("release")).ok()?;
    let field = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
            .map(|v| v.trim().trim_matches('"').to_string())
    };
    Some((field("JAVA_VERSION")?, field("IMPLEMENTOR").unwrap_or_default()))
}

/// Asks the executable itself; slower, so only used when there is no `release` file.
fn query_version(exe: &Path) -> Option<String> {
    // `javaw` has no console, but `java` next to it prints the same thing.
    let console = exe.with_file_name(if cfg!(windows) { "java.exe" } else { "java" });
    let mut cmd = Command::new(if console.is_file() { &console } else { exe });
    cmd.arg("-version");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = cmd.output().ok()?;
    // First line looks like: openjdk version "21.0.4" 2024-07-16
    let text = String::from_utf8_lossy(&output.stderr);
    text.lines().next()?.split('"').nth(1).map(String::from)
}

fn describe(path: PathBuf) -> JavaInstall {
    let (version, vendor) = read_release(&path)
        .or_else(|| query_version(&path).map(|v| (v, String::new())))
        .unwrap_or_default();
    JavaInstall { major: parse_major(&version), version, vendor, path }
}

/// Java installations found via JAVA_HOME, PATH, common install folders, the official
/// Minecraft launcher and `runtimes_dir` (runtimes this launcher downloaded), newest first.
pub fn detect_installations(runtimes_dir: &Path) -> Vec<JavaInstall> {
    let mut homes: Vec<PathBuf> = Vec::new();
    if let Some(home) = std::env::var_os("JAVA_HOME") {
        homes.push(home.into());
    }
    if let Some(path) = std::env::var_os("PATH") {
        // Each PATH entry is a bin dir; its parent is the java home.
        homes.extend(std::env::split_paths(&path).filter_map(|p| p.parent().map(Path::to_path_buf)));
    }
    for root in install_roots().iter().chain([&runtimes_dir.to_path_buf()]) {
        homes.extend(subdirs(root));
    }
    for root in mojang_launcher_roots() {
        for component in subdirs(&root) {
            let Some(name) = component.file_name().map(|n| n.to_os_string()) else { continue };
            homes.extend(subdirs(&component).into_iter().map(|platform| platform.join(&name)));
        }
    }

    // Dedupe by real location (PATH shims and JAVA_HOME often point at the same install).
    let mut seen = BTreeSet::new();
    let exes: Vec<PathBuf> = homes
        .iter()
        .filter_map(|home| find_java(home))
        .filter(|exe| seen.insert(fs::canonicalize(exe).unwrap_or_else(|_| exe.clone())))
        .collect();
    let mut found: Vec<JavaInstall> = exes.into_iter().map(describe).collect();
    found.sort_by(|a, b| b.major.cmp(&a.major).then_with(|| a.path.cmp(&b.path)));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn major_versions() {
        assert_eq!(parse_major("1.8.0_412"), 8);
        assert_eq!(parse_major("17.0.12"), 17);
        assert_eq!(parse_major("21"), 21);
        assert_eq!(parse_major("25-ea"), 25);
        assert_eq!(parse_major(""), 0);
    }

    #[test]
    fn reads_release_file() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(dir.path().join("release"), "IMPLEMENTOR=\"Eclipse Adoptium\"\nJAVA_VERSION=\"21.0.4\"\n").unwrap();
        let (version, vendor) = read_release(&bin.join("java")).unwrap();
        assert_eq!(version, "21.0.4");
        assert_eq!(vendor, "Eclipse Adoptium");
    }
}
