use std::path::PathBuf;

pub struct BinaryLocator;

impl BinaryLocator {
    pub fn locate(name: &str) -> PathBuf {
        let mut path = std::env::current_exe().expect("Failed to get current executable path");
        path.pop();

        if path.ends_with("deps") {
            path.pop();
        }

        let mut name = name.to_string();
        if cfg!(target_os = "windows") && !name.ends_with(".exe") {
            name.push_str(".exe");
        }

        path.push(name);
        path
    }
}
