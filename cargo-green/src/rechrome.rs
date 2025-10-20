use crate::{add::ENV_ADD_APT, green::ENV_SET_ENVS, PKG};

#[must_use]
fn suggest(original: &str, suggestion: &str, msg: &str) -> Option<String> {
    let mut data: serde_json::Value = serde_json::from_str(msg).ok()?;
    let rendered = data.get_mut("rendered")?;
    let txt = rendered.as_str()?;

    // '= ' is an ANSI colors -safe choice of separator
    let existing = txt.split("= ").find(|help| help.contains(original))?;

    let mut to = existing.to_owned();
    to.push_str("= ");
    to.push_str(&existing.replace(original, suggestion));

    *rendered = serde_json::json!(txt.replace(existing, &to));
    serde_json::to_string(&data).ok()
}

// Matches (ANSI colors dropped) '''= note: /usr/bin/ld: cannot find -lpq: No such file or directory'''
#[must_use]
pub(crate) fn lib_not_found(msg: &str) -> Option<&str> {
    if let Some((_, rhs)) = msg.split_once(r#"cannot find -l"#) {
        if let Some((lib, _)) = rhs.split_once(": No such file or directory") {
            return Some(lib);
        }
    }
    None
}

// TODO: cleanup how this suggestion appears
#[must_use]
pub(crate) fn suggest_add(lib: &str, msg: &str) -> Option<String> {
    let original = format!("cannot find -l{lib}: No such file or directory");

    let lib = match lib {
        "z" => "zlib1g-dev".to_owned(),
        _ => format!("lib{lib}-dev"),
    };
    let suggestion = format!(
        r#"{PKG}: add `{lib:?}` to either ${ENV_ADD_APT} (apk, apt-get) or to this crate's or your root crate's [package.metadata.green.add] apt list"#
    );

    suggest(&original, &suggestion, msg)
}

