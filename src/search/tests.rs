use super::*;

#[test]
fn sizes_are_exact_inclusive_and_never_rounded_or_saturated() {
    for (text, bytes) in [
        ("1.5MB", 1_500_000),
        ("1.5MiB", 1_572_864),
        ("0.001KB", 1),
        ("18446744073709551615B", u64::MAX),
        ("0", 0),
        (
            "1.000000000000000000000000000000000000000000GB",
            1_000_000_000,
        ),
    ] {
        let query = SearchQuery::parse(&format!("size:={text}")).unwrap();
        assert!(
            query.matches("x", bytes, "", || panic!("no path expected")),
            "{text}"
        );
        if bytes > 0 {
            assert!(!query.matches("x", bytes - 1, "", String::new), "{text}");
        }
    }
    for text in [
        "0.1B",
        "1.0000001MB",
        "1.2.3GB",
        "1e3",
        "NaN",
        "inf",
        "-1",
        "1.",
        ".5MB",
        "18446744073709551616",
        "999999999999999999999999999999999999999999999999999GB",
    ] {
        assert!(
            SearchQuery::parse(&format!("size:{text}")).is_err(),
            "{text}"
        );
    }
}

#[test]
fn query_limits_include_both_layout_and_non_gui_callers() {
    assert!(SearchQuery::parse(&"é".repeat(MAX_QUERY_CHARS)).is_ok());
    assert!(SearchQuery::parse(&"é".repeat(MAX_QUERY_CHARS + 1)).is_err());
    assert!(SearchQuery::parse(&vec!["a"; MAX_QUERY_TERMS].join(" ")).is_ok());
    assert!(SearchQuery::parse(&vec!["a"; MAX_QUERY_TERMS + 1].join(" ")).is_err());
    assert!(!SearchQuery::parse(" \t ").unwrap().is_active());
    for query in [
        "\"\"",
        "name:",
        "-size:",
        "ext:,zip",
        "ext:.",
        "ext:*.",
        "size:==1",
        "path:\"unterminated",
    ] {
        assert!(SearchQuery::parse(query).is_err(), "{query}");
    }
}

#[test]
fn literal_minus_unknown_prefixes_unicode_and_windows_paths_are_searchable() {
    for (query, name, path) in [
        ("\"-draft\"", "-DRAFT.zip", ""),
        ("-", "-notes", ""),
        ("unknown:thing", "unknown:thing.txt", ""),
        ("\"summer holiday\"", "summer holiday.zip", ""),
        ("name:été", "ÉTÉ.zip", ""),
        (r#"path:"D:\Some Folder\Été""#, "x", "D:/Some Folder/ÉTÉ/x"),
    ] {
        assert!(
            SearchQuery::parse(query)
                .unwrap()
                .matches(name, 0, "", || path.into()),
            "{query}"
        );
    }
}

#[test]
fn path_is_built_once_and_only_after_cheaper_predicates_pass() {
    let query =
        SearchQuery::parse("path:downloads path:cache name:archive size:>10 ext:zip").unwrap();
    for (name, size, extension) in [
        ("archive", 0, ".zip"),
        ("other", 11, ".zip"),
        ("archive", 11, ".dat"),
    ] {
        assert!(!query.matches(name, size, extension, || panic!(
            "rejected without building path"
        )));
    }
    let mut calls = 0;
    assert!(query.matches("archive", 11, ".zip", || {
        calls += 1;
        "D:/Downloads/cache/archive.zip".into()
    }));
    assert_eq!(calls, 1);
    assert!(
        SearchQuery::parse("archive")
            .unwrap()
            .matches("ARCHIVE.zip", 0, "", || panic!(
                "plain names never build paths"
            ))
    );
}

#[test]
fn extension_alternatives_and_negative_groups_are_consistent() {
    let query = SearchQuery::parse("ext:\"(no extension)\" -name:draft").unwrap();
    assert!(query.matches("README", 0, "(no extension)", String::new));
    assert!(!query.matches("DRAFT", 0, "(no extension)", String::new));
    let query = SearchQuery::parse("-ext:zip,7z").unwrap();
    for ext in [".zip", ".7z"] {
        assert!(!query.matches("x", 0, ext, String::new));
    }
    assert!(query.matches("x", 0, ".txt", String::new));
}
