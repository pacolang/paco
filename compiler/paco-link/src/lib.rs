//! Links a compiled Paco program's objects with the Paco runtime into a
//! native executable, calling `rust-lld` directly.

use std::path::{Path, PathBuf};
use std::process::Command;

pub mod toolchain;

use toolchain::{RUNTIME_ARCHIVE, Toolchain};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkMode {
    /// musl, statically linked: no dynamic loader, no shared libraries.
    Static,
    /// The target system's glibc and every `extern` library, as shared
    /// objects.
    Dynamic,
}

pub struct LinkRequest<'a> {
    pub objects: &'a [PathBuf],
    pub output: &'a Path,
    pub mode: LinkMode,
    /// Libraries named by the program's `extern` blocks (`m` → `libm`).
    pub extra_libs: &'a [String],
    /// A complete triple, such as `aarch64-unknown-linux-musl`.
    pub target: &'a str,
    /// The target system's root for dynamic mode; `/` when `None`.
    pub sysroot: Option<&'a Path>,
    /// Whether the objects carry debug line tables that panic traces read;
    /// on macOS they are collected into `<output>.dwarf`.
    pub debug: bool,
}

/// The oldest macOS release Paco programs and the runtime target.
pub const MACOS_DEPLOYMENT_TARGET: &str = "11.0";

pub fn host_triple() -> &'static str {
    env!("PACO_HOST_TRIPLE")
}