#[test]
fn suggesting_add() {
    let input = roundtrip(
        r#"
{
    "$message_type": "diagnostic",
    "message": "linking with `cc` failed: exit status: 1",
    "code": null,
    "level": "error",
    "spans": [],
    "children": [
        {
            "message": " \"cc\" \"-m64\" \"/tmp/rustc7H5UYy/symbols.o\" \"<17 object files omitted>\" \"-Wl,--as-needed\" \"-Wl,-Bstatic\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libdiffy-5f350840256b90b3.rlib,libnu_ansi_term-ecef502768b2f22a.rlib,liboverload-45aafc635a57cff1.rlib,liburl-036f69e82fdcf501.rlib,libidna-ffbc39c7a5d1e77c.rlib,libunicode_normalization-da6cfe0b315d21ba.rlib,libtinyvec-0e7c6d99fc4ecd2c.rlib,libtinyvec_macros-9126bcde4c5f1615.rlib,libunicode_bidi-23766e683251f25e.rlib,libform_urlencoded-ed0427b193415122.rlib,libpercent_encoding-a9ca6250cc102bd0.rlib,libmatches-27d9c5e1e6de7509.rlib,libdotenvy-9fa159acfce4885d.rlib,libchrono-4d3d56d73bf46ec0.rlib,libiana_time_zone-5ed377f5b3d451ef.rlib,libnum_integer-82fb0132f8b53906.rlib,libnum_traits-2f9bcd2a0c30dcff.rlib,libheck-40dbaec9d09d443f.rlib,libdiesel_table_macro_syntax-9f09f66c20aa386f.rlib,libsyn-abced2e57ae6b47a.rlib,libquote-6740271d31439b66.rlib,libproc_macro2-ae7c4c38eaf4593c.rlib,libunicode_ident-aa12b7412dfc4c29.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libproc_macro-*}.rlib\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libclap_complete-2bc2ccaa5088f99a.rlib,libdiesel_migrations-8a52bbbeef5e01db.rlib,libmigrations_internals-52b495bc29f74724.rlib,libtoml-c89277bcdcbf47a1.rlib,libtoml_edit-5bbd23cd7ed53a4f.rlib,libserde_spanned-333f86fbbb2cb152.rlib,libindexmap-6b2daab44e782afb.rlib,libhashbrown-c275e17bff8ef48e.rlib,libwinnow-5ed65ae865e1214b.rlib,libtoml_datetime-69f07545405ed040.rlib,libdiesel-8a4b75e5cbe6865c.rlib,libitoa-f0986793f4dc0c4b.rlib,libbitflags-e95363e9370d640e.rlib,libbyteorder-62ae682b1bf4015e.rlib,libpq_sys-4db52583e4ef9bff.rlib,libserde_regex-b49f06d76e9ca5eb.rlib,libregex-51a29a962320663c.rlib,libaho_corasick-ec90a4b45e50196f.rlib,libmemchr-35aa984256b2a6dc.rlib,libregex_syntax-f2754ca68026052b.rlib,libclap-6885138d353f613d.rlib,libclap_builder-64fd68ff56e9c3b1.rlib,libstrsim-6185e9224c6e4564.rlib,libanstream-d6e3047ceacd4591.rlib,libanstyle_query-5a9f17d21dd97f4e.rlib,libis_terminal-c352a962fc4f6cab.rlib,librustix-2b2c39f9f9d03b14.rlib,liblinux_raw_sys-f6a722b30bf667ab.rlib,libio_lifetimes-a415eef40056a286.rlib,liblibc-859d6bf52555fe98.rlib,libanstyle-587cffdda4d8c45a.rlib,libcolorchoice-6d2fdffc0ac55bb0.rlib,libanstyle_parse-1e447f4b0544156a.rlib,libutf8parse-811cc7a6fc8bc58b.rlib,libclap_lex-c13afce077d893db.rlib,libbitflags-852662162838ab1a.rlib,libonce_cell-c147a48c1dcf8469.rlib,libserde-fa2e373f0760a32a.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libstd-*,libpanic_unwind-*,libobject-*,libmemchr-*,libaddr2line-*,libgimli-*,librustc_demangle-*,libstd_detect-*,libhashbrown-*,librustc_std_workspace_alloc-*,libminiz_oxide-*,libadler2-*,libunwind-*,libcfg_if-*,liblibc-*,liballoc-*,librustc_std_workspace_core-*,libcore-*,libcompiler_builtins-*}.rlib\" \"-Wl,-Bdynamic\" \"-lpq\" \"-lgcc_s\" \"-lutil\" \"-lrt\" \"-lpthread\" \"-lm\" \"-ldl\" \"-lc\" \"-Wl,--eh-frame-hdr\" \"-Wl,-z,noexecstack\" \"-L\" \"/usr/lib/x86_64-linux-gnu\" \"-L\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib\" \"-o\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/diesel-6a254b742936f321\" \"-Wl,--gc-sections\" \"-pie\" \"-Wl,-z,relro,-z,now\" \"-Wl,-O1\" \"-Wl,--strip-debug\" \"-nodefaultlibs\"",
            "code": null,
            "level": "note",
            "spans": [],
            "children": [],
            "rendered": null
        },
        {
            "message": "some arguments are omitted. use `--verbose` to show all linker arguments",
            "code": null,
            "level": "note",
            "spans": [],
            "children": [],
            "rendered": null
        },
        {
            "message": "/usr/bin/ld: cannot find -lpq: No such file or directory\ncollect2: error: ld returned 1 exit status\n",
            "code": null,
            "level": "note",
            "spans": [],
            "children": [],
            "rendered": null
        }
    ],
    "rendered": "error: linking with `cc` failed: exit status: 1\n  |\n  = note:  \"cc\" \"-m64\" \"/tmp/rustc7H5UYy/symbols.o\" \"<17 object files omitted>\" \"-Wl,--as-needed\" \"-Wl,-Bstatic\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libdiffy-5f350840256b90b3.rlib,libnu_ansi_term-ecef502768b2f22a.rlib,liboverload-45aafc635a57cff1.rlib,liburl-036f69e82fdcf501.rlib,libidna-ffbc39c7a5d1e77c.rlib,libunicode_normalization-da6cfe0b315d21ba.rlib,libtinyvec-0e7c6d99fc4ecd2c.rlib,libtinyvec_macros-9126bcde4c5f1615.rlib,libunicode_bidi-23766e683251f25e.rlib,libform_urlencoded-ed0427b193415122.rlib,libpercent_encoding-a9ca6250cc102bd0.rlib,libmatches-27d9c5e1e6de7509.rlib,libdotenvy-9fa159acfce4885d.rlib,libchrono-4d3d56d73bf46ec0.rlib,libiana_time_zone-5ed377f5b3d451ef.rlib,libnum_integer-82fb0132f8b53906.rlib,libnum_traits-2f9bcd2a0c30dcff.rlib,libheck-40dbaec9d09d443f.rlib,libdiesel_table_macro_syntax-9f09f66c20aa386f.rlib,libsyn-abced2e57ae6b47a.rlib,libquote-6740271d31439b66.rlib,libproc_macro2-ae7c4c38eaf4593c.rlib,libunicode_ident-aa12b7412dfc4c29.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libproc_macro-*}.rlib\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libclap_complete-2bc2ccaa5088f99a.rlib,libdiesel_migrations-8a52bbbeef5e01db.rlib,libmigrations_internals-52b495bc29f74724.rlib,libtoml-c89277bcdcbf47a1.rlib,libtoml_edit-5bbd23cd7ed53a4f.rlib,libserde_spanned-333f86fbbb2cb152.rlib,libindexmap-6b2daab44e782afb.rlib,libhashbrown-c275e17bff8ef48e.rlib,libwinnow-5ed65ae865e1214b.rlib,libtoml_datetime-69f07545405ed040.rlib,libdiesel-8a4b75e5cbe6865c.rlib,libitoa-f0986793f4dc0c4b.rlib,libbitflags-e95363e9370d640e.rlib,libbyteorder-62ae682b1bf4015e.rlib,libpq_sys-4db52583e4ef9bff.rlib,libserde_regex-b49f06d76e9ca5eb.rlib,libregex-51a29a962320663c.rlib,libaho_corasick-ec90a4b45e50196f.rlib,libmemchr-35aa984256b2a6dc.rlib,libregex_syntax-f2754ca68026052b.rlib,libclap-6885138d353f613d.rlib,libclap_builder-64fd68ff56e9c3b1.rlib,libstrsim-6185e9224c6e4564.rlib,libanstream-d6e3047ceacd4591.rlib,libanstyle_query-5a9f17d21dd97f4e.rlib,libis_terminal-c352a962fc4f6cab.rlib,librustix-2b2c39f9f9d03b14.rlib,liblinux_raw_sys-f6a722b30bf667ab.rlib,libio_lifetimes-a415eef40056a286.rlib,liblibc-859d6bf52555fe98.rlib,libanstyle-587cffdda4d8c45a.rlib,libcolorchoice-6d2fdffc0ac55bb0.rlib,libanstyle_parse-1e447f4b0544156a.rlib,libutf8parse-811cc7a6fc8bc58b.rlib,libclap_lex-c13afce077d893db.rlib,libbitflags-852662162838ab1a.rlib,libonce_cell-c147a48c1dcf8469.rlib,libserde-fa2e373f0760a32a.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libstd-*,libpanic_unwind-*,libobject-*,libmemchr-*,libaddr2line-*,libgimli-*,librustc_demangle-*,libstd_detect-*,libhashbrown-*,librustc_std_workspace_alloc-*,libminiz_oxide-*,libadler2-*,libunwind-*,libcfg_if-*,liblibc-*,liballoc-*,librustc_std_workspace_core-*,libcore-*,libcompiler_builtins-*}.rlib\" \"-Wl,-Bdynamic\" \"-lpq\" \"-lgcc_s\" \"-lutil\" \"-lrt\" \"-lpthread\" \"-lm\" \"-ldl\" \"-lc\" \"-Wl,--eh-frame-hdr\" \"-Wl,-z,noexecstack\" \"-L\" \"/usr/lib/x86_64-linux-gnu\" \"-L\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib\" \"-o\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/diesel-6a254b742936f321\" \"-Wl,--gc-sections\" \"-pie\" \"-Wl,-z,relro,-z,now\" \"-Wl,-O1\" \"-Wl,--strip-debug\" \"-nodefaultlibs\"\n  = note: some arguments are omitted. use `--verbose` to show all linker arguments\n  = note: /usr/bin/ld: cannot find -lpq: No such file or directory\n          collect2: error: ld returned 1 exit status\n          \n\n"
}"#,
    );

    let output = roundtrip(
        r#"
{
    "$message_type": "diagnostic",
    "message": "linking with `cc` failed: exit status: 1",
    "code": null,
    "level": "error",
    "spans": [],
    "children": [
        {
            "message": " \"cc\" \"-m64\" \"/tmp/rustc7H5UYy/symbols.o\" \"<17 object files omitted>\" \"-Wl,--as-needed\" \"-Wl,-Bstatic\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libdiffy-5f350840256b90b3.rlib,libnu_ansi_term-ecef502768b2f22a.rlib,liboverload-45aafc635a57cff1.rlib,liburl-036f69e82fdcf501.rlib,libidna-ffbc39c7a5d1e77c.rlib,libunicode_normalization-da6cfe0b315d21ba.rlib,libtinyvec-0e7c6d99fc4ecd2c.rlib,libtinyvec_macros-9126bcde4c5f1615.rlib,libunicode_bidi-23766e683251f25e.rlib,libform_urlencoded-ed0427b193415122.rlib,libpercent_encoding-a9ca6250cc102bd0.rlib,libmatches-27d9c5e1e6de7509.rlib,libdotenvy-9fa159acfce4885d.rlib,libchrono-4d3d56d73bf46ec0.rlib,libiana_time_zone-5ed377f5b3d451ef.rlib,libnum_integer-82fb0132f8b53906.rlib,libnum_traits-2f9bcd2a0c30dcff.rlib,libheck-40dbaec9d09d443f.rlib,libdiesel_table_macro_syntax-9f09f66c20aa386f.rlib,libsyn-abced2e57ae6b47a.rlib,libquote-6740271d31439b66.rlib,libproc_macro2-ae7c4c38eaf4593c.rlib,libunicode_ident-aa12b7412dfc4c29.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libproc_macro-*}.rlib\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libclap_complete-2bc2ccaa5088f99a.rlib,libdiesel_migrations-8a52bbbeef5e01db.rlib,libmigrations_internals-52b495bc29f74724.rlib,libtoml-c89277bcdcbf47a1.rlib,libtoml_edit-5bbd23cd7ed53a4f.rlib,libserde_spanned-333f86fbbb2cb152.rlib,libindexmap-6b2daab44e782afb.rlib,libhashbrown-c275e17bff8ef48e.rlib,libwinnow-5ed65ae865e1214b.rlib,libtoml_datetime-69f07545405ed040.rlib,libdiesel-8a4b75e5cbe6865c.rlib,libitoa-f0986793f4dc0c4b.rlib,libbitflags-e95363e9370d640e.rlib,libbyteorder-62ae682b1bf4015e.rlib,libpq_sys-4db52583e4ef9bff.rlib,libserde_regex-b49f06d76e9ca5eb.rlib,libregex-51a29a962320663c.rlib,libaho_corasick-ec90a4b45e50196f.rlib,libmemchr-35aa984256b2a6dc.rlib,libregex_syntax-f2754ca68026052b.rlib,libclap-6885138d353f613d.rlib,libclap_builder-64fd68ff56e9c3b1.rlib,libstrsim-6185e9224c6e4564.rlib,libanstream-d6e3047ceacd4591.rlib,libanstyle_query-5a9f17d21dd97f4e.rlib,libis_terminal-c352a962fc4f6cab.rlib,librustix-2b2c39f9f9d03b14.rlib,liblinux_raw_sys-f6a722b30bf667ab.rlib,libio_lifetimes-a415eef40056a286.rlib,liblibc-859d6bf52555fe98.rlib,libanstyle-587cffdda4d8c45a.rlib,libcolorchoice-6d2fdffc0ac55bb0.rlib,libanstyle_parse-1e447f4b0544156a.rlib,libutf8parse-811cc7a6fc8bc58b.rlib,libclap_lex-c13afce077d893db.rlib,libbitflags-852662162838ab1a.rlib,libonce_cell-c147a48c1dcf8469.rlib,libserde-fa2e373f0760a32a.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libstd-*,libpanic_unwind-*,libobject-*,libmemchr-*,libaddr2line-*,libgimli-*,librustc_demangle-*,libstd_detect-*,libhashbrown-*,librustc_std_workspace_alloc-*,libminiz_oxide-*,libadler2-*,libunwind-*,libcfg_if-*,liblibc-*,liballoc-*,librustc_std_workspace_core-*,libcore-*,libcompiler_builtins-*}.rlib\" \"-Wl,-Bdynamic\" \"-lpq\" \"-lgcc_s\" \"-lutil\" \"-lrt\" \"-lpthread\" \"-lm\" \"-ldl\" \"-lc\" \"-Wl,--eh-frame-hdr\" \"-Wl,-z,noexecstack\" \"-L\" \"/usr/lib/x86_64-linux-gnu\" \"-L\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib\" \"-o\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/diesel-6a254b742936f321\" \"-Wl,--gc-sections\" \"-pie\" \"-Wl,-z,relro,-z,now\" \"-Wl,-O1\" \"-Wl,--strip-debug\" \"-nodefaultlibs\"",
            "code": null,
            "level": "note",
            "spans": [],
            "children": [],
            "rendered": null
        },
        {
            "message": "some arguments are omitted. use `--verbose` to show all linker arguments",
            "code": null,
            "level": "note",
            "spans": [],
            "children": [],
            "rendered": null
        },
        {
            "message": "/usr/bin/ld: cannot find -lpq: No such file or directory\ncollect2: error: ld returned 1 exit status\n",
            "code": null,
            "level": "note",
            "spans": [],
            "children": [],
            "rendered": null
        }
    ],
    "rendered": "error: linking with `cc` failed: exit status: 1\n  |\n  = note:  \"cc\" \"-m64\" \"/tmp/rustc7H5UYy/symbols.o\" \"<17 object files omitted>\" \"-Wl,--as-needed\" \"-Wl,-Bstatic\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libdiffy-5f350840256b90b3.rlib,libnu_ansi_term-ecef502768b2f22a.rlib,liboverload-45aafc635a57cff1.rlib,liburl-036f69e82fdcf501.rlib,libidna-ffbc39c7a5d1e77c.rlib,libunicode_normalization-da6cfe0b315d21ba.rlib,libtinyvec-0e7c6d99fc4ecd2c.rlib,libtinyvec_macros-9126bcde4c5f1615.rlib,libunicode_bidi-23766e683251f25e.rlib,libform_urlencoded-ed0427b193415122.rlib,libpercent_encoding-a9ca6250cc102bd0.rlib,libmatches-27d9c5e1e6de7509.rlib,libdotenvy-9fa159acfce4885d.rlib,libchrono-4d3d56d73bf46ec0.rlib,libiana_time_zone-5ed377f5b3d451ef.rlib,libnum_integer-82fb0132f8b53906.rlib,libnum_traits-2f9bcd2a0c30dcff.rlib,libheck-40dbaec9d09d443f.rlib,libdiesel_table_macro_syntax-9f09f66c20aa386f.rlib,libsyn-abced2e57ae6b47a.rlib,libquote-6740271d31439b66.rlib,libproc_macro2-ae7c4c38eaf4593c.rlib,libunicode_ident-aa12b7412dfc4c29.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libproc_macro-*}.rlib\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libclap_complete-2bc2ccaa5088f99a.rlib,libdiesel_migrations-8a52bbbeef5e01db.rlib,libmigrations_internals-52b495bc29f74724.rlib,libtoml-c89277bcdcbf47a1.rlib,libtoml_edit-5bbd23cd7ed53a4f.rlib,libserde_spanned-333f86fbbb2cb152.rlib,libindexmap-6b2daab44e782afb.rlib,libhashbrown-c275e17bff8ef48e.rlib,libwinnow-5ed65ae865e1214b.rlib,libtoml_datetime-69f07545405ed040.rlib,libdiesel-8a4b75e5cbe6865c.rlib,libitoa-f0986793f4dc0c4b.rlib,libbitflags-e95363e9370d640e.rlib,libbyteorder-62ae682b1bf4015e.rlib,libpq_sys-4db52583e4ef9bff.rlib,libserde_regex-b49f06d76e9ca5eb.rlib,libregex-51a29a962320663c.rlib,libaho_corasick-ec90a4b45e50196f.rlib,libmemchr-35aa984256b2a6dc.rlib,libregex_syntax-f2754ca68026052b.rlib,libclap-6885138d353f613d.rlib,libclap_builder-64fd68ff56e9c3b1.rlib,libstrsim-6185e9224c6e4564.rlib,libanstream-d6e3047ceacd4591.rlib,libanstyle_query-5a9f17d21dd97f4e.rlib,libis_terminal-c352a962fc4f6cab.rlib,librustix-2b2c39f9f9d03b14.rlib,liblinux_raw_sys-f6a722b30bf667ab.rlib,libio_lifetimes-a415eef40056a286.rlib,liblibc-859d6bf52555fe98.rlib,libanstyle-587cffdda4d8c45a.rlib,libcolorchoice-6d2fdffc0ac55bb0.rlib,libanstyle_parse-1e447f4b0544156a.rlib,libutf8parse-811cc7a6fc8bc58b.rlib,libclap_lex-c13afce077d893db.rlib,libbitflags-852662162838ab1a.rlib,libonce_cell-c147a48c1dcf8469.rlib,libserde-fa2e373f0760a32a.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libstd-*,libpanic_unwind-*,libobject-*,libmemchr-*,libaddr2line-*,libgimli-*,librustc_demangle-*,libstd_detect-*,libhashbrown-*,librustc_std_workspace_alloc-*,libminiz_oxide-*,libadler2-*,libunwind-*,libcfg_if-*,liblibc-*,liballoc-*,librustc_std_workspace_core-*,libcore-*,libcompiler_builtins-*}.rlib\" \"-Wl,-Bdynamic\" \"-lpq\" \"-lgcc_s\" \"-lutil\" \"-lrt\" \"-lpthread\" \"-lm\" \"-ldl\" \"-lc\" \"-Wl,--eh-frame-hdr\" \"-Wl,-z,noexecstack\" \"-L\" \"/usr/lib/x86_64-linux-gnu\" \"-L\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib\" \"-o\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/diesel-6a254b742936f321\" \"-Wl,--gc-sections\" \"-pie\" \"-Wl,-z,relro,-z,now\" \"-Wl,-O1\" \"-Wl,--strip-debug\" \"-nodefaultlibs\"\n  = note: some arguments are omitted. use `--verbose` to show all linker arguments\n  = note: /usr/bin/ld: cannot find -lpq: No such file or directory\n          collect2: error: ld returned 1 exit status\n          \n\n= note: /usr/bin/ld: cargo-green: add `\"libpq-dev\"` to either $CARGOGREEN_ADD_APT (apk, apt-get) or to this crate's or your root crate's [package.metadata.green.add] apt list\n          collect2: error: ld returned 1 exit status\n          \n\n"
}"#,
    );

    assert_eq!(lib_not_found(&input), Some("pq"));

    pretty_assertions::assert_eq!(roundtrip(&suggest_add("pq", &input).unwrap()), output);
}

