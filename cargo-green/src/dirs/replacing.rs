/// With `bounded`, requires a token boundary (or $) after `from`:
/// Bare /a/b must not match inside /a/b.bak nor /a/bc.
pub(crate) fn replace_tokens(txt: &str, from: &str, to: &str, bounded: bool) -> String {
    assert!(!from.is_empty());
    fn boundary_before(prefix: &str) -> bool {
        matches!(prefix.chars().next_back(), None | Some(' ' | '\n' | '\'' | '"' | '=' | '`'))
    }
    fn boundary_after(suffix: &str) -> bool {
        matches!(
            suffix.chars().next(),
            None | Some(' ' | '\n' | '\'' | '"' | '=' | '`' | ':' | ',')
        )
    }

    let mut out = String::with_capacity(txt.len());
    let mut last = 0;
    for (start, _) in txt.match_indices(from) {
        let end = start + from.len();
        let replace = boundary_before(&txt[..start]) && (!bounded || boundary_after(&txt[end..]));
        out.push_str(&txt[last..start]);
        out.push_str(if replace { to } else { from });
        last = end;
    }
    out.push_str(&txt[last..]);
    out
}

#[test]
fn replace_bounded_replaces_whole_tokens() {
    let f = |txt| replace_tokens(txt, "/a/b", "/work", true);
    assert_eq!(f("/a/b"), "/work"); // whole input: both boundaries are ends of text
    assert_eq!(f("cd /a/b"), "cd /work");
    assert_eq!(f("PATH=/a/b"), "PATH=/work");
    assert_eq!(f("'/a/b'"), "'/work'");
    assert_eq!(f("\"/a/b\""), "\"/work\"");
    assert_eq!(f("`/a/b`"), "`/work`");
    assert_eq!(f("/a/b:/elsewhere"), "/work:/elsewhere");
    assert_eq!(f("dirs: /a/b, /a/b\n"), "dirs: /work, /work\n");
}

#[test]
fn replace_bounded_requires_token_boundaries() {
    let f = |txt| replace_tokens(txt, "/a/b", "/work", true);
    assert_eq!(f("/a/b.bak"), "/a/b.bak");
    assert_eq!(f("/a/bc"), "/a/bc");
    assert_eq!(f("/a/b2"), "/a/b2");
    // Subpaths are replace_carefully's job: `/` after is not a boundary here
    assert_eq!(f("/a/b/c"), "/a/b/c");
    // Nor is `/` (or any other char) before
    assert_eq!(f("/x/a/b"), "/x/a/b");
    assert_eq!(f("X/a/b"), "X/a/b");
}

#[test]
fn replace_bounded_repeated_and_mixed_matches() {
    assert_eq!(replace_tokens("/a/b /a/b.bak /a/b", "/a/b", "/work", true), "/work /a/b.bak /work");
    // A `to` containing `from` must not be rescanned
    assert_eq!(replace_tokens("/a/b", "/a/b", "/a/b/sub", true), "/a/b/sub");
    // Matches never overlap: in "aaa" only the occurrence at offset 0 is considered
    assert_eq!(replace_tokens("aaa aa", "aa", "Z", true), "aaa Z");
}
