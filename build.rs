use std::process::Command;

fn main() {
    // Get the current git commit hash
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("Failed to execute git command");
    let git_commit = String::from_utf8(output.stdout).unwrap();
    println!("cargo:rustc-env=GIT_COMMIT={}", git_commit.trim());
}
