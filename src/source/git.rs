//! Git source adapter powered by **gix** (gitoxide).
//!
//! No `git` CLI subprocess — repository discovery, tree/index/worktree
//! comparison, and unified-diff text are all produced in-process.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use gix::bstr::{BStr, BString, ByteSlice};
use gix::diff::blob::platform::prepare_diff::Operation;
use gix::diff::blob::unified_diff::{ConsumeBinaryHunk, ContextSize};
use gix::diff::blob::{ResourceKind, UnifiedDiff};
use gix::objs::{self, tree::EntryKind, Write as _};
use gix::status::{index_worktree::Item as IwItem, UntrackedFiles};
use gix::{ObjectId, Repository};

/// Discover the repository containing `start` and return its worktree root
/// (or the git dir for bare repos).
pub fn find_repo(start: &Path) -> Result<PathBuf> {
    let repo = open_repo(start)?;
    if let Some(wt) = repo.workdir() {
        Ok(wt.to_owned())
    } else {
        Ok(repo.git_dir().to_owned())
    }
}

/// Open a repository discovered from `start`.
pub fn open_repo(start: &Path) -> Result<Repository> {
    gix::discover(start)
        .with_context(|| format!("not a git repository (or any parent): {}", start.display()))
}

/// Diff two arbitrary files on disk, producing a unified-diff string.
///
/// Uses gix's diff engine (same as git diff) but does not require the files
/// to be tracked by git. The `repo` is used for its object store.
pub fn git_file_diff(repo: &Repository, old_path: &Path, new_path: &Path) -> Result<String> {
    let workdir = repo
        .workdir()
        .ok_or_else(|| anyhow!("bare repository has no worktree"))?;

    let old_abs = if old_path.is_absolute() {
        old_path.to_owned()
    } else {
        workdir.join(old_path)
    };
    let new_abs = if new_path.is_absolute() {
        new_path.to_owned()
    } else {
        workdir.join(new_path)
    };

    let old_content =
        std::fs::read(&old_abs).with_context(|| format!("read old file {}", old_abs.display()))?;
    let new_content =
        std::fs::read(&new_abs).with_context(|| format!("read new file {}", new_abs.display()))?;

    // Write blobs to the object store so the diff engine can look them up.
    let old_id = repo
        .write_buf(objs::Kind::Blob, &old_content)
        .map_err(|e| anyhow!("write old file blob: {e}"))?;
    let new_id = repo
        .write_buf(objs::Kind::Blob, &new_content)
        .map_err(|e| anyhow!("write new file blob: {e}"))?;

    let mut cache = repo
        .diff_resource_cache(
            gix::diff::blob::pipeline::Mode::ToGit,
            gix::diff::blob::pipeline::WorktreeRoots {
                old_root: None,
                new_root: Some(workdir.to_owned()),
            },
        )
        .context("diff resource cache (file diff)")?;

    // Use the file name for the diff header (cross-platform safe).
    // Strip workdir prefix for a clean relative path when possible.
    let old_label = old_abs
        .strip_prefix(workdir)
        .unwrap_or(&old_abs)
        .to_string_lossy()
        .replace('\\', "/");
    let new_label = new_abs
        .strip_prefix(workdir)
        .unwrap_or(&new_abs)
        .to_string_lossy()
        .replace('\\', "/");
    let old_rela = BString::from(old_label.as_str());
    let new_rela = BString::from(new_label.as_str());

    let mut out = String::new();
    cache
        .set_resource(
            old_id,
            EntryKind::Blob,
            old_rela.as_ref(),
            ResourceKind::OldOrSource,
            &repo.objects,
        )
        .context("set old resource")?;
    cache
        .set_resource(
            new_id,
            EntryKind::Blob,
            new_rela.as_ref(),
            ResourceKind::NewOrDestination,
            &repo.objects,
        )
        .context("set new resource")?;

    let old_display = path_display(old_rela.as_ref());
    let new_display = path_display(new_rela.as_ref());
    render_file_patch(&mut out, Some(&old_display), Some(&new_display), &mut cache)?;

    Ok(out)
}

