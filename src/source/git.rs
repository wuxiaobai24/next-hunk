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

/// Diff a single revision (commit → parent) or a range `A..B` / `A...B`.
pub fn git_show(repo_path: &Path, rev: &str) -> Result<String> {
    let repo = open_repo(repo_path)?;
    if let Some((a, b, merge_base)) = parse_range(rev) {
        let old = if merge_base {
            let left = peel_to_oid(&repo, a)?;
            let right = peel_to_oid(&repo, b)?;
            repo.merge_base(left, right)
                .with_context(|| format!("no merge base for {rev}"))?
                .detach()
        } else {
            peel_to_oid(&repo, a)?
        };
        let new = peel_to_oid(&repo, b)?;
        return diff_tree_oids(&repo, Some(old), new);
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

// ─── staged / worktree ───────────────────────────────────────────────────────

fn diff_staged(repo: &Repository, pathspecs: &[String]) -> Result<String> {
    let index = repo
        .index_or_load_from_head_or_empty()
        .context("open index")?;
    let head_tree = repo
        .head_tree_id_or_empty()
        .context("resolve HEAD^{tree}")?
        .detach();

    let mut out = String::new();
    let mut resource_cache = repo
        .diff_resource_cache_for_tree_diff()
        .context("diff resource cache")?;

    // IndexPersistedOrInMemory → File → State via Deref.
    repo.tree_index_status(
        &head_tree,
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
