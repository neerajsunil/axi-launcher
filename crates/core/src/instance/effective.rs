use super::InstanceConfig;
use crate::settings::{GlobalSettings, WindowSettings};

/// Global settings with an instance's overrides applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveSettings {
    /// Empty means the Mojang runtime the game version asks for.
    pub java_path: String,
    pub min_memory_mb: u32,
    pub max_memory_mb: u32,
    pub jvm_args: Vec<String>,
    pub window: WindowSettings,
}

impl EffectiveSettings {
    pub fn resolve(global: &GlobalSettings, instance: &InstanceConfig) -> Self {
        let o = &instance.overrides;
        let (min, max) = o
            .memory
            .map_or((global.java.min_memory_mb, global.java.max_memory_mb), |m| (m.min_mb, m.max_mb));
        let jvm_args = o.jvm_args.as_deref().unwrap_or(&global.java.jvm_args);
        Self {
            java_path: o.java_path.clone().unwrap_or_else(|| global.java.path.clone()),
            min_memory_mb: min.min(max),
            max_memory_mb: max,
            jvm_args: jvm_args.split_whitespace().map(String::from).collect(),
            window: o.window.unwrap_or(global.window),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::{MemoryOverride, Overrides};

    #[test]
    fn overrides_win_over_global() {
        let mut global = GlobalSettings::default();
        global.java.path = "C:/java/bin/javaw.exe".into();
        global.java.jvm_args = "-XX:+UseG1GC".into();

        let mut inst = InstanceConfig::default();
        let e = EffectiveSettings::resolve(&global, &inst);
        assert_eq!(e.java_path, global.java.path);
        assert_eq!(e.max_memory_mb, global.java.max_memory_mb);
        assert_eq!(e.jvm_args, ["-XX:+UseG1GC"]);

        inst.overrides = Overrides {
            java_path: Some(String::new()),
            memory: Some(MemoryOverride { min_mb: 1024, max_mb: 8192 }),
            jvm_args: Some("-Da=1 -Db=2".into()),
            window: Some(WindowSettings { width: 1280, height: 720, fullscreen: true }),
        };
        let e = EffectiveSettings::resolve(&global, &inst);
        assert_eq!(e.java_path, "");
        assert_eq!((e.min_memory_mb, e.max_memory_mb), (1024, 8192));
        assert_eq!(e.jvm_args, ["-Da=1", "-Db=2"]);
        assert!(e.window.fullscreen);
    }
}
