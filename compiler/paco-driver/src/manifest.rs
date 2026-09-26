//! `paco.mod`/`paco.lock` parsing and writing (`git-module-fetch`): the git
//! dependency manifest and its resolved-commit lock file, both TOML
//! (design.md's "TOML, Go-`go.mod`-shaped" decision), parsed by hand against
//! `toml::Table` like `paco-driver`'s existing diagnostics-registry parsing
//! (`explain_in`) — no serde dependency added.

use std::path::Path;

/// A `paco.mod` dependency's value: a bare string is still a tag
/// (unchanged); a table with exactly one of `branch`/`rev`/`path` is one
/// of the extended sources (`git-module-fetch` task 8, matching
/// `Cargo.toml`'s own dependency-table shape — one key per kind, not a
/// `source`/`value` pair, so no separate validation step is needed to
/// check they're a compatible pair).
#[derive(Clone, Debug, PartialEq)]
pub enum DependencySource {
    /// A pinned tag, resolved and re-verified against `paco.lock` exactly
    /// as before task 8.
    Tag(String),
    /// A branch name; resolved to its tip commit on first fetch, then
    /// locked and treated exactly like `Tag` from then on.
    Branch(String),
    /// An exact commit; needs a full (non-shallow) clone, since
    /// `clone_at_tag`'s shallow `--branch` clone does not accept a bare
    /// commit.
    Rev(String),
    /// A local directory (relative to the `paco.mod` that declares it):
    /// no git, no cache, no `paco get` fetch step, no `paco.lock` entry.
    Path(String),
}

impl DependencySource {
    /// The opaque key used for cache-directory naming (`<path>@<key>/`)
    /// and for matching against a `paco.lock` entry: the tag, branch name
    /// or rev, whichever this source is. `None` for `Path`, which has no
    /// cache directory and no lock entry at all.
    pub fn cache_key(&self) -> Option<&str> {
        match self {
            DependencySource::Tag(key) | DependencySource::Branch(key) | DependencySource::Rev(key) => Some(key),
            DependencySource::Path(_) => None,
        }
    }
}

/// `paco.mod`: the current module's own name and its git dependencies, each
/// a domain-shaped path mapped to a `DependencySource`. Dependencies are
/// sorted by path (`toml::Table`'s own key order), not declaration order —
/// irrelevant here since nothing depends on manifest declaration order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Manifest {
    pub module: String,
    pub dependencies: Vec<(String, DependencySource)>,
    /// The optional `paco` field: the compiler versions this module
    /// supports (spec: "`paco.mod` may declare the compiler versions a
    /// module supports").
    pub paco: Option<VersionRange>,
}

impl Manifest {
    /// Parses `paco.mod`'s TOML text. A missing/mistyped `module`, a
    /// `dependencies` table whose values are not a tag string or a
    /// single-key `branch`/`rev`/`path` table, or a malformed `paco`
    /// version range, is a specific named error, never a panic (spec: "A
    /// malformed `paco.mod` is a clear compile error").
    pub fn parse(source: &str) -> Result<Manifest, String> {
        let table: toml::Table = source.parse().map_err(|error| format!("paco.mod is not valid TOML: {error}"))?;
        let module = table
            .get("module")
            .ok_or_else(|| "paco.mod is missing the required `module` field".to_string())?
            .as_str()
            .ok_or_else(|| "paco.mod's `module` field must be a string".to_string())?
            .to_string();
        let mut dependencies = Vec::new();
        if let Some(value) = table.get("dependencies") {
            let deps = value.as_table().ok_or_else(|| "paco.mod's `dependencies` must be a table".to_string())?;
            for (path, value) in deps {
                dependencies.push((path.clone(), parse_dependency_source(path, value)?));
            }
        }
        let paco = match table.get("paco") {
            Some(value) => {
                let raw = value.as_str().ok_or_else(|| "paco.mod's `paco` field must be a string".to_string())?;
                Some(VersionRange::parse(raw).map_err(|error| format!("paco.mod's `paco` field is malformed: {error}"))?)
            }
            None => None,
        };
        Ok(Manifest { module, dependencies, paco })
    }