#[test]
fn suggesting_add_ansi() {
    let input = roundtrip(
        r#"
{
    "$message_type": "diagnostic",
    "message": "linking with `cc` failed: exit status: 1",
    "code": null,
    "level": "error",
    "spans": [],
    "children": [
        {
            "message": " \"cc\" \"-m64\" \"/tmp/rustc7H5UYy/symbols.o\" \"<17 object files omitted>\" \"-Wl,--as-needed\" \"-Wl,-Bstatic\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libdiffy-5f350840256b90b3.rlib,libnu_ansi_term-ecef502768b2f22a.rlib,liboverload-45aafc635a57cff1.rlib,liburl-036f69e82fdcf501.rlib,libidna-ffbc39c7a5d1e77c.rlib,libunicode_normalization-da6cfe0b315d21ba.rlib,libtinyvec-0e7c6d99fc4ecd2c.rlib,libtinyvec_macros-9126bcde4c5f1615.rlib,libunicode_bidi-23766e683251f25e.rlib,libform_urlencoded-ed0427b193415122.rlib,libpercent_encoding-a9ca6250cc102bd0.rlib,libmatches-27d9c5e1e6de7509.rlib,libdotenvy-9fa159acfce4885d.rlib,libchrono-4d3d56d73bf46ec0.rlib,libiana_time_zone-5ed377f5b3d451ef.rlib,libnum_integer-82fb0132f8b53906.rlib,libnum_traits-2f9bcd2a0c30dcff.rlib,libheck-40dbaec9d09d443f.rlib,libdiesel_table_macro_syntax-9f09f66c20aa386f.rlib,libsyn-abced2e57ae6b47a.rlib,libquote-6740271d31439b66.rlib,libproc_macro2-ae7c4c38eaf4593c.rlib,libunicode_ident-aa12b7412dfc4c29.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libproc_macro-*}.rlib\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libclap_complete-2bc2ccaa5088f99a.rlib,libdiesel_migrations-8a52bbbeef5e01db.rlib,libmigrations_internals-52b495bc29f74724.rlib,libtoml-c89277bcdcbf47a1.rlib,libtoml_edit-5bbd23cd7ed53a4f.rlib,libserde_spanned-333f86fbbb2cb152.rlib,libindexmap-6b2daab44e782afb.rlib,libhashbrown-c275e17bff8ef48e.rlib,libwinnow-5ed65ae865e1214b.rlib,libtoml_datetime-69f07545405ed040.rlib,libdiesel-8a4b75e5cbe6865c.rlib,libitoa-f0986793f4dc0c4b.rlib,libbitflags-e95363e9370d640e.rlib,libbyteorder-62ae682b1bf4015e.rlib,libpq_sys-4db52583e4ef9bff.rlib,libserde_regex-b49f06d76e9ca5eb.rlib,libregex-51a29a962320663c.rlib,libaho_corasick-ec90a4b45e50196f.rlib,libmemchr-35aa984256b2a6dc.rlib,libregex_syntax-f2754ca68026052b.rlib,libclap-6885138d353f613d.rlib,libclap_builder-64fd68ff56e9c3b1.rlib,libstrsim-6185e9224c6e4564.rlib,libanstream-d6e3047ceacd4591.rlib,libanstyle_query-5a9f17d21dd97f4e.rlib,libis_terminal-c352a962fc4f6cab.rlib,librustix-2b2c39f9f9d03b14.rlib,liblinux_raw_sys-f6a722b30bf667ab.rlib,libio_lifetimes-a415eef40056a286.rlib,liblibc-859d6bf52555fe98.rlib,libanstyle-587cffdda4d8c45a.rlib,libcolorchoice-6d2fdffc0ac55bb0.rlib,libanstyle_parse-1e447f4b0544156a.rlib,libutf8parse-811cc7a6fc8bc58b.rlib,libclap_lex-c13afce077d893db.rlib,libbitflags-852662162838ab1a.rlib,libonce_cell-c147a48c1dcf8469.rlib,libserde-fa2e373f0760a32a.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libstd-*,libpanic_unwind-*,libobject-*,libmemchr-*,libaddr2line-*,libgimli-*,librustc_demangle-*,libstd_detect-*,libhashbrown-*,librustc_std_workspace_alloc-*,libminiz_oxide-*,libadler2-*,libunwind-*,libcfg_if-*,liblibc-*,liballoc-*,librustc_std_workspace_core-*,libcore-*,libcompiler_builtins-*}.rlib\" \"-Wl,-Bdynamic\" \"-lpq\" \"-lgcc_s\" \"-lutil\" \"-lrt\" \"-lpthread\" \"-lm\" \"-ldl\" \"-lc\" \"-Wl,--eh-frame-hdr\" \"-Wl,-z,noexecstack\" \"-L\" \"/usr/lib/x86_64-linux-gnu\" \"-L\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib\" \"-o\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/diesel-6a254b742936f321\" \"-Wl,--gc-sections\" \"-pie\" \"-Wl,-z,relro,-z,now\" \"-Wl,-O1\" \"-Wl,--strip-debug\" \"-nodefaultlibs\"",
            "code": null,
            "level": "note",
            "spans": [],
            "children": [],
            "rendered": null
        },
        {
            "message": "some arguments are omitted. use `--verbose` to show all linker arguments",
            "code": null,
            "level": "note",
            "spans": [],
            "children": [],
            "rendered": null
        },
        {
            "message": "/usr/bin/ld: cannot find -lpq: No such file or directory\ncollect2: error: ld returned 1 exit status\n",
            "code": null,
            "level": "note",
            "spans": [],
            "children": [],
            "rendered": null
        }
    ],
    "rendered": "\u001b[0m\u001b[1m\u001b[38;5;9merror\u001b[0m\u001b[0m\u001b[1m: linking with `cc` failed: exit status: 1\u001b[0m\n\u001b[0m  \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\n\u001b[0m  \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mnote\u001b[0m\u001b[0m:  \"cc\" \"-m64\" \"/tmp/rustc7H5UYy/symbols.o\" \"<17 object files omitted>\" \"-Wl,--as-needed\" \"-Wl,-Bstatic\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libdiffy-5f350840256b90b3.rlib,libnu_ansi_term-ecef502768b2f22a.rlib,liboverload-45aafc635a57cff1.rlib,liburl-036f69e82fdcf501.rlib,libidna-ffbc39c7a5d1e77c.rlib,libunicode_normalization-da6cfe0b315d21ba.rlib,libtinyvec-0e7c6d99fc4ecd2c.rlib,libtinyvec_macros-9126bcde4c5f1615.rlib,libunicode_bidi-23766e683251f25e.rlib,libform_urlencoded-ed0427b193415122.rlib,libpercent_encoding-a9ca6250cc102bd0.rlib,libmatches-27d9c5e1e6de7509.rlib,libdotenvy-9fa159acfce4885d.rlib,libchrono-4d3d56d73bf46ec0.rlib,libiana_time_zone-5ed377f5b3d451ef.rlib,libnum_integer-82fb0132f8b53906.rlib,libnum_traits-2f9bcd2a0c30dcff.rlib,libheck-40dbaec9d09d443f.rlib,libdiesel_table_macro_syntax-9f09f66c20aa386f.rlib,libsyn-abced2e57ae6b47a.rlib,libquote-6740271d31439b66.rlib,libproc_macro2-ae7c4c38eaf4593c.rlib,libunicode_ident-aa12b7412dfc4c29.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libproc_macro-*}.rlib\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libclap_complete-2bc2ccaa5088f99a.rlib,libdiesel_migrations-8a52bbbeef5e01db.rlib,libmigrations_internals-52b495bc29f74724.rlib,libtoml-c89277bcdcbf47a1.rlib,libtoml_edit-5bbd23cd7ed53a4f.rlib,libserde_spanned-333f86fbbb2cb152.rlib,libindexmap-6b2daab44e782afb.rlib,libhashbrown-c275e17bff8ef48e.rlib,libwinnow-5ed65ae865e1214b.rlib,libtoml_datetime-69f07545405ed040.rlib,libdiesel-8a4b75e5cbe6865c.rlib,libitoa-f0986793f4dc0c4b.rlib,libbitflags-e95363e9370d640e.rlib,libbyteorder-62ae682b1bf4015e.rlib,libpq_sys-4db52583e4ef9bff.rlib,libserde_regex-b49f06d76e9ca5eb.rlib,libregex-51a29a962320663c.rlib,libaho_corasick-ec90a4b45e50196f.rlib,libmemchr-35aa984256b2a6dc.rlib,libregex_syntax-f2754ca68026052b.rlib,libclap-6885138d353f613d.rlib,libclap_builder-64fd68ff56e9c3b1.rlib,libstrsim-6185e9224c6e4564.rlib,libanstream-d6e3047ceacd4591.rlib,libanstyle_query-5a9f17d21dd97f4e.rlib,libis_terminal-c352a962fc4f6cab.rlib,librustix-2b2c39f9f9d03b14.rlib,liblinux_raw_sys-f6a722b30bf667ab.rlib,libio_lifetimes-a415eef40056a286.rlib,liblibc-859d6bf52555fe98.rlib,libanstyle-587cffdda4d8c45a.rlib,libcolorchoice-6d2fdffc0ac55bb0.rlib,libanstyle_parse-1e447f4b0544156a.rlib,libutf8parse-811cc7a6fc8bc58b.rlib,libclap_lex-c13afce077d893db.rlib,libbitflags-852662162838ab1a.rlib,libonce_cell-c147a48c1dcf8469.rlib,libserde-fa2e373f0760a32a.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libstd-*,libpanic_unwind-*,libobject-*,libmemchr-*,libaddr2line-*,libgimli-*,librustc_demangle-*,libstd_detect-*,libhashbrown-*,librustc_std_workspace_alloc-*,libminiz_oxide-*,libadler2-*,libunwind-*,libcfg_if-*,liblibc-*,liballoc-*,librustc_std_workspace_core-*,libcore-*,libcompiler_builtins-*}.rlib\" \"-Wl,-Bdynamic\" \"-lpq\" \"-lgcc_s\" \"-lutil\" \"-lrt\" \"-lpthread\" \"-lm\" \"-ldl\" \"-lc\" \"-Wl,--eh-frame-hdr\" \"-Wl,-z,noexecstack\" \"-L\" \"/usr/lib/x86_64-linux-gnu\" \"-L\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib\" \"-o\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/diesel-6a254b742936f321\" \"-Wl,--gc-sections\" \"-pie\" \"-Wl,-z,relro,-z,now\" \"-Wl,-O1\" \"-Wl,--strip-debug\" \"-nodefaultlibs\"\u001b[0m\n\u001b[0m  \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mnote\u001b[0m\u001b[0m: some arguments are omitted. use `--verbose` to show all linker arguments\u001b[0m\n\u001b[0m  \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mnote\u001b[0m\u001b[0m: /usr/bin/ld: cannot find -lpq: No such file or directory\u001b[0m\n\u001b[0m          collect2: error: ld returned 1 exit status\u001b[0m\n\u001b[0m          \u001b[0m\n\n"
}"#,
    );

    let output = roundtrip(
        r#"
{
    "$message_type": "diagnostic",
    "message": "linking with `cc` failed: exit status: 1",
    "code": null,
    "level": "error",
    "spans": [],
    "children": [
        {
            "message": " \"cc\" \"-m64\" \"/tmp/rustc7H5UYy/symbols.o\" \"<17 object files omitted>\" \"-Wl,--as-needed\" \"-Wl,-Bstatic\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libdiffy-5f350840256b90b3.rlib,libnu_ansi_term-ecef502768b2f22a.rlib,liboverload-45aafc635a57cff1.rlib,liburl-036f69e82fdcf501.rlib,libidna-ffbc39c7a5d1e77c.rlib,libunicode_normalization-da6cfe0b315d21ba.rlib,libtinyvec-0e7c6d99fc4ecd2c.rlib,libtinyvec_macros-9126bcde4c5f1615.rlib,libunicode_bidi-23766e683251f25e.rlib,libform_urlencoded-ed0427b193415122.rlib,libpercent_encoding-a9ca6250cc102bd0.rlib,libmatches-27d9c5e1e6de7509.rlib,libdotenvy-9fa159acfce4885d.rlib,libchrono-4d3d56d73bf46ec0.rlib,libiana_time_zone-5ed377f5b3d451ef.rlib,libnum_integer-82fb0132f8b53906.rlib,libnum_traits-2f9bcd2a0c30dcff.rlib,libheck-40dbaec9d09d443f.rlib,libdiesel_table_macro_syntax-9f09f66c20aa386f.rlib,libsyn-abced2e57ae6b47a.rlib,libquote-6740271d31439b66.rlib,libproc_macro2-ae7c4c38eaf4593c.rlib,libunicode_ident-aa12b7412dfc4c29.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libproc_macro-*}.rlib\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libclap_complete-2bc2ccaa5088f99a.rlib,libdiesel_migrations-8a52bbbeef5e01db.rlib,libmigrations_internals-52b495bc29f74724.rlib,libtoml-c89277bcdcbf47a1.rlib,libtoml_edit-5bbd23cd7ed53a4f.rlib,libserde_spanned-333f86fbbb2cb152.rlib,libindexmap-6b2daab44e782afb.rlib,libhashbrown-c275e17bff8ef48e.rlib,libwinnow-5ed65ae865e1214b.rlib,libtoml_datetime-69f07545405ed040.rlib,libdiesel-8a4b75e5cbe6865c.rlib,libitoa-f0986793f4dc0c4b.rlib,libbitflags-e95363e9370d640e.rlib,libbyteorder-62ae682b1bf4015e.rlib,libpq_sys-4db52583e4ef9bff.rlib,libserde_regex-b49f06d76e9ca5eb.rlib,libregex-51a29a962320663c.rlib,libaho_corasick-ec90a4b45e50196f.rlib,libmemchr-35aa984256b2a6dc.rlib,libregex_syntax-f2754ca68026052b.rlib,libclap-6885138d353f613d.rlib,libclap_builder-64fd68ff56e9c3b1.rlib,libstrsim-6185e9224c6e4564.rlib,libanstream-d6e3047ceacd4591.rlib,libanstyle_query-5a9f17d21dd97f4e.rlib,libis_terminal-c352a962fc4f6cab.rlib,librustix-2b2c39f9f9d03b14.rlib,liblinux_raw_sys-f6a722b30bf667ab.rlib,libio_lifetimes-a415eef40056a286.rlib,liblibc-859d6bf52555fe98.rlib,libanstyle-587cffdda4d8c45a.rlib,libcolorchoice-6d2fdffc0ac55bb0.rlib,libanstyle_parse-1e447f4b0544156a.rlib,libutf8parse-811cc7a6fc8bc58b.rlib,libclap_lex-c13afce077d893db.rlib,libbitflags-852662162838ab1a.rlib,libonce_cell-c147a48c1dcf8469.rlib,libserde-fa2e373f0760a32a.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libstd-*,libpanic_unwind-*,libobject-*,libmemchr-*,libaddr2line-*,libgimli-*,librustc_demangle-*,libstd_detect-*,libhashbrown-*,librustc_std_workspace_alloc-*,libminiz_oxide-*,libadler2-*,libunwind-*,libcfg_if-*,liblibc-*,liballoc-*,librustc_std_workspace_core-*,libcore-*,libcompiler_builtins-*}.rlib\" \"-Wl,-Bdynamic\" \"-lpq\" \"-lgcc_s\" \"-lutil\" \"-lrt\" \"-lpthread\" \"-lm\" \"-ldl\" \"-lc\" \"-Wl,--eh-frame-hdr\" \"-Wl,-z,noexecstack\" \"-L\" \"/usr/lib/x86_64-linux-gnu\" \"-L\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib\" \"-o\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/diesel-6a254b742936f321\" \"-Wl,--gc-sections\" \"-pie\" \"-Wl,-z,relro,-z,now\" \"-Wl,-O1\" \"-Wl,--strip-debug\" \"-nodefaultlibs\"",
            "code": null,
            "level": "note",
            "spans": [],
            "children": [],
            "rendered": null
        },
        {
            "message": "some arguments are omitted. use `--verbose` to show all linker arguments",
            "code": null,
            "level": "note",
            "spans": [],
            "children": [],
            "rendered": null
        },
        {
            "message": "/usr/bin/ld: cannot find -lpq: No such file or directory\ncollect2: error: ld returned 1 exit status\n",
            "code": null,
            "level": "note",
            "spans": [],
            "children": [],
            "rendered": null
        }
    ],
    "rendered": "\u001b[0m\u001b[1m\u001b[38;5;9merror\u001b[0m\u001b[0m\u001b[1m: linking with `cc` failed: exit status: 1\u001b[0m\n\u001b[0m  \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\n\u001b[0m  \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mnote\u001b[0m\u001b[0m:  \"cc\" \"-m64\" \"/tmp/rustc7H5UYy/symbols.o\" \"<17 object files omitted>\" \"-Wl,--as-needed\" \"-Wl,-Bstatic\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libdiffy-5f350840256b90b3.rlib,libnu_ansi_term-ecef502768b2f22a.rlib,liboverload-45aafc635a57cff1.rlib,liburl-036f69e82fdcf501.rlib,libidna-ffbc39c7a5d1e77c.rlib,libunicode_normalization-da6cfe0b315d21ba.rlib,libtinyvec-0e7c6d99fc4ecd2c.rlib,libtinyvec_macros-9126bcde4c5f1615.rlib,libunicode_bidi-23766e683251f25e.rlib,libform_urlencoded-ed0427b193415122.rlib,libpercent_encoding-a9ca6250cc102bd0.rlib,libmatches-27d9c5e1e6de7509.rlib,libdotenvy-9fa159acfce4885d.rlib,libchrono-4d3d56d73bf46ec0.rlib,libiana_time_zone-5ed377f5b3d451ef.rlib,libnum_integer-82fb0132f8b53906.rlib,libnum_traits-2f9bcd2a0c30dcff.rlib,libheck-40dbaec9d09d443f.rlib,libdiesel_table_macro_syntax-9f09f66c20aa386f.rlib,libsyn-abced2e57ae6b47a.rlib,libquote-6740271d31439b66.rlib,libproc_macro2-ae7c4c38eaf4593c.rlib,libunicode_ident-aa12b7412dfc4c29.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libproc_macro-*}.rlib\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/{libclap_complete-2bc2ccaa5088f99a.rlib,libdiesel_migrations-8a52bbbeef5e01db.rlib,libmigrations_internals-52b495bc29f74724.rlib,libtoml-c89277bcdcbf47a1.rlib,libtoml_edit-5bbd23cd7ed53a4f.rlib,libserde_spanned-333f86fbbb2cb152.rlib,libindexmap-6b2daab44e782afb.rlib,libhashbrown-c275e17bff8ef48e.rlib,libwinnow-5ed65ae865e1214b.rlib,libtoml_datetime-69f07545405ed040.rlib,libdiesel-8a4b75e5cbe6865c.rlib,libitoa-f0986793f4dc0c4b.rlib,libbitflags-e95363e9370d640e.rlib,libbyteorder-62ae682b1bf4015e.rlib,libpq_sys-4db52583e4ef9bff.rlib,libserde_regex-b49f06d76e9ca5eb.rlib,libregex-51a29a962320663c.rlib,libaho_corasick-ec90a4b45e50196f.rlib,libmemchr-35aa984256b2a6dc.rlib,libregex_syntax-f2754ca68026052b.rlib,libclap-6885138d353f613d.rlib,libclap_builder-64fd68ff56e9c3b1.rlib,libstrsim-6185e9224c6e4564.rlib,libanstream-d6e3047ceacd4591.rlib,libanstyle_query-5a9f17d21dd97f4e.rlib,libis_terminal-c352a962fc4f6cab.rlib,librustix-2b2c39f9f9d03b14.rlib,liblinux_raw_sys-f6a722b30bf667ab.rlib,libio_lifetimes-a415eef40056a286.rlib,liblibc-859d6bf52555fe98.rlib,libanstyle-587cffdda4d8c45a.rlib,libcolorchoice-6d2fdffc0ac55bb0.rlib,libanstyle_parse-1e447f4b0544156a.rlib,libutf8parse-811cc7a6fc8bc58b.rlib,libclap_lex-c13afce077d893db.rlib,libbitflags-852662162838ab1a.rlib,libonce_cell-c147a48c1dcf8469.rlib,libserde-fa2e373f0760a32a.rlib}.rlib\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib/{libstd-*,libpanic_unwind-*,libobject-*,libmemchr-*,libaddr2line-*,libgimli-*,librustc_demangle-*,libstd_detect-*,libhashbrown-*,librustc_std_workspace_alloc-*,libminiz_oxide-*,libadler2-*,libunwind-*,libcfg_if-*,liblibc-*,liballoc-*,librustc_std_workspace_core-*,libcore-*,libcompiler_builtins-*}.rlib\" \"-Wl,-Bdynamic\" \"-lpq\" \"-lgcc_s\" \"-lutil\" \"-lrt\" \"-lpthread\" \"-lm\" \"-ldl\" \"-lc\" \"-Wl,--eh-frame-hdr\" \"-Wl,-z,noexecstack\" \"-L\" \"/usr/lib/x86_64-linux-gnu\" \"-L\" \"<sysroot>/lib/rustlib/x86_64-unknown-linux-gnu/lib\" \"-o\" \"/tmp/clis-diesel_cli_2-1-1/release/deps/diesel-6a254b742936f321\" \"-Wl,--gc-sections\" \"-pie\" \"-Wl,-z,relro,-z,now\" \"-Wl,-O1\" \"-Wl,--strip-debug\" \"-nodefaultlibs\"\u001b[0m\n\u001b[0m  \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mnote\u001b[0m\u001b[0m: some arguments are omitted. use `--verbose` to show all linker arguments\u001b[0m\n\u001b[0m  \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mnote\u001b[0m\u001b[0m: /usr/bin/ld: cannot find -lpq: No such file or directory\u001b[0m\n\u001b[0m          collect2: error: ld returned 1 exit status\u001b[0m\n\u001b[0m          \u001b[0m\n\n= \u001b[0m\u001b[0m\u001b[1mnote\u001b[0m\u001b[0m: /usr/bin/ld: cargo-green: add `\"libpq-dev\"` to either $CARGOGREEN_ADD_APT (apk, apt-get) or to this crate's or your root crate's [package.metadata.green.add] apt list\u001b[0m\n\u001b[0m          collect2: error: ld returned 1 exit status\u001b[0m\n\u001b[0m          \u001b[0m\n\n"
}"#,
    );

    assert_eq!(lib_not_found(&input), Some("pq"));

    pretty_assertions::assert_eq!(roundtrip(&suggest_add("pq", &input).unwrap()), output);
}

