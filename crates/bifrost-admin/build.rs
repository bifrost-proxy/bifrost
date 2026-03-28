use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=BIFROST_VERSION");
    println!("cargo:rerun-if-env-changed=SKIP_FRONTEND_BUILD");
    if let Ok(version) = env::var("BIFROST_VERSION") {
        println!("cargo:rustc-env=CARGO_PKG_VERSION={}", version);
    }

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let manifest_path = PathBuf::from(&manifest_dir);
    let web_dir = manifest_path
        .join("..")
        .join("..")
        .join("web")
        .canonicalize()
        .unwrap_or_else(|_| manifest_path.join("../../web"));
    let dist_dir = web_dir.join("dist");

    println!("cargo:rerun-if-changed={}", web_dir.join("src").display());
    println!(
        "cargo:rerun-if-changed={}",
        web_dir.join("package.json").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        web_dir.join("vite.config.ts").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        web_dir.join("tsconfig.json").display()
    );

    if env::var("SKIP_FRONTEND_BUILD").is_ok() {
        println!("cargo:warning=Skipping frontend build (SKIP_FRONTEND_BUILD is set)");
        ensure_dist_exists(&dist_dir);
        return;
    }

    if !web_dir.exists() {
        println!(
            "cargo:warning=Web directory not found at {}, skipping frontend build",
            web_dir.display()
        );
        ensure_dist_exists(&dist_dir);
        return;
    }

    let node_modules = web_dir.join("node_modules");
    if !node_modules.exists() {
        if dist_dir.exists() && dist_dir.join("index.html").exists() {
            println!("cargo:warning=node_modules not found but pre-built frontend exists, skipping install");
        } else {
            println!("cargo:warning=Installing frontend dependencies with pnpm...");
            let pnpm_cmd = resolve_command(if cfg!(windows) { "pnpm.cmd" } else { "pnpm" });
            let output = Command::new(&pnpm_cmd)
                .args(["install", "--frozen-lockfile"])
                .current_dir(&web_dir)
                .env("COREPACK_ENABLE_STRICT", "0")
                .env("PATH", frontend_command_path())
                .output()
                .or_else(|_| {
                    Command::new(&pnpm_cmd)
                        .arg("install")
                        .current_dir(&web_dir)
                        .env("COREPACK_ENABLE_STRICT", "0")
                        .env("PATH", frontend_command_path())
                        .output()
                });

            match output {
                Ok(output) if output.status.success() => {}
                Ok(output) => {
                    eprintln!(
                        "pnpm install stdout: {}",
                        String::from_utf8_lossy(&output.stdout)
                    );
                    eprintln!(
                        "pnpm install stderr: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                    panic!(
                        "pnpm install failed with exit code: {:?}",
                        output.status.code()
                    );
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    panic!(
                        "pnpm not found and no pre-built frontend. \
                         Please install pnpm or set SKIP_FRONTEND_BUILD=1"
                    );
                }
                Err(e) => panic!("Failed to run pnpm install: {:?}", e),
            }
        }
    }

    println!(
        "cargo:warning=Building frontend at {}...",
        web_dir.display()
    );
    let pnpm_cmd = resolve_command(if cfg!(windows) { "pnpm.cmd" } else { "pnpm" });
    let output = match Command::new(&pnpm_cmd)
        .args(["run", "build"])
        .current_dir(&web_dir)
        .env("COREPACK_ENABLE_STRICT", "0")
        .env("PATH", frontend_command_path())
        .output()
    {
        Ok(output) => output,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("cargo:warning=pnpm not found, checking if dist already exists...");
            if dist_dir.exists() && dist_dir.join("index.html").exists() {
                println!(
                    "cargo:warning=Pre-built frontend found at {}",
                    dist_dir.display()
                );
                return;
            }
            panic!(
                "pnpm not found and no pre-built frontend at {}. \
                 Please install pnpm or build frontend first.",
                dist_dir.display()
            );
        }
        Err(e) => panic!("Failed to run pnpm run build: {:?}", e),
    };

    if !output.status.success() {
        eprintln!(
            "Frontend build stdout: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        eprintln!(
            "Frontend build stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        panic!(
            "Frontend build failed with exit code: {:?}",
            output.status.code()
        );
    }

    if !dist_dir.exists() {
        panic!(
            "Frontend build did not produce dist directory at {}",
            dist_dir.display()
        );
    }

    println!(
        "cargo:warning=Frontend build completed successfully at {}",
        dist_dir.display()
    );
}

fn ensure_dist_exists(dist_dir: &Path) {
    if !dist_dir.exists() {
        std::fs::create_dir_all(dist_dir).expect("Failed to create dist directory");
    }
    let index_html = dist_dir.join("index.html");
    if !index_html.exists() {
        std::fs::write(
            &index_html,
            r#"<!DOCTYPE html>
<html>
<head><title>Bifrost Admin</title></head>
<body>
<h1>Frontend not built</h1>
<p>Run <code>pnpm run build</code> in the web directory or rebuild with <code>cargo build</code></p>
</body>
</html>"#,
        )
        .expect("Failed to create placeholder index.html");
    }
}

fn resolve_command(command: &str) -> String {
    let Ok(output) = Command::new("which").arg("-a").arg(command).output() else {
        return command.to_string();
    };
    if !output.status.success() {
        return command.to_string();
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .map(str::trim)
        .find(|path| !path.is_empty() && !path.contains("/mise/shims/"))
        .or_else(|| stdout.lines().map(str::trim).find(|path| !path.is_empty()))
        .unwrap_or(command)
        .to_string()
}

fn frontend_command_path() -> OsString {
    let mut paths = vec![];
    let node_path = resolve_command(if cfg!(windows) { "node.exe" } else { "node" });
    if let Some(parent) = Path::new(&node_path).parent() {
        paths.push(parent.to_path_buf());
    }
    if let Some(existing) = env::var_os("PATH") {
        paths.extend(env::split_paths(&existing));
    }

    env::join_paths(paths).unwrap_or_else(|_| env::var_os("PATH").unwrap_or_default())
}