    pub fn read(path: &Path) -> Result<Manifest, String> {
        let source = std::fs::read_to_string(path).map_err(|error| format!("failed to read `{}`: {error}", path.display()))?;
        Manifest::parse(&source)
    }
}

fn parse_dependency_source(dependency_path: &str, value: &toml::Value) -> Result<DependencySource, String> {
    if let Some(tag) = value.as_str() {
        return Ok(DependencySource::Tag(tag.to_string()));
    }
    let Some(table) = value.as_table() else {
        return Err(format!("paco.mod dependency `{dependency_path}` must be a tag string or a table with `branch`, `rev` or `path`"));
    };
    let field = |name: &str| table.get(name).and_then(toml::Value::as_str).map(str::to_string);
    let present: Vec<(&str, String)> =
        [("branch", field("branch")), ("rev", field("rev")), ("path", field("path"))].into_iter().filter_map(|(name, value)| Some((name, value?))).collect();
    match present.as_slice() {
        [("branch", value)] => Ok(DependencySource::Branch(value.clone())),
        [("rev", value)] => Ok(DependencySource::Rev(value.clone())),
        [("path", value)] => Ok(DependencySource::Path(value.clone())),
        [] => Err(format!("paco.mod dependency `{dependency_path}` must have exactly one of `branch`, `rev` or `path`")),
        _ => Err(format!("paco.mod dependency `{dependency_path}` has more than one of `branch`, `rev` and `path`")),
    }
}

/// One `paco.lock` entry: a dependency's declared path, exactly one of the
/// tag/branch/rev it was resolved from (a `path` dependency never gets a
/// lock entry at all — see `DependencySource::Path`), and the exact commit
/// `paco get` resolved it to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LockedPackage {
    pub path: String,
    pub tag: Option<String>,
    pub branch: Option<String>,
    pub rev: Option<String>,
    pub commit: String,
}

impl LockedPackage {
    /// Whether this lock entry was resolved from the same declared source
    /// (same kind, same tag/branch/rev value) — `paco get` reuses an
    /// existing lock entry's commit only when this holds, so a `paco.mod`
    /// edit that changes which tag/branch/rev a dependency names triggers
    /// a fresh resolution instead of silently keeping a stale pin.
    pub fn matches_source(&self, source: &DependencySource) -> bool {
        match source {
            DependencySource::Tag(tag) => self.tag.as_deref() == Some(tag.as_str()),
            DependencySource::Branch(branch) => self.branch.as_deref() == Some(branch.as_str()),
            DependencySource::Rev(rev) => self.rev.as_deref() == Some(rev.as_str()),
            DependencySource::Path(_) => false,
        }
    }
}

/// `paco.lock`: an array of resolved packages (design.md: `[[package]]`),
/// the source of truth for reproducible fetches.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lockfile {
    pub packages: Vec<LockedPackage>,
}

impl Lockfile {
    pub fn parse(source: &str) -> Result<Lockfile, String> {
        let table: toml::Table = source.parse().map_err(|error| format!("paco.lock is not valid TOML: {error}"))?;
        let mut packages = Vec::new();
        if let Some(value) = table.get("package") {
            let entries = value.as_array().ok_or_else(|| "paco.lock's `package` must be an array of tables".to_string())?;
            for entry in entries {
                let entry = entry.as_table().ok_or_else(|| "paco.lock's `package` entries must be tables".to_string())?;
                let field = |name: &str| -> Result<String, String> {
                    entry
                        .get(name)
                        .and_then(toml::Value::as_str)
                        .map(str::to_string)
                        .ok_or_else(|| format!("paco.lock package entry is missing a string `{name}` field"))
                };
                let field_opt = |name: &str| entry.get(name).and_then(toml::Value::as_str).map(str::to_string);
                packages.push(LockedPackage {
                    path: field("path")?,
                    tag: field_opt("tag"),
                    branch: field_opt("branch"),
                    rev: field_opt("rev"),
                    commit: field("commit")?,
                });
            }
        }
        Ok(Lockfile { packages })
    }