pub fn link_program(request: &LinkRequest<'_>) -> Result<(), String> {
    let toolchain = Toolchain::locate();
    if request.target.ends_with("-windows-msvc") {
        return link_coff(&toolchain, request);
    }
    if request.target.ends_with("-apple-darwin") && sanitizer().is_none() {
        return link_macho(&toolchain, request);
    }
    if sanitizer().is_some() || !request.target.contains("-linux-") {
        return link_with_system_driver(&toolchain, request);
    }
    let arguments = match request.mode {
        LinkMode::Static => static_arguments(&toolchain, request)?,
        LinkMode::Dynamic => dynamic_arguments(&toolchain, request)?,
    };
    let linker = toolchain.linker()?;
    let output = Command::new(&linker)
        .args(["-flavor", "gnu"])
        .args(&arguments)
        .output()
        .map_err(|error| format!("failed to invoke `{}`: {error}", linker.display()))?;
    if !output.status.success() {
        return Err(format!("`{}` failed: {}", linker.display(), String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(())
}

pub fn static_arguments(toolchain: &Toolchain, request: &LinkRequest<'_>) -> Result<Vec<String>, String> {
    let component = |file: &str| toolchain.component(request.target, file).map(|path| path.display().to_string());
    let mut arguments: Vec<String> = ["-static", "--threads=1", "--eh-frame-hdr", "--gc-sections", "-z", "noexecstack"]
        .map(String::from)
        .to_vec();
    arguments.extend(["--defsym=main=paco_rt_main".to_string(), "--undefined=paco_rt_main".to_string()]);
    arguments.extend([component("crt1.o")?, component("crti.o")?, component("crtbegin.o")?]);
    arguments.extend(request.objects.iter().map(|path| path.display().to_string()));
    arguments.extend([
        component(RUNTIME_ARCHIVE)?,
        component("libunwind.a")?,
        component("libc.a")?,
        component("crtend.o")?,
        component("crtn.o")?,
    ]);
    arguments.extend(["-o".to_string(), request.output.display().to_string()]);
    Ok(arguments)
}

pub fn dynamic_arguments(toolchain: &Toolchain, request: &LinkRequest<'_>) -> Result<Vec<String>, String> {
    let arch = request.target.split('-').next().unwrap_or_default();
    let dynamic_linker = match arch {
        "x86_64" => "/lib64/ld-linux-x86-64.so.2",
        "aarch64" => "/lib/ld-linux-aarch64.so.1",
        other => return Err(format!("dynamic linking is not supported for the `{other}` architecture")),
    };
    let root = request.sysroot.unwrap_or(Path::new("/"));
    let dirs = search_dirs(root, arch, request.sysroot.is_none());
    let mut arguments: Vec<String> = [
        "-pie",
        "--threads=1",
        "--eh-frame-hdr",
        "--gc-sections",
        "-z",
        "noexecstack",
        "-z",
        "relro",
        "-z",
        "now",
        "--dynamic-linker",
        dynamic_linker,
        "-e",
        "paco_rt_start",
    ]
    .map(String::from)
    .to_vec();
    if request.sysroot.is_some() {
        arguments.push(format!("--sysroot={}", root.display()));
    }
    arguments.extend(request.objects.iter().map(|path| path.display().to_string()));
    let runtime = if std::env::var_os("PACO_SYSTEM_ALLOC").is_some() && request.target == host_triple() {
        toolchain.system_alloc_runtime()?
    } else {
        toolchain.component(request.target, RUNTIME_ARCHIVE)?
    };
    arguments.push(runtime.display().to_string());
    for lib in request.extra_libs {
        let found = find_library(&dirs, lib).ok_or_else(|| {
            format!(
                "error[PACO-E0803]: library `lib{lib}` for the `extern` block in module `{lib}` was not found for target `{}`; \
                 searched {}; pass the target system's root with `--sysroot <dir>`",
                request.target,
                list(&dirs)
            )
        })?;
        arguments.push(found.display().to_string());
    }
    for file in ["libc.so.6", "libm.so.6", "libgcc_s.so.1"] {
        let found = dirs.iter().map(|dir| dir.join(file)).find(|path| path.exists()).ok_or_else(|| {
            format!(
                "error[PACO-E0802]: the target glibc (`{file}`) for `{}` was not found; searched {}; \
                 pass the target system's root with `--sysroot <dir>`",
                request.target,
                list(&dirs)
            )
        })?;
        arguments.push(found.display().to_string());
    }
    let loader = Path::new(dynamic_linker).file_name().unwrap();
    if let Some(found) = dirs.iter().map(|dir| dir.join(loader)).find(|path| path.exists()) {
        arguments.extend(["--as-needed".to_string(), found.display().to_string()]);
    }
    arguments.extend(["-o".to_string(), request.output.display().to_string()]);
    Ok(arguments)
}

fn list(dirs: &[PathBuf]) -> String {
    dirs.iter().map(|dir| format!("`{}`", dir.display())).collect::<Vec<_>>().join(", ")
}

fn search_dirs(root: &Path, arch: &str, host: bool) -> Vec<PathBuf> {
    let multiarch = format!("{arch}-linux-gnu");
    let mut dirs: Vec<PathBuf> = if host {
        std::env::var_os("LIBRARY_PATH").map(|paths| std::env::split_paths(&paths).collect()).unwrap_or_default()
    } else {
        Vec::new()
    };
    dirs.extend(
        [
            format!("lib/{multiarch}"),
            format!("usr/lib/{multiarch}"),
            "lib64".into(),
            "usr/lib64".into(),
            "usr/local/lib".into(),
            "lib".into(),
            "usr/lib".into(),
        ]
        .iter()
        .map(|dir| root.join(dir)),
    );
    dirs
}

/// `lib<name>` as `.so`, `.dylib`, `.tbd` or `.a`, else the first versioned
/// `lib<name>.so.N`.
fn find_library(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    let shared = format!("lib{name}.so");
    let files = [shared.clone(), format!("lib{name}.dylib"), format!("lib{name}.tbd"), format!("lib{name}.a")];
    for dir in dirs {
        for file in &files {
            if dir.join(file).exists() {
                return Some(dir.join(file));
            }
        }
    }
    let versioned = format!("{shared}.");
    dirs.iter().find_map(|dir| {
        let mut candidates: Vec<PathBuf> = std::fs::read_dir(dir)
            .ok()?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&versioned)))
            .collect();
        candidates.sort();
        candidates.into_iter().next()
    })
}

/// `LIBRARY_PATH`, the SDK's `usr/lib` and the Homebrew prefixes.
fn macos_library_dirs(sdk: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> =
        std::env::var_os("LIBRARY_PATH").map(|paths| std::env::split_paths(&paths).collect()).unwrap_or_default();
    dirs.push(sdk.join("usr/lib"));
    dirs.extend(["/opt/homebrew/lib", "/usr/local/lib"].map(PathBuf::from));
    dirs
}

