use std::path::{Path, PathBuf};

pub const RUNTIME_ARCHIVE: &str = "libpaco_runtime.a";

/// Where the linker and each target's link inputs come from: a
/// distribution's `lib/paco`, or in a development checkout the rustup
/// sysroot and `runtime/target`.
pub struct Toolchain {
    pub distribution: Option<PathBuf>,
    pub rust_sysroot: PathBuf,
    pub runtime_dir: PathBuf,
}

impl Toolchain {
    pub fn locate() -> Self {
        let distribution = std::env::current_exe()
            .ok()
            .and_then(|exe| Some(exe.parent()?.parent()?.join("lib/paco")))
            .filter(|dir| dir.is_dir());
        Self {
            distribution,
            rust_sysroot: PathBuf::from(env!("PACO_RUST_SYSROOT")),
            runtime_dir: PathBuf::from(env!("PACO_RUNTIME_DIR")),
        }
    }

    pub fn linker(&self) -> Result<PathBuf, String> {
        let name = format!("rust-lld{}", std::env::consts::EXE_SUFFIX);
        let mut candidates: Vec<PathBuf> = self.distribution.iter().map(|dir| dir.join("bin").join(&name)).collect();
        candidates.push(self.rust_sysroot.join("lib/rustlib").join(env!("PACO_HOST_TRIPLE")).join("bin").join(&name));
        if let Some(found) = candidates.iter().find(|path| path.is_file()) {
            return Ok(found.clone());
        }
        std::env::var_os("PATH")
            .and_then(|paths| std::env::split_paths(&paths).map(|dir| dir.join("ld.lld")).find(|path| path.is_file()))
            .ok_or_else(|| {
                let searched: Vec<String> = candidates.iter().map(|path| path.display().to_string()).collect();
                format!("no linker found: looked for {} and `ld.lld` on PATH", searched.join(", "))
            })
    }

    /// `file` for `target`: a musl startup object or archive, or
    /// [`RUNTIME_ARCHIVE`].
    pub fn component(&self, target: &str, file: &str) -> Result<PathBuf, String> {
        let path = match &self.distribution {
            Some(dir) => dir.join(target).join(file),
            None if file == RUNTIME_ARCHIVE => self.runtime_dir.join("target").join(target).join("release").join(built_archive(target)),
            None => self.rust_sysroot.join("lib/rustlib").join(target).join("lib/self-contained").join(file),
        };
        if path.is_file() { Ok(path) } else { Err(missing_component(file, target, &path)) }
    }

    /// The host runtime built with libc `malloc`, for sanitizer builds and
    /// allocator benchmarks (`PACO_SYSTEM_ALLOC`).
    pub fn system_alloc_runtime(&self) -> Result<PathBuf, String> {
        let host = env!("PACO_HOST_TRIPLE");
        let path = self.runtime_dir.join("target/system-alloc").join(host).join("release").join(built_archive(host));
        if path.is_file() { Ok(path) } else { Err(missing_component(RUNTIME_ARCHIVE, host, &path)) }
    }
}

/// The file name Cargo gives the `paco-runtime-ffi` static library for
/// `target`.
fn built_archive(target: &str) -> &'static str {
    if target.ends_with("-msvc") { "paco_runtime_ffi.lib" } else { "libpaco_runtime_ffi.a" }
}

fn missing_component(file: &str, target: &str, path: &Path) -> String {
    format!(
        "error[PACO-E0805]: component `{file}` for target `{target}` is missing from the Paco distribution at `{}`",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const MUSL_FILES: [&str; 8] =
        ["crt1.o", "crti.o", "crtn.o", "crtbegin.o", "crtend.o", "libc.a", "libunwind.a", RUNTIME_ARCHIVE];

    fn fake_distribution() -> (tempfile::TempDir, Toolchain) {
        let root = tempfile::tempdir().unwrap();
        let lib = root.path().join("lib/paco");
        std::fs::create_dir_all(lib.join("bin")).unwrap();
        std::fs::write(lib.join("bin").join(format!("rust-lld{}", std::env::consts::EXE_SUFFIX)), "").unwrap();
        for target in ["x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl"] {
            std::fs::create_dir_all(lib.join(target)).unwrap();
            for file in MUSL_FILES {
                std::fs::write(lib.join(target).join(file), "").unwrap();
            }
        }
        let toolchain = Toolchain {
            distribution: Some(lib),
            rust_sysroot: root.path().join("no-rustup"),
            runtime_dir: root.path().join("no-runtime"),
        };
        (root, toolchain)
    }

    #[test]
    fn resolves_every_component_from_a_distribution() {
        let (_root, toolchain) = fake_distribution();
        let linker = toolchain.linker().unwrap();
        assert!(linker.ends_with(format!("lib/paco/bin/rust-lld{}", std::env::consts::EXE_SUFFIX)), "{}", linker.display());
        for file in MUSL_FILES {
            let path = toolchain.component("aarch64-unknown-linux-musl", file).unwrap();
            assert!(path.is_file() && path.ends_with(format!("aarch64-unknown-linux-musl/{file}")));
        }
    }

    #[test]
    fn a_missing_component_is_reported_with_file_target_and_path() {
        let (_root, toolchain) = fake_distribution();
        let lib = toolchain.distribution.clone().unwrap();
        std::fs::remove_file(lib.join("x86_64-unknown-linux-musl").join(RUNTIME_ARCHIVE)).unwrap();
        let error = toolchain.component("x86_64-unknown-linux-musl", RUNTIME_ARCHIVE).unwrap_err();
        assert!(error.contains("PACO-E0805"), "{error}");
        assert!(error.contains("`libpaco_runtime.a`") && error.contains("`x86_64-unknown-linux-musl`"), "{error}");
        assert!(error.contains(&lib.join("x86_64-unknown-linux-musl").display().to_string()), "{error}");
    }

    #[test]
    fn resolves_the_linker_and_musl_objects_from_the_rustup_sysroot() {
        let toolchain = Toolchain { distribution: None, ..Toolchain::locate() };
        assert!(toolchain.linker().unwrap().is_file());
        let host_arch_musl = format!("{}-unknown-linux-musl", std::env::consts::ARCH);
        for file in MUSL_FILES.iter().filter(|file| cfg!(target_os = "linux") || **file != RUNTIME_ARCHIVE) {
            assert!(toolchain.component(&host_arch_musl, file).unwrap().is_file(), "{file}");
        }
        assert!(toolchain.component(env!("PACO_HOST_TRIPLE"), RUNTIME_ARCHIVE).unwrap().is_file());
    }
}