/// Working-tree or staged diff as a unified-diff string.
///
/// * `staged = false` → index vs worktree (`git diff`)
/// * `staged = true`  → HEAD tree vs index (`git diff --cached`)
///
/// `pathspecs` filters by path prefix (best-effort); empty means all.
/// `include_untracked` includes untracked files in the worktree diff.
pub fn git_diff(
    repo_path: &Path,
    staged: bool,
    pathspecs: &[String],
    include_untracked: bool,
) -> Result<String> {
    let repo = open_repo(repo_path)?;
    if staged {
        diff_staged(&repo, pathspecs)
    } else {
        diff_worktree(&repo, pathspecs, include_untracked)
    }
}

/// Diff against a target revision or range, git-style:
///
/// * range `A..B` / `A...B` → tree-to-tree diff (like `git diff A..B`)
/// * single rev             → that tree vs the worktree (like
///   `git diff <rev>`); with `staged`, that tree vs the index (like
///   `git diff --cached <rev>`)
///
/// `pathspecs` filters by path prefix (best-effort); empty means all.
/// Untracked files are not included (matching `git diff <rev>`).
pub fn git_diff_target(
    repo_path: &Path,
    target: &str,
    pathspecs: &[String],
    staged: bool,
) -> Result<String> {
    let repo = open_repo(repo_path)?;
    if let Some((a, b, merge_base)) = parse_range(target) {
        return diff_range(&repo, target, a, b, merge_base);
    }
    let tree = target_tree(&repo, target)?;
    if staged {
        diff_tree_index(&repo, tree, pathspecs)
    } else {
        diff_tree_worktree(&repo, tree, pathspecs)
    }
}

/// Cheap "does this resolve as a revision/range" probe. Used by the CLI to
/// disambiguate `diff <target>` between a rev and a pathspec-on-disk.
pub fn rev_resolves(repo_path: &Path, spec: &str) -> bool {
    let Ok(repo) = open_repo(repo_path) else {
        return false;
    };
    if let Some((a, b, _)) = parse_range(spec) {
        return peel_to_oid(&repo, a).is_ok() && peel_to_oid(&repo, b).is_ok();
    }
    peel_to_oid(&repo, spec).is_ok()
}

/// Diff a single revision (commit → parent) or a range `A..B` / `A...B`.
pub fn git_show(repo_path: &Path, rev: &str) -> Result<String> {
    let repo = open_repo(repo_path)?;
    if let Some((a, b, merge_base)) = parse_range(rev) {
        return diff_range(&repo, rev, a, b, merge_base);
    }

    let id = peel_to_oid(&repo, rev)?;
    let obj = repo.find_object(id).context("load revision object")?;
    let commit = obj
        .try_into_commit()
        .map_err(|_| anyhow!("revision `{rev}` does not peel to a commit"))?;
    let new_tree = commit.tree_id()?.detach();
    let old_tree = match commit.parent_ids().next() {
        Some(parent) => {
            let p = parent
                .object()?
                .try_into_commit()
                .map_err(|_| anyhow!("parent is not a commit"))?;
            Some(p.tree_id()?.detach())
        }
        None => None,
    };
    diff_tree_oids(&repo, old_tree, new_tree)
}

/// Tree-to-tree diff for `A..B` (or merge-base-to-B for `A...B`).
fn diff_range(repo: &Repository, rev: &str, a: &str, b: &str, merge_base: bool) -> Result<String> {
    let old = if merge_base {
        let left = peel_to_oid(repo, a)?;
        let right = peel_to_oid(repo, b)?;
        repo.merge_base(left, right)
            .with_context(|| format!("no merge base for {rev}"))?
            .detach()
    } else {
        peel_to_oid(repo, a)?
    };
    let new = peel_to_oid(repo, b)?;
    diff_tree_oids(repo, Some(old), new)
}

// ─── staged / worktree ───────────────────────────────────────────────────────

fn diff_staged(repo: &Repository, pathspecs: &[String]) -> Result<String> {
    let head_tree = repo
        .head_tree_id_or_empty()
        .context("resolve HEAD^{tree}")?
        .detach();
    diff_tree_index(repo, head_tree, pathspecs)
}

