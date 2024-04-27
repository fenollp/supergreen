use nutype::nutype;

#[nutype(
    sanitize(trim, lowercase),
    validate(not_empty, len_char_max = 20),
    derive(Debug, PartialEq),
)]
pub struct Username(String);

// Now we can create usernames:
assert_eq!(
    Username::new("   FooBar  ").unwrap().into_inner(),
    "foobar"
);

// But we cannot create invalid ones:
assert_eq!(
    Username::new("   "),
    Err(UsernameError::NotEmptyViolated),
);

assert_eq!(
    Username::new("TheUserNameIsVeryVeryLong"),
    Err(UsernameError::LenCharMaxViolated),
);


// 0 0s rustcbuildx.git wip λ ge -A10 nutyp
// src/ir.rs:94:// FIXME: nutype?
// src/ir.rs-95-#[inline]
// src/ir.rs-96-#[must_use]
// src/ir.rs-97-pub(crate) fn safe_stage(stage: String) -> String {
// src/ir.rs-98-    stage
// src/ir.rs-99-        .to_lowercase()
// src/ir.rs-100-        .replace(|c: char| c != '.' && !c.is_alphanumeric(), "-")
// src/ir.rs-101-        .replace("___", "_")
// src/ir.rs-102-        .to_owned()
// src/ir.rs-103-}

#[test]
fn safe_stages() {
    pretty_assertions::assert_eq!(
        safe_stage("libgit2-sys-0.14.2+1.5.1-index.crates.io-6f17d22bba15001f".to_owned()),
        "libgit2-sys-0.14.2-1.5.1-index.crates.io-6f17d22bba15001f".to_owned()
    );
}
