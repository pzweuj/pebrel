//! Selected, embedded Material Icon Theme assets; no runtime network or font lookup.
use crate::ssh_sftp::SftpEntryKind;
use gpui::{
    InteractiveElement as _, IntoElement as _, ParentElement as _, Styled as _, div, img, px,
};

macro_rules! assets {
    ($($name:literal),* $(,)?) => { &[$((concat!("file-icons/material/", $name, ".svg"), include_bytes!(concat!("../../../assets/file-icons/material/", $name, ".svg")) as &[u8])),*] };
}

pub(super) const ASSETS: &[(&str, &[u8])] = assets![
    "c",
    "console",
    "cpp",
    "csharp",
    "css",
    "database",
    "docker",
    "document",
    "folder-base",
    "git",
    "go",
    "html",
    "image",
    "java",
    "javascript",
    "json",
    "lock",
    "log",
    "markdown",
    "pdf",
    "powershell",
    "python",
    "rust",
    "settings",
    "table",
    "toml",
    "typescript",
    "xml",
    "yaml",
    "zip",
];

pub(super) fn path(name: &str, kind: SftpEntryKind) -> &'static str {
    let lower = name.to_ascii_lowercase();
    let icon = if kind == SftpEntryKind::Directory {
        "folder-base"
    } else if lower.starts_with(".git") {
        "git"
    } else if lower == "dockerfile" || lower.starts_with("docker-compose.") {
        "docker"
    } else if matches!(lower.as_str(), ".env" | ".bashrc" | ".zshrc" | "makefile") {
        "settings"
    } else {
        match lower.rsplit_once('.').map(|(_, extension)| extension).unwrap_or("") {
            "rs" => "rust",
            "py" | "pyw" => "python",
            "js" | "jsx" | "mjs" | "cjs" => "javascript",
            "ts" | "tsx" => "typescript",
            "json" | "jsonl" | "ndjson" => "json",
            "yaml" | "yml" => "yaml",
            "toml" => "toml",
            "xml" => "xml",
            "html" | "htm" => "html",
            "css" | "scss" | "less" => "css",
            "java" | "jar" => "java",
            "c" | "h" => "c",
            "cc" | "cpp" | "cxx" | "hpp" => "cpp",
            "cs" => "csharp",
            "go" => "go",
            "sh" | "bash" | "zsh" | "bat" | "cmd" => "console",
            "ps1" | "psm1" | "psd1" => "powershell",
            "sql" | "db" | "sqlite" | "csv" | "tsv" | "bam" | "bai" | "sam" | "vcf" | "bcf" => {
                "database"
            },
            "xls" | "xlsx" | "xlsm" | "xlsb" | "xlt" | "xltx" | "xltm" | "ods" | "numbers" => {
                "table"
            },
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "svg" => "image",
            "zip" | "7z" | "rar" | "gz" | "tar" | "xz" | "zst" | "bz2" | "tgz" => "zip",
            "pdf" => "pdf",
            "md" | "markdown" => "markdown",
            "log" => "log",
            "lock" => "lock",
            "ini" | "conf" | "cfg" => "settings",
            _ => "document",
        }
    };
    ASSETS
        .iter()
        .find(|(path, _)| {
            path.rsplit('/').next().is_some_and(|file| file.strip_suffix(".svg") == Some(icon))
        })
        .map(|(path, _)| *path)
        .expect("mapped material icon is embedded")
}

pub(super) fn render(name: &str, kind: SftpEntryKind) -> gpui::AnyElement {
    let asset = path(name, kind);
    div()
        .relative()
        .size(px(16.0))
        .flex_shrink_0()
        .debug_selector(move || format!("sftp-file-icon-{asset}"))
        .child(img(asset).size(px(16.0)))
        .children((kind == SftpEntryKind::Symlink).then(|| {
            div()
                .absolute()
                .bottom_0()
                .right_0()
                .text_size(px(9.0))
                .font_family(crate::font_install::SYMBOL_FONT_FAMILY)
                .child("\u{ea71}")
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::AssetSource as _;
    #[test]
    fn material_file_icons_are_embedded_and_cover_remote_names() {
        for (name, icon) in [
            ("main.PY", "python"),
            ("Dockerfile", "docker"),
            (".gitignore", "git"),
            ("reads.bam", "database"),
            ("budget.xlsx", "table"),
            ("legacy.XLS", "table"),
            ("macros.xlsm", "table"),
            ("binary.xlsb", "table"),
            ("sheet.ods", "table"),
            ("report.docx", "document"),
            ("archive.tar.gz", "zip"),
            ("unknown", "document"),
        ] {
            let asset = path(name, SftpEntryKind::File);
            assert!(asset.ends_with(&format!("/{icon}.svg")), "{name}: {asset}");
        }
        assert!(path("src.py", SftpEntryKind::Directory).ends_with("/folder-base.svg"));
        for (asset, _) in ASSETS {
            let bytes = super::super::assets::NebulaAssets.load(asset).unwrap().unwrap();
            assert!(resvg::usvg::Tree::from_data(&bytes, &Default::default()).is_ok(), "{asset}");
        }
    }

    #[test]
    fn material_python_asset_rasterizes_in_blue_and_yellow() {
        let bytes = super::super::assets::NebulaAssets
            .load(path("main.py", SftpEntryKind::File))
            .unwrap()
            .unwrap();
        let tree = resvg::usvg::Tree::from_data(&bytes, &Default::default()).unwrap();
        let mut pixels = resvg::tiny_skia::Pixmap::new(24, 24).unwrap();
        resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pixels.as_mut());
        assert!(
            pixels.data().chunks_exact(4).any(|p| p[3] > 200 && p[2] > p[0].saturating_add(50))
        );
        assert!(
            pixels
                .data()
                .chunks_exact(4)
                .any(|p| p[3] > 200 && p[0] > 180 && p[1] > 120 && p[2] < 100)
        );
    }

    #[test]
    fn material_spreadsheet_asset_rasterizes_in_green() {
        for name in ["report.xlsx", "legacy.xls", "sheet.ods"] {
            let bytes = super::super::assets::NebulaAssets
                .load(path(name, SftpEntryKind::File))
                .unwrap()
                .unwrap();
            let tree = resvg::usvg::Tree::from_data(&bytes, &Default::default()).unwrap();
            let mut pixels = resvg::tiny_skia::Pixmap::new(24, 24).unwrap();
            resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pixels.as_mut());
            assert!(
                pixels.data().chunks_exact(4).any(|p| {
                    p[3] > 200 && p[1] > p[0].saturating_add(40) && p[1] > p[2].saturating_add(40)
                }),
                "{name} should render with green pixels",
            );
            assert!(
                !pixels.data().chunks_exact(4).any(|p| p[3] > 200 && p[2] > p[1] && p[2] > p[0]),
                "{name} must not render as blue",
            );
        }
    }
}