/// Tree-vs-index diff — the engine behind `git diff --cached <tree>`.
fn diff_tree_index(repo: &Repository, tree: ObjectId, pathspecs: &[String]) -> Result<String> {
    let index = repo
        .index_or_load_from_head_or_empty()
        .context("open index")?;

    let mut out = String::new();
    let mut resource_cache = repo
        .diff_resource_cache_for_tree_diff()
        .context("diff resource cache")?;

    // IndexPersistedOrInMemory → File → State via Deref.
    repo.tree_index_status(
        &tree,
        &index,
        None,
        gix::status::tree_index::TrackRenames::Disabled,
        |change, _tree_index, _worktree_index| -> Result<gix::diff::index::Action, anyhow::Error> {
            if !pathspec_match(change.location().as_bstr(), pathspecs) {
                return Ok(std::ops::ControlFlow::Continue(()));
            }
            if let Err(e) = append_index_change(repo, &mut resource_cache, &mut out, &change) {
                eprintln!("warning: skip staged change {}: {e:#}", change.location());
            }
            resource_cache.clear_resource_cache_keep_allocation();
            Ok(std::ops::ControlFlow::Continue(()))
        },
    )
    .context("tree-index status (staged)")?;

    Ok(out)
}

/// Tree-vs-worktree diff — the engine behind `git diff <commit>`.
///
/// Considers every blob path in the tree plus every tracked path from the
/// index (files added since the target commit are tracked too), comparing
/// the tree blob against the file on disk. Paths unchanged on both sides are
/// skipped; deleted-on-disk paths render as deletions.
fn diff_tree_worktree(repo: &Repository, tree: ObjectId, pathspecs: &[String]) -> Result<String> {
    use std::collections::{BTreeMap, BTreeSet};

    let workdir = repo
        .workdir()
        .ok_or_else(|| anyhow!("bare repository has no worktree to diff"))?
        .to_owned();

    let mut out = String::new();
    let mut resource_cache = repo
        .diff_resource_cache(
            gix::diff::blob::pipeline::Mode::ToGit,
            gix::diff::blob::pipeline::WorktreeRoots {
                old_root: None,
                new_root: Some(workdir.clone()),
            },
        )
        .context("diff resource cache (target)")?;

    // Old side: blob entries of the target tree, keyed by path.
    let tree_side: BTreeMap<BString, (ObjectId, EntryKind)> = {
        let mut m = BTreeMap::new();
        collect_tree_blobs(repo, tree, BString::default(), &mut m)?;
        m
    };
    // Tracked-on-the-new-side state from the index (id/kind/stat, stage 0).
    let index_side: BTreeMap<BString, (ObjectId, EntryKind, gix::index::entry::Stat)> = {
        let index = repo
            .index_or_load_from_head_or_empty()
            .context("open index")?;
        let mut m = BTreeMap::new();
        for entry in index.entries() {
            if entry.stage() != gix::index::entry::Stage::Unconflicted {
                continue; // skip unmerged (conflicted) entries
            }
            let kind = match entry.mode.to_tree_entry_mode() {
                Some(mode) => mode.kind(),
                None => continue,
            };
            if matches!(kind, EntryKind::Tree | EntryKind::Commit) {
                continue; // directories and gitlinks (submodules)
            }
            m.insert(
                entry.path(&index).to_owned(),
                (ObjectId::from(entry.id.as_ref()), kind, entry.stat),
            );
        }
        m
    };
    // Candidate paths: tree ∪ index (BTreeSet keeps deterministic order).
    let paths: BTreeSet<&BString> = tree_side.keys().chain(index_side.keys()).collect();

    let null = repo.object_hash().null();
    for path in paths {
        if !pathspec_match(path.as_bstr(), pathspecs) {
            continue;
        }
        let display = path_display(path.as_bstr());
        let disk_path = workdir.join(display.as_str());
        let disk_meta = std::fs::symlink_metadata(&disk_path).ok();

        // New side: when the index entry's stat matches the file on disk the
        // content is the git-normalized index blob already — use it instead
        // of re-hashing (and re-normalizing) the raw bytes from disk.
        let new: Option<(ObjectId, EntryKind)> = match (&disk_meta, index_side.get(path)) {
            (None, _) => None, // not on disk → deletion
            (Some(meta), Some((idx_id, idx_kind, idx_stat))) if stat_matches(idx_stat, meta) => {
                Some((*idx_id, *idx_kind))
            }
            (Some(meta), _) => Some(hash_disk(repo, &disk_path, meta)?),
        };
        let old = tree_side.get(path).copied();

        match (old, new) {
            (Some((old_id, old_kind)), Some((new_id, new_kind))) => {
                if old_id == new_id {
                    continue; // unchanged since the target
                }
                set_pair(
                    &mut resource_cache,
                    repo,
                    old_id,
                    old_kind,
                    new_id,
                    new_kind,
                    path.as_bstr(),
                    path.as_bstr(),
                )?;
                render_file_patch(
                    &mut out,
                    Some(&display),
                    Some(&display),
                    &mut resource_cache,
                )?;
            }
            (Some((old_id, old_kind)), None) => {
                set_pair(
                    &mut resource_cache,
                    repo,
                    old_id,
                    old_kind,
                    null,
                    old_kind,
                    path.as_bstr(),
                    path.as_bstr(),
                )?;
                render_file_patch(&mut out, Some(&display), None, &mut resource_cache)?;
            }
            (None, Some((new_id, new_kind))) => {
                set_pair(
                    &mut resource_cache,
                    repo,
                    null,
                    new_kind,
                    new_id,
                    new_kind,
                    path.as_bstr(),
                    path.as_bstr(),
                )?;
                render_file_patch(&mut out, None, Some(&display), &mut resource_cache)?;
            }
            (None, None) => continue, // in neither place — nothing to show
        }
        resource_cache.clear_resource_cache_keep_allocation();
    }

    Ok(out)
}

