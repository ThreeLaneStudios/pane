use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaneRoot {
    Pane,
    Projects,
    Internal,
    System,
    Attachments,
    /// #3050 PR 9, `docs/spec/runtime.md` §36.9.2 — the Home surface's own
    /// top-level tree, parallel to `Projects`. Created lazily on first
    /// touch via the same `create_dir_all` every `PaneRoot` variant already
    /// gets (see `PaneFs::create_dir_all`) — no dedicated "initialize Home"
    /// step. `Home` is a reserved name, never a user-nameable project slot,
    /// so there is no #1824-style "project literally named Home" ambiguity
    /// for the watcher's skip-check to resolve (`watcher.rs::
    /// is_home_assets_subdir`).
    Home,
}

impl PaneRoot {
    fn relative_path(self) -> &'static Path {
        match self {
            Self::Pane => Path::new(""),
            Self::Projects => Path::new("Projects"),
            Self::Internal => Path::new(".pane"),
            Self::System => Path::new(".pane/system"),
            Self::Attachments => Path::new(".pane/attachments"),
            Self::Home => Path::new("Home"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExistingKind {
    Any,
    File,
    Directory,
}

#[derive(Debug, Clone)]
pub(crate) struct AuthorizedPath {
    path: PathBuf,
}

impl AuthorizedPath {
    pub(crate) fn as_path(&self) -> &Path {
        &self.path
    }
}

/// #2874 item 1: distinguishes "the target simply doesn't exist" from every
/// other authorization/I-O failure (permissions, symlink rejection, policy
/// violation, etc). `purge_note_internal`'s "a trashed note whose file is
/// already gone purges cleanly" ruling needs to tolerate exactly the former
/// and keep surfacing the latter as a real error — this is the structured
/// signal that makes that distinction possible without string-matching a
/// formatted error message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FsPolicyErrorKind {
    NotFound,
    Other,
}

#[derive(Debug)]
pub(crate) struct FsPolicyError {
    message: String,
    kind: FsPolicyErrorKind,
}

impl FsPolicyError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: FsPolicyErrorKind::Other,
        }
    }

    /// #2874 item 1: constructed only where the underlying `io::Error`'s
    /// kind is confirmed `NotFound` (see `authorize_existing` below) — never
    /// a blanket "any failure looks like missing" guess, so a real
    /// permission/I-O error is never misreported as "already gone."
    fn not_found(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: FsPolicyErrorKind::NotFound,
        }
    }

    pub(crate) fn is_not_found(&self) -> bool {
        self.kind == FsPolicyErrorKind::NotFound
    }
}

impl fmt::Display for FsPolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for FsPolicyError {}

#[derive(Debug, Clone)]
pub(crate) struct PaneFs {
    pane_root: PathBuf,
}

impl PaneFs {
    pub(crate) fn for_runtime() -> Result<Self, FsPolicyError> {
        Self::from_pane_root(crate::database::get_pane_root())
    }

    pub(crate) fn from_pane_root(pane_root: impl AsRef<Path>) -> Result<Self, FsPolicyError> {
        let pane_root = pane_root.as_ref();
        fs::create_dir_all(pane_root).map_err(|error| {
            FsPolicyError::new(format!(
                "failed to create Pane root {}: {error}",
                pane_root.display()
            ))
        })?;
        let pane_root = fs::canonicalize(pane_root).map_err(|error| {
            FsPolicyError::new(format!(
                "failed to canonicalize Pane root {}: {error}",
                pane_root.display()
            ))
        })?;
        Ok(Self { pane_root })
    }

    pub(crate) fn validate_name(name: &str) -> Result<(), FsPolicyError> {
        if name.is_empty() {
            return Err(FsPolicyError::new("filesystem name must not be empty"));
        }
        if name.contains(['/', '\\', '\0']) {
            return Err(FsPolicyError::new(format!(
                "filesystem name contains a path separator: {name:?}"
            )));
        }

        let mut components = Path::new(name).components();
        match (components.next(), components.next()) {
            (Some(Component::Normal(_)), None) => Ok(()),
            _ => Err(FsPolicyError::new(format!(
                "filesystem name must be one normal path component: {name:?}"
            ))),
        }
    }

