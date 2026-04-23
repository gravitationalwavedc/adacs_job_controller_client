//! Temporary directory fixture with symlink support.
//!
//! Creates temporary directories with test files and symlinks
//! for testing file operations. Matches C++ TemporaryDirectoryFixture.

use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

pub struct TemporaryDirectoryFixture {
    pub temp_dir: TempDir,
    pub symlink_dir: PathBuf,
    pub symlink_file: PathBuf,
    pub temp_file: PathBuf,
    pub temp_file2: PathBuf,
    pub temp_dir2: PathBuf,
}

impl TemporaryDirectoryFixture {
    pub fn new() -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let temp_path = temp_dir.path();

        // Create nested directory structure
        let temp_dir2 = temp_path.join("subdir");
        fs::create_dir_all(&temp_dir2).expect("Failed to create subdir");

        // Create test files
        let temp_file = temp_path.join("file1.txt");
        fs::write(&temp_file, "12345").expect("Failed to write file1");

        let temp_file2 = temp_dir2.join("file2.txt");
        fs::write(&temp_file2, "12345678").expect("Failed to write file2");

        // Create symlinks (Unix only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::{symlink, symlink as symlink_dir};

            let symlink_dir_path = temp_path.join("symlink_dir");
            let symlink_file_path = temp_path.join("symlink_file");

            // Create valid symlinks
            let _ = symlink_dir(temp_path.join("subdir"), &symlink_dir_path);
            let _ = symlink(&temp_file, &symlink_file_path);

            // Create broken symlinks
            let broken_symlink_dir = temp_path.join("symlink_dir_broken");
            let broken_symlink_file = temp_path.join("symlink_file_broken");
            let _ = symlink_dir("/not/a/real/path", &broken_symlink_dir);
            let _ = symlink("/not/a/real/path", &broken_symlink_file);

            Self {
                temp_dir,
                symlink_dir: symlink_dir_path,
                symlink_file: symlink_file_path,
                temp_file,
                temp_file2,
                temp_dir2,
            }
        }

        #[cfg(not(unix))]
        {
            // On Windows, just use regular directories
            Self {
                temp_dir,
                symlink_dir: temp_path.join("symlink_dir"),
                symlink_file: temp_path.join("symlink_file"),
                temp_file,
                temp_file2,
                temp_dir2,
            }
        }
    }

    pub fn get_temp_path(&self) -> &Path {
        self.temp_dir.path()
    }

    pub fn get_temp_file_path(&self) -> &Path {
        &self.temp_file
    }

    pub fn get_temp_file2_path(&self) -> &Path {
        &self.temp_file2
    }

    pub fn get_temp_dir2_path(&self) -> &Path {
        &self.temp_dir2
    }

    pub fn create_test_file(&self, name: &str, content: &str) -> PathBuf {
        let path = self.temp_dir.path().join(name);
        fs::write(&path, content).expect("Failed to write test file");
        path
    }

    pub fn create_test_directory(&self, name: &str) -> PathBuf {
        let path = self.temp_dir.path().join(name);
        fs::create_dir_all(&path).expect("Failed to create test directory");
        path
    }

    pub fn write_large_file(&self, name: &str, size_mb: usize) -> PathBuf {
        use std::io::Write;

        let path = self.temp_dir.path().join(name);
        let mut file = fs::File::create(&path).expect("Failed to create large file");

        // Write random data
        let chunk_size = 1024 * 1024; // 1MB chunks
        let mut written = 0;

        while written < size_mb * 1024 * 1024 {
            let to_write = std::cmp::min(chunk_size, size_mb * 1024 * 1024 - written);
            let data: Vec<u8> = (0..to_write).map(|_| rand::random::<u8>()).collect();
            file.write_all(&data).expect("Failed to write data");
            written += to_write;
        }

        file.flush().expect("Failed to flush file");
        path
    }
}

impl Default for TemporaryDirectoryFixture {
    fn default() -> Self {
        Self::new()
    }
}