/// Recursively collect the blob entries of `tree` into `out` (path →
/// id/kind). Skips submodule (gitlink) entries; recurses into subtrees.
fn collect_tree_blobs(
    repo: &Repository,
    tree: ObjectId,
    prefix: BString,
    out: &mut std::collections::BTreeMap<BString, (ObjectId, EntryKind)>,
) -> Result<()> {
    if tree.is_empty_tree() {
        return Ok(());
    }
    let tree_obj = repo
        .find_object(tree)
        .context("load tree")?
        .try_into_tree()
        .with_context(|| format!("tree {tree} is not a tree"))?;
    for entry in tree_obj.iter() {
        let entry = entry.context("tree entry")?;
        let mut path = prefix.clone();
        path.extend_from_slice(entry.filename());
        match entry.mode().kind() {
            EntryKind::Tree => {
                path.push(b'/');
                collect_tree_blobs(repo, ObjectId::from(entry.oid()), path, out)?;
            }
            EntryKind::Blob | EntryKind::BlobExecutable | EntryKind::Link => {
                out.insert(path, (ObjectId::from(entry.oid()), entry.mode().kind()));
            }
            // Submodules (gitlinks) and anything exotic: skip.
            _ => {}
        }
    }
    Ok(())
}

/// Resolve a rev (commit, annotated tag, or tree) to the tree it points at.
fn target_tree(repo: &Repository, rev: &str) -> Result<ObjectId> {
    let id = peel_to_oid(repo, rev)?;
    let obj = repo.find_object(id).context("load revision object")?;
    match obj.kind {
        gix::objs::Kind::Tree => Ok(id),
        _ => Ok(obj
            .peel_to_tree()
            .with_context(|| format!("revision `{rev}` does not point at a tree"))?
            .id),
    }
}

/// Whether the index entry's recorded stat still matches the file on disk
/// (best-effort "unchanged since indexing" check, like git's racily-clean
/// logic but simpler: size and mtime seconds).
fn stat_matches(stat: &gix::index::entry::Stat, meta: &std::fs::Metadata) -> bool {
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    u64::from(stat.size) == meta.len() && u64::from(stat.mtime.secs) == mtime
}