// Matches (ANSI colors dropped) '''"rendered":"error: environment variable `[^`]+` not defined at compile time'''
#[must_use]
pub(crate) fn env_not_comptime_defined(msg: &str) -> Option<&str> {
    if let Some((_, rhs)) = msg.split_once(r#"environment variable `"#) {
        if let Some((var, _)) = rhs.split_once("` not defined at compile time") {
            return Some(var);
        }
    }
    None
}

#[must_use]
pub(crate) fn suggest_set_envs(var: &str, msg: &str) -> Option<String> {
    let original = format!(r#"use `std::env::var("{var}")` to read the variable at run time"#);
    let suggestion = format!(
        r#"{PKG}: add `"{var}"` to either ${ENV_SET_ENVS} or to this crate's or your root crate's [package.metadata.green] set-envs list"#
    );
    suggest(&original, &suggestion, msg)
}

#[cfg(test)]
fn roundtrip(json: &str) -> String {
    let msg: serde_json::Value = serde_json::from_str(json).unwrap();
    serde_json::to_string_pretty(&msg).unwrap()
}

#[test]
fn suggesting_set_envs() {
    let input = roundtrip(
        r#"
{
    "$message_type": "diagnostic",
    "message": "environment variable `MIME_TYPES_GENERATED_PATH` not defined at compile time",
    "code": null,
    "level": "error",
    "spans": [
        {
            "file_name": "/home/pete/.cargo/registry/src/index.crates.io-0000000000000000/mime_guess-2.0.5/src/impl_bin_search.rs",
            "byte_start": 62,
            "byte_end": 95,
            "line_start": 4,
            "line_end": 4,
            "column_start": 10,
            "column_end": 43,
            "is_primary": true,
            "text": [
                {
                    "text": "include!(env!(\"MIME_TYPES_GENERATED_PATH\"));",
                    "highlight_start": 10,
                    "highlight_end": 43
                }
            ],
            "label": null,
            "suggested_replacement": null,
            "suggestion_applicability": null,
            "expansion": {
                "span": {
                    "file_name": "/home/pete/.cargo/registry/src/index.crates.io-0000000000000000/mime_guess-2.0.5/src/impl_bin_search.rs",
                    "byte_start": 62,
                    "byte_end": 95,
                    "line_start": 4,
                    "line_end": 4,
                    "column_start": 10,
                    "column_end": 43,
                    "is_primary": false,
                    "text": [
                        {
                            "text": "include!(env!(\"MIME_TYPES_GENERATED_PATH\"));",
                            "highlight_start": 10,
                            "highlight_end": 43
                        }
                    ],
                    "label": null,
                    "suggested_replacement": null,
                    "suggestion_applicability": null,
                    "expansion": null
                },
                "macro_decl_name": "env!",
                "def_site_span": {
                    "file_name": "/rustc/4d91de4e48198da2e33413efdcd9cd2cc0c46688/library/core/src/macros/mod.rs",
                    "byte_start": 38805,
                    "byte_end": 38821,
                    "line_start": 1101,
                    "line_end": 1101,
                    "column_start": 5,
                    "column_end": 21,
                    "is_primary": false,
                    "text": [],
                    "label": null,
                    "suggested_replacement": null,
                    "suggestion_applicability": null,
                    "expansion": null
                }
            }
        }
    ],
    "children": [
        {
            "message": "use `std::env::var(\"MIME_TYPES_GENERATED_PATH\")` to read the variable at run time",
            "code": null,
            "level": "help",
            "spans": [],
            "children": [],
            "rendered": null
        }
    ],
    "rendered": "error: environment variable `MIME_TYPES_GENERATED_PATH` not defined at compile time\n --> /home/pete/.cargo/registry/src/index.crates.io-0000000000000000/mime_guess-2.0.5/src/impl_bin_search.rs:4:10\n  |\n4 | include!(env!(\"MIME_TYPES_GENERATED_PATH\"));\n  |          ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n  |\n  = help: use `std::env::var(\"MIME_TYPES_GENERATED_PATH\")` to read the variable at run time\n  = note: this error originates in the macro `env` (in Nightly builds, run with -Z macro-backtrace for more info)\n\n"
}"#,
    );

    let output = roundtrip(
        r#"
{
    "$message_type": "diagnostic",
    "message": "environment variable `MIME_TYPES_GENERATED_PATH` not defined at compile time",
    "code": null,
    "level": "error",
    "spans": [
        {
            "file_name": "/home/pete/.cargo/registry/src/index.crates.io-0000000000000000/mime_guess-2.0.5/src/impl_bin_search.rs",
            "byte_start": 62,
            "byte_end": 95,
            "line_start": 4,
            "line_end": 4,
            "column_start": 10,
            "column_end": 43,
            "is_primary": true,
            "text": [
                {
                    "text": "include!(env!(\"MIME_TYPES_GENERATED_PATH\"));",
                    "highlight_start": 10,
                    "highlight_end": 43
                }
            ],
            "label": null,
            "suggested_replacement": null,
            "suggestion_applicability": null,
            "expansion": {
                "span": {
                    "file_name": "/home/pete/.cargo/registry/src/index.crates.io-0000000000000000/mime_guess-2.0.5/src/impl_bin_search.rs",
                    "byte_start": 62,
                    "byte_end": 95,
                    "line_start": 4,
                    "line_end": 4,
                    "column_start": 10,
                    "column_end": 43,
                    "is_primary": false,
                    "text": [
                        {
                            "text": "include!(env!(\"MIME_TYPES_GENERATED_PATH\"));",
                            "highlight_start": 10,
                            "highlight_end": 43
                        }
                    ],
                    "label": null,
                    "suggested_replacement": null,
                    "suggestion_applicability": null,
                    "expansion": null
                },
                "macro_decl_name": "env!",
                "def_site_span": {
                    "file_name": "/rustc/4d91de4e48198da2e33413efdcd9cd2cc0c46688/library/core/src/macros/mod.rs",
                    "byte_start": 38805,
                    "byte_end": 38821,
                    "line_start": 1101,
                    "line_end": 1101,
                    "column_start": 5,
                    "column_end": 21,
                    "is_primary": false,
                    "text": [],
                    "label": null,
                    "suggested_replacement": null,
                    "suggestion_applicability": null,
                    "expansion": null
                }
            }
        }
    ],
    "children": [
        {
            "message": "use `std::env::var(\"MIME_TYPES_GENERATED_PATH\")` to read the variable at run time",
            "code": null,
            "level": "help",
            "spans": [],
            "children": [],
            "rendered": null
        }
    ],
    "rendered": "error: environment variable `MIME_TYPES_GENERATED_PATH` not defined at compile time\n --> /home/pete/.cargo/registry/src/index.crates.io-0000000000000000/mime_guess-2.0.5/src/impl_bin_search.rs:4:10\n  |\n4 | include!(env!(\"MIME_TYPES_GENERATED_PATH\"));\n  |          ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n  |\n  = help: use `std::env::var(\"MIME_TYPES_GENERATED_PATH\")` to read the variable at run time\n  = help: cargo-green: add `\"MIME_TYPES_GENERATED_PATH\"` to either $CARGOGREEN_SET_ENVS or to this crate's or your root crate's [package.metadata.green] set-envs list\n  = note: this error originates in the macro `env` (in Nightly builds, run with -Z macro-backtrace for more info)\n\n"
}"#,
    );

    assert_eq!(env_not_comptime_defined(&input), Some("MIME_TYPES_GENERATED_PATH"));

    pretty_assertions::assert_eq!(
        roundtrip(&suggest_set_envs("MIME_TYPES_GENERATED_PATH", &input).unwrap()),
        output
    );
}

#[test]
fn suggesting_set_envs_ansi() {
    let input = roundtrip(
        r#"
{
    "$message_type": "diagnostic",
    "message": "environment variable `TYPENUM_BUILD_OP` not defined at compile time",
    "code": null,
    "level": "error",
    "spans": [
        {
            "file_name": "/home/pete/.cargo/registry/src/index.crates.io-0000000000000000/typenum-1.12.0/src/lib.rs",
            "byte_start": 2460,
            "byte_end": 2484,
            "line_start": 76,
            "line_end": 76,
            "column_start": 14,
            "column_end": 38,
            "is_primary": true,
            "text": [
                {
                    "text": "    include!(env!(\"TYPENUM_BUILD_OP\"));",
                    "highlight_start": 14,
                    "highlight_end": 38
                }
            ],
            "label": null,
            "suggested_replacement": null,
            "suggestion_applicability": null,
            "expansion": {
                "span": {
                    "file_name": "/home/pete/.cargo/registry/src/index.crates.io-0000000000000000/typenum-1.12.0/src/lib.rs",
                    "byte_start": 2460,
                    "byte_end": 2484,
                    "line_start": 76,
                    "line_end": 76,
                    "column_start": 14,
                    "column_end": 38,
                    "is_primary": false,
                    "text": [
                        {
                            "text": "    include!(env!(\"TYPENUM_BUILD_OP\"));",
                            "highlight_start": 14,
                            "highlight_end": 38
                        }
                    ],
                    "label": null,
                    "suggested_replacement": null,
                    "suggestion_applicability": null,
                    "expansion": null
                },
                "macro_decl_name": "env!",
                "def_site_span":
                {
                    "file_name": "/rustc/05f9846f893b09a1be1fc8560e33fc3c815cfecb/library/core/src/macros/mod.rs",
                    "byte_start": 40546,
                    "byte_end": 40562,
                    "line_start": 1164,
                    "line_end": 1164,
                    "column_start": 5,
                    "column_end": 21,
                    "is_primary": false,
                    "text": [],
                    "label": null,
                    "suggested_replacement": null,
                    "suggestion_applicability": null,
                    "expansion": null
                }
            }
        }
    ],
    "children": [
        {
            "message": "use `std::env::var(\"TYPENUM_BUILD_OP\")` to read the variable at run time",
            "code": null,
            "level": "help",
            "spans": [],
            "children": [],
            "rendered": null
        }
    ],
    "rendered": "\u001b[0m\u001b[1m\u001b[38;5;9merror\u001b[0m\u001b[0m\u001b[1m: environment variable `TYPENUM_BUILD_OP` not defined at compile time\u001b[0m\n\u001b[0m  \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m--> \u001b[0m\u001b[0m/home/pete/.cargo/registry/src/index.crates.io-0000000000000000/typenum-1.12.0/src/lib.rs:76:14\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\n\u001b[0m\u001b[1m\u001b[38;5;12m76\u001b[0m\u001b[0m \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\u001b[0m \u001b[0m\u001b[0m    include!(env!(\"TYPENUM_BUILD_OP\"));\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\u001b[0m              \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;9m^^^^^^^^^^^^^^^^^^^^^^^^\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mhelp\u001b[0m\u001b[0m: use `std::env::var(\"TYPENUM_BUILD_OP\")` to read the variable at run time\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mnote\u001b[0m\u001b[0m: this error originates in the macro `env` (in Nightly builds, run with -Z macro-backtrace for more info)\u001b[0m\n\n"
}"#,
    );

    let output = roundtrip(
        r#"{
    "$message_type": "diagnostic",
    "message": "environment variable `TYPENUM_BUILD_OP` not defined at compile time",
    "code": null,
    "level": "error",
    "spans": [
        {
            "file_name": "/home/pete/.cargo/registry/src/index.crates.io-0000000000000000/typenum-1.12.0/src/lib.rs",
            "byte_start": 2460,
            "byte_end": 2484,
            "line_start": 76,
            "line_end": 76,
            "column_start": 14,
            "column_end": 38,
            "is_primary": true,
            "text": [
                {
                    "text": "    include!(env!(\"TYPENUM_BUILD_OP\"));",
                    "highlight_start": 14,
                    "highlight_end": 38
                }
            ],
            "label": null,
            "suggested_replacement": null,
            "suggestion_applicability": null,
            "expansion": {
                "span": {
                    "file_name": "/home/pete/.cargo/registry/src/index.crates.io-0000000000000000/typenum-1.12.0/src/lib.rs",
                    "byte_start": 2460,
                    "byte_end": 2484,
                    "line_start": 76,
                    "line_end": 76,
                    "column_start": 14,
                    "column_end": 38,
                    "is_primary": false,
                    "text": [
                        {
                            "text": "    include!(env!(\"TYPENUM_BUILD_OP\"));",
                            "highlight_start": 14,
                            "highlight_end": 38
                        }
                    ],
                    "label": null,
                    "suggested_replacement": null,
                    "suggestion_applicability": null,
                    "expansion": null
                },
                "macro_decl_name": "env!",
                "def_site_span": {
                    "file_name": "/rustc/05f9846f893b09a1be1fc8560e33fc3c815cfecb/library/core/src/macros/mod.rs",
                    "byte_start": 40546,
                    "byte_end": 40562,
                    "line_start": 1164,
                    "line_end": 1164,
                    "column_start": 5,
                    "column_end": 21,
                    "is_primary": false,
                    "text": [],
                    "label": null,
                    "suggested_replacement": null,
                    "suggestion_applicability": null,
                    "expansion": null
                }
            }
        }
    ],
    "children": [
        {
            "message": "use `std::env::var(\"TYPENUM_BUILD_OP\")` to read the variable at run time",
            "code": null,
            "level": "help",
            "spans": [],
            "children": [],
            "rendered": null
        }
    ],
    "rendered": "\u001b[0m\u001b[1m\u001b[38;5;9merror\u001b[0m\u001b[0m\u001b[1m: environment variable `TYPENUM_BUILD_OP` not defined at compile time\u001b[0m\n\u001b[0m  \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m--> \u001b[0m\u001b[0m/home/pete/.cargo/registry/src/index.crates.io-0000000000000000/typenum-1.12.0/src/lib.rs:76:14\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\n\u001b[0m\u001b[1m\u001b[38;5;12m76\u001b[0m\u001b[0m \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\u001b[0m \u001b[0m\u001b[0m    include!(env!(\"TYPENUM_BUILD_OP\"));\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\u001b[0m              \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;9m^^^^^^^^^^^^^^^^^^^^^^^^\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m|\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mhelp\u001b[0m\u001b[0m: use `std::env::var(\"TYPENUM_BUILD_OP\")` to read the variable at run time\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mhelp\u001b[0m\u001b[0m: cargo-green: add `\"TYPENUM_BUILD_OP\"` to either $CARGOGREEN_SET_ENVS or to this crate's or your root crate's [package.metadata.green] set-envs list\u001b[0m\n\u001b[0m   \u001b[0m\u001b[0m\u001b[1m\u001b[38;5;12m= \u001b[0m\u001b[0m\u001b[1mnote\u001b[0m\u001b[0m: this error originates in the macro `env` (in Nightly builds, run with -Z macro-backtrace for more info)\u001b[0m\n\n"
}"#,
    );

    assert_eq!(env_not_comptime_defined(&input), Some("TYPENUM_BUILD_OP"));

    pretty_assertions::assert_eq!(
        roundtrip(&suggest_set_envs("TYPENUM_BUILD_OP", &input).unwrap()),
        output
    );
}
