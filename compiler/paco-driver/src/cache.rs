//! The build cache behind `paco run`: binaries keyed by every input that can
//! change them, like Go's `GOCACHE`.
//!
//! `index/<index key>` is the manifest of the last build of one entry file
//! (the files it read with their hashes, and its output key); the binary is
//! `bin/<output key>`. Writes go to `tmp/` first and are renamed into place.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use twox_hash::xxhash3_128::Hasher;

/// When entries are touched and pruned.
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub max_age: Duration,
    pub max_size: u64,
    pub prune_interval: Duration,
    pub touch_interval: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            max_age: Duration::from_secs(30 * 24 * 60 * 60),
            max_size: 1 << 30,
            prune_interval: Duration::from_secs(24 * 60 * 60),
            touch_interval: Duration::from_secs(60 * 60),
        }
    }
}

pub struct Cache {
    root: PathBuf,
    /// A directory made for this process because the cache could not be
    /// used; removed when the cache is dropped.
    temporary: bool,
    policy: Policy,
}

/// The directory the cache lives in: `PACO_CACHE`, else
/// `$XDG_CACHE_HOME/paco`, else `$HOME/.cache/paco`.
pub fn cache_dir(env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let set = |name: &str| env(name).filter(|value| !value.is_empty()).map(PathBuf::from);
    set("PACO_CACHE")
        .or_else(|| set("XDG_CACHE_HOME").map(|dir| dir.join("paco")))
        .or_else(|| set("HOME").map(|dir| dir.join(".cache").join("paco")))
}

fn unique(prefix: &str) -> String {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_nanos());
    let count = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{prefix}-{}-{nanos}-{count}", std::process::id())
}

fn hex(value: u128) -> String {
    format!("{value:032x}")
}

/// A file a build read, or a directory whose listing it read.
#[derive(Clone, Debug)]
pub struct Input {
    pub path: PathBuf,
    pub directory: bool,
    pub hash: u128,
}

impl Input {
    pub fn file(path: PathBuf, contents: &[u8]) -> Self {
        Self { path, directory: false, hash: Hasher::oneshot(contents) }
    }

    /// The `.paco` files directly in `path`, as the prelude loader lists them.
    pub fn directory(path: PathBuf) -> Self {
        let hash = listing_hash(&path).unwrap_or(0);
        Self { path, directory: true, hash }
    }

    fn current_hash(&self) -> Option<u128> {
        if self.directory { listing_hash(&self.path) } else { fs::read(&self.path).ok().map(|bytes| Hasher::oneshot(&bytes)) }
    }
}

fn listing_hash(path: &Path) -> Option<u128> {
    let mut names: Vec<String> = fs::read_dir(path)
        .ok()?
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".paco"))
        .collect();
    names.sort();
    Some(Hasher::oneshot(names.join("\n").as_bytes()))
}

fn encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode(text: &str) -> Option<Vec<u8>> {
    (0..text.len()).step_by(2).map(|index| u8::from_str_radix(text.get(index..index + 2)?, 16).ok()).collect()
}

/// The inputs a cached build depends on besides its source files.
pub struct Key {
    pub compiler: String,
    pub settings: Vec<String>,
}

impl Key {
    fn index(&self, entry: &Path) -> String {
        let mut hasher = Hasher::new();
        let canonical = fs::canonicalize(entry).unwrap_or_else(|_| entry.to_path_buf());
        for part in [self.compiler.as_str(), &canonical.to_string_lossy(), &entry.to_string_lossy()]
            .into_iter()
            .chain(self.settings.iter().map(String::as_str))
        {
            hasher.write(part.as_bytes());
            hasher.write(&[0]);
        }
        hex(hasher.finish_128())
    }
}

/// A binary in the cache and what its build printed at compile time.
pub struct Entry {
    pub binary: PathBuf,
    pub stdout: String,
    pub stderr: String,
}

struct Manifest {
    output: String,
    inputs: Vec<Input>,
    stdout: String,
    stderr: String,
}

impl Manifest {
    fn render(&self) -> String {
        let mut text = format!("paco-cache 1\noutput {}\n", self.output);
        for input in &self.inputs {
            let kind = if input.directory { "dir" } else { "file" };
            text.push_str(&format!("{kind} {} {}\n", hex(input.hash), input.path.display()));
        }
        text.push_str(&format!("stdout {}\nstderr {}\n", encode(self.stdout.as_bytes()), encode(self.stderr.as_bytes())));
        text
    }

    fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        (lines.next()? == "paco-cache 1").then_some(())?;
        let output = lines.next()?.strip_prefix("output ")?.to_string();
        let mut manifest = Manifest { output, inputs: Vec::new(), stdout: String::new(), stderr: String::new() };
        for line in lines {
            let (kind, rest) = line.split_once(' ').unwrap_or((line, ""));
            match kind {
                "file" | "dir" => {
                    let (hash, path) = rest.split_once(' ')?;
                    let hash = u128::from_str_radix(hash, 16).ok()?;
                    manifest.inputs.push(Input { path: PathBuf::from(path), directory: kind == "dir", hash });
                }
                "stdout" => manifest.stdout = String::from_utf8(decode(rest)?).ok()?,
                "stderr" => manifest.stderr = String::from_utf8(decode(rest)?).ok()?,
                _ => return None,
            }
        }
        Some(manifest)
    }
}

impl Cache {
    /// The cache at `root`, or, when it cannot be created, a temporary
    /// directory for this build with a warning on stderr.
    pub fn open(root: Option<PathBuf>, policy: Policy) -> Self {
        let created = root.as_ref().map(|root| (root, fs::create_dir_all(root.join("tmp"))));
        if let Some((root, Ok(()))) = created {
            return Self { root: root.clone(), temporary: false, policy };
        }
        let temporary = std::env::temp_dir().join(unique("paco-build"));
        let _ = fs::create_dir_all(temporary.join("tmp"));
        match created {
            Some((root, Err(error))) => eprintln!(
                "warning: cannot use the build cache at {}: {error}; building in {}",
                root.display(),
                temporary.display()
            ),
            _ => eprintln!("warning: no build cache directory (set PACO_CACHE); building in {}", temporary.display()),
        }
        Self { root: temporary, temporary: true, policy }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn is_temporary(&self) -> bool {
        self.temporary
    }

    fn binary(&self, output: &str) -> PathBuf {
        self.root.join("bin").join(format!("{output}{}", std::env::consts::EXE_SUFFIX))
    }

    fn manifest_path(&self, index: &str) -> PathBuf {
        self.root.join("index").join(index)
    }

    /// The cached binary for `entry`, when every file its last build read
    /// is unchanged.
    pub fn lookup(&self, entry: &Path, key: &Key) -> Option<Entry> {
        let path = self.manifest_path(&key.index(entry));
        let manifest = Manifest::parse(&fs::read_to_string(&path).ok()?)?;
        if manifest.inputs.iter().any(|input| input.current_hash() != Some(input.hash)) {
            return None;
        }
        let binary = self.binary(&manifest.output);
        if !binary.is_file() {
            return None;
        }
        self.touch(&path);
        Some(Entry { binary, stdout: manifest.stdout, stderr: manifest.stderr })
    }

    fn touch(&self, path: &Path) {
        let stale = fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_none_or(|age| age >= self.policy.touch_interval);
        if stale && let Ok(file) = fs::File::options().append(true).open(path) {
            let _ = file.set_modified(SystemTime::now());
        }
    }

    /// A fresh directory under `tmp/` to build in.
    pub fn scratch(&self) -> Result<PathBuf, String> {
        let dir = self.root.join("tmp").join(unique("build"));
        fs::create_dir_all(&dir).map_err(|error| format!("failed to create `{}`: {error}", dir.display()))?;
        Ok(dir)
    }

    /// Retries a filesystem operation briefly on Windows, where a
    /// just-created file can be transiently locked by antivirus scanning or
    /// another handle and an operation reports access-denied/sharing-
    /// violation instead of waiting for the lock to clear.
    fn retrying<T>(mut op: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
        let attempts = if cfg!(windows) { 30 } else { 1 };
        let mut error = None;
        let mut delay = Duration::from_millis(10);
        for attempt in 0..attempts {
            match op() {
                Ok(value) => return Ok(value),
                Err(err) if matches!(err.kind(), std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::AlreadyExists) => {
                    error = Some(err)
                }
                Err(err) => return Err(err),
            }
            if attempt + 1 < attempts {
                std::thread::sleep(delay);
                delay = (delay * 2).min(Duration::from_millis(500));
            }
        }
        Err(error.expect("looped at least once"))
    }

    fn rename_retrying(from: &Path, to: &Path) -> std::io::Result<()> {
        Self::retrying(|| fs::rename(from, to))
    }

    /// Moves the freshly built `binary` into the cache for `entry` and
    /// records the `inputs` it was built from.
    pub fn publish(
        &self,
        entry: &Path,
        key: &Key,
        binary: &Path,
        inputs: Vec<Input>,
        stdout: &str,
        stderr: &str,
    ) -> Result<Entry, String> {
        let index = key.index(entry);
        let mut hasher = Hasher::new();
        hasher.write(index.as_bytes());
        for input in &inputs {
            hasher.write(&input.hash.to_le_bytes());
            hasher.write(input.path.to_string_lossy().as_bytes());
        }
        let output = hex(hasher.finish_128());
        fn failure(step: &str, path: &Path, error: std::io::Error) -> String {
            format!("failed to write the build cache ({step} `{}`): {error}", path.display())
        }
        let bin_dir = self.root.join("bin");
        Self::retrying(|| fs::create_dir_all(&bin_dir)).map_err(|error| failure("create_dir_all", &bin_dir, error))?;
        let index_dir = self.root.join("index");
        Self::retrying(|| fs::create_dir_all(&index_dir)).map_err(|error| failure("create_dir_all", &index_dir, error))?;
        let cached = self.binary(&output);
        // Opening a just-linked executable can be denied for a long time
        // on Windows; the rename below does not need this handle's flush.
        if !cfg!(windows) {
            Self::retrying(|| fs::File::open(binary).and_then(|file| file.sync_all())).map_err(|error| failure("sync", binary, error))?;
        }
        Self::rename_retrying(binary, &cached).map_err(|error| failure("rename", &cached, error))?;
        if debug_file(binary).is_file() {
            Self::rename_retrying(&debug_file(binary), &debug_file(&cached))
                .map_err(|error| failure("rename", &debug_file(&cached), error))?;
        }
        let manifest = Manifest { output, inputs, stdout: stdout.to_string(), stderr: stderr.to_string() };
        let staged = self.root.join("tmp").join(unique("manifest"));
        let mut file = Self::retrying(|| fs::File::create(&staged)).map_err(|error| failure("create", &staged, error))?;
        file.write_all(manifest.render().as_bytes()).and_then(|()| file.sync_all()).map_err(|error| failure("write", &staged, error))?;
        let index_path = self.manifest_path(&index);
        Self::rename_retrying(&staged, &index_path).map_err(|error| failure("rename", &index_path, error))?;
        self.prune_if_due();
        Ok(Entry { binary: cached, stdout: stdout.to_string(), stderr: stderr.to_string() })
    }

    fn prune_if_due(&self) {
        let marker = self.root.join("last-prune");
        let due = fs::metadata(&marker)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_none_or(|age| age >= self.policy.prune_interval);
        if due {
            let _ = fs::write(&marker, b"");
            self.prune();
        }
    }

    /// Removes entries unused for longer than the age limit, then the least
    /// recently used until the cache fits its size limit.
    pub fn prune(&self) {
        let entries = |dir: &str| fs::read_dir(self.root.join(dir)).into_iter().flatten().flatten().map(|entry| entry.path());
        let size = |path: &Path| fs::metadata(path).map_or(0, |metadata| metadata.len());
        let age = |path: &Path| fs::metadata(path).and_then(|metadata| metadata.modified()).ok().and_then(|time| time.elapsed().ok());
        let mut manifests: Vec<(Duration, PathBuf, String)> = entries("index")
            .filter_map(|path| {
                let output = Manifest::parse(&fs::read_to_string(&path).ok()?)?.output;
                Some((age(&path).unwrap_or_default(), path, output))
            })
            .collect();
        manifests.sort_by_key(|entry| std::cmp::Reverse(entry.0));
        let mut references: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for (_, _, output) in &manifests {
            *references.entry(output.clone()).or_default() += 1;
        }
        for binary in entries("bin") {
            let name = binary.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            let name = name.strip_suffix(".dwarf").unwrap_or(&name);
            let name = name.strip_suffix(std::env::consts::EXE_SUFFIX).unwrap_or(name);
            if !references.contains_key(name) && age(&binary).is_some_and(|age| age >= self.policy.touch_interval) {
                let _ = fs::remove_file(&binary);
            }
        }
        let mut total: u64 = entries("index").chain(entries("bin")).map(|path| size(&path)).sum();
        for (age, manifest, output) in manifests {
            if age <= self.policy.max_age && total <= self.policy.max_size {
                continue;
            }
            total = total.saturating_sub(size(&manifest));
            let _ = fs::remove_file(&manifest);
            let count = references.get_mut(&output).expect("counted above");
            *count -= 1;
            if *count == 0 {
                let binary = self.binary(&output);
                total = total.saturating_sub(size(&binary) + size(&debug_file(&binary)));
                let _ = fs::remove_file(&binary);
                let _ = fs::remove_file(debug_file(&binary));
            }
        }
    }

    /// Removes everything in the cache.
    pub fn clear(&self) -> Result<(), String> {
        for entry in fs::read_dir(&self.root).map_err(|error| format!("failed to read `{}`: {error}", self.root.display()))? {
            let path = entry.map_err(|error| error.to_string())?.path();
            let removed = Self::retrying(|| if path.is_dir() { fs::remove_dir_all(&path) } else { fs::remove_file(&path) });
            removed.map_err(|error| format!("failed to remove `{}`: {error}", path.display()))?;
        }
        Ok(())
    }
}

impl Drop for Cache {
    fn drop(&mut self) {
        if self.temporary {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

/// The line tables macOS links collect beside `binary` for panic traces.
pub fn debug_file(binary: &Path) -> PathBuf {
    PathBuf::from(format!("{}.dwarf", binary.display()))
}
