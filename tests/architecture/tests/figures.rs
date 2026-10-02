//! The front page's figures are generated from the committed reports, so a
//! report cannot change without its picture changing too.

use metron_architecture_tests::workspace_root;
use metron_cli::figures::FIGURES_DIR;
use metron_cli::{render_figures, stale_figures};
use std::fs;

#[test]
fn front_page_figures_match_the_committed_reports() {
    let root = workspace_root();
    let stale = stale_figures(&root, &root.join(FIGURES_DIR)).expect("figures render");
    assert!(
        stale.is_empty(),
        "figures out of date: {stale:?}; run `cargo run -p metron-cli -- figures`"
    );
}

#[test]
fn the_readme_shows_every_figure_and_nothing_else_from_the_figures_dir() {
    let root = workspace_root();
    let readme = fs::read_to_string(root.join("README.md")).expect("README.md");
    let figures = render_figures(&root).expect("figures render");
    for figure in &figures {
        let path = format!("{FIGURES_DIR}/{}", figure.file);
        assert!(readme.contains(&path), "README.md does not show {path}");
    }
    for entry in fs::read_dir(root.join(FIGURES_DIR)).expect("figures dir") {
        let name = entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .into_owned();
        assert!(
            figures.iter().any(|f| f.file == name),
            "{FIGURES_DIR}/{name} is not produced by `metron figures`"
        );
    }
}

#[test]
fn figures_are_well_formed_svg() {
    let root = workspace_root();
    for figure in render_figures(&root).expect("figures render") {
        let svg = &figure.svg;
        assert!(
            svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""),
            "{}",
            figure.file
        );
        assert!(svg.trim_end().ends_with("</svg>"), "{}", figure.file);
        assert!(svg.contains("<title>"), "{} has no title", figure.file);
        assert_eq!(
            svg.matches("<text").count(),
            svg.matches("</text>").count(),
            "{}",
            figure.file
        );
        assert!(
            !svg.contains("NaN") && !svg.contains("inf"),
            "{}",
            figure.file
        );
    }
}
