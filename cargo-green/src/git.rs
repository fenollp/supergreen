// Adapted from https://github.com/Byron/gitoxide/blob/gitoxide-core-v0.39.1/gitoxide-core/src/repository/index/entries.rs#L39
// See https://github.com/Byron/gitoxide/discussions/1525#discussioncomment-10369906

use std::borrow::Cow;

use anyhow::{anyhow, Result};
use camino::Utf8PathBuf;
use gix::{
    attrs::search::Outcome,
    bstr::{BStr, BString},
    discover,
    index::{entry::Mode, Entry, File},
    path::to_unix_separators_on_windows,
    pathspec::Search,
    worktree::{
        stack::{
            state::{attributes::Source as AttrSource, ignore::Source as IgnSource},
            Statistics,
        },
        IndexPersistedOrInMemory,
    },
    AttributeStack, Repository,
};

/// Returns `git ls-files <pwd>` paths relative to pwd
pub(crate) fn ls_files(pwd: &Utf8PathBuf) -> Result<Vec<Utf8PathBuf>> {
    let repo = discover(pwd).map_err(|e| anyhow!("Failed git-ing dir {pwd}: {e}"))?;
    let mut out = vec![];
    let _ = list_entries(&repo, "".into(), &mut out)
        .map_err(|e| anyhow!("Failed `git ls-files` {pwd}: {e}"))?;
    Ok(out)
}

fn list_entries(
    repo: &Repository,
    prefix: &BStr,
    out: &mut Vec<Utf8PathBuf>,
) -> Result<StatisticsBis> {
    let (mut pathspec, index, mut cache) = init_cache(repo)?;

    let submodules_by_path = repo
        .submodules()
        .map(|opt| {
            opt.map(|submodules| {
                submodules
                    .map(|sm| sm.path().map(Cow::into_owned).map(move |path| (path, sm)))
                    .collect::<Result<Vec<_>, _>>()
            })
        })
        .transpose()
        .transpose()?
        .transpose()?;

    let mut stats = StatisticsBis { entries: index.entries().len(), ..Default::default() };

    if let Some(entries) = index.prefixed_entries(pathspec.common_prefix()) {
        stats.entries_after_prune = entries.len();
        for entry in entries.iter().peekable() {
            // Note that we intentionally ignore `_case` so that we act like git does, attribute matching case is determined
            // by the repository, not the pathspec.
            let entry_is_excluded = pathspec
                .pattern_matching_relative_path(
                    entry.path(&index),
                    Some(false),
                    &mut |rela_path, _case, is_dir, out| {
                        cache
                            .as_mut()
                            .map(|(_attrs, cache)| {
                                cache
                                    .at_entry(
                                        rela_path,
                                        Some(if is_dir { Mode::DIR } else { Mode::FILE }),
                                    )
                                    .ok()
                                    .map(|platform| platform.matching_attributes(out))
                                    .unwrap_or_default()
                            })
                            .unwrap_or_default()
                    },
                )
                .map_or(true, |m| m.is_excluded());

            let entry_is_submodule = entry.mode.is_submodule();
            if entry_is_excluded && !entry_is_submodule {
                continue;
            }

            if let Some(sm) =
                submodules_by_path.as_ref().filter(|_| entry_is_submodule).and_then(|sms_by_path| {
                    let entry_path = entry.path(&index);
                    sms_by_path
                        .iter()
                        .find_map(|(path, sm)| (path == entry_path).then_some(sm))
                        .filter(|sm| {
                            sm.git_dir_try_old_form().map_or(false, |dot_git| dot_git.exists())
                        })
                })
            {
                let sm_path = to_unix_separators_on_windows(sm.path()?);
                let sm_repo = sm.open()?.expect("we checked it exists");
                let mut prefix = prefix.to_owned();
                prefix.extend_from_slice(sm_path.as_ref());
                if !sm_path.ends_with(b"/") {
                    prefix.push(b'/');
                }
                let sm_stats = list_entries(&sm_repo, prefix.as_ref(), out)?;
                stats.submodule.push((sm_path.into_owned(), sm_stats));
            } else {
                to_human_simple(out, &index, entry, prefix);
            }
        }
    }

    stats.cache = cache.map(|c| *c.1.statistics());
    Ok(stats)
}

#[allow(clippy::type_complexity)]
fn init_cache(
    repo: &Repository,
) -> Result<(Search, IndexPersistedOrInMemory, Option<(Outcome, AttributeStack<'_>)>)> {
    let index = repo.index_or_load_from_head()?;
    let pathspec = repo.pathspec(
        true,
        Vec::<BString>::new(),
        false,
        &index,
        AttrSource::WorktreeThenIdMapping.adjust_for_bare(repo.is_bare()),
    )?;
    let cache = pathspec
        .search()
        .patterns()
        .any(|spec| !spec.attributes.is_empty())
        .then(|| {
            repo.attributes(&index, AttrSource::IdMapping, IgnSource::IdMapping, None)
                .map(|cache| (cache.attribute_matches(), cache))
        })
        .transpose()?;
    Ok((pathspec.into_parts().0, index, cache))
}

#[derive(Default, Debug)]
struct StatisticsBis {
    #[allow(dead_code)]
    // Not really dead, but Debug doesn't count for it even though it's crucial.
    entries: usize,
    entries_after_prune: usize,
    cache: Option<Statistics>,
    submodule: Vec<(BString, StatisticsBis)>,
}

fn to_human_simple(out: &mut Vec<Utf8PathBuf>, file: &File, entry: &Entry, prefix: &BStr) {
    out.push(Utf8PathBuf::from(if prefix.is_empty() {
        entry.path(file).to_string()
    } else {
        prefix.to_string() + entry.path(file).to_string().as_str()
    }));
}