/// Hash the file on disk as a blob (symlinks hash their target path, matching
/// git's blob encoding) and derive its entry kind from the metadata.
fn hash_disk(
    repo: &Repository,
    disk_path: &Path,
    meta: &std::fs::Metadata,
) -> Result<(ObjectId, EntryKind)> {
    let ft = meta.file_type();
    let kind = if ft.is_symlink() {
        EntryKind::Link
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o111 != 0 {
                EntryKind::BlobExecutable
            } else {
                EntryKind::Blob
            }
        }
        #[cfg(not(unix))]
        {
            EntryKind::Blob
        }
    };
    // Symlink blobs store the link target bytes (no trailing newline).
    let content = if ft.is_symlink() {
        let target = std::fs::read_link(disk_path)
            .with_context(|| format!("read symlink {}", disk_path.display()))?;
        BString::from(target.as_os_str().as_encoded_bytes())
    } else {
        BString::from(
            std::fs::read(disk_path).with_context(|| format!("read {}", disk_path.display()))?,
        )
    };
    let id = gix::objs::compute_hash(repo.object_hash(), gix::objs::Kind::Blob, &content)
        .with_context(|| format!("hash {}", disk_path.display()))?;
    Ok((id, kind))
}

fn diff_worktree(
    repo: &Repository,
    pathspecs: &[String],
    include_untracked: bool,
) -> Result<String> {
    let workdir = repo
        .workdir()
        .ok_or_else(|| anyhow!("bare repository has no worktree to diff"))?
        .to_owned();

    let mut out = String::new();
    let mut resource_cache = repo
        .diff_resource_cache(
            gix::diff::blob::pipeline::Mode::ToGit,
            gix::diff::blob::pipeline::WorktreeRoots {
                old_root: None,
                new_root: Some(workdir),
            },
        )
        .context("diff resource cache (worktree)")?;

    let untracked = if include_untracked {
        UntrackedFiles::Files
    } else {
        UntrackedFiles::None
    };

    let platform = repo
        .status(gix::progress::Discard)
        .context("status platform")?
        .untracked_files(untracked)
        .index_worktree_rewrites(None);

    let iter = platform
        .into_index_worktree_iter(std::iter::empty::<BString>())
        .context("index-worktree iterator")?;

    for item in iter {
        let item = item.context("index-worktree item")?;
        if let Err(e) = append_worktree_item(repo, &mut resource_cache, &mut out, &item, pathspecs)
        {
            eprintln!("warning: skip worktree change {}: {e:#}", item.rela_path());
        }
        resource_cache.clear_resource_cache_keep_allocation();
    }

    Ok(out)
}

fn append_worktree_item(
    repo: &Repository,
    cache: &mut gix::diff::blob::Platform,
    out: &mut String,
    item: &IwItem,
    pathspecs: &[String],
) -> Result<()> {
    use gix::status::plumbing::index_as_worktree::{Change, EntryStatus};

    match item {
        IwItem::Modification {
            entry,
            rela_path,
            status,
            ..
        } => {
            if !pathspec_match(rela_path.as_bstr(), pathspecs) {
                return Ok(());
            }
            let path = path_display(rela_path.as_bstr());
            let mode = entry
                .mode
                .to_tree_entry_mode()
                .map(|m| m.kind())
                .unwrap_or(EntryKind::Blob);
            let null = repo.object_hash().null();

            match status {
                EntryStatus::Change(Change::Removed) => {
                    set_pair(
                        cache,
                        repo,
                        entry.id,
                        mode,
                        null,
                        mode,
                        rela_path.as_bstr(),
                        rela_path.as_bstr(),
                    )?;
                    render_file_patch(out, Some(&path), None, cache)?;
                }
                EntryStatus::Change(Change::Modification { .. })
                | EntryStatus::Change(Change::Type { .. })
                | EntryStatus::IntentToAdd => {
                    set_pair(
                        cache,
                        repo,
                        entry.id,
                        mode,
                        null,
                        mode,
                        rela_path.as_bstr(),
                        rela_path.as_bstr(),
                    )?;
                    render_file_patch(out, Some(&path), Some(&path), cache)?;
                }
                EntryStatus::Change(Change::SubmoduleModification(_))
                | EntryStatus::Conflict { .. }
                | EntryStatus::NeedsUpdate(_) => {}
            }
        }
        IwItem::DirectoryContents {
            collapsed_directory_status,
            ..
        } => {
            // Skip collapsed directories — they don't represent individual files.
            if collapsed_directory_status.is_some() {
                return Ok(());
            }
            let rela_path = item.rela_path();
            if !pathspec_match(rela_path, pathspecs) {
                return Ok(());
            }
            let path = path_display(rela_path);
            let null = repo.object_hash().null();
            // Untracked file: diff from /dev/null to the file on disk.
            // Use the worktree root + rela_path to construct the on-disk path,
            // and set up the resource cache to diff from empty to file content.
            let worktree_path = repo
                .workdir()
                .ok_or_else(|| anyhow!("bare repo has no workdir"))?
                .join(path.as_str());
            let disk_content = std::fs::read(&worktree_path)
                .with_context(|| format!("read untracked file {}", worktree_path.display()))?;
            let id =
                gix::objs::compute_hash(repo.object_hash(), gix::objs::Kind::Blob, &disk_content)
                    .with_context(|| format!("hash untracked file {}", worktree_path.display()))?;
            cache
                .set_resource(
                    null,
                    EntryKind::Blob,
                    rela_path,
                    ResourceKind::OldOrSource,
                    &repo.objects,
                )
                .context("set old (null) resource for untracked")?;
            cache
                .set_resource(
                    id,
                    EntryKind::Blob,
                    rela_path,
                    ResourceKind::NewOrDestination,
                    &repo.objects,
                )
                .context("set new resource for untracked")?;
            render_file_patch(out, None, Some(&path), cache)?;
        }
        IwItem::Rewrite { .. } => {}
    }
    Ok(())
}