    pub(crate) fn relative_from_names<'a>(
        names: impl IntoIterator<Item = &'a str>,
    ) -> Result<PathBuf, FsPolicyError> {
        let mut relative = PathBuf::new();
        for name in names {
            Self::validate_name(name)?;
            relative.push(name);
        }
        if relative.as_os_str().is_empty() {
            return Err(FsPolicyError::new("relative path must not be empty"));
        }
        Ok(relative)
    }

    pub(crate) fn validate_relative(relative: &Path) -> Result<(), FsPolicyError> {
        if relative.as_os_str().is_empty() || relative.is_absolute() {
            return Err(FsPolicyError::new(format!(
                "path must be a non-empty relative path: {}",
                relative.display()
            )));
        }

        for component in relative.components() {
            match component {
                Component::Normal(value) => {
                    let value = value.to_string_lossy();
                    if value.contains(['\\', '\0']) {
                        return Err(FsPolicyError::new(format!(
                            "path contains a non-portable separator: {}",
                            relative.display()
                        )));
                    }
                }
                _ => {
                    return Err(FsPolicyError::new(format!(
                        "path contains a forbidden component: {}",
                        relative.display()
                    )));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn create_dir_all(
        &self,
        root: PaneRoot,
        relative: &Path,
    ) -> Result<AuthorizedPath, FsPolicyError> {
        let authorized = self.authorize_destination(root, relative, true)?;
        fs::create_dir_all(authorized.as_path()).map_err(|error| {
            FsPolicyError::new(format!(
                "failed to create authorized directory {}: {error}",
                authorized.as_path().display()
            ))
        })?;
        self.authorize_existing(root, relative, ExistingKind::Directory)
    }

    pub(crate) fn authorize_write(
        &self,
        root: PaneRoot,
        relative: &Path,
    ) -> Result<AuthorizedPath, FsPolicyError> {
        self.authorize_destination(root, relative, false)
    }

    pub(crate) fn authorize_existing(
        &self,
        root: PaneRoot,
        relative: &Path,
        expected: ExistingKind,
    ) -> Result<AuthorizedPath, FsPolicyError> {
        Self::validate_relative(relative)?;
        let root_path = self.ensure_root(root)?;
        let candidate = self.walk_beneath_root(&root_path, relative, false)?;
        let metadata = fs::symlink_metadata(&candidate).map_err(|error| {
            // #2874 item 1: only a confirmed `NotFound` io::Error kind is
            // reported as "not found" — a permission error or any other I/O
            // failure on the same symlink_metadata() call keeps the generic
            // `Other` kind, so callers that tolerate "already gone" (purge)
            // never silently swallow a real failure mode instead.
            if error.kind() == std::io::ErrorKind::NotFound {
                FsPolicyError::not_found(format!(
                    "authorized path does not exist {}: {error}",
                    candidate.display()
                ))
            } else {
                FsPolicyError::new(format!(
                    "authorized path does not exist {}: {error}",
                    candidate.display()
                ))
            }
        })?;
        if metadata.file_type().is_symlink() {
            return Err(FsPolicyError::new(format!(
                "symlink rejected: {}",
                candidate.display()
            )));
        }
        match expected {
            ExistingKind::Any => {}
            ExistingKind::File if !metadata.is_file() => {
                return Err(FsPolicyError::new(format!(
                    "expected a regular file: {}",
                    candidate.display()
                )));
            }
            ExistingKind::Directory if !metadata.is_dir() => {
                return Err(FsPolicyError::new(format!(
                    "expected a directory: {}",
                    candidate.display()
                )));
            }
            _ => {}
        }
        let canonical = fs::canonicalize(&candidate).map_err(|error| {
            FsPolicyError::new(format!(
                "failed to canonicalize authorized path {}: {error}",
                candidate.display()
            ))
        })?;
        if canonical == root_path || !canonical.starts_with(&root_path) {
            return Err(FsPolicyError::new(format!(
                "path is not strictly below authorized root {}: {}",
                root_path.display(),
                canonical.display()
            )));
        }
        Ok(AuthorizedPath { path: canonical })
    }

    pub(crate) fn rename(
        &self,
        root: PaneRoot,
        source: &Path,
        destination: &Path,
    ) -> Result<(), FsPolicyError> {
        let source = self.authorize_existing(root, source, ExistingKind::Any)?;
        let destination = self.authorize_destination(root, destination, false)?;
        if destination.as_path().exists() {
            return Err(FsPolicyError::new(format!(
                "rename destination already exists: {}",
                destination.as_path().display()
            )));
        }
        fs::rename(source.as_path(), destination.as_path()).map_err(|error| {
            FsPolicyError::new(format!(
                "failed to rename {} to {}: {error}",
                source.as_path().display(),
                destination.as_path().display()
            ))
        })
    }

    pub(crate) fn trash(
        &self,
        root: PaneRoot,
        relative: &Path,
        expected: ExistingKind,
    ) -> Result<(), FsPolicyError> {
        let target = self.authorize_existing(root, relative, expected)?;
        trash::delete(target.as_path()).map_err(|error| {
            FsPolicyError::new(format!(
                "failed to move authorized path to trash {}: {error}",
                target.as_path().display()
            ))
        })
    }

    // #3029: its sole caller, the pure-filesystem dedup-cleanup function, was removed
    // (Matt's ruling 4 — that function had no DB correlation and could delete
    // whichever file the separate DB-level dedup had just decided to keep).
    // Kept as a general-purpose authorized-delete primitive on this policy
    // wrapper rather than deleted along with its one caller.
    #[allow(dead_code)]
    pub(crate) fn remove_file(&self, root: PaneRoot, relative: &Path) -> Result<(), FsPolicyError> {
        let target = self.authorize_existing(root, relative, ExistingKind::File)?;
        fs::remove_file(target.as_path()).map_err(|error| {
            FsPolicyError::new(format!(
                "failed to delete authorized file {}: {error}",
                target.as_path().display()
            ))
        })
    }

    pub(crate) fn remove_dir_all(
        &self,
        root: PaneRoot,
        relative: &Path,
    ) -> Result<(), FsPolicyError> {
        let target = self.authorize_existing(root, relative, ExistingKind::Directory)?;
        fs::remove_dir_all(target.as_path()).map_err(|error| {
            FsPolicyError::new(format!(
                "failed to recursively delete authorized directory {}: {error}",
                target.as_path().display()
            ))
        })
    }

    /// #2874 item 65 (folder-delete trashes leftovers, Reed's WARN closure):
    /// non-recursive directory removal — `std::fs::remove_dir`, NOT
    /// `remove_dir_all`. This only ever succeeds on an already-EMPTY
    /// directory (`fs::remove_dir` itself refuses on `ENOTEMPTY`/similar if
    /// anything is still inside); unlike `remove_dir_all` it can never take
    /// an untracked file down with it. Callers that need "delete this
    /// directory and whatever a user may have dropped into it via Finder"
    /// must empty it first (e.g. via repeated `trash()` calls on each
    /// leftover entry) and only call this once it is actually empty.
    pub(crate) fn remove_dir(&self, root: PaneRoot, relative: &Path) -> Result<(), FsPolicyError> {
        let target = self.authorize_existing(root, relative, ExistingKind::Directory)?;
        fs::remove_dir(target.as_path()).map_err(|error| {
            FsPolicyError::new(format!(
                "failed to remove authorized directory {}: {error}",
                target.as_path().display()
            ))
        })
    }

    fn authorize_destination(
        &self,
        root: PaneRoot,
        relative: &Path,
        directory: bool,
    ) -> Result<AuthorizedPath, FsPolicyError> {
        Self::validate_relative(relative)?;
        let root_path = self.ensure_root(root)?;
        let candidate = self.walk_beneath_root(&root_path, relative, true)?;

        if let Ok(metadata) = fs::symlink_metadata(&candidate) {
            if metadata.file_type().is_symlink() {
                return Err(FsPolicyError::new(format!(
                    "symlink destination rejected: {}",
                    candidate.display()
                )));
            }
            if directory && !metadata.is_dir() {
                return Err(FsPolicyError::new(format!(
                    "directory destination is not a directory: {}",
                    candidate.display()
                )));
            }
            if !directory && !metadata.is_file() {
                return Err(FsPolicyError::new(format!(
                    "file destination is not a regular file: {}",
                    candidate.display()
                )));
            }
        }

        Ok(AuthorizedPath { path: candidate })
    }

    fn ensure_root(&self, root: PaneRoot) -> Result<PathBuf, FsPolicyError> {
        if root == PaneRoot::Pane {
            return Ok(self.pane_root.clone());
        }

        let mut current = self.pane_root.clone();
        for component in root.relative_path().components() {
            let Component::Normal(component) = component else {
                return Err(FsPolicyError::new("invalid configured Pane root"));
            };
            current.push(component);
            match fs::symlink_metadata(&current) {
                Ok(metadata) => {
                    if metadata.file_type().is_symlink() {
                        return Err(FsPolicyError::new(format!(
                            "configured Pane root contains a symlink: {}",
                            current.display()
                        )));
                    }
                    if !metadata.is_dir() {
                        return Err(FsPolicyError::new(format!(
                            "configured Pane root is not a directory: {}",
                            current.display()
                        )));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    fs::create_dir(&current).map_err(|create_error| {
                        FsPolicyError::new(format!(
                            "failed to create configured Pane root {}: {create_error}",
                            current.display()
                        ))
                    })?;
                }
                Err(error) => {
                    return Err(FsPolicyError::new(format!(
                        "failed to inspect configured Pane root {}: {error}",
                        current.display()
                    )));
                }
            }
        }

        let canonical = fs::canonicalize(&current).map_err(|error| {
            FsPolicyError::new(format!(
                "failed to canonicalize configured Pane root {}: {error}",
                current.display()
            ))
        })?;
        if !canonical.starts_with(&self.pane_root) {
            return Err(FsPolicyError::new(format!(
                "configured root escapes Pane root: {}",
                canonical.display()
            )));
        }
        Ok(canonical)
    }

    fn walk_beneath_root(
        &self,
        root: &Path,
        relative: &Path,
        allow_missing: bool,
    ) -> Result<PathBuf, FsPolicyError> {
        let mut current = root.to_path_buf();
        let mut missing = false;
        let component_count = relative.components().count();

        for (index, component) in relative.components().enumerate() {
            let Component::Normal(component) = component else {
                return Err(FsPolicyError::new("forbidden path component"));
            };
            current.push(component);
            if missing {
                continue;
            }

            match fs::symlink_metadata(&current) {
                Ok(metadata) => {
                    if metadata.file_type().is_symlink() {
                        return Err(FsPolicyError::new(format!(
                            "symlink path component rejected: {}",
                            current.display()
                        )));
                    }
                    if index + 1 < component_count && !metadata.is_dir() {
                        return Err(FsPolicyError::new(format!(
                            "non-directory path component rejected: {}",
                            current.display()
                        )));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound && allow_missing => {
                    missing = true;
                }
                // #2874 item 1: this is the actual site that fires for the
                // common "file (or one of its ancestor directories) simply
                // doesn't exist" case when `allow_missing` is false (every
                // `authorize_existing` call, including the one
                // `delete_note_file_tolerating_missing` depends on) —
                // `authorize_existing`'s own follow-up `symlink_metadata`
                // check on the final candidate is normally never reached
                // because this loop already walks (and fails on) that same
                // final component first. Tag it `not_found()` on a
                // confirmed `NotFound` io::Error kind, same rule as
                // `authorize_existing`'s own check — any other I/O error
                // (permissions, etc.) keeps the generic `Other` kind.
                Err(error) => {
                    if error.kind() == std::io::ErrorKind::NotFound {
                        return Err(FsPolicyError::not_found(format!(
                            "failed to inspect authorized path {}: {error}",
                            current.display()
                        )));
                    }
                    return Err(FsPolicyError::new(format!(
                        "failed to inspect authorized path {}: {error}",
                        current.display()
                    )));
                }
            }
        }

        if !current.starts_with(root) {
            return Err(FsPolicyError::new(format!(
                "path escapes authorized root {}: {}",
                root.display(),
                current.display()
            )));
        }
        Ok(current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn policy() -> (TempDir, PaneFs) {
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        let policy = PaneFs::from_pane_root(&pane_root).unwrap();
        (temp, policy)
    }

    #[test]
    fn rejects_unsafe_names_and_paths() {
        for name in ["", ".", "..", "a/b", "a\\b", "/tmp"] {
            assert!(PaneFs::validate_name(name).is_err(), "accepted {name:?}");
        }
        for path in ["../x", "a/../../x", "/tmp/x", "./x", "a\\..\\x"] {
            assert!(
                PaneFs::validate_relative(Path::new(path)).is_err(),
                "accepted {path:?}"
            );
        }
    }

    #[test]
    fn creates_and_authorizes_paths_below_explicit_root() {
        let (_temp, policy) = policy();
        let directory = policy
            .create_dir_all(PaneRoot::Projects, Path::new("Project/Folder"))
            .unwrap();
        assert!(directory.as_path().is_dir());

        let target = policy
            .authorize_write(PaneRoot::Projects, Path::new("Project/Folder/note.md"))
            .unwrap();
        fs::write(target.as_path(), "safe").unwrap();
        let existing = policy
            .authorize_existing(
                PaneRoot::Projects,
                Path::new("Project/Folder/note.md"),
                ExistingKind::File,
            )
            .unwrap();
        assert_eq!(fs::read_to_string(existing.as_path()).unwrap(), "safe");
    }

    #[test]
    fn refuses_authorized_root_mutation() {
        let (_temp, policy) = policy();
        assert!(policy
            .remove_dir_all(PaneRoot::Projects, Path::new("."))
            .is_err());
        assert!(policy
            .remove_dir_all(PaneRoot::Pane, Path::new("Projects/.."))
            .is_err());
    }

    /// #2874 item 65: `remove_dir` must refuse (not silently succeed) on a
    /// non-empty directory, and must succeed on a genuinely empty one. The
    /// refusal is the entire point — it is structurally impossible for this
    /// call to take an untracked leftover file down with it, unlike
    /// `remove_dir_all`.
    #[test]
    fn remove_dir_refuses_non_empty_and_succeeds_on_empty() {
        let (_temp, policy) = policy();
        let directory = policy
            .create_dir_all(PaneRoot::Projects, Path::new("Project/Folder"))
            .unwrap();
        fs::write(directory.as_path().join("leftover.md"), "content").unwrap();

        let result = policy.remove_dir(PaneRoot::Projects, Path::new("Project/Folder"));
        assert!(
            result.is_err(),
            "remove_dir must refuse to remove a non-empty directory, got: {:?}",
            result
        );
        assert!(
            directory.as_path().join("leftover.md").exists(),
            "the leftover file must still be there after the refusal"
        );

        std::fs::remove_file(directory.as_path().join("leftover.md")).unwrap();
        policy
            .remove_dir(PaneRoot::Projects, Path::new("Project/Folder"))
            .expect("remove_dir must succeed once the directory is actually empty");
        assert!(!directory.as_path().exists());
    }

    #[test]
    fn rename_rejects_existing_destination() {
        let (_temp, policy) = policy();
        policy
            .create_dir_all(PaneRoot::Projects, Path::new("Project"))
            .unwrap();
        fs::write(
            policy
                .authorize_write(PaneRoot::Projects, Path::new("Project/a.md"))
                .unwrap()
                .as_path(),
            "a",
        )
        .unwrap();
        fs::write(
            policy
                .authorize_write(PaneRoot::Projects, Path::new("Project/b.md"))
                .unwrap()
                .as_path(),
            "b",
        )
        .unwrap();

        assert!(policy
            .rename(
                PaneRoot::Projects,
                Path::new("Project/a.md"),
                Path::new("Project/b.md"),
            )
            .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_components_and_targets() {
        use std::os::unix::fs::symlink;

        let (temp, policy) = policy();
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let projects = temp.path().join("Pane/Projects");
        fs::create_dir_all(&projects).unwrap();
        symlink(&outside, projects.join("escape")).unwrap();

        assert!(policy
            .authorize_write(PaneRoot::Projects, Path::new("escape/secret.md"))
            .is_err());
        assert!(policy
            .remove_dir_all(PaneRoot::Projects, Path::new("escape"))
            .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_derived_root() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let pane = temp.path().join("Pane");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&pane).unwrap();
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, pane.join("Projects")).unwrap();
        let policy = PaneFs::from_pane_root(&pane).unwrap();

        assert!(policy
            .authorize_write(PaneRoot::Projects, Path::new("note.md"))
            .is_err());
    }
}