/// The active developer directory `xcode-select` records, read without
/// running it.
fn developer_dir() -> PathBuf {
    std::env::var_os("DEVELOPER_DIR")
        .map(PathBuf::from)
        .or_else(|| std::fs::read_link("/var/db/xcode_select_link").ok())
        .unwrap_or_else(|| PathBuf::from("/Library/Developer/CommandLineTools"))
}

/// `SDKROOT`, the active developer directory's macOS SDK, or what `xcrun`
/// reports.
fn macos_sdk() -> Result<PathBuf, String> {
    if let Some(sdk) = std::env::var_os("SDKROOT").map(PathBuf::from).filter(|sdk| sdk.is_dir()) {
        return Ok(sdk);
    }
    let developer = developer_dir();
    let candidates =
        [developer.join("Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk"), developer.join("SDKs/MacOSX.sdk")];
    if let Some(sdk) = candidates.into_iter().find(|sdk| sdk.is_dir()) {
        return Ok(sdk);
    }
    Command::new("xcrun")
        .arg("--show-sdk-path")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
        .ok_or_else(|| "no macOS SDK found: install the Xcode command line tools (`xcode-select --install`)".to_string())
}

/// Links a Mach-O executable with `rust-lld -flavor darwin` against the
/// SDK, then collects its line tables with `dsymutil`.
fn link_macho(toolchain: &Toolchain, request: &LinkRequest<'_>) -> Result<(), String> {
    let sdk = macos_sdk()?;
    let arch = if request.target.starts_with("aarch64") { "arm64" } else { "x86_64" };
    let linker = toolchain.linker()?;
    let mut command = Command::new(&linker);
    command
        .args(["-flavor", "darwin", "-arch", arch, "-platform_version", "macos"])
        .args([MACOS_DEPLOYMENT_TARGET, MACOS_DEPLOYMENT_TARGET, "-syslibroot"])
        .arg(&sdk)
        .args(request.objects);
    let runtime = if std::env::var_os("PACO_SYSTEM_ALLOC").is_some() {
        toolchain.system_alloc_runtime()?
    } else {
        toolchain.component(request.target, RUNTIME_ARCHIVE)?
    };
    command.arg(runtime);
    if !request.extra_libs.is_empty() {
        let dirs = macos_library_dirs(&sdk);
        for lib in request.extra_libs {
            let found = find_library(&dirs, lib).ok_or_else(|| {
                format!(
                    "error[PACO-E0803]: library `lib{lib}` for the `extern` block in module `{lib}` was not found for target `{}`; \
                     searched {}",
                    request.target,
                    list(&dirs)
                )
            })?;
            command.arg(found);
        }
    }
    command.args(["-lSystem", "-liconv", "-dead_strip_dylibs", "-alias", "_paco_rt_main", "_main", "-o"]).arg(request.output);
    run(&mut command, &linker.display().to_string())?;
    let dwarf = PathBuf::from(format!("{}.dwarf", request.output.display()));
    let _ = std::fs::remove_file(&dwarf);
    if request.debug {
        let developer = developer_dir();
        let dsymutil = [developer.join("Toolchains/XcodeDefault.xctoolchain/usr/bin/dsymutil"), developer.join("usr/bin/dsymutil")]
            .into_iter()
            .find(|path| path.is_file())
            .unwrap_or_else(|| PathBuf::from("dsymutil"));
        run(Command::new(dsymutil).arg("--flat").arg("-o").arg(&dwarf).arg(request.output), "dsymutil")?;
    }
    Ok(())
}

fn sanitizer() -> Option<String> {
    std::env::var("PACO_SANITIZE").ok().filter(|name| !name.is_empty())
}