    pub fn read(path: &Path) -> Result<Lockfile, String> {
        let source = std::fs::read_to_string(path).map_err(|error| format!("failed to read `{}`: {error}", path.display()))?;
        Lockfile::parse(&source)
    }

    pub fn to_toml(&self) -> String {
        let mut out = String::new();
        for package in &self.packages {
            out.push_str("[[package]]\n");
            out.push_str(&format!("path = {:?}\n", package.path));
            if let Some(tag) = &package.tag {
                out.push_str(&format!("tag = {tag:?}\n"));
            }
            if let Some(branch) = &package.branch {
                out.push_str(&format!("branch = {branch:?}\n"));
            }
            if let Some(rev) = &package.rev {
                out.push_str(&format!("rev = {rev:?}\n"));
            }
            out.push_str(&format!("commit = {:?}\n\n", package.commit));
        }
        out
    }

    pub fn write(&self, path: &Path) -> Result<(), String> {
        std::fs::write(path, self.to_toml()).map_err(|error| format!("failed to write `{}`: {error}", path.display()))
    }

    /// The locked entry for a dependency path, if `paco get` has resolved it.
    pub fn find(&self, path: &str) -> Option<&LockedPackage> {
        self.packages.iter().find(|package| package.path == path)
    }
}

/// Splits a `paco.mod` dependency key (e.g. `"example.com/team/json"`)
/// into the same flattened segment form a parsed `Domain`-kind
/// `UseDecl.path` has: ADR 0015's domain grammar collapses `.` and `/`
/// into the same segment boundary at parse time (`module_path()`,
/// `paco-syntax`), so this is the only representation both a manifest key
/// and a parsed `use` path share.
pub fn key_segments(dependency_path: &str) -> Vec<String> {
    dependency_path.split(['.', '/']).filter(|segment| !segment.is_empty()).map(str::to_string).collect()
}

fn is_prefix(prefix: &[String], path: &[String]) -> bool {
    path.len() >= prefix.len() && path[..prefix.len()] == *prefix
}

/// The manifest dependency whose key is the longest segment-prefix of a
/// `Domain`-kind `use` path's segments, and that path's unmatched
/// remainder (design.md's "longest-prefix match, then a rooted
/// `use_path_to_file`" resolution decision).
pub fn longest_prefix_match<'a>(
    dependencies: &'a [(String, DependencySource)],
    path: &[String],
) -> Option<(&'a (String, DependencySource), Vec<String>)> {
    dependencies
        .iter()
        .filter_map(|dependency| {
            let key = key_segments(&dependency.0);
            is_prefix(&key, path).then_some((dependency, key.len()))
        })
        .max_by_key(|(_, len)| *len)
        .map(|(dependency, len)| (dependency, path[len..].to_vec()))
}

/// Whether `dependency_path` (a `paco.mod` key) is a prefix of (or equal
/// to) any path in `used_paths` — i.e. whether the program actually `use`s
/// that declared dependency.
pub fn is_used(dependency_path: &str, used_paths: &[Vec<String>]) -> bool {
    let key = key_segments(dependency_path);
    used_paths.iter().any(|path| is_prefix(&key, path))
}

/// A `paco.mod` `paco` field: a comma-separated set of version
/// constraints (e.g. `">=0.3, <0.5"`), every one of which a compiler
/// version must satisfy to be accepted (spec: "`paco.mod` may declare the
/// compiler versions a module supports").
#[derive(Clone, Debug, PartialEq)]
pub struct VersionRange {
    raw: String,
    constraints: Vec<Constraint>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Eq,
    Ge,
    Le,
    Gt,
    Lt,
}

