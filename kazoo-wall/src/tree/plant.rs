//! Planting the wall's tree where there is none, so nobody has to: a
//! starting wall does it, and so does the tree's view opened before any wall
//! has played.

use std::fmt;
use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

use super::{TreeError, git, rings};

/// What planting did about the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Planting {
    /// A tree with a trunk was there already.
    Kept,
    /// The tree was grown from the change log.
    Grown,
    /// An empty tree was planted, to grow from here as the wall plays.
    Empty,
    /// A wall or an import held the state, so nothing was done; a tree that
    /// is there is left as it is.
    Left,
    /// There is no tree, for this reason.
    Failed(String),
}

/// Plant the wall's tree at `dir` when it has none.
///
/// It is grown from the change log in `state` as `kazoo wall tree import`
/// grows it, or planted empty when the log gives no wall to grow from (a new
/// wall, or one whose past cannot be worked out). A tree that is there with
/// a trunk is left as it is. What it does on the way (growing, and what
/// the log could not give) goes to `say`, a line at a time; a failure
/// leaves nothing half planted.
pub fn plant(state: &Path, dir: &Path, say: &mut dyn FnMut(&str)) -> Planting {
    match grow(state, dir, say) {
        Ok(planting) => planting,
        Err(why) => Planting::Failed(why),
    }
}

fn grow(state: &Path, dir: &Path, say: &mut dyn FnMut(&str)) -> Result<Planting, String> {
    let unreadable =
        |err: &dyn fmt::Display| format!("the tree at {} cannot be read ({err})", dir.display());
    if git::is_bare(dir).map_err(|err| unreadable(&err))?
        && rings::reference(dir, "refs/heads/trunk")
            .map_err(|err| unreadable(&err))?
            .is_some()
    {
        return Ok(Planting::Kept);
    }
    // What was there: a failure may take away only what it left.
    let before = match fs::read_dir(dir) {
        Ok(mut entries) => {
            if entries.next().is_none() {
                Before::Empty
            } else {
                Before::Occupied
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Before::Absent,
        Err(err) => return Err(unreadable(&err)),
    };
    if let Some(parent) = dir.parent() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|err| format!("{} cannot be made ({err})", parent.display()))?;
    }
    if !state.is_dir() {
        return plant_empty(dir);
    }
    say(&format!(
        "growing the tree at {} from the change log (once; the wall answers when it is grown)",
        dir.display()
    ));
    match rings::grow(state, dir, false) {
        Ok(rings) => {
            for line in rings.report.lines() {
                say(&format!("tree: {line}"));
            }
            Ok(Planting::Grown)
        }
        Err(rings::RingsError::EmptyLog) => plant_empty(dir),
        Err(rings::RingsError::WallRunning) => Ok(Planting::Left),
        Err(rings::RingsError::NoWall(why)) => {
            clear(dir, before)?;
            say(&format!(
                "the changes already in the log cannot go into the tree ({why})"
            ));
            plant_empty(dir)
        }
        Err(err) => Err(failed(dir, before, &err)),
    }
}

/// An empty tree at `dir`, unless one is planted there already.
fn plant_empty(dir: &Path) -> Result<Planting, String> {
    plant_empty_with(dir, git::plant)
}

/// [`plant_empty`], planting each new tree with `make`.
///
/// Planting is held to one planter at a time by a lock beside the tree
/// (`.<name>.planting.lock`), so a wall and the tree's view planting at once
/// take turns: the second finds the first's tree and takes it as planted.
/// Where nothing is, a failure takes away what it made. An empty folder
/// that is there (a mount point, perhaps) is planted in place, and emptied
/// again if that fails. A folder with something in it that is not a tree is
/// never touched.
fn plant_empty_with(
    dir: &Path,
    make: impl FnOnce(&Path) -> Result<(), TreeError>,
) -> Result<Planting, String> {
    let cannot =
        |err: &dyn fmt::Display| format!("the tree cannot be planted at {} ({err})", dir.display());
    let held = planting_lock(dir).map_err(|err| cannot(&err))?;
    let planted = plant_held(dir, make, &cannot);
    drop(held);
    planted
}

fn plant_held(
    dir: &Path,
    make: impl FnOnce(&Path) -> Result<(), TreeError>,
    cannot: &dyn Fn(&dyn fmt::Display) -> String,
) -> Result<Planting, String> {
    if git::is_bare(dir)
        .map_err(|err| format!("the tree at {} cannot be read ({err})", dir.display()))?
    {
        return Ok(Planting::Empty);
    }
    let before = match fs::read_dir(dir) {
        Ok(mut entries) => {
            if entries.next().is_none() {
                Before::Empty
            } else {
                Before::Occupied
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Before::Absent,
        Err(err) => return Err(cannot(&err)),
    };
    if before == Before::Occupied {
        return Err(format!(
            "{} is there and is not a tree, so it is left as it is",
            dir.display()
        ));
    }
    match make(dir) {
        Ok(()) => Ok(Planting::Empty),
        Err(err) => {
            let why = cannot(&err);
            Err(match clear(dir, before) {
                Ok(()) => why,
                Err(also) => format!("{why}; {also}"),
            })
        }
    }
}

/// The lock that keeps planting at `dir` to one planter at a time, held
/// until the file it returns is dropped. It waits for a planter that holds
/// it. It is a file beside the tree; where nothing can be made beside it
/// (an empty mount point, whose parent is not ours to write in), it is the
/// tree's folder itself, which every planter there then locks alike.
fn planting_lock(dir: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let name = dir.file_name().map_or_else(
        || "tree".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let path = dir.with_file_name(format!(".{name}.planting.lock"));
    let beside = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(&path);
    let (lock, what) = match beside {
        Ok(lock) => (lock, path),
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
            ) && dir.is_dir() =>
        {
            (fs::File::open(dir)?, dir.to_path_buf())
        }
        Err(err) => return Err(err),
    };
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive).map_err(|errno| {
        io::Error::other(format!("{} cannot be locked ({errno})", what.display()))
    })?;
    Ok(lock)
}

/// What looking at the tree did about it (see [`plant_to_look`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToLook {
    /// A tree was there already.
    There,
    /// The wall has logged nothing to grow a tree from, so an empty one was
    /// planted, to grow as the wall plays.
    Planted,
    /// The wall has played, and grows the tree from its log as it starts:
    /// nothing was done.
    ForTheWall,
    /// There is no tree, for this reason.
    Failed(String),
}

