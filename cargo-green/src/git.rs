// Adapted from https://github.com/Byron/gitoxide/blob/gitoxide-core-v0.39.1/gitoxide-core/src/repository/index/entries.rs#L39
// See https://github.com/Byron/gitoxide/discussions/1525#discussioncomment-10369906

use std::{borrow::Cow, collections::BTreeSet, ffi::OsString, io::BufWriter};

use anyhow::{anyhow, Result};
use camino::Utf8PathBuf;
use gix::{
    attrs::{search::Outcome, Assignment},
    bstr::{BStr, BString},
    discover,
    index::{
        entry::{Mode, Stage},
        Entry, File,
    },
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

#[derive(Debug, Eq, PartialEq, Hash, Clone, Copy)]
enum OutputFormat {
    Human,
}

pub(crate) fn ls_files(pwd: &Utf8PathBuf) -> Result<BTreeSet<OsString>> {
    let repo = discover(pwd).map_err(|e| anyhow!("Failed git-ing dir {pwd}: {e}"))?;
    let mut stdout = vec![];
    list_entries(repo, vec![], &mut stdout)
        .map_err(|e| anyhow!("Failed `git ls-files` {pwd}: {e}"))?;
    let stdout = String::from_utf8(stdout).map_err(|e| anyhow!("Failed parsing stdout: {e}"))?;

    Ok(stdout.lines().map(OsString::from).collect::<BTreeSet<_>>())
}

#[derive(Debug, Copy, Clone)]
enum Attributes {
    /// Look at attributes from index files only.
    Index,
}

fn list_entries(repo: Repository, pathspecs: Vec<BString>, out: impl std::io::Write) -> Result<()> {
    let mut out = BufWriter::with_capacity(64 * 1024, out);

    let _ = print_entries(
        &repo,
        pathspecs.iter(),
        OutputFormat::Human,
        true,
        "".into(),
        true,
        &mut out,
    )?;

    Ok(())
}

fn is_dir_to_mode(is_dir: bool) -> Mode {
    if is_dir {
        Mode::DIR
    } else {
        Mode::FILE
    }
}

fn print_entries(
    repo: &Repository,
    pathspecs: impl IntoIterator<Item = impl AsRef<BStr>> + Clone,
    format: OutputFormat,
    simple: bool,
    prefix: &BStr,
    recurse_submodules: bool,
    out: &mut impl std::io::Write,
) -> Result<StatisticsBis> {
    let (mut pathspec, index, mut cache) = init_cache(repo, pathspecs.clone())?;
    let submodules_by_path = recurse_submodules
        .then(|| {
            repo.submodules()
                .map(|opt| {
                    opt.map(|submodules| {
                        submodules
                            .map(|sm| sm.path().map(Cow::into_owned).map(move |path| (path, sm)))
                            .collect::<Result<Vec<_>, _>>()
                    })
                })
                .transpose()
        })
        .flatten()
        .transpose()?
        .transpose()?;
    let mut stats = StatisticsBis { entries: index.entries().len(), ..Default::default() };
    if let Some(entries) = index.prefixed_entries(pathspec.common_prefix()) {
        stats.entries_after_prune = entries.len();
        let mut entries = entries.iter().peekable();
        while let Some(entry) = entries.next() {
            let attrs =
                cache.as_mut().and_then(|(_attrs, _cache)| None::<Result<Attrs>>).transpose()?;

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
                                    .at_entry(rela_path, Some(is_dir_to_mode(is_dir)))
                                    .ok()
                                    .map(|platform| platform.matching_attributes(out))
                                    .unwrap_or_default()
                            })
                            .unwrap_or_default()
                    },
                )
                .map_or(true, |m| m.is_excluded());

            let entry_is_submodule = entry.mode.is_submodule();
            if entry_is_excluded && (!entry_is_submodule || !recurse_submodules) {
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
                let sm_stats = print_entries(
                    &sm_repo,
                    pathspecs.clone(),
                    format,
                    simple,
                    prefix.as_ref(),
                    recurse_submodules,
                    out,
                )?;
                stats.submodule.push((sm_path.into_owned(), sm_stats));
            } else {
                match format {
                    OutputFormat::Human => {
                        if simple {
                            to_human_simple(out, &index, entry, attrs, prefix)
                        } else {
                            to_human(out, &index, entry, attrs, prefix)
                        }?
                    }
                }
            }
        }
    }

    stats.cache = cache.map(|c| *c.1.statistics());
    Ok(stats)
}

fn init_cache(
    repo: &Repository,
    pathspecs: impl IntoIterator<Item = impl AsRef<BStr>>,
) -> Result<(Search, IndexPersistedOrInMemory, Option<(Outcome, AttributeStack<'_>)>)> {
    let index = repo.index_or_load_from_head()?;
    let pathspec = repo.pathspec(
        true,
        pathspecs,
        false,
        &index,
        AttrSource::WorktreeThenIdMapping.adjust_for_bare(repo.is_bare()),
    )?;
    let cache = None
        .or_else(|| {
            pathspec
                .search()
                .patterns()
                .any(|spec| !spec.attributes.is_empty())
                .then_some(Attributes::Index)
        })
        .map(|attrs| {
            repo.attributes(
                &index,
                match attrs {
                    Attributes::Index => AttrSource::IdMapping,
                },
                match attrs {
                    Attributes::Index => IgnSource::IdMapping,
                },
                None,
            )
            .map(|cache| (cache.attribute_matches(), cache))
        })
        .transpose()?;
    Ok((pathspec.into_parts().0, index, cache))
}

struct Attrs {
    is_excluded: bool,
    attributes: Vec<Assignment>,
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

fn to_human_simple(
    out: &mut impl std::io::Write,
    file: &File,
    entry: &Entry,
    attrs: Option<Attrs>,
    prefix: &BStr,
) -> std::io::Result<()> {
    if !prefix.is_empty() {
        out.write_all(prefix)?;
    }
    match attrs {
        Some(attrs) => {
            out.write_all(entry.path(file))?;
            out.write_all(print_attrs(Some(attrs), entry.mode).as_bytes())
        }
        None => out.write_all(entry.path(file)),
    }?;
    out.write_all(b"\n")
}

fn to_human(
    out: &mut impl std::io::Write,
    file: &File,
    entry: &Entry,
    attrs: Option<Attrs>,
    prefix: &BStr,
) -> std::io::Result<()> {
    writeln!(
        out,
        "{} {}{:?} {} {}{}{}",
        match entry.flags.stage() {
            Stage::Unconflicted => "       ",
            Stage::Base => "BASE   ",
            Stage::Ours => "OURS   ",
            Stage::Theirs => "THEIRS ",
        },
        if entry.flags.is_empty() { "".to_owned() } else { format!("{:?} ", entry.flags) },
        entry.mode,
        entry.id,
        prefix,
        entry.path(file),
        print_attrs(attrs, entry.mode)
    )
}

fn print_attrs(attrs: Option<Attrs>, mode: Mode) -> Cow<'static, str> {
    attrs.map_or(Cow::Borrowed(""), |a| {
        let mut buf = String::new();
        if mode.is_sparse() {
            buf.push_str(" 📁 ");
        } else if mode.is_submodule() {
            buf.push_str(" ➡ ");
        }
        if a.is_excluded {
            buf.push_str(" 🗑️");
        }
        if !a.attributes.is_empty() {
            buf.push_str(" (");
            for assignment in a.attributes {
                use std::fmt::Write;
                write!(&mut buf, "{}", assignment.as_ref()).ok();
                buf.push_str(", ");
            }
            buf.pop();
            buf.pop();
            buf.push(')');
        }
        buf.into()
    })
}
