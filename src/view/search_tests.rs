use super::*;

fn fixture() -> Model {
    let mut model = Model::new("C:\\Data".into());
    let entry = |name: &str, is_dir, logical| Entry {
        name: name.into(),
        is_dir,
        logical,
        allocated: None,
        modified: None,
        reparse: false,
        identity: None,
    };
    model.insert_batch(0, vec![entry("Downloads", true, 0), entry("Work", true, 0)]);
    model.insert_batch(
        1,
        vec![
            entry("Annual Report.ZIP", false, 1_500_000),
            entry("Annual Report draft.zip", false, 2_000_000),
            entry("archive.7z", false, 1_048_576),
            entry("notes.txt", false, 0),
        ],
    );
    model.insert_batch(
        2,
        vec![
            entry("Annual Report.zip", false, 3_000_000),
            entry("ÉTÉ.txt", false, 1),
        ],
    );
    model
}

#[test]
fn search_terms_phrases_exclusions_and_metadata_intersect() {
    let model = fixture();
    for (search, expected) in [
        ("annual report -draft", vec![7, 3]),
        (
            "\"annual report\" -name:draft path:downloads ext:zip size:>=1.5MB",
            vec![3],
        ),
        ("ext:.ZIP,7z size:<=2MiB -draft", vec![3, 5]),
        ("path:WORK name:report", vec![7]),
        ("size:=0", vec![6]),
        ("name:été", vec![8]),
        ("-ext:zip", vec![5, 8, 6]),
        ("size:>1048575 size:<1048577", vec![5]),
        ("\"Annual Report.ZIP\"", vec![7, 3]),
    ] {
        let view = build_view(
            &model,
            &ViewRequest {
                mode: ViewMode::Largest,
                search: search.into(),
                ..Default::default()
            },
            1,
        );
        assert_eq!(
            view.rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            expected,
            "{search}"
        );
        assert_eq!(view.total_rows, expected.len());
        assert_eq!(view.matching_files, expected.len() as u64);
    }
}

#[test]
fn malformed_search_is_an_explicit_error_and_never_broadens_results() {
    let model = fixture();
    for search in [
        "size:elephant",
        "size:>1XB",
        "path:",
        "ext:zip,",
        "\"unclosed",
        "size:>18446744073709551616",
    ] {
        let view = build_view(
            &model,
            &ViewRequest {
                mode: ViewMode::Largest,
                search: search.into(),
                ..Default::default()
            },
            1,
        );
        assert!(view.rows.is_empty(), "{search}");
        assert!(view.search_error.is_some(), "{search}");
        assert_eq!(view.total_rows, 0);
        assert!(view.filters_active);
    }
}

#[test]
fn structured_search_respects_scope_filters_and_keeps_map_geometry() {
    let model = fixture();
    let base = ViewRequest {
        root: 2,
        mode: ViewMode::Largest,
        ..Default::default()
    };
    let original = build_view(&model, &base, 1);
    let view = build_view(
        &model,
        &ViewRequest {
            search: "path:downloads ext:zip".into(),
            search_all: true,
            filters: QueryFilter {
                max_bytes: Some(1_500_000),
                ..Default::default()
            },
            ..base
        },
        1,
    );
    assert_eq!(view.rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![3]);
    assert_eq!((view.row_scope_root, view.root), (0, 2));
    assert_eq!(view.matching_logical, 1_500_000);
    assert_eq!(
        view.map.iter().map(|t| t.rect).collect::<Vec<_>>(),
        original.map.iter().map(|t| t.rect).collect::<Vec<_>>()
    );
    assert!(view.map.iter().all(|t| !t.matched));
}