#[derive(Clone, Debug, PartialEq)]
struct Constraint {
    op: Op,
    version: (u64, u64, u64),
}

impl VersionRange {
    pub fn parse(raw: &str) -> Result<VersionRange, String> {
        let mut constraints = Vec::new();
        for part in raw.split(',') {
            let part = part.trim();
            if part.is_empty() {
                return Err(format!("`{raw}` has an empty constraint"));
            }
            constraints.push(Constraint::parse(part).map_err(|error| format!("`{raw}`: {error}"))?);
        }
        Ok(VersionRange { raw: raw.to_string(), constraints })
    }

    /// Whether `version` (major, minor, patch) satisfies every constraint.
    pub fn contains(&self, version: (u64, u64, u64)) -> bool {
        self.constraints.iter().all(|constraint| constraint.matches(version))
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }
}

impl Constraint {
    fn parse(part: &str) -> Result<Constraint, String> {
        let (op, rest) = if let Some(rest) = part.strip_prefix(">=") {
            (Op::Ge, rest)
        } else if let Some(rest) = part.strip_prefix("<=") {
            (Op::Le, rest)
        } else if let Some(rest) = part.strip_prefix('>') {
            (Op::Gt, rest)
        } else if let Some(rest) = part.strip_prefix('<') {
            (Op::Lt, rest)
        } else if let Some(rest) = part.strip_prefix('=') {
            (Op::Eq, rest)
        } else {
            (Op::Eq, part)
        };
        Ok(Constraint { op, version: parse_version(rest.trim())? })
    }

    fn matches(&self, version: (u64, u64, u64)) -> bool {
        match self.op {
            Op::Eq => version == self.version,
            Op::Ge => version >= self.version,
            Op::Le => version <= self.version,
            Op::Gt => version > self.version,
            Op::Lt => version < self.version,
        }
    }
}