fn append_index_change(
    repo: &Repository,
    cache: &mut gix::diff::blob::Platform,
    out: &mut String,
    change: &gix::diff::index::ChangeRef<'_, '_>,
) -> Result<()> {
    use gix::diff::index::ChangeRef;

    let null = repo.object_hash().null();
    match change {
        ChangeRef::Addition {
            location,
            entry_mode,
            id,
            ..
        } => {
            let path = path_display(location.as_ref());
            let kind = entry_mode
                .to_tree_entry_mode()
                .map(|m| m.kind())
                .unwrap_or(EntryKind::Blob);
            set_pair(
                cache,
                repo,
                null,
                kind,
                ObjectId::from(id.as_ref()),
                kind,
                location.as_ref(),
                location.as_ref(),
            )?;
            render_file_patch(out, None, Some(&path), cache)?;
        }
        ChangeRef::Deletion {
            location,
            entry_mode,
            id,
            ..
        } => {
            let path = path_display(location.as_ref());
            let kind = entry_mode
                .to_tree_entry_mode()
                .map(|m| m.kind())
                .unwrap_or(EntryKind::Blob);
            set_pair(
                cache,
                repo,
                ObjectId::from(id.as_ref()),
                kind,
                null,
                kind,
                location.as_ref(),
                location.as_ref(),
            )?;
            render_file_patch(out, Some(&path), None, cache)?;
        }
        ChangeRef::Modification {
            location,
            previous_entry_mode,
            previous_id,
            entry_mode,
            id,
            ..
        } => {
            let path = path_display(location.as_ref());
            let old_kind = previous_entry_mode
                .to_tree_entry_mode()
                .map(|m| m.kind())
                .unwrap_or(EntryKind::Blob);
            let new_kind = entry_mode
                .to_tree_entry_mode()
                .map(|m| m.kind())
                .unwrap_or(EntryKind::Blob);
            set_pair(
                cache,
                repo,
                ObjectId::from(previous_id.as_ref()),
                old_kind,
                ObjectId::from(id.as_ref()),
                new_kind,
                location.as_ref(),
                location.as_ref(),
            )?;
            render_file_patch(out, Some(&path), Some(&path), cache)?;
        }
        ChangeRef::Rewrite {
            source_location,
            source_entry_mode,
            source_id,
            location,
            entry_mode,
            id,
            ..
        } => {
            let old_path = path_display(source_location.as_ref());
            let new_path = path_display(location.as_ref());
            let old_kind = source_entry_mode
                .to_tree_entry_mode()
                .map(|m| m.kind())
                .unwrap_or(EntryKind::Blob);
            let new_kind = entry_mode
                .to_tree_entry_mode()
                .map(|m| m.kind())
                .unwrap_or(EntryKind::Blob);
            set_pair(
                cache,
                repo,
                ObjectId::from(source_id.as_ref()),
                old_kind,
                ObjectId::from(id.as_ref()),
                new_kind,
                source_location.as_ref(),
                location.as_ref(),
            )?;
            render_file_patch(out, Some(&old_path), Some(&new_path), cache)?;
        }
    }
    Ok(())
}