/// AddressSanitizer's runtime ships with the system C compiler, so
/// sanitizer builds link through `cc`.
fn link_with_system_driver(toolchain: &Toolchain, request: &LinkRequest<'_>) -> Result<(), String> {
    let mut command = Command::new("cc");
    command.args(request.objects);
    if let Some(name) = sanitizer() {
        command.arg(format!("-fsanitize={name}"));
    }
    command.arg(toolchain.system_alloc_runtime()?);
    if cfg!(target_os = "macos") {
        command.arg(format!("-mmacosx-version-min={MACOS_DEPLOYMENT_TARGET}")).arg("-Wl,-alias,_paco_rt_main,_main");
        command.arg("-liconv");
    } else {
        command.arg("-Wl,--defsym=main=paco_rt_main").arg("-Wl,--undefined=paco_rt_main");
    }
    let dirs = if request.extra_libs.is_empty() || cfg!(target_os = "macos") {
        Vec::new()
    } else {
        search_dirs(Path::new("/"), std::env::consts::ARCH, true)
    };
    for lib in request.extra_libs {
        match find_library(&dirs, lib) {
            Some(path) if !path.to_string_lossy().ends_with(".so") && !path.to_string_lossy().ends_with(".a") => {
                command.arg(path)
            }
            _ => command.arg(format!("-l{lib}")),
        };
    }
    if cfg!(target_os = "linux") {
        command.arg("-lm");
    }
    command.arg("-o").arg(request.output);
    run(&mut command, "cc")?;
    if cfg!(target_os = "macos") && request.debug {
        let dwarf = PathBuf::from(format!("{}.dwarf", request.output.display()));
        run(Command::new("dsymutil").arg("--flat").arg("-o").arg(&dwarf).arg(request.output), "dsymutil")?;
    }
    Ok(())
}

/// The import libraries Rust's standard library and the runtime's C
/// allocator need from the Windows SDK and the MSVC CRT, discovered at
/// build time (`paco-link/build.rs`) from `rustc --print native-static-libs`
/// so a toolchain or dependency bump can't silently drift out of sync.
pub fn windows_system_libs() -> Vec<&'static str> {
    env!("PACO_WINDOWS_NATIVE_LIBS").split_whitespace().collect()
}

/// Links a PE/COFF executable with `rust-lld -flavor link`, taking the CRT
/// and SDK import libraries from the MSVC environment.
fn link_coff(toolchain: &Toolchain, request: &LinkRequest<'_>) -> Result<(), String> {
    let linker = toolchain.linker()?;
    let mut command = Command::new(&linker);
    let environment = msvc_environment(request.target);
    let lib = environment
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("LIB"))
        .map(|(_, value)| value.clone())
        .or_else(|| std::env::var_os("LIB"))
        .unwrap_or_default();
    let dirs: Vec<PathBuf> = std::env::split_paths(&lib).filter(|dir| !dir.as_os_str().is_empty()).collect();
    command.envs(environment);
    command.args([
        "-flavor",
        "link",
        "/nologo",
        "/subsystem:console",
        "/entry:mainCRTStartup",
        "/alternatename:main=paco_rt_main",
        "/include:paco_rt_main",
    ]);
    if request.debug {
        command.arg("/debug:dwarf");
    }
    command.args(request.objects);
    let runtime = if std::env::var_os("PACO_SYSTEM_ALLOC").is_some() {
        toolchain.system_alloc_runtime()?
    } else {
        toolchain.component(request.target, RUNTIME_ARCHIVE)?
    };
    command.arg(runtime);
    for lib in request.extra_libs {
        let file = format!("{lib}.lib");
        let found = dirs.iter().map(|dir| dir.join(&file)).find(|path| path.is_file()).ok_or_else(|| {
            format!(
                "error[PACO-E0803]: library `{file}` for the `extern` block in module `{lib}` was not found for target `{}`; searched {}",
                request.target,
                list(&dirs)
            )
        })?;
        command.arg(found);
    }
    for dir in env!("PACO_WINDOWS_LIB_DIRS").split(';').filter(|dir| !dir.is_empty()) {
        command.arg(format!("/libpath:{dir}"));
    }
    command.args(windows_system_libs().iter().map(|lib| format!("{lib}.lib")));
    command.arg(format!("/out:{}", request.output.display()));
    run(&mut command, &linker.display().to_string())
}