/// Plant the tree at `dir` for someone who wants to look at it before a
/// wall has.
///
/// It is planted empty when the wall in `state` has logged nothing to grow
/// it from, and otherwise left to the wall, which grows it from its log as
/// it starts. It never takes the wall's state lock, so a wall starting
/// meanwhile is never held up by it.
#[must_use]
pub fn plant_to_look(state: &Path, dir: &Path) -> ToLook {
    match look(state, dir) {
        Ok(looked) => looked,
        Err(why) => ToLook::Failed(why),
    }
}

fn look(state: &Path, dir: &Path) -> Result<ToLook, String> {
    if git::is_bare(dir)
        .map_err(|err| format!("the tree at {} cannot be read ({err})", dir.display()))?
    {
        return Ok(ToLook::There);
    }
    if has_played(state)? {
        return Ok(ToLook::ForTheWall);
    }
    if let Some(parent) = dir.parent() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|err| format!("{} cannot be made ({err})", parent.display()))?;
    }
    plant_empty(dir).map(|_| ToLook::Planted)
}

/// What was at the tree's place before it was grown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Before {
    /// Nothing: whatever is there now, a failure left.
    Absent,
    /// An empty directory (made by someone, perhaps a mount point): it
    /// stays, and only what is in it now goes.
    Empty,
    /// Something: it is never touched.
    Occupied,
}

/// Why growing failed, with what it left at `dir` taken away.
pub(crate) fn failed(dir: &Path, before: Before, err: &rings::RingsError) -> String {
    let why = format!("the tree could not be grown from the change log ({err})");
    match clear(dir, before) {
        Ok(()) => why,
        Err(also) => format!("{why}; {also}"),
    }
}

/// Take away what a failed grow left at `dir`, as `before` allows.
fn clear(dir: &Path, before: Before) -> Result<(), String> {
    let cannot = |err: io::Error| {
        format!(
            "what it left at {} cannot be taken away ({err})",
            dir.display()
        )
    };
    match before {
        Before::Occupied => Ok(()),
        Before::Absent => match fs::remove_dir_all(dir) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(cannot(err)),
        },
        Before::Empty => {
            for entry in fs::read_dir(dir).map_err(cannot)? {
                let path = entry.map_err(cannot)?.path();
                let removed = if path.is_dir() && !path.is_symlink() {
                    fs::remove_dir_all(&path)
                } else {
                    fs::remove_file(&path)
                };
                removed.map_err(cannot)?;
            }
            Ok(())
        }
    }
}

/// What a wall on its socket says about its tree (see [`wall_tree`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WallTree {
    /// No wall answers there.
    NoWall,
    /// A wall has the socket but does not answer yet: it is starting, and
    /// plants its tree as it does.
    Starting,
    /// A wall is playing with its tree.
    Planted,
    /// A wall is playing without a tree, and says why when it knows.
    Unplanted(Option<String>),
    /// A wall answers but could not be asked, for this reason.
    Unclear(String),
}

/// Ask the wall on `socket`, as a watcher (never listed, changing
/// nothing), whether it has its tree, waiting at most `timeout` for it.
#[must_use]
pub fn wall_tree(socket: &Path, timeout: std::time::Duration) -> WallTree {
    use crate::protocol::client::WallClient;
    let mut wall = match WallClient::watch_within(socket, "kazoo-tree", "kazoo-tree", timeout) {
        Ok(wall) => wall,
        Err(err) if err.is_timeout() => return WallTree::Starting,
        Err(err) => {
            return if crate::daemon::is_running(socket) {
                WallTree::Unclear(err.to_string())
            } else {
                WallTree::NoWall
            };
        }
    };
    match wall.look() {
        Ok(look) => match look.tree {
            Some(tree) if tree.planted => WallTree::Planted,
            Some(tree) => WallTree::Unplanted(tree.error),
            None => WallTree::Unplanted(None),
        },
        Err(err) if err.is_timeout() => WallTree::Starting,
        Err(err) => WallTree::Unclear(err.to_string()),
    }
}

/// Whether the wall in `state` has played: either of its logs holds
/// anything (the log rotates, so the new one may be empty and the past in
/// the one before it).
///
/// # Errors
///
/// When a log is there but cannot be read.
pub fn has_played(state: &Path) -> Result<bool, String> {
    for name in [crate::store::LOG_FILE, crate::store::OLD_LOG_FILE] {
        let log = state.join(name);
        let played = match fs::metadata(&log) {
            Ok(meta) => meta.len() > 0,
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                false
            }
            Err(err) => return Err(format!("{} cannot be read ({err})", log.display())),
        };
        if played {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests;
