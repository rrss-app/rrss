use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

fn main() {
    let desktop = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/desktop");
    watch_frontend(&desktop);
    println!("cargo:rerun-if-env-changed=RRSS_SKIP_FRONTEND");
    if std::env::var_os("RRSS_SKIP_FRONTEND").is_none() {
        ensure_frontend(&desktop);
    }
    tauri_build::build();
}

fn watch_frontend(desktop: &Path) {
    for path in frontend_inputs(desktop) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn ensure_frontend(desktop: &Path) {
    let dist = desktop.join("dist/index.html");
    let stale = match newest_mtime(&frontend_inputs(desktop)) {
        Some(src) => dist_mtime(&dist).map(|built| src > built).unwrap_or(true),
        None => !dist.exists(),
    };
    if !stale {
        return;
    }
    println!(
        "cargo:warning=frontend {}；running pnpm --dir apps/desktop build",
        if dist.exists() {
            "changed"
        } else {
            "dist missing"
        }
    );
    ensure_node_modules(desktop);
    run_pnpm(desktop, &["build"]);
}

fn ensure_node_modules(desktop: &Path) {
    let vite = desktop.join("node_modules/.bin/vite");
    let lock = desktop.join("pnpm-lock.yaml");
    let need_install = match (vite.metadata().ok(), lock.metadata().ok()) {
        (None, _) => true,
        (Some(bin), Some(lock)) => lock
            .modified()
            .ok()
            .zip(bin.modified().ok())
            .is_some_and(|(l, b)| l > b),
        (Some(_), None) => false,
    };
    if need_install {
        println!("cargo:warning=pnpm install (node_modules missing or lockfile newer)");
        run_pnpm(desktop, &["install", "--frozen-lockfile"]);
    }
}

fn run_pnpm(desktop: &Path, args: &[&str]) {
    let mut cmd = pnpm_command();
    let status = cmd
        .args(args)
        .current_dir(desktop)
        .status()
        .unwrap_or_else(|err| {
            panic!("pnpm not found ({err}); install Node/pnpm, or set RRSS_SKIP_FRONTEND=1")
        });
    if !status.success() {
        panic!("`pnpm {}` failed in apps/desktop", args.join(" "));
    }
}

fn pnpm_command() -> Command {
    if cfg!(windows) {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", "pnpm"]);
        cmd
    } else {
        Command::new("pnpm")
    }
}

fn frontend_inputs(desktop: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for name in [
        "index.html",
        "package.json",
        "pnpm-lock.yaml",
        "vite.config.ts",
        "tsconfig.json",
        ".npmrc",
    ] {
        let path = desktop.join(name);
        if path.exists() {
            files.push(path);
        }
    }
    collect_sources(&desktop.join("src"), &mut files);
    collect_sources(&desktop.join("public"), &mut files);
    files
}

fn collect_sources(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.')
            || matches!(
                name.as_ref(),
                "node_modules" | "dist" | "e2e" | "test-results" | "playwright-report"
            )
        {
            continue;
        }
        if path.is_dir() {
            collect_sources(&path, files);
            continue;
        }
        if name.ends_with(".test.ts") || name.ends_with(".test.tsx") {
            continue;
        }
        files.push(path);
    }
}

fn newest_mtime(files: &[PathBuf]) -> Option<SystemTime> {
    files.iter().filter_map(|p| dist_mtime(p)).max()
}

fn dist_mtime(path: &Path) -> Option<SystemTime> {
    path.metadata().ok()?.modified().ok()
}