#[cfg(windows)]
fn msvc_environment(target: &str) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    cc::windows_registry::find_tool(target, "link.exe").map(|tool| tool.env().to_vec()).unwrap_or_default()
}

#[cfg(not(windows))]
fn msvc_environment(_target: &str) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    Vec::new()
}

fn run(command: &mut Command, name: &str) -> Result<(), String> {
    let output = command.output().map_err(|error| format!("failed to invoke `{name}`: {error}"))?;
    if !output.status.success() {
        return Err(format!("`{name}` exited with {}: {}", output.status, String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_link_line_orders_startup_objects_program_runtime_and_libc() {
        let root = tempfile::tempdir().unwrap();
        let lib = root.path().join("lib/paco");
        let target = "aarch64-unknown-linux-musl";
        std::fs::create_dir_all(lib.join(target)).unwrap();
        for file in ["crt1.o", "crti.o", "crtn.o", "crtbegin.o", "crtend.o", "libc.a", "libunwind.a", RUNTIME_ARCHIVE] {
            std::fs::write(lib.join(target).join(file), "").unwrap();
        }
        let toolchain =
            Toolchain { distribution: Some(lib.clone()), rust_sysroot: root.path().into(), runtime_dir: root.path().into() };
        let objects = [PathBuf::from("/p/main.o")];
        let request = LinkRequest {
            objects: &objects,
            output: Path::new("/p/main"),
            mode: LinkMode::Static,
            extra_libs: &[],
            target,
            sysroot: None,
            debug: false,
        };
        let arguments = static_arguments(&toolchain, &request).unwrap();
        let names: Vec<&str> = arguments
            .iter()
            .map(|argument| Path::new(argument).file_name().and_then(|name| name.to_str()).unwrap_or(argument))
            .skip_while(|a| *a != "crt1.o")
            .collect();
        assert_eq!(
            names,
            [
                "crt1.o",
                "crti.o",
                "crtbegin.o",
                "main.o",
                RUNTIME_ARCHIVE,
                "libunwind.a",
                "libc.a",
                "crtend.o",
                "crtn.o",
                "-o",
                "main"
            ]
        );
        assert!(arguments.contains(&"-static".to_string()));
        assert!(arguments.contains(&"--threads=1".to_string()), "a small link is slower with lld's thread pool");
    }

    #[test]
    fn dynamic_link_reports_missing_glibc_and_missing_extern_libraries() {
        let sysroot = tempfile::tempdir().unwrap();
        let objects = [PathBuf::from("/p/main.o")];
        let libs = ["mylib".to_string()];
        let mut request = LinkRequest {
            objects: &objects,
            output: Path::new("/p/main"),
            mode: LinkMode::Dynamic,
            extra_libs: &[],
            target: host_triple(),
            sysroot: Some(sysroot.path()),
            debug: false,
        };
        let error = dynamic_arguments(&Toolchain::locate(), &request).unwrap_err();
        assert!(error.contains("PACO-E0802") && error.contains("libc.so.6") && error.contains("--sysroot"), "{error}");
        assert!(error.contains(&sysroot.path().join("usr/lib").display().to_string()), "{error}");

        let libdir = sysroot.path().join(format!("usr/lib/{}-linux-gnu", std::env::consts::ARCH));
        std::fs::create_dir_all(&libdir).unwrap();
        for file in ["libc.so.6", "libm.so.6", "libgcc_s.so.1"] {
            std::fs::write(libdir.join(file), "").unwrap();
        }
        assert!(dynamic_arguments(&Toolchain::locate(), &request).is_ok());
        request.extra_libs = &libs;
        let error = dynamic_arguments(&Toolchain::locate(), &request).unwrap_err();
        assert!(error.contains("PACO-E0803") && error.contains("libmylib") && error.contains("--sysroot"), "{error}");
        std::fs::write(libdir.join("libmylib.so.3"), "").unwrap();
        let arguments = dynamic_arguments(&Toolchain::locate(), &request).unwrap();
        assert!(arguments.iter().any(|argument| argument.ends_with("libmylib.so.3")));
    }
}