// ─── tree ↔ tree ─────────────────────────────────────────────────────────────

fn diff_tree_oids(repo: &Repository, old: Option<ObjectId>, new: ObjectId) -> Result<String> {
    let empty = repo.empty_tree();
    let old_tree_owned = match old {
        Some(id) if !id.is_empty_tree() => Some(
            repo.find_object(id)
                .context("load old tree")?
                .peel_to_tree()
                .context("peel old tree")?,
        ),
        _ => None,
    };
    let new_tree_owned = if new.is_empty_tree() {
        None
    } else {
        Some(
            repo.find_object(new)
                .context("load new tree")?
                .peel_to_tree()
                .context("peel new tree")?,
        )
    };

    let old_ref = old_tree_owned.as_ref().unwrap_or(&empty);
    let new_ref = new_tree_owned.as_ref().unwrap_or(&empty);

    let changes = repo
        .diff_tree_to_tree(Some(old_ref), Some(new_ref), None)
        .context("diff_tree_to_tree")?;

    let mut out = String::new();
    let mut cache = repo
        .diff_resource_cache_for_tree_diff()
        .context("diff resource cache")?;

    for change in &changes {
        if change.entry_mode().is_tree() {
            continue;
        }
        if let Err(e) = append_tree_change(repo, &mut cache, &mut out, change) {
            eprintln!(
                "warning: skip tree change {}: {e:#}",
                change.location().to_str_lossy()
            );
        }
        cache.clear_resource_cache_keep_allocation();
    }

    Ok(out)
}

fn append_tree_change(
    repo: &Repository,
    cache: &mut gix::diff::blob::Platform,
    out: &mut String,
    change: &gix::object::tree::diff::ChangeDetached,
) -> Result<()> {
    use gix::diff::tree_with_rewrites::Change;

    let null = repo.object_hash().null();
    match change {
        Change::Addition {
            location,
            entry_mode,
            id,
            ..
        } => {
            let path = path_display(location.as_bstr());
            set_pair(
                cache,
                repo,
                null,
                entry_mode.kind(),
                *id,
                entry_mode.kind(),
                location.as_bstr(),
                location.as_bstr(),
            )?;
            render_file_patch(out, None, Some(&path), cache)?;
        }
        Change::Deletion {
            location,
            entry_mode,
            id,
            ..
        } => {
            let path = path_display(location.as_bstr());
            set_pair(
                cache,
                repo,
                *id,
                entry_mode.kind(),
                null,
                entry_mode.kind(),
                location.as_bstr(),
                location.as_bstr(),
            )?;
            render_file_patch(out, Some(&path), None, cache)?;
        }
        Change::Modification {
            location,
            previous_entry_mode,
            previous_id,
            entry_mode,
            id,
            ..
        } => {
            let path = path_display(location.as_bstr());
            set_pair(
                cache,
                repo,
                *previous_id,
                previous_entry_mode.kind(),
                *id,
                entry_mode.kind(),
                location.as_bstr(),
                location.as_bstr(),
            )?;
            render_file_patch(out, Some(&path), Some(&path), cache)?;
        }
        Change::Rewrite {
            source_location,
            source_entry_mode,
            source_id,
            location,
            entry_mode,
            id,
            ..
        } => {
            let old_path = path_display(source_location.as_bstr());
            let new_path = path_display(location.as_bstr());
            set_pair(
                cache,
                repo,
                *source_id,
                source_entry_mode.kind(),
                *id,
                entry_mode.kind(),
                source_location.as_bstr(),
                location.as_bstr(),
            )?;
            render_file_patch(out, Some(&old_path), Some(&new_path), cache)?;
        }
    }
    Ok(())
}

// ─── unified patch helpers ───────────────────────────────────────────────────

/// Populate a diff platform with both sides of a file pair.
///
/// The argument list mirrors gix's resource model (two resources × id/kind/path)
/// and is kept flat to read at the call sites; grouping into a struct would
/// just add ceremony without clarifying intent.
#[allow(clippy::too_many_arguments)]
fn set_pair(
    cache: &mut gix::diff::blob::Platform,
    repo: &Repository,
    old_id: ObjectId,
    old_kind: EntryKind,
    new_id: ObjectId,
    new_kind: EntryKind,
    old_path: &BStr,
    new_path: &BStr,
) -> Result<()> {
    cache
        .set_resource(
            old_id,
            old_kind,
            old_path,
            ResourceKind::OldOrSource,
            &repo.objects,
        )
        .context("set old resource")?;
    cache
        .set_resource(
            new_id,
            new_kind,
            new_path,
            ResourceKind::NewOrDestination,
            &repo.objects,
        )
        .context("set new resource")?;
    Ok(())
}