/// `major[.minor[.patch]]`, missing components defaulting to `0`.
fn parse_version(text: &str) -> Result<(u64, u64, u64), String> {
    let invalid = || format!("`{text}` is not a valid version");
    let mut parts = text.split('.');
    let mut next = || -> Result<u64, String> { parts.next().unwrap_or("0").parse::<u64>().map_err(|_| invalid()) };
    let version = (next()?, next()?, next()?);
    if parts.next().is_some() {
        return Err(invalid());
    }
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_manifest_parses_module_and_dependencies() {
        let manifest = Manifest::parse(
            "module = \"example.com/myproject\"\n\n[dependencies]\n\"example.com/team/json\" = \"v1.2.0\"\n",
        )
        .unwrap();
        assert_eq!(manifest.module, "example.com/myproject");
        assert_eq!(manifest.dependencies, vec![("example.com/team/json".to_string(), DependencySource::Tag("v1.2.0".to_string()))]);
    }

    #[test]
    fn manifest_with_no_dependencies_table_parses_to_empty() {
        let manifest = Manifest::parse("module = \"example.com/myproject\"\n").unwrap();
        assert!(manifest.dependencies.is_empty());
    }

    #[test]
    fn manifest_missing_module_is_a_named_error_not_a_panic() {
        let error = Manifest::parse("[dependencies]\n\"a\" = \"v1\"\n").unwrap_err();
        assert!(error.contains("module"), "{error}");
    }

    #[test]
    fn manifest_with_wrong_module_type_is_a_named_error() {
        let error = Manifest::parse("module = 5\n").unwrap_err();
        assert!(error.contains("module"), "{error}");
    }

    #[test]
    fn manifest_with_non_string_dependency_tag_is_a_named_error() {
        let error = Manifest::parse("module = \"m\"\n\n[dependencies]\n\"a\" = 5\n").unwrap_err();
        assert!(error.contains('a'), "{error}");
    }

    #[test]
    fn manifest_with_invalid_toml_is_a_named_error_not_a_panic() {
        let error = Manifest::parse("this is not toml =").unwrap_err();
        assert!(!error.is_empty());
    }

    #[test]
    fn manifest_parses_a_branch_dependency() {
        let manifest = Manifest::parse("module = \"m\"\n\n[dependencies]\n\"example.com/team/dev\" = { branch = \"main\" }\n").unwrap();
        assert_eq!(manifest.dependencies, vec![("example.com/team/dev".to_string(), DependencySource::Branch("main".to_string()))]);
    }

    #[test]
    fn manifest_parses_a_rev_dependency() {
        let manifest = Manifest::parse("module = \"m\"\n\n[dependencies]\n\"example.com/team/pinned\" = { rev = \"a1b2c3d\" }\n").unwrap();
        assert_eq!(manifest.dependencies, vec![("example.com/team/pinned".to_string(), DependencySource::Rev("a1b2c3d".to_string()))]);
    }

    #[test]
    fn manifest_parses_a_path_dependency() {
        let manifest = Manifest::parse("module = \"m\"\n\n[dependencies]\n\"example.com/team/local\" = { path = \"../local\" }\n").unwrap();
        assert_eq!(manifest.dependencies, vec![("example.com/team/local".to_string(), DependencySource::Path("../local".to_string()))]);
    }

    #[test]
    fn manifest_dependency_table_with_two_keys_is_a_named_error_not_a_panic() {
        let error =
            Manifest::parse("module = \"m\"\n\n[dependencies]\n\"a\" = { branch = \"main\", rev = \"a1b2c3d\" }\n").unwrap_err();
        assert!(error.contains('a'), "{error}");
    }

    #[test]
    fn manifest_dependency_table_with_no_keys_is_a_named_error_not_a_panic() {
        let error = Manifest::parse("module = \"m\"\n\n[dependencies]\n\"a\" = {}\n").unwrap_err();
        assert!(error.contains('a'), "{error}");
    }

    #[test]
    fn manifest_with_no_paco_field_accepts_any_compiler_version() {
        let manifest = Manifest::parse("module = \"m\"\n").unwrap();
        assert!(manifest.paco.is_none());
    }

    #[test]
    fn manifest_with_a_valid_paco_field_parses_the_range() {
        let manifest = Manifest::parse("module = \"m\"\npaco = \">=0.3, <0.5\"\n").unwrap();
        let range = manifest.paco.unwrap();
        assert!(range.contains((0, 4, 0)));
        assert!(!range.contains((0, 5, 0)));
        assert!(!range.contains((0, 2, 9)));
    }

    #[test]
    fn manifest_with_a_malformed_paco_field_is_a_named_error_not_a_panic() {
        let error = Manifest::parse("module = \"m\"\npaco = \"latest\"\n").unwrap_err();
        assert!(error.contains("paco"), "{error}");
    }

    #[test]
    fn version_range_parses_a_single_lower_bound() {
        let range = VersionRange::parse(">=0.4").unwrap();
        assert!(range.contains((0, 4, 0)));
        assert!(range.contains((1, 0, 0)));
        assert!(!range.contains((0, 3, 2)));
    }

    #[test]
    fn version_range_defaults_missing_version_components_to_zero() {
        let range = VersionRange::parse("=1").unwrap();
        assert!(range.contains((1, 0, 0)));
        assert!(!range.contains((1, 0, 1)));
    }

    #[test]
    fn version_range_rejects_a_non_numeric_version() {
        assert!(VersionRange::parse("latest").is_err());
        assert!(VersionRange::parse(">=0.3,").is_err());
        assert!(VersionRange::parse("").is_err());
    }

    #[test]
    fn lockfile_round_trips_through_toml() {
        let lockfile = Lockfile {
            packages: vec![
                LockedPackage {
                    path: "example.com/team/json".to_string(),
                    tag: Some("v1.2.0".to_string()),
                    commit: "a1b2c3".to_string(),
                    ..Default::default()
                },
                LockedPackage {
                    path: "example.com/team/dev".to_string(),
                    branch: Some("main".to_string()),
                    commit: "d4e5f6".to_string(),
                    ..Default::default()
                },
                LockedPackage {
                    path: "example.com/team/pinned".to_string(),
                    rev: Some("a1b2c3d".to_string()),
                    commit: "a1b2c3d".to_string(),
                    ..Default::default()
                },
            ],
        };
        let parsed = Lockfile::parse(&lockfile.to_toml()).unwrap();
        assert_eq!(parsed, lockfile);
    }

    #[test]
    fn lockfile_find_locates_a_package_by_path() {
        let lockfile = Lockfile {
            packages: vec![LockedPackage {
                path: "example.com/team/json".to_string(),
                tag: Some("v1.2.0".to_string()),
                commit: "a1b2c3".to_string(),
                ..Default::default()
            }],
        };
        assert_eq!(lockfile.find("example.com/team/json").map(|package| package.commit.as_str()), Some("a1b2c3"));
        assert!(lockfile.find("example.com/team/missing").is_none());
    }

    #[test]
    fn locked_package_matches_source_checks_kind_and_value() {
        let package = LockedPackage {
            path: "example.com/team/dev".to_string(),
            branch: Some("main".to_string()),
            commit: "abc".to_string(),
            ..Default::default()
        };
        assert!(package.matches_source(&DependencySource::Branch("main".to_string())));
        assert!(!package.matches_source(&DependencySource::Branch("other".to_string())));
        assert!(!package.matches_source(&DependencySource::Tag("main".to_string())));
        assert!(!package.matches_source(&DependencySource::Path("../local".to_string())));
    }

    fn segs(strings: &[&str]) -> Vec<String> {
        strings.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn key_segments_splits_on_both_dot_and_slash() {
        assert_eq!(key_segments("example.com/team/json"), segs(&["example", "com", "team", "json"]));
    }

    #[test]
    fn longest_prefix_match_picks_the_dependency_naming_a_multi_module_repo_over_a_shorter_one() {
        let deps = vec![
            ("example.com/team".to_string(), DependencySource::Tag("v1.0.0".to_string())),
            ("example.com/team/json".to_string(), DependencySource::Tag("v1.2.0".to_string())),
        ];
        let path = segs(&["example", "com", "team", "json"]);
        let (matched, remainder) = longest_prefix_match(&deps, &path).unwrap();
        assert_eq!(matched.0, "example.com/team/json");
        assert!(remainder.is_empty());
    }

    #[test]
    fn longest_prefix_match_leaves_a_remainder_for_a_module_inside_a_multi_module_repo() {
        let deps = vec![("github.com/pacolang/numerics".to_string(), DependencySource::Tag("v1.0.0".to_string()))];
        let path = segs(&["github", "com", "pacolang", "numerics", "blas"]);
        let (matched, remainder) = longest_prefix_match(&deps, &path).unwrap();
        assert_eq!(matched.0, "github.com/pacolang/numerics");
        assert_eq!(remainder, segs(&["blas"]));
    }

    #[test]
    fn longest_prefix_match_finds_nothing_for_an_unrelated_path() {
        let deps = vec![("example.com/team/json".to_string(), DependencySource::Tag("v1.2.0".to_string()))];
        let path = segs(&["other", "com", "thing"]);
        assert!(longest_prefix_match(&deps, &path).is_none());
    }

    #[test]
    fn is_used_matches_a_dependency_used_exactly_or_as_a_prefix() {
        let used = vec![segs(&["example", "com", "team", "json"])];
        assert!(is_used("example.com/team/json", &used));
        assert!(!is_used("example.com/team/other", &used));
    }
}