fn render_file_patch(
    out: &mut String,
    old_path: Option<&str>,
    new_path: Option<&str>,
    cache: &mut gix::diff::blob::Platform,
) -> Result<()> {
    let display_old = old_path.unwrap_or("/dev/null");
    let display_new = new_path.unwrap_or("/dev/null");
    let git_old = old_path.unwrap_or(new_path.unwrap_or("unknown"));
    let git_new = new_path.unwrap_or(old_path.unwrap_or("unknown"));

    cache.options.skip_internal_diff_if_external_is_configured = false;
    let prep = cache.prepare_diff().context("prepare_diff")?;

    match prep.operation {
        Operation::InternalDiff { algorithm } => {
            let input = prep.interned_input();
            let diff = gix::diff::blob::diff_with_slider_heuristics(algorithm, &input);

            use std::fmt::Write as _;
            let _ = writeln!(out, "diff --git a/{git_old} b/{git_new}");
            match (old_path, new_path) {
                (None, Some(n)) => {
                    let _ = writeln!(out, "new file mode 100644");
                    let _ = writeln!(out, "--- /dev/null");
                    let _ = writeln!(out, "+++ b/{n}");
                }
                (Some(o), None) => {
                    let _ = writeln!(out, "deleted file mode 100644");
                    let _ = writeln!(out, "--- a/{o}");
                    let _ = writeln!(out, "+++ /dev/null");
                }
                (Some(o), Some(n)) => {
                    let _ = writeln!(out, "--- a/{o}");
                    let _ = writeln!(out, "+++ b/{n}");
                }
                (None, None) => bail!("both sides of diff missing"),
            }

            let body = Vec::<u8>::new();
            let consumer = ConsumeBinaryHunk::new(body, "\n");
            let body = UnifiedDiff::new(&diff, &input, consumer, ContextSize::symmetrical(3))
                .consume()
                .context("render unified diff")?;
            out.push_str(&String::from_utf8_lossy(&body));
        }
        Operation::SourceOrDestinationIsBinary => {
            use std::fmt::Write as _;
            let _ = writeln!(out, "diff --git a/{git_old} b/{git_new}");
            let _ = writeln!(
                out,
                "Binary files a/{display_old} and b/{display_new} differ"
            );
        }
        Operation::ExternalCommand { .. } => {}
    }
    Ok(())
}

// ─── rev helpers ─────────────────────────────────────────────────────────────

fn peel_to_oid(repo: &Repository, spec: &str) -> Result<ObjectId> {
    let parsed = repo
        .rev_parse(spec)
        .with_context(|| format!("rev-parse `{spec}`"))?;
    let id = parsed
        .single()
        .ok_or_else(|| anyhow!("revision `{spec}` did not resolve to a single object"))?;
    Ok(id.detach())
}

/// Returns `(left, right, is_triple_dot)` for `A..B` / `A...B`.
fn parse_range(rev: &str) -> Option<(&str, &str, bool)> {
    if let Some((a, b)) = rev.split_once("...") {
        if !a.is_empty() && !b.is_empty() {
            return Some((a, b, true));
        }
    }
    if let Some((a, b)) = rev.split_once("..") {
        if !a.is_empty() && !b.is_empty() && !rev.contains("...") {
            return Some((a, b, false));
        }
    }
    None
}

fn path_display(path: &BStr) -> String {
    path.to_str_lossy().into_owned()
}

fn pathspec_match(path: &BStr, pathspecs: &[String]) -> bool {
    if pathspecs.is_empty() {
        return true;
    }
    let p = path.to_str_lossy();
    pathspecs.iter().any(|spec| {
        p == *spec
            || p.starts_with(spec.trim_end_matches('/'))
            || Path::new(p.as_ref()).starts_with(spec)
    })
}
